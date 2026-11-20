//! Wireless gaming headsets that answer a battery request on a vendor HID collection: HyperX, Logitech G,
//! SteelSeries Arctis, Corsair, Sony INZONE and a few more. The request/response bytes
//! are the protocol facts collected by HeadsetControl (github.com/Sapd/HeadsetControl, lib/devices/*), for the HyperX
//! Cloud III Wireless by HyperHeadset (github.com/LennardKittner/HyperHeadset) and for the Corsair HS80 RGB Wireless
//! by OpenLinkHub (github.com/jurkovic-nikola/OpenLinkHub); this is an independent implementation. Byte positions
//! below are as hidapi returns them: for devices without report IDs Windows puts a 0x00 in front of every input
//! report, which `Io::data` strips the same way hidapi does.
//! Read-only status queries; not yet verified on hardware by this project, which is why the first exchange with
//! each headset goes to the log.

use super::{group_by, hex_trimmed as hex, one_per_id, pad, remaining_ms, secs, sleep_ms, Provider};
use crate::hid::{self, HidDevice, HidInfo};
use crate::loc::{t, tf};
use crate::log;
use crate::reading::{DeviceKind, DeviceReading};
use crate::win::{self, Error};
use std::collections::HashSet;
use std::time::{Duration, Instant};

const TIMEOUT_MS: u64 = 2000;

/// `detail`: a text key or a ready text. `full`: the headset itself says it is full, with the charger connected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub online: bool,
    pub level: Option<i32>,
    pub charging: bool,
    pub detail: Option<String>,
    pub full: bool,
}

impl State {
    fn new(online: bool, level: Option<i32>, charging: bool) -> Self {
        Self { online, level, charging, detail: None, full: false }
    }
}

fn off() -> State {
    State { detail: Some("d.offOrAway".into()), ..State::new(false, None, false) }
}

fn off_because(detail: impl Into<String>) -> State {
    State { detail: Some(detail.into()), ..off() }
}

fn on(level: i32, charging: bool) -> State {
    State::new(true, Some(level.clamp(0, 100)), charging)
}

type Query = fn(&mut Io) -> win::Result<State>;

/// `page`: usage page of the collection to talk to, 0 = any vendor collection. `interface`: USB interface, -1 = any.
struct Spec {
    key: &'static str,
    name: &'static str,
    vid: u16,
    pids: &'static [u16],
    page: u16,
    usage: u16,
    interface: i32,
    query: Query,
}

// Voltage (mV) -> percent tables HeadsetControl uses for the Logitech headsets
const G533: &[(i32, i32)] = &[(4200, 100), (3850, 50), (3790, 30), (3750, 20), (3680, 5), (3330, 0)];
const G535: &[(i32, i32)] = &[(4175, 100), (3817, 50), (3766, 30), (3730, 20), (3664, 5), (3310, 0)];
const G633: &[(i32, i32)] =
    &[(4100, 100), (3950, 80), (3850, 60), (3750, 40), (3650, 20), (3500, 10), (3300, 5), (3150, 0)];
const GPRO: &[(i32, i32)] = &[(4150, 100), (3830, 50), (3780, 30), (3740, 20), (3670, 5), (3320, 0)];

static SPECS: &[Spec] = &[
    // ---- HyperX ----
    Spec {
        key: "hyperx-cloud-alpha-wireless",
        name: "HyperX Cloud Alpha Wireless",
        vid: 0x03F0,
        pids: &[0x098D],
        page: 0,
        usage: 0,
        interface: -1,
        query: hyperx_cloud_alpha,
    },
    Spec {
        key: "hyperx-cloud-ii-wireless",
        name: "HyperX Cloud II Wireless",
        vid: 0x03F0,
        pids: &[0x0696, 0x018B],
        page: 0xFF90,
        usage: 0x0303,
        interface: -1,
        query: hyperx_cloud_ii_hp,
    },
    Spec {
        key: "hyperx-cloud-ii-wireless",
        name: "HyperX Cloud II Wireless",
        vid: 0x0951,
        pids: &[0x1718],
        page: 0,
        usage: 0,
        interface: -1,
        query: hyperx_cloud_ii_kingston,
    },
    Spec {
        key: "hyperx-cloud-iii-wireless",
        name: "HyperX Cloud III Wireless",
        vid: 0x03F0,
        pids: &[0x05B7, 0x0C9D],
        page: 0,
        usage: 0,
        interface: -1,
        query: hyperx_cloud_iii,
    },
    Spec {
        key: "hyperx-cloud-flight",
        name: "HyperX Cloud Flight",
        vid: 0x0951,
        pids: &[0x16C4, 0x1723],
        page: 0,
        usage: 0,
        interface: -1,
        query: hyperx_cloud_flight,
    },
    // ---- Logitech G (HID++ with a fixed feature index; the reply carries the battery voltage) ----
    Spec {
        key: "logitech-g533",
        name: "Logitech G533",
        vid: 0x046D,
        pids: &[0x0A66],
        page: 0xFF43,
        usage: 0x0202,
        interface: 3,
        query: |io| logitech_voltage(io, 0x07, 0x01, G533),
    },
    Spec {
        key: "logitech-g535",
        name: "Logitech G535",
        vid: 0x046D,
        pids: &[0x0AC4],
        page: 0x000C,
        usage: 0x0001,
        interface: 3,
        query: |io| logitech_voltage(io, 0x05, 0x0D, G535),
    },
    Spec {
        key: "logitech-g-headset",
        name: "Logitech G633 / G635 / G733 / G933 / G935",
        vid: 0x046D,
        pids: &[0x0A5C, 0x0A89, 0x0A5B, 0x0A87, 0x0AB5, 0x0AFE, 0x0B1F],
        page: 0xFF43,
        usage: 0x0202,
        interface: -1,
        query: |io| logitech_voltage(io, 0x08, 0x0A, G633),
    },
    Spec {
        key: "logitech-g-pro",
        name: "Logitech G PRO / PRO X Wireless",
        vid: 0x046D,
        pids: &[0x0AA7, 0x0AAA, 0x0ABA, 0x0AFB, 0x0AFC],
        page: 0xFF43,
        usage: 0x0202,
        interface: -1,
        query: |io| logitech_voltage(io, 0x06, 0x0D, GPRO),
    },
    Spec {
        key: "logitech-g930",
        name: "Logitech G930",
        vid: 0x046D,
        pids: &[0x0A1F],
        page: 0xFF00,
        usage: 0x0001,
        interface: -1,
        query: logitech_g930,
    },
    Spec {
        key: "astro-a50",
        name: "ASTRO A50",
        vid: 0x046D,
        pids: &[0x0B1C],
        page: 0xFF32,
        usage: 0x0074,
        interface: -1,
        query: astro_a50,
    },
    // ---- SteelSeries Arctis ----
    Spec {
        key: "arctis-1",
        name: "SteelSeries Arctis 1 Wireless",
        vid: 0x1038,
        pids: &[0x12B3, 0x12B6, 0x12D7, 0x12D5],
        page: 0xFF43,
        usage: 0x0202,
        interface: 3,
        query: arctis_1,
    },
    Spec {
        key: "arctis-7",
        name: "SteelSeries Arctis 7 / Arctis Pro (2019)",
        vid: 0x1038,
        pids: &[0x1260, 0x12AD, 0x1252, 0x1280],
        page: 0,
        usage: 0,
        interface: 5,
        query: arctis_7,
    },
    Spec {
        key: "arctis-9",
        name: "SteelSeries Arctis 9",
        vid: 0x1038,
        pids: &[0x12C2],
        page: 0,
        usage: 0,
        interface: -1,
        query: arctis_9,
    },
    Spec {
        key: "arctis-pro-wireless",
        name: "SteelSeries Arctis Pro Wireless",
        vid: 0x1038,
        pids: &[0x1290],
        page: 0,
        usage: 0,
        interface: -1,
        query: arctis_pro_wireless,
    },
    Spec {
        key: "arctis-nova-pro-wireless",
        name: "SteelSeries Arctis Nova Pro Wireless",
        vid: 0x1038,
        pids: &[0x12E0, 0x12E5],
        page: 0,
        usage: 0,
        interface: 4,
        query: arctis_nova_pro,
    },
    Spec {
        key: "arctis-7-plus",
        name: "SteelSeries Arctis 7+",
        vid: 0x1038,
        pids: &[0x220E, 0x2212, 0x2216, 0x2236],
        page: 0xFFC0,
        usage: 0x0001,
        interface: 3,
        query: arctis_7_plus,
    },
    Spec {
        key: "arctis-nova-7",
        name: "SteelSeries Arctis Nova 7",
        vid: 0x1038,
        pids: &[
            0x2202, 0x22A1, 0x227E, 0x2206, 0x2258, 0x229E, 0x22AD, 0x223A, 0x22A9, 0x227A, 0x22A4, 0x22A5, 0x220A,
            0x22A7, 0x2298,
        ],
        page: 0xFFC0,
        usage: 0x0001,
        interface: 3,
        query: arctis_nova_7,
    },
    Spec {
        key: "arctis-nova-5",
        name: "SteelSeries Arctis Nova 5 / Nova 3P",
        vid: 0x1038,
        pids: &[0x2232, 0x2253, 0x2269, 0x226D],
        page: 0xFFC0,
        usage: 0x0001,
        interface: 3,
        query: arctis_nova_5,
    },
    Spec {
        key: "arctis-gamebuds",
        name: "SteelSeries Arctis GameBuds",
        vid: 0x1038,
        pids: &[0x230A],
        page: 0xFFC0,
        usage: 0x0001,
        interface: 3,
        query: arctis_gamebuds,
    },
    // ---- Corsair ----
    Spec {
        key: "corsair-void",
        name: "Corsair Void / HS70",
        vid: 0x1B1C,
        pids: &[
            0x1B1C, 0x1B27, 0x0A14, 0x0A16, 0x0A17, 0x0A1D, 0x0A1A, 0x1B2A, 0x1B23, 0x1B29, 0x0A55, 0x0A51, 0x0A52,
            0x0A38, 0x0A4F, 0x0A2B, 0x0A75, 0x0A56,
        ],
        page: 0xFFC5,
        usage: 0x0001,
        interface: 3,
        query: corsair_void,
    },
    Spec {
        key: "corsair-wireless-v2",
        name: "Corsair Void / Virtuoso / HS80 MAX Wireless",
        vid: 0x1B1C,
        pids: &[0x2A08, 0x2A02, 0x0A97],
        page: 0,
        usage: 0,
        interface: 4,
        query: corsair_wireless_v2,
    },
    // Receiver and cable IDs share a key: a Virtuoso XT on its cable leaves the receiver and shows up under the cable
    // ID, and the reading that reaches the headset wins (see poll)
    Spec {
        key: "corsair-virtuoso-xt",
        name: "Corsair Virtuoso XT / SE",
        vid: 0x1B1C,
        pids: &[0x0A64, 0x0A62, 0x0A3E, 0x0A3D],
        page: 0xFF42,
        usage: 0x0001,
        interface: 3,
        query: |io| {
            let wired = matches!(io.product_id(), 0x0A62 | 0x0A3D);
            bragi(io, wired)
        },
    },
    Spec {
        key: "corsair-hs80-rgb-wireless",
        name: "Corsair HS80 RGB Wireless",
        vid: 0x1B1C,
        pids: &[0x0A6B, 0x0A73, 0x0A69],
        page: 0xFF42,
        usage: 0x0001,
        interface: 3,
        query: |io| {
            let wired = io.product_id() == 0x0A69;
            bragi(io, wired)
        },
    },
    // ---- Others ----
    Spec {
        key: "sony-inzone-h5",
        name: "Sony INZONE H5",
        vid: 0x054C,
        pids: &[0x0EBF],
        page: 0xFF04,
        usage: 0x0002,
        interface: -1,
        query: sony_inzone_h5,
    },
    Spec {
        key: "mchose-x9",
        name: "MCHOSE X9",
        vid: 0x3837,
        pids: &[0x6045],
        page: 0xFF90,
        usage: 0x0001,
        interface: -1,
        query: mchose_x9,
    },
    Spec {
        key: "lenovo-wireless-voip",
        name: "Lenovo Wireless VoIP Headset",
        vid: 0x17EF,
        pids: &[0xA07D],
        page: 0xFF07,
        usage: 0x0222,
        interface: 3,
        query: lenovo_voip,
    },
];

/// Product IDs handled here, so the general Logitech HID++ provider leaves them alone.
pub fn handled(vid: u16, pid: u16) -> bool {
    SPECS.iter().any(|s| s.vid == vid && s.pids.contains(&pid))
}

fn hyperx_cloud_alpha(io: &mut Io) -> win::Result<State> {
    // Mute events 21 BB 23 arrive on the same collection: match the command echo
    let ask = |io: &mut Io, cmd: u8| {
        io.ask(&[0x21, 0xBB, cmd], |d| d.len() > 3 && d[0] == 0x21 && d[1] == 0xBB && d[2] == cmd, 4, false)
    };
    let Some(link) = ask(io, 0x03)? else { return Ok(off()) };
    if link[3] == 0x01 {
        return Ok(off());
    }
    let charging = ask(io, 0x0C)?.is_some_and(|c| c[3] == 0x01);
    Ok(match ask(io, 0x0B)? {
        Some(b) => on(b[3] as i32, charging),
        None => off(),
    })
}

fn hyperx_cloud_ii_hp(io: &mut Io) -> win::Result<State> {
    let ask = |io: &mut Io, cmd: u8| {
        io.ask(
            &pad(52, &[0x06, 0xFF, 0xBB, cmd]),
            |d| d.len() >= 8 && d[0] == 0x06 && d[1] == 0xFF && d[2] == 0xBB && d[3] == cmd,
            4,
            false,
        )
    };
    let Some(b) = ask(io, 0x02)? else { return Ok(off()) };
    let charging = ask(io, 0x03)?.is_some_and(|c| c[4] == 0x01);
    Ok(on(b[7] as i32, charging))
}

fn hyperx_cloud_ii_kingston(io: &mut Io) -> win::Result<State> {
    let ask = |io: &mut Io, cmd: u8| {
        io.get_input_report(0x06); // HeadsetControl wakes the receiver this way; the reply is ignored
        io.ask(
            &pad(62, &[0x06, 0x00, 0x02, 0x00, 0x9A, 0x00, 0x00, 0x68, 0x4A, 0x8E, 0x0A, 0, 0, 0, 0xBB, cmd]),
            |d| d.len() >= 8 && d[0] == 0x0B && d[2] == 0xBB && d[3] == cmd,
            4,
            false,
        )
    };
    let Some(b) = ask(io, 0x02)? else { return Ok(off()) };
    let charging = ask(io, 0x03)?.is_some_and(|c| c[4] == 0x01);
    Ok(on(b[7] as i32, charging))
}

fn hyperx_cloud_iii(io: &mut Io) -> win::Result<State> {
    // The dongle also sends the same reports by itself when something changes, with 0B / 0C / 0D in place of the
    // command: either one is the answer
    let ask = |io: &mut Io, cmd: u8, unasked: u8| {
        io.ask(&pad(62, &[0x66, cmd]), |d| d.len() > 4 && d[0] == 0x66 && (d[1] == cmd || d[1] == unasked), 4, true)
    };
    match ask(io, 0x82, 0x0B)? {
        Some(link) if link[2] == 0x01 => {}
        _ => return Ok(off()),
    }
    let power = ask(io, 0x8A, 0x0C)?.map_or(0, |c| c[2]); // 1 = charging, 2 = full with the cable still in
                                                          // The level counts only when one of the two bytes in front of it is set
    let level = ask(io, 0x89, 0x0D)?.filter(|b| (b[2] | b[3]) != 0).map(|b| (b[4] as i32).clamp(0, 100));
    Ok(State {
        online: true,
        level,
        charging: power == 1,
        detail: level.is_none().then(|| "d.levelUnknown".to_string()),
        full: power == 2,
    })
}

fn hyperx_cloud_flight(io: &mut Io) -> win::Result<State> {
    // 64 xx (power) and 65 xx (mute) are events, not the reply
    let Some(r) = io.ask(&pad(20, &[0x21, 0xFF, 0x05]), |d| d.len() >= 5 && !matches!(d[0], 0x64 | 0x65), 4, false)?
    else {
        return Ok(off());
    };
    let mv = (r[3] as i32) << 8 | r[4] as i32;
    if mv > 4107 {
        return Ok(State::new(true, None, true)); // charging: the voltage says nothing then
    }
    if mv < 3000 {
        return Ok(off());
    }
    let v = mv as f64;
    let percent = if mv <= 3648 {
        0.00125 * v
    } else if mv > 3975 {
        100.0
    } else {
        2.547505e-8 * v.powi(4) - 3.900299e-4 * v.powi(3) + 2.238321 * v * v - 5706.256 * v + 5452299.0
    };
    Ok(on(percent.round_ties_even() as i32, false))
}

fn logitech_g930(io: &mut Io) -> win::Result<State> {
    if !io.set_feature(&pad(64, &[0xFF, 0x09, 0x00, 0xFD, 0xF4, 0x10, 0x05, 0xB1, 0xBF, 0xA0, 0x04])) {
        return Ok(off());
    }
    let mut r = None;
    for _ in 0..3 {
        sleep_ms(100);
        r = io.get_feature(0xFF).or(r);
    }
    Ok(match r {
        Some(r) if r.len() > 13 => on(map(r[13] as i32, 44, 91, 0, 100), false),
        _ => off(),
    })
}

fn astro_a50(io: &mut Io) -> win::Result<State> {
    let r = io.ask(
        &pad(64, &[0x02, 0x0C, 0x03, 0x00, 0x06, 0x0C]),
        |d| d.len() > 8 && d[0] == 0x02 && d[1] == 0x0C && d[4] == 0x06,
        8,
        false,
    )?;
    Ok(r.map_or_else(off, |r| on(r[6] as i32, r[8] != 0)))
}

fn arctis_1(io: &mut Io) -> win::Result<State> {
    Ok(match io.ask(&[0x06, 0x12], |d| d.len() > 3, 4, false)? {
        Some(r) if r[2] != 0x01 => on(r[3] as i32, false),
        _ => off(),
    })
}

fn arctis_7(io: &mut Io) -> win::Result<State> {
    Ok(io.ask(&[0x06, 0x18], |d| d.len() > 2, 4, false)?.map_or_else(off, |r| on(r[2] as i32, false)))
}

fn arctis_9(io: &mut Io) -> win::Result<State> {
    Ok(match io.ask(&[0x00, 0x20], |d| d.len() > 4, 4, false)? {
        Some(r) if r[4] != 0xFF => on(map(r[3] as i32, 0x64, 0x9A, 0, 100), r[4] == 0x01),
        _ => off(),
    })
}

fn arctis_pro_wireless(io: &mut Io) -> win::Result<State> {
    match io.ask(&pad(31, &[0x41, 0xAA]), |d| !d.is_empty(), 4, false)? {
        Some(link) if link[0] != 0x02 => {}
        _ => return Ok(off()),
    }
    Ok(io
        .ask(&pad(31, &[0x40, 0xAA]), |d| !d.is_empty(), 4, false)?
        .map_or_else(off, |r| on(map(r[0] as i32, 0, 4, 0, 100), false)))
}

fn arctis_nova_pro(io: &mut Io) -> win::Result<State> {
    // The base station also streams volume / chat-mix / EQ frames: take the B0 status reply only
    Ok(match io.ask(&pad(31, &[0x06, 0xB0]), |d| d.len() >= 16 && d[1] == 0xB0, 4, false)? {
        Some(r) if r[15] != 0x01 => on(map(r[6] as i32, 0, 8, 0, 100), r[15] == 0x02),
        _ => off(),
    })
}

fn arctis_7_plus(io: &mut Io) -> win::Result<State> {
    Ok(match nova_status(io)? {
        Some(r) if r[1] != 0x01 => on(map(r[2] as i32, 0, 4, 0, 100), r[3] == 0x01),
        _ => off(),
    })
}

fn arctis_nova_7(io: &mut Io) -> win::Result<State> {
    let pid = io.product_id();
    Ok(match nova_status(io)? {
        Some(r) if r[3] != 0x00 => {
            // Older firmware reports five steps (0-4), newer ones a percentage
            let steps = matches!(pid, 0x2202 | 0x2206 | 0x220A | 0x223A | 0x227A | 0x22A4);
            on(if steps { map(r[2] as i32, 0, 4, 0, 100) } else { r[2] as i32 }, matches!(r[3], 0x01 | 0x02))
        }
        _ => off(),
    })
}

fn arctis_nova_5(io: &mut Io) -> win::Result<State> {
    Ok(match nova_status(io)? {
        Some(r) if r[1] != 0x02 => on(r[3] as i32, r[4] == 0x01),
        _ => off(),
    })
}

fn arctis_gamebuds(io: &mut Io) -> win::Result<State> {
    let Some(r) = nova_status(io)?.filter(|r| r.len() >= 7) else { return Ok(off()) };
    // 03 = out of the case; a bud in the case (02) reports 0% and no charging state
    let lowest = [(r[3], r[5]), (r[4], r[6])].iter().filter(|b| b.0 == 0x03).map(|b| b.1 as i32).min();
    Ok(lowest.map_or_else(off, |level| on(level, false)))
}

fn corsair_void(io: &mut Io) -> win::Result<State> {
    Ok(match io.ask(&[0xC9, 0x64], |d| d.len() > 4 && d[0] == 0x64, 4, false)? {
        Some(r) if r[4] != 0 => on((r[2] & 0x7F) as i32, matches!(r[4], 4 | 5)),
        _ => off(),
    })
}

fn corsair_wireless_v2(io: &mut Io) -> win::Result<State> {
    // Read-only "get" requests: 00 02 <endpoint: 08 receiver, 09 headset> 02 <property>
    let get = |endpoint: u8, property: u8| pad(65, &[0x00, 0x02, endpoint, 0x02, property]);
    io.send(&get(0x08, 0x13))?;
    io.send(&get(0x08, 0x12))?;
    io.drain()?;
    if io.ask(&get(0x09, 0x12), |d| !d.is_empty(), 4, false)?.is_none() {
        return Ok(off());
    }
    io.drain()?;
    for _ in 0..3 {
        let Some(r) = io.ask(&get(0x09, 0x0F), |d| d.len() > 5, 4, false)? else { return Ok(off()) };
        let value = r[4] as i32 | (r[5] as i32) << 8;
        if (1..=1000).contains(&value) {
            return Ok(on(value / 10, false));
        }
    }
    Ok(off())
}

fn sony_inzone_h5(io: &mut Io) -> win::Result<State> {
    let mut request = pad(64, &[0x02, 0x0C, 0x01, 0x00, 0xFC, 0x08, 0x96, 0xC3, 0x41, 0x04, 0x01, 0x02, 0x00]);
    request[13] = request[6..=12].iter().fold(0u8, |sum, &b| sum.wrapping_add(b));
    // Our transaction ID (02 00) and a return / active-notification type, so unsolicited events don't count
    let r = io.ask(
        &request,
        |d| {
            d.len() > 14
                && d[0] == 0x02
                && d[1] >= 14
                && d[2] == 0x04
                && d[3] == 0xFF
                && d[6] == 0x96
                && d[7] == 0xC3
                && (d[8] >> 4) == 1
                && d[9] == 0x04
                && matches!(d[10], 0x10 | 0xA0)
                && d[11] == 0x02
                && d[12] == 0x00
        },
        10,
        false,
    )?;
    Ok(match r {
        Some(r) if r[14] != 0xFF => on(r[14] as i32, r[13] != 0),
        _ => off(),
    })
}

fn mchose_x9(io: &mut Io) -> win::Result<State> {
    let r = io.ask(
        &pad(64, &[0x55, 0x65, 0x01]),
        |d| d.len() > 3 && d[0] == 0x55 && d[1] == 0x65 && d[3] == 0x02,
        32,
        false,
    )?;
    Ok(r.map_or_else(off, |r| on(r[2] as i32, false)))
}

fn lenovo_voip(io: &mut Io) -> win::Result<State> {
    if !io.set_feature(&pad(61, &[0x24, 0x01])) {
        return Ok(off());
    }
    Ok(match io.receive(|d| d.len() > 7 && d[0] == 0x27 && d[1] == 0x01, 4)? {
        Some(r) if r[2] == 0 => on(r[7] as i32, false),
        _ => off(),
    })
}

/// Status reports (B0) can also arrive unasked on this interface; other replies start with 20.
fn nova_status(io: &mut Io) -> win::Result<Option<Vec<u8>>> {
    io.ask(&[0x00, 0xB0], |d| d.len() > 4 && d[0] == 0xB0, 4, false)
}

// Corsair Bragi: who a request goes to, and the properties read here
const BRAGI_SELF: u8 = 0x08;
const BRAGI_HEADSET: u8 = 0x09;
const BRAGI_LEVEL: u8 = 0x0F;
const BRAGI_POWER: u8 = 0x10;

/// Corsair's Bragi property protocol on report 02 of the FF42 collection, as the Virtuoso XT / SE and the HS80 RGB
/// Wireless speak it. `02 TT 02 PP` reads property PP of target TT; the reply comes on report 01 as
/// `01 FF 02 SS VV VV VV VV`: FF who answered (01 the headset behind a receiver, 00 the device itself), 02 the echo,
/// SS the status (00 ok, 05 no such property), then the value, little-endian. A receiver relays to its headset on
/// target 09; a headset on its cable answers on 08, except the Virtuoso SE, which takes 09 there too and refuses 08.
/// Read-only: properties are only read, the headset stays in hardware mode.
fn bragi(io: &mut Io, wired: bool) -> win::Result<State> {
    let targets: &[u8] = if wired { &[BRAGI_SELF, BRAGI_HEADSET] } else { &[BRAGI_HEADSET] };
    for &target in targets {
        // A receiver without its headset is silent or answers "no such property" for itself
        let Some(level) = bragi_get(io, target, BRAGI_LEVEL)? else { continue };
        let power = bragi_get(io, target, BRAGI_POWER)?;
        return Ok(bragi_state(level, power));
    }
    Ok(off())
}

fn bragi_get(io: &mut Io, target: u8, property: u8) -> win::Result<Option<u32>> {
    Ok(io
        .ask(&pad(64, &[0x02, target, 0x02, property]), |d| is_bragi_reply(d, target), 8, false)?
        .and_then(|r| bragi_value(&r)))
}

/// A reply to a read sent to `target`; volume events on report 0E and what a receiver answers for itself don't count.
pub fn is_bragi_reply(d: &[u8], target: u8) -> bool {
    d.len() >= 8 && d[0] == 0x01 && d[1] == if target == BRAGI_HEADSET { 0x01 } else { 0x00 } && d[2] == 0x02
}

/// The value of a reply, or None when the status isn't ok.
pub fn bragi_value(d: &[u8]) -> Option<u32> {
    (d[3] == 0x00).then(|| u32::from_le_bytes([d[4], d[5], d[6], d[7]]))
}

/// `level`: tenths of a percent, 0-1000. `power`: charge state, if the firmware has it: 1 charging, 2 on battery,
/// 3 charged (ckb-next's BatteryStatus).
pub fn bragi_state(level: u32, power: Option<u32>) -> State {
    let (charging, full) = (power == Some(1), power == Some(3));
    if level <= 1000 {
        State { full, ..State::new(true, Some(level as i32 / 10), charging) }
    } else {
        State { detail: Some("d.levelUnknown".into()), full, ..State::new(true, None, charging) }
    }
}

fn logitech_voltage(io: &mut Io, feature_index: u8, function: u8, table: &[(i32, i32)]) -> win::Result<State> {
    let r = io.ask(
        &pad(20, &[0x11, 0xFF, feature_index, function]),
        |d| {
            d.len() >= 7
                && d[0] == 0x11
                && d[1] == 0xFF
                && ((d[2] == feature_index && d[3] == function) || d[2] == 0xFF)
        },
        4,
        false,
    )?;
    let Some(r) = r.filter(|r| r[2] != 0xFF) else { return Ok(off()) }; // HID++ error: headset off
    let mv = (r[4] as i32) << 8 | r[5] as i32;
    if mv < table[table.len() - 1].0 {
        return Ok(off());
    }
    Ok(on(interpolate(mv, table), r[6] == 0x03))
}

fn interpolate(mv: i32, table: &[(i32, i32)]) -> i32 {
    if mv >= table[0].0 {
        return 100;
    }
    for i in 1..table.len() {
        if mv < table[i].0 {
            continue;
        }
        let (hi_mv, hi_p) = table[i - 1];
        let (lo_mv, lo_p) = table[i];
        return lo_p + (mv - lo_mv) * (hi_p - lo_p) / (hi_mv - lo_mv);
    }
    0
}

/// Clamps v to a..b and scales it to c..d (integer arithmetic, as HeadsetControl's map()).
fn map(v: i32, a: i32, b: i32, c: i32, d: i32) -> i32 {
    let v = v.clamp(a, b);
    c + (v - a) * (d - c) / (b - a)
}

#[derive(Default)]
pub struct HeadsetProvider {
    /// What this session's log already has: see `log_once`.
    logged: HashSet<String>,
}

impl Provider for HeadsetProvider {
    fn name(&self) -> &'static str {
        "Headsets"
    }

    fn poll(&mut self) -> Vec<DeviceReading> {
        let mut result = Vec::new();
        let found = hid::enumerate(handled);
        for ((vid, pid), collections) in group_by(found, |i| (i.vendor_id, i.product_id)) {
            let Some(spec) = SPECS.iter().find(|s| s.vid == vid && s.pids.contains(&pid)) else { continue };
            let mut asked = Vec::new();
            let state = query(spec, &collections, &mut asked);
            self.log_once(spec, &collections, &asked, state.online);
            result.push(DeviceReading {
                online: state.online,
                level: state.level,
                charging: state.charging,
                full_on_charger: state.full,
                detail: state.detail.as_deref().map(t),
                ..DeviceReading::new(format!("headset:{}", spec.key), spec.name, DeviceKind::Headset, t("via.24"))
            });
        }
        one_per_id(result, |r| r.online)
    }

    fn delay(&self, last: &[DeviceReading]) -> Duration {
        if last.iter().any(|r| r.online) {
            secs(60)
        } else {
            secs(30)
        }
    }
}

impl HeadsetProvider {
    /// Logs how a headset was queried - its HID collections, the requests and the replies - the first time it is
    /// found off and the first time it is found on in a session.
    fn log_once(&mut self, spec: &Spec, collections: &[HidInfo], asked: &[(String, Vec<String>)], online: bool) {
        let id = format!("{:04x}:{:04x}", collections[0].vendor_id, collections[0].product_id);
        if !self.logged.insert(format!("{id}{online}")) {
            return;
        }
        log::write(&format!(
            "{} {id}: found {}, {} HID collections",
            spec.name,
            if online { "on" } else { "off" },
            collections.len()
        ));
        for info in collections {
            let trace =
                asked.iter().find(|a| a.0 == info.path).map(|a| a.1.join("  ")).unwrap_or_else(|| "not asked".into());
            log::write(&format!("  mi={} {info}: {trace}", info.interface_number()));
        }
    }
}

/// `asked` is filled with the collections that were tried and what was sent to and received from each.
fn query(spec: &Spec, collections: &[HidInfo], asked: &mut Vec<(String, Vec<String>)>) -> State {
    let mut candidates: Vec<&HidInfo> = collections
        .iter()
        .filter(|i| spec.interface < 0 || i.interface_number().max(0) == spec.interface)
        .filter(|i| {
            i.access_denied
                || if spec.page == 0 {
                    i.usage_page >= 0xFF00 || spec.interface >= 0
                } else {
                    i.usage_page == spec.page && i.usage == spec.usage
                }
        })
        .collect();
    candidates.sort_by_key(|i| if i.output_length > 0 { 0 } else { 1 });
    if candidates.is_empty() {
        return off_because("d.noLink");
    }
    if candidates.iter().all(|i| i.access_denied) {
        return off_because("d.needsAdmin");
    }

    // A clean "headset off" from one collection beats an error from another that can't take the request
    let mut clean = None;
    let mut error = off();
    for info in candidates.into_iter().filter(|i| !i.access_denied) {
        let mut trace = Vec::new();
        let result = HidDevice::open(info).and_then(|dev| (spec.query)(&mut Io { dev, trace: &mut trace }));
        match result {
            Ok(state) if state.online => {
                asked.push((info.path.clone(), trace));
                return state;
            }
            Ok(state) => {
                clean.get_or_insert(state);
            }
            Err(e) => {
                trace.push(format!("error: {e}"));
                error = off_because(tf("d.error", &[&e]));
            }
        }
        asked.push((info.path.clone(), trace));
    }
    clean.unwrap_or(error)
}

/// Request/response helpers over one HID collection, with hidapi's view of input reports.
pub struct Io<'a> {
    dev: HidDevice,
    trace: &'a mut Vec<String>, // what was sent and received, for log_once
}

impl Io<'_> {
    const ERROR_INVALID_FUNCTION: u32 = 1;

    fn product_id(&self) -> u16 {
        self.dev.info.product_id
    }

    /// An input report as hidapi returns it: without the 0x00 Windows adds when there are no report IDs.
    pub fn data(raw: &[u8]) -> &[u8] {
        if raw.first() == Some(&0x00) {
            &raw[1..]
        } else {
            raw
        }
    }

    fn send(&mut self, report: &[u8]) -> win::Result<bool> {
        self.trace.push(format!("> {}", hex(report)));
        let sent = self.dev.write(report, TIMEOUT_MS as u32)?;
        if !sent {
            self.trace.push("(not sent)".into());
        }
        Ok(sent)
    }

    /// Sends a request and returns the first input report that `matches` accepts. `or_feature`: send the request as
    /// a feature report when the collection doesn't take it as an output one - HyperHeadset found HyperX dongles that
    /// take theirs only that way on Windows.
    fn ask(
        &mut self,
        request: &[u8],
        matches: impl Fn(&[u8]) -> bool,
        reads: usize,
        or_feature: bool,
    ) -> win::Result<Option<Vec<u8>>> {
        self.dev.flush();
        let sent = match self.send(request) {
            Ok(sent) => sent,
            // What Windows answers to an output report the collection doesn't have
            Err(Error { code: Some(Self::ERROR_INVALID_FUNCTION), .. }) if or_feature => false,
            Err(e) => return Err(e),
        };
        if sent || (or_feature && self.set_feature(request)) {
            self.receive(matches, reads)
        } else {
            Ok(None)
        }
    }

    fn receive(&mut self, matches: impl Fn(&[u8]) -> bool, reads: usize) -> win::Result<Option<Vec<u8>>> {
        let deadline = Instant::now() + Duration::from_millis(TIMEOUT_MS);
        for _ in 0..reads {
            if Instant::now() >= deadline {
                break;
            }
            let Some(raw) = self.dev.read(remaining_ms(deadline))? else {
                self.trace.push("< nothing".into());
                return Ok(None);
            };
            let data = Self::data(&raw);
            self.trace.push(format!("< {}", hex(data)));
            if matches(data) {
                return Ok(Some(data.to_vec()));
            }
        }
        Ok(None)
    }

    /// Throws away whatever the device sent unasked.
    fn drain(&mut self) -> win::Result<()> {
        for _ in 0..16 {
            if self.dev.read(5)?.is_none() {
                break;
            }
        }
        Ok(())
    }

    fn set_feature(&mut self, report: &[u8]) -> bool {
        // Exactly the length the device declares, as hidapi does
        let length = if self.dev.info.feature_length > 0 { self.dev.info.feature_length } else { report.len() };
        let mut buf = vec![0u8; length];
        let n = report.len().min(length);
        buf[..n].copy_from_slice(&report[..n]);
        self.trace.push(format!("feature > {}", hex(&buf)));
        let taken = self.dev.set_feature(&buf);
        if !taken {
            self.trace.push("(not taken)".into());
        }
        taken
    }

    fn get_feature(&mut self, report_id: u8) -> Option<Vec<u8>> {
        if self.dev.info.feature_length == 0 {
            return None;
        }
        let r = self.dev.get_feature(report_id);
        self.traced("feature < ", r)
    }

    fn get_input_report(&mut self, report_id: u8) -> Option<Vec<u8>> {
        if self.dev.info.input_length == 0 {
            return None;
        }
        let r = self.dev.get_input_report(report_id);
        self.traced("input < ", r)
    }

    fn traced(&mut self, what: &str, report: Option<Vec<u8>>) -> Option<Vec<u8>> {
        self.trace.push(format!("{what}{}", report.as_deref().map(hex).unwrap_or_else(|| "nothing".into())));
        report
    }
}

#[cfg(test)]
mod tests {
    //! Corsair's Bragi property replies as the Virtuoso XT / SE and the HS80 RGB Wireless send them on report 01:
    //! `01 FF 02 SS VV VV VV VV`, with the layout from HeadsetControl (corsair_virtuoso_xt.hpp) and OpenLinkHub. The
    //! report ID comes first, so nothing is stripped in front of it.
    use super::*;

    const RECEIVER: u8 = 0x09;
    const SELF: u8 = 0x08;

    fn reply(data: &[u8]) -> Vec<u8> {
        pad(64, data)
    }

    fn state(online: bool, level: Option<i32>, charging: bool) -> State {
        State::new(online, level, charging)
    }

    #[test]
    fn headset_behind_the_receiver_answers() {
        // 87.3% = 873 = 0x0369
        let r = reply(&[0x01, 0x01, 0x02, 0x00, 0x69, 0x03]);
        assert!(is_bragi_reply(&r, RECEIVER));
        assert_eq!(bragi_value(&r), Some(873));
    }

    #[test]
    fn headset_on_its_cable_answers_for_itself() {
        let r = reply(&[0x01, 0x00, 0x02, 0x00, 0xE8, 0x03]);
        assert!(is_bragi_reply(&r, SELF));
        assert!(!is_bragi_reply(&r, RECEIVER));
        assert_eq!(bragi_value(&r), Some(1000));
    }

    #[test]
    fn receiver_answering_for_itself_is_not_the_headset() {
        assert!(!is_bragi_reply(&reply(&[0x01, 0x00, 0x02, 0x00, 0x1C, 0x1B]), RECEIVER));
    }

    #[test]
    fn no_such_property_is_no_value() {
        assert_eq!(bragi_value(&reply(&[0x01, 0x01, 0x02, 0x05])), None);
    }

    #[test]
    fn other_reports_are_not_the_reply() {
        assert!(!is_bragi_reply(&reply(&[0x0E, 0x01]), RECEIVER)); // volume event
        assert!(!is_bragi_reply(&reply(&[0x01, 0x01, 0x01, 0x00]), RECEIVER)); // reply to a write
        assert!(!is_bragi_reply(&[0x01, 0x01, 0x02, 0x00], RECEIVER)); // too short
    }

    #[test]
    fn level_is_in_tenths_of_a_percent() {
        assert_eq!(bragi_state(873, Some(2)), state(true, Some(87), false));
        assert_eq!(bragi_state(9, Some(2)), state(true, Some(0), false));
        assert_eq!(bragi_state(1000, None), state(true, Some(100), false));
    }

    #[test]
    fn charging_and_charged() {
        assert_eq!(bragi_state(540, Some(1)), state(true, Some(54), true));
        assert_eq!(bragi_state(1000, Some(3)), State { full: true, ..state(true, Some(100), false) });
    }

    #[test]
    fn level_out_of_range_is_not_known() {
        assert_eq!(
            bragi_state(0x0A69, Some(1)),
            State { detail: Some("d.levelUnknown".into()), ..state(true, None, true) }
        );
    }

    #[test]
    fn logitech_voltage_tables() {
        assert_eq!(interpolate(4200, G533), 100);
        assert_eq!(interpolate(3850, G533), 50);
        assert_eq!(interpolate(3820, G533), 40);
        assert_eq!(interpolate(3000, G533), 0);
        assert_eq!(map(2, 0, 4, 0, 100), 50);
        assert_eq!(map(0xA0, 0x64, 0x9A, 0, 100), 100);
    }
}

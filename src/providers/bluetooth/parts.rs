//! Devices whose battery comes in parts, worked out from what they send, without the system: the earbuds and the case
//! of AirPods and Beats, from the proximity pairing message they advertise to the Apple devices around them, and the
//! halves of a split keyboard, from the Battery Services it has one of per half. Only Windows reads these (the Mac
//! has the earbuds' levels from macOS itself); the Mac compiles this for its tests too.
//!
//! The proximity pairing message (Apple Continuity, type 0x07, 25 bytes): a prefix byte, the model (the product ID,
//! little-endian), a status byte, the two earbuds' levels in a byte, the case's level and the charging bits in
//! another, then the lid, the color and an encrypted part. The earbuds aren't named left and right in it: it carries
//! the level of the earbud that sends it and the other one's, and status bit 5 says whether the sender is the left
//! one. Levels are in tens of percent, 0-10; 15 is "not known" (the case's, while the earbuds are out of it).
//! Protocol facts from AirPodsDesktop (SpriteOvO, Source/Core/AppleCP.h and AppleCP.cpp, its model table, and the
//! captured messages in Tests/AirPodsDomainTests.cpp) and furiousMAC's continuity (messages/proximity_pairing.md);
//! the Beats earbuds' product IDs from AppleDB (their "Device1,N" identifiers); own implementation.

use crate::loc::tf;
use std::time::{Duration, Instant};

/// Earbuds with a case, by the product IDs their paired device and their messages carry. Two IDs in one row are one
/// model, as AirPodsDesktop counts them. Over-ear headphones and neckbands have one battery, which the system's own
/// level already is, so they aren't here.
const EARBUDS: &[&[u16]] = &[
    &[0x2002],         // AirPods
    &[0x200F],         // AirPods (2nd generation)
    &[0x2013],         // AirPods (3rd generation)
    &[0x2019],         // AirPods 4
    &[0x201B],         // AirPods 4 with Active Noise Cancellation
    &[0x2030, 0x2036], // AirPods 5
    &[0x200E],         // AirPods Pro
    &[0x2014],         // AirPods Pro 2
    &[0x2024],         // AirPods Pro 2 (USB-C)
    &[0x2027],         // AirPods Pro 3
    &[0x200B],         // Powerbeats Pro
    &[0x201D],         // Powerbeats Pro 2
    &[0x202F],         // Powerbeats Fit
    &[0x2011],         // Beats Studio Buds
    &[0x2016],         // Beats Studio Buds +
    &[0x2012],         // Beats Fit Pro
    &[0x2026],         // Beats Solo Buds
];

/// A message counts this long after it was heard. Earbuds out of their case advertise far more often, so a message
/// this old is from earbuds that have gone quiet or away.
pub const RECENT: Duration = Duration::from_secs(30);

/// Weaker messages are left alone: AirPodsDesktop's limit for telling the earbuds in your ears from a stranger's. It
/// also keeps out the messages Windows has no signal strength for (-127).
const WEAKEST: i16 = -80;

/// What a pair of earbuds says about itself, in percent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Earbuds {
    pub left: Option<u8>,
    pub right: Option<u8>,
    pub case: Option<u8>,
    pub left_charging: bool,
    pub right_charging: bool,
}

impl Earbuds {
    /// The level of the earbud that runs out first.
    pub fn level(&self) -> Option<i32> {
        match (self.left, self.right) {
            (Some(l), Some(r)) => Some(l.min(r) as i32),
            (one, other) => one.or(other).map(i32::from),
        }
    }

    /// Charging when every earbud with a level is: a case charging, or one earbud in it, isn't the earbuds charging.
    pub fn charging(&self) -> bool {
        let charging = [(self.left, self.left_charging), (self.right, self.right_charging)];
        let known: Vec<bool> = charging.iter().filter(|(level, _)| level.is_some()).map(|&(_, c)| c).collect();
        !known.is_empty() && known.iter().all(|&c| c)
    }

    /// The earbuds and the case, each with its level, as the Mac shows them.
    pub fn detail(&self) -> Option<String> {
        let parts: Vec<String> = [("d.budLeft", self.left), ("d.budRight", self.right), ("d.case", self.case)]
            .into_iter()
            .filter_map(|(key, level)| level.map(|l| tf(key, &[&l])))
            .collect();
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

/// The model and the levels in Apple's manufacturer data (what follows the company ID): Continuity messages of a
/// type byte and a length byte each, among which the proximity pairing one. None when there is none, or it isn't
/// of the length AirPods send.
pub fn proximity_pairing(data: &[u8]) -> Option<(u16, Earbuds)> {
    let mut rest = data;
    while let [kind, length, tail @ ..] = rest {
        let length = *length as usize;
        if tail.len() < length {
            return None;
        }
        let (message, next) = tail.split_at(length);
        if *kind == 0x07 && length == 25 {
            return Some(earbuds_of(message));
        }
        rest = next;
    }
    None
}

/// A proximity pairing message after its type and length bytes.
fn earbuds_of(message: &[u8]) -> (u16, Earbuds) {
    let model = u16::from_le_bytes([message[1], message[2]]);
    let from_left = message[3] & 0x20 != 0;
    let tens = |nibble: u8| (nibble <= 10).then_some(nibble * 10);
    // The sender's level in the low half, the other earbud's in the high half
    let (sender, other) = (tens(message[4] & 0x0F), tens(message[4] >> 4));
    // The case's level in the low half; bit 4: the sender charges, bit 5: the other earbud, bit 6: the case
    let (sender_charging, other_charging) = (message[5] & 0x10 != 0, message[5] & 0x20 != 0);
    let case = tens(message[5] & 0x0F);
    let earbuds = if from_left {
        Earbuds { left: sender, right: other, case, left_charging: sender_charging, right_charging: other_charging }
    } else {
        Earbuds { left: other, right: sender, case, left_charging: other_charging, right_charging: sender_charging }
    };
    (model, earbuds)
}

fn earbuds_index(id: u16) -> Option<usize> {
    EARBUDS.iter().position(|ids| ids.contains(&id))
}

/// Whether a product ID is of earbuds with a case.
pub fn is_earbuds(product: u16) -> bool {
    earbuds_index(product).is_some()
}

/// Whether a paired device and a message are of one model of earbuds with a case.
pub fn same_earbuds(product: u16, model: u16) -> bool {
    earbuds_index(product).is_some_and(|i| earbuds_index(model) == Some(i))
}

/// A proximity pairing message as it was heard.
#[derive(Clone, Copy, Debug)]
pub struct Heard {
    /// The sender's address: an earbud's own, which changes every few minutes.
    pub address: u64,
    pub model: u16,
    /// dBm
    pub signal: i16,
    pub at: Instant,
    pub earbuds: Earbuds,
}

/// Keeps the latest message of each sender, and only the recent ones.
pub fn remember(heard: &mut Vec<Heard>, new: Heard) {
    heard.retain(|h| h.address != new.address && new.at.saturating_duration_since(h.at) <= RECENT);
    heard.push(new);
}

/// The levels of the paired earbuds `product`: the recent message of their model that came in strongest, as the
/// earbuds in the user's ears are the nearest. None when no such earbuds were heard, so the earbuds of other people
/// around are never listed: they could only show in place of the user's own of the same model, when those are quiet.
pub fn earbuds_for(heard: &[Heard], product: u16, now: Instant) -> Option<Earbuds> {
    heard
        .iter()
        .filter(|h| same_earbuds(product, h.model) && h.signal >= WEAKEST)
        .filter(|h| now.saturating_duration_since(h.at) <= RECENT)
        .filter(|h| h.earbuds.level().is_some())
        .max_by_key(|h| h.signal)
        .map(|h| h.earbuds)
}

/// A device's level from its Battery Level characteristics, each a (name, level): the lowest, and with several,
/// each part with its level. The name is the characteristic's user description, which ZMK gives the battery of each
/// half that reaches the computer through the other one; the battery of the device itself has none.
pub fn of_parts(parts: &[(String, u8)]) -> Option<(i32, Option<String>)> {
    let parts: Vec<(&str, u8)> = parts
        .iter()
        .filter(|(_, level)| *level <= 100)
        .map(|(name, level)| (name.trim_matches(|c: char| c.is_whitespace() || c.is_control()), *level))
        .collect();
    let lowest = parts.iter().map(|&(_, level)| level).min()?;
    let detail = (parts.len() > 1).then(|| {
        let named = |&(name, level): &(&str, u8)| {
            if name.is_empty() {
                tf("d.mainPart", &[&level])
            } else {
                format!("{name} {level}%")
            }
        };
        parts.iter().map(named).collect::<Vec<_>>().join(", ")
    });
    Some((lowest as i32, detail))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A captured message with its encrypted part zeroed, as AirPodsDesktop's tests keep them.
    fn captured(head: &[u8]) -> Vec<u8> {
        let mut message = head.to_vec();
        message.resize(27, 0);
        message
    }

    #[test]
    fn airpods_5_from_either_earbud() {
        // AirPods 5 in their case, lid open, both charging, the case full: sent by the left earbud (status 0x35, bit 5
        // set), then by the right one (0x55)
        for status in [0x35, 0x55] {
            let message = captured(&[0x07, 0x19, 0x01, 0x30, 0x20, status, 0xaa, 0xba, 0x32, 0x00, 0x05]);
            let (model, earbuds) = proximity_pairing(&message).unwrap();
            assert_eq!(model, 0x2030);
            assert_eq!(
                earbuds,
                Earbuds {
                    left: Some(100),
                    right: Some(100),
                    case: Some(100),
                    left_charging: true,
                    right_charging: true
                }
            );
            assert!(earbuds.charging());
        }
    }

    #[test]
    fn airpods_pro_2_in_the_ears() {
        // AirPods Pro 2 (USB-C) in both ears, 80% each, the case closed away (its level unknown): from the left earbud
        // (0x2b) and from the right one (0x0b)
        for status in [0x2b, 0x0b] {
            let message = captured(&[0x07, 0x19, 0x01, 0x24, 0x20, status, 0x88, 0x8f, 0x11, 0x00, 0x05]);
            let (model, earbuds) = proximity_pairing(&message).unwrap();
            assert_eq!(model, 0x2024);
            assert_eq!(earbuds, Earbuds { left: Some(80), right: Some(80), ..Default::default() });
            assert_eq!(earbuds.level(), Some(80));
            assert!(!earbuds.charging());
            assert_eq!(earbuds.detail().as_deref(), Some("left 80%, right 80%"));
        }
        // Bit 4: the sender charges - the left earbud when it sends
        let message = captured(&[0x07, 0x19, 0x01, 0x24, 0x20, 0x2b, 0x88, 0x9f, 0x11, 0x00, 0x05]);
        let earbuds = proximity_pairing(&message).unwrap().1;
        assert!(earbuds.left_charging && !earbuds.right_charging && !earbuds.charging());
    }

    #[test]
    fn the_sender_decides_which_level_is_whose() {
        // AirPodsDesktop's test messages: left 80%, right 70%, case 50%, from the left earbud and from the right one
        let from_left = captured(&[0x07, 0x19, 0x00, 0x14, 0x20, 0x2e, 0x78, 0x05, 0x00]);
        let from_right = captured(&[0x07, 0x19, 0x00, 0x14, 0x20, 0x0e, 0x87, 0x05, 0x00]);
        for message in [from_left, from_right] {
            let (model, earbuds) = proximity_pairing(&message).unwrap();
            assert_eq!(model, 0x2014);
            assert_eq!((earbuds.left, earbuds.right, earbuds.case), (Some(80), Some(70), Some(50)));
            assert_eq!(earbuds.level(), Some(70));
            assert_eq!(earbuds.detail().as_deref(), Some("left 80%, right 70%, case 50%"));
        }
    }

    #[test]
    fn other_messages_are_not_earbuds() {
        let airpods = captured(&[0x07, 0x19, 0x01, 0x24, 0x20, 0x2b, 0x88, 0x8f]);
        assert!(proximity_pairing(&[]).is_none());
        assert!(proximity_pairing(&airpods[..20]).is_none(), "cut short");
        let mut airdrop = airpods.clone();
        airdrop[0] = 0x05;
        assert!(proximity_pairing(&airdrop).is_none());
        let mut shorter = airpods.clone();
        shorter[1] = 0x18;
        assert!(proximity_pairing(&shorter).is_none());
        // After another message (nearby info) in the same manufacturer data
        let mut after = vec![0x10, 0x05, 0x01, 0x18, 0x00, 0x00, 0x00];
        after.extend_from_slice(&airpods);
        assert_eq!(proximity_pairing(&after).map(|(model, _)| model), Some(0x2024));
        // An earbud that isn't out: one level, which is then the level
        let one = captured(&[0x07, 0x19, 0x01, 0x14, 0x20, 0x2b, 0xf6, 0x0f]);
        let earbuds = proximity_pairing(&one).unwrap().1;
        assert_eq!((earbuds.left, earbuds.right, earbuds.level()), (Some(60), None, Some(60)));
        assert_eq!(earbuds.detail().as_deref(), Some("left 60%"));
        assert_eq!(
            proximity_pairing(&captured(&[0x07, 0x19, 0x01, 0x14, 0x20, 0x2b, 0xff, 0x0f])).unwrap().1.level(),
            None
        );
    }

    fn heard(address: u64, model: u16, signal: i16, at: Instant, left: u8) -> Heard {
        let earbuds = Earbuds { left: Some(left), right: Some(left), ..Default::default() };
        Heard { address, model, signal, at, earbuds }
    }

    #[test]
    fn the_paired_models_strongest_recent_message() {
        // Later than every time below, which an Instant can't be earlier than the system's start
        let now = Instant::now() + Duration::from_secs(60);
        let mut all = Vec::new();
        remember(&mut all, heard(1, 0x2014, -60, now - Duration::from_secs(5), 50));
        remember(&mut all, heard(2, 0x2014, -75, now - Duration::from_secs(1), 40));
        remember(&mut all, heard(3, 0x200E, -40, now, 30));
        remember(&mut all, heard(4, 0x2014, -30, now - Duration::from_secs(40), 20));
        remember(&mut all, heard(5, 0x2014, -127, now, 10));
        assert_eq!(earbuds_for(&all, 0x2014, now).and_then(|e| e.left), Some(50));
        // A newer message from the same earbud takes its place
        remember(&mut all, heard(1, 0x2014, -90, now, 60));
        assert_eq!(earbuds_for(&all, 0x2014, now).and_then(|e| e.left), Some(40));
        assert_eq!(all.len(), 4, "one per sender");
        // Too old once they are: the 40 s one goes when a newer one comes
        assert!(all.iter().all(|h| h.address != 4));
        // Nothing heard of the paired model; a model that isn't earbuds with a case
        assert_eq!(earbuds_for(&all, 0x2013, now), None);
        assert!(!is_earbuds(0x200A) && !same_earbuds(0x200A, 0x200A), "AirPods Max");
        assert!(is_earbuds(0x2012), "Beats Fit Pro");
        assert!(same_earbuds(0x2030, 0x2036) && !same_earbuds(0x2014, 0x2024));
        assert_eq!(earbuds_for(&all, 0x2014, now + RECENT), None);
    }

    #[test]
    fn a_split_keyboards_halves() {
        let parts = [(String::new(), 90), ("Peripheral 0\0".to_string(), 70)];
        assert_eq!(of_parts(&parts), Some((70, Some("main 90%, Peripheral 0 70%".into()))));
        assert_eq!(of_parts(&[(String::new(), 55)]), Some((55, None)));
        // A level that isn't one is left out
        assert_eq!(of_parts(&[(String::new(), 80), ("Peripheral 0".into(), 0xff)]), Some((80, None)));
        assert_eq!(of_parts(&[]), None);
    }
}

//! AirPods' and Beats' levels as they advertise them: each earbud's and the case's, in the proximity pairing message
//! they send to the Apple devices around them (parts.rs reads it), where Windows' own level, from the hands-free
//! profile, is one number. A BluetoothLEAdvertisementWatcher hears them - passive, so it only listens and never asks
//! a device anything; a desktop app needs no package identity for it. It runs while paired earbuds of such a model
//! are connected. What it hears stays in memory, for `parts::RECENT`.
//! As AirPodsDesktop (SpriteOvO, Source/Core/Bluetooth_win.cpp) listens; own implementation.

use super::parts::{self, Earbuds, Heard};
use super::windows::{bytes, note_once};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows::Devices::Bluetooth::Advertisement::{
    BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher, BluetoothLEAdvertisementWatcherStatus,
    BluetoothLEScanningMode,
};
use windows::Foundation::TypedEventHandler;

/// Apple's company identifier, in front of the manufacturer data of its advertisements.
const APPLE: u16 = 0x004C;

static WATCHER: Mutex<Option<BluetoothLEAdvertisementWatcher>> = Mutex::new(None);
static HEARD: Mutex<Vec<Heard>> = Mutex::new(Vec::new());

/// How long the first look waits for the earbuds after the watcher has started: they advertise every few seconds.
const FIRST_WAIT: Duration = Duration::from_secs(4);

/// What the paired earbuds of model `product` said lately. Starts listening if it isn't, and then waits a little for
/// them, so that earbuds are shown with their parts from the poll that finds them connected.
pub fn earbuds(product: u16) -> Option<Earbuds> {
    let deadline = Instant::now() + if listen() { FIRST_WAIT } else { Duration::ZERO };
    loop {
        let heard = parts::earbuds_for(&HEARD.lock().unwrap_or_else(|e| e.into_inner()), product, Instant::now());
        if heard.is_some() || Instant::now() >= deadline {
            return heard;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Stops listening, when no earbuds are connected, and forgets what was heard.
pub fn stop() {
    if let Some(watcher) = WATCHER.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        if watcher.Status().is_ok_and(|s| s == BluetoothLEAdvertisementWatcherStatus::Started) {
            let _ = watcher.Stop();
        }
    }
    HEARD.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Starts the watcher unless it is running; true when it has just started. Windows stops it when the radio goes off
/// (aborted), and it is started again here once that is over.
fn listen() -> bool {
    let mut slot = WATCHER.lock().unwrap_or_else(|e| e.into_inner());
    let started = (|| -> windows::core::Result<bool> {
        let watcher = match slot.as_ref() {
            Some(watcher) => watcher.clone(),
            None => slot.insert(new_watcher()?).clone(),
        };
        let status = watcher.Status()?;
        if status == BluetoothLEAdvertisementWatcherStatus::Started
            || status == BluetoothLEAdvertisementWatcherStatus::Stopping
        {
            return Ok(false);
        }
        watcher.Start()?;
        Ok(true)
    })();
    started.unwrap_or_else(|e| {
        note_once(format!("bluetooth: can't listen for earbuds: {}", e.message()));
        false
    })
}

fn new_watcher() -> windows::core::Result<BluetoothLEAdvertisementWatcher> {
    let watcher = BluetoothLEAdvertisementWatcher::new()?;
    watcher.SetScanningMode(BluetoothLEScanningMode::Passive)?;
    type Received = TypedEventHandler<BluetoothLEAdvertisementWatcher, BluetoothLEAdvertisementReceivedEventArgs>;
    watcher.Received(&Received::new(|_, args| {
        // One that can't be read is skipped: an error would only go back to Windows
        if let Ok(args) = args.ok() {
            let _ = received(args);
        }
        Ok(())
    }))?;
    Ok(watcher)
}

/// Keeps the proximity pairing message in an advertisement of Apple's, if there is one.
fn received(args: &BluetoothLEAdvertisementReceivedEventArgs) -> windows::core::Result<()> {
    for data in args.Advertisement()?.ManufacturerData()? {
        if data.CompanyId()? != APPLE {
            continue;
        }
        let Some((model, earbuds)) = parts::proximity_pairing(&bytes(&data.Data()?)?) else { continue };
        let heard = Heard {
            address: args.BluetoothAddress()?,
            model,
            signal: args.RawSignalStrengthInDBm()?,
            at: Instant::now(),
            earbuds,
        };
        parts::remember(&mut HEARD.lock().unwrap_or_else(|e| e.into_inner()), heard);
    }
    Ok(())
}

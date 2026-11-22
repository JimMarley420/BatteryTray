//! Polls every provider on its own thread and schedule (HID queries block for seconds, so they never wait for
//! each other). A provider whose devices push changes cuts its own wait short.

use crate::providers::Provider;
use crate::reading::DeviceReading;
use crate::state::StateStore;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Asks for a poll right away: what a pushing provider calls when its devices report a change.
pub type Pusher = Arc<dyn Fn() + Send + Sync>;

pub fn no_push() -> Pusher {
    Arc::new(|| {})
}

#[derive(Default)]
struct Wake {
    flag: Mutex<bool>,
    signal: Condvar,
}

impl Wake {
    fn set(&self) {
        *self.flag.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.signal.notify_all();
    }

    fn clear(&self) {
        *self.flag.lock().unwrap_or_else(|e| e.into_inner()) = false;
    }

    /// Waits `timeout`, or less when woken or stopped.
    fn wait(&self, timeout: Duration, stop: &AtomicBool) {
        let guard = self.flag.lock().unwrap_or_else(|e| e.into_inner());
        let _ = self.signal.wait_timeout_while(guard, timeout, |woken| !*woken && !stop.load(Ordering::Relaxed));
    }
}

struct Shared {
    state: Arc<StateStore>,
    stop: AtomicBool,
    latest: Mutex<Vec<Vec<DeviceReading>>>,
    logged: Mutex<HashMap<String, String>>,
    changed: Box<dyn Fn() + Send + Sync>,
}

pub struct Monitor {
    shared: Arc<Shared>,
    wakes: Vec<Arc<Wake>>,
    threads: Vec<JoinHandle<()>>,
    /// Counts the system's device changes: a burst of them is one round of polls, the latest one's
    changes: Arc<AtomicU64>,
}

/// After the system says a device came or went: when every provider polls again. A device's level often comes a
/// little after the device itself (a Bluetooth headset's, a mouse a receiver has just woken), hence three rounds.
const AFTER_A_CHANGE: [Duration; 3] = [Duration::from_secs(1), Duration::from_secs(5), Duration::from_secs(15)];

/// How a reading shows in the log: online with its level, or offline and why.
pub fn summary(r: &DeviceReading) -> String {
    if r.online {
        format!(
            "online, {}%{}{}{}",
            r.level.map(|l| l.to_string()).unwrap_or_else(|| "?".into()),
            if r.estimate { " (estimate)" } else { "" },
            if r.charging { ", charging" } else { "" },
            if r.full_on_charger { ", charged" } else { "" }
        )
    } else {
        format!("offline{}", r.detail.as_ref().map(|d| format!(" ({d})")).unwrap_or_default())
    }
}

/// Runs one poll, a panic counting as a poll that found nothing (as an exception did in C#).
pub fn poll_safely(provider: &mut dyn Provider) -> Vec<DeviceReading> {
    let name = provider.name();
    match catch_unwind(AssertUnwindSafe(|| provider.poll())) {
        Ok(result) => result,
        Err(panic) => {
            let text = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "panic".into());
            crate::log::write(&format!("{name}: poll failed: {text}"));
            Vec::new()
        }
    }
}

impl Monitor {
    /// Starts polling; `changed` is called (on a polling thread) after every poll.
    pub fn start(
        providers: Vec<Box<dyn Provider>>,
        state: Arc<StateStore>,
        changed: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            state,
            stop: AtomicBool::new(false),
            latest: Mutex::new(vec![Vec::new(); providers.len()]),
            logged: Mutex::new(HashMap::new()),
            changed: Box::new(changed),
        });
        let mut wakes = Vec::new();
        let mut threads = Vec::new();
        for (index, mut provider) in providers.into_iter().enumerate() {
            let wake = Arc::new(Wake::default());
            let w = wake.clone();
            provider.set_pusher(Arc::new(move || w.set()));
            let shared = shared.clone();
            let w = wake.clone();
            let thread = std::thread::Builder::new()
                .name(provider.name().to_string())
                .spawn(move || Self::run(index, provider, shared, w))
                .expect("cannot start a polling thread");
            wakes.push(wake);
            threads.push(thread);
        }
        Self { shared, wakes, threads, changes: Arc::new(AtomicU64::new(0)) }
    }

    fn run(index: usize, mut provider: Box<dyn Provider>, shared: Arc<Shared>, wake: Arc<Wake>) {
        while !shared.stop.load(Ordering::Relaxed) {
            // Cleared before the poll, so a wake-up that arrives during it isn't lost: the wait below ends at once
            wake.clear();
            let result = poll_safely(provider.as_mut());
            let delay = provider.delay(&result);
            shared.publish(index, result);
            wake.wait(delay, &shared.stop);
        }
    }

    /// Cuts every provider's wait short, so all of them poll right away.
    pub fn refresh_now(&self) {
        for w in &self.wakes {
            w.set();
        }
    }

    /// The system says a device came or went (plugged in, connected over Bluetooth, gone): every provider polls a
    /// second later and twice more after it, rather than when its schedule says. The first second gathers a burst
    /// of changes, which one device often makes, into one round.
    pub fn devices_changed(&self) {
        let change = self.changes.fetch_add(1, Ordering::AcqRel) + 1;
        let (changes, wakes, shared) = (self.changes.clone(), self.wakes.clone(), self.shared.clone());
        let _ = std::thread::Builder::new().name("device change".into()).spawn(move || {
            for wait in AFTER_A_CHANGE {
                std::thread::sleep(wait);
                // A later change has its own round, and a stopped monitor none
                if changes.load(Ordering::Acquire) != change || shared.stop.load(Ordering::Relaxed) {
                    return;
                }
                for w in &wakes {
                    w.set();
                }
            }
        });
    }

    pub fn current(&self) -> Vec<DeviceReading> {
        self.shared.latest.lock().unwrap_or_else(|e| e.into_inner()).iter().flatten().cloned().collect()
    }

    /// Asks every thread to stop and waits for the ones that are between polls.
    pub fn stop(self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        for w in &self.wakes {
            w.set();
        }
        drop(self.threads); // a thread in the middle of a poll finishes it by itself
    }
}

impl Shared {
    fn publish(&self, index: usize, result: Vec<DeviceReading>) {
        {
            let mut logged = self.logged.lock().unwrap_or_else(|e| e.into_inner());
            for r in &result {
                self.state.update(r);
                // State changes only (not every poll), so the log shows the battery history at a glance
                let s = summary(r);
                if logged.get(&r.id) != Some(&s) {
                    crate::log::write(&format!("{} [{}]: {}", r.name, r.via, s));
                    logged.insert(r.id.clone(), s);
                }
            }
            self.latest.lock().unwrap_or_else(|e| e.into_inner())[index] = result;
        }
        (self.changed)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::Provider;
    use std::sync::atomic::AtomicUsize;

    struct Counting(Arc<AtomicUsize>);

    impl Provider for Counting {
        fn name(&self) -> &'static str {
            "counting"
        }

        fn poll(&mut self) -> Vec<DeviceReading> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Vec::new()
        }

        fn delay(&self, _last: &[DeviceReading]) -> Duration {
            Duration::from_secs(3600)
        }
    }

    /// A burst of changes is one round of polls a second later; then two more follow.
    #[test]
    fn a_device_change_brings_the_polls_forward() {
        let polls = Arc::new(AtomicUsize::new(0));
        let monitor = Monitor::start(vec![Box::new(Counting(polls.clone()))], Arc::new(StateStore::empty()), || {});
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(polls.load(Ordering::SeqCst), 1, "the first poll, then an hour's wait");
        for _ in 0..5 {
            monitor.devices_changed();
        }
        std::thread::sleep(Duration::from_millis(1300));
        assert_eq!(polls.load(Ordering::SeqCst), 2, "one poll for the whole burst");
        std::thread::sleep(Duration::from_secs(5));
        assert_eq!(polls.load(Ordering::SeqCst), 3);
        monitor.stop();
    }
}

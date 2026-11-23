//! Polls every provider on its own thread and schedule (HID queries block for seconds, so they never wait for
//! each other). A provider whose devices push changes cuts its own wait short, and so does "Refresh". When the system
//! says a device came or went, the providers poll early too - those that don't keep a pace of their own.

use crate::providers::Provider;
use crate::reading::DeviceReading;
use crate::state::StateStore;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Asks for a poll right away: what a pushing provider calls when its devices report a change.
pub type Pusher = Arc<dyn Fn() + Send + Sync>;

pub fn no_push() -> Pusher {
    Arc::new(|| {})
}

/// Settings > General, "Look for new devices automatically": whether a device the system reports, or one a provider
/// sees arrive by itself, brings the polls forward. Off, a new device waits for the schedules or a refresh; the levels
/// of the devices already found are polled either way. Only the macOS app sets it, from its settings; on Windows it
/// stays on.
static ON_CONNECT: AtomicBool = AtomicBool::new(true);

pub fn set_polls_on_connect(on: bool) {
    ON_CONNECT.store(on, Ordering::Relaxed);
}

pub fn polls_on_connect() -> bool {
    ON_CONNECT.load(Ordering::Relaxed)
}

/// Why a polling thread is woken before its time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reason {
    /// "Refresh", or the provider's own devices pushed a change: poll now.
    Refresh,
    /// The system says a device came or went: poll now, unless the provider keeps its own pace
    /// (`Provider::polls_on_device_change`).
    DeviceChange,
}

#[derive(Default)]
struct Wake {
    reason: Mutex<Option<Reason>>,
    signal: Condvar,
}

impl Wake {
    fn set(&self, reason: Reason) {
        let mut pending = self.reason.lock().unwrap_or_else(|e| e.into_inner());
        // A refresh outranks a device change
        if *pending != Some(Reason::Refresh) {
            *pending = Some(reason);
        }
        self.signal.notify_all();
    }

    fn clear(&self) {
        *self.reason.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Waits until `deadline`, or less when woken or stopped; why it was woken (None: the time came, or the stop).
    fn wait_until(&self, deadline: Instant, stop: &AtomicBool) -> Option<Reason> {
        let guard = self.reason.lock().unwrap_or_else(|e| e.into_inner());
        let timeout = deadline.saturating_duration_since(Instant::now());
        let (mut pending, _) = self
            .signal
            .wait_timeout_while(guard, timeout, |r| r.is_none() && !stop.load(Ordering::Relaxed))
            .unwrap_or_else(|e| e.into_inner());
        pending.take()
    }
}

/// The rounds of early polls the system's device changes call for (see `Monitor::devices_changed`).
#[derive(Default)]
struct Changes {
    rounds: Mutex<Rounds>,
    signal: Condvar,
}

#[derive(Default)]
struct Rounds {
    /// The latest change that still has rounds to come
    change: Option<Instant>,
    /// How many of its rounds have been
    done: usize,
    last_round: Option<Instant>,
}

/// After the system says a device came or went: when the providers poll early. A device's level often comes a
/// little after the device itself (a Bluetooth headset's, a mouse a receiver has just woken), hence three rounds; the
/// first second gathers the burst of changes one device makes. A change that comes meanwhile starts them again.
const AFTER_A_CHANGE: [Duration; 3] = [Duration::from_secs(1), Duration::from_secs(5), Duration::from_secs(15)];

/// The least time between two rounds, however often the system reports changes (a USB device that keeps
/// reconnecting): the providers' own schedules go on meanwhile, so nothing waits on this.
const BETWEEN_ROUNDS: Duration = Duration::from_secs(5);

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
    changes: Arc<Changes>,
}

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
            provider.set_pusher(Arc::new(move || w.set(Reason::Refresh)));
            let shared = shared.clone();
            let w = wake.clone();
            let thread = std::thread::Builder::new()
                .name(provider.name().to_string())
                .spawn(move || Self::run(index, provider, shared, w))
                .expect("cannot start a polling thread");
            wakes.push(wake);
            threads.push(thread);
        }
        let changes = Arc::new(Changes::default());
        let (c, w, s) = (changes.clone(), wakes.clone(), shared.clone());
        let thread = std::thread::Builder::new()
            .name("device changes".into())
            .spawn(move || Self::rounds(c, w, s))
            .expect("cannot start the device changes' thread");
        threads.push(thread);
        Self { shared, wakes, threads, changes }
    }

    fn run(index: usize, mut provider: Box<dyn Provider>, shared: Arc<Shared>, wake: Arc<Wake>) {
        while !shared.stop.load(Ordering::Relaxed) {
            // Cleared before the poll, so a wake-up that arrives during it isn't lost: the wait below ends at once
            wake.clear();
            let result = poll_safely(provider.as_mut());
            let deadline = Instant::now() + provider.delay(&result);
            let eager = provider.polls_on_device_change(&result);
            shared.publish(index, result);
            // A device change doesn't make a provider that keeps its own pace ask early: it waits on
            while wake.wait_until(deadline, &shared.stop) == Some(Reason::DeviceChange) && !eager {}
        }
    }

    /// Runs the rounds the device changes call for, one at a time, on a thread of its own.
    fn rounds(changes: Arc<Changes>, wakes: Vec<Arc<Wake>>, shared: Arc<Shared>) {
        let mut rounds = changes.rounds.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if shared.stop.load(Ordering::Relaxed) {
                return;
            }
            let Some(change) = rounds.change else {
                rounds = changes.signal.wait(rounds).unwrap_or_else(|e| e.into_inner());
                continue;
            };
            let mut due = change + AFTER_A_CHANGE[rounds.done];
            if let Some(last) = rounds.last_round {
                due = due.max(last + BETWEEN_ROUNDS);
            }
            let now = Instant::now();
            if now < due {
                rounds = changes.signal.wait_timeout(rounds, due - now).unwrap_or_else(|e| e.into_inner()).0;
                continue;
            }
            rounds.done += 1;
            if rounds.done == AFTER_A_CHANGE.len() {
                rounds.change = None;
            }
            rounds.last_round = Some(now);
            for w in &wakes {
                w.set(Reason::DeviceChange);
            }
        }
    }

    /// Cuts every provider's wait short, so all of them poll right away.
    pub fn refresh_now(&self) {
        for w in &self.wakes {
            w.set(Reason::Refresh);
        }
    }

    /// The system says a device came or went (plugged in, connected over Bluetooth, gone): the providers poll a
    /// second later and twice more over the next 15 seconds, rather than when their schedules say - all but those that
    /// keep a pace of their own while they have a device (keyboards that a request keeps awake). Cheap to call often:
    /// a burst of changes is one set of rounds, and rounds are 5 seconds apart at least. Nothing when the settings
    /// say to wait for the schedules (`polls_on_connect`).
    pub fn devices_changed(&self) {
        if !polls_on_connect() {
            return;
        }
        let mut rounds = self.changes.rounds.lock().unwrap_or_else(|e| e.into_inner());
        rounds.change = Some(Instant::now());
        rounds.done = 0;
        self.changes.signal.notify_all();
    }

    pub fn current(&self) -> Vec<DeviceReading> {
        self.shared.latest.lock().unwrap_or_else(|e| e.into_inner()).iter().flatten().cloned().collect()
    }

    /// Asks every thread to stop and waits for the ones that are between polls.
    pub fn stop(self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        for w in &self.wakes {
            w.set(Reason::Refresh);
        }
        // Taken while the rounds' thread isn't between its check and its wait, so it can't miss the stop
        drop(self.changes.rounds.lock().unwrap_or_else(|e| e.into_inner()));
        self.changes.signal.notify_all();
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

    /// Counts its polls; waits an hour between them. `paced`: keeps its own pace on a device change.
    struct Counting(Arc<AtomicUsize>, bool);

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

        fn polls_on_device_change(&self, _last: &[DeviceReading]) -> bool {
            !self.1
        }
    }

    fn started(paced: bool) -> (Monitor, Arc<AtomicUsize>) {
        let polls = Arc::new(AtomicUsize::new(0));
        let provider = Counting(polls.clone(), paced);
        let monitor = Monitor::start(vec![Box::new(provider)], Arc::new(StateStore::empty()), || {});
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(polls.load(Ordering::SeqCst), 1, "the first poll, then an hour's wait");
        (monitor, polls)
    }

    /// A burst of changes is one round a second later; the next one follows 5 seconds after it.
    #[test]
    fn a_device_change_brings_the_polls_forward() {
        let (monitor, polls) = started(false);
        for _ in 0..5 {
            monitor.devices_changed();
        }
        std::thread::sleep(Duration::from_millis(1400));
        assert_eq!(polls.load(Ordering::SeqCst), 2, "one poll for the whole burst");
        std::thread::sleep(Duration::from_millis(5000));
        assert_eq!(polls.load(Ordering::SeqCst), 3);
        monitor.stop();
    }

    /// A provider that keeps its own pace isn't moved by a device change, but "Refresh" still moves it.
    #[test]
    fn a_paced_provider_waits_for_its_time_or_a_refresh() {
        let (monitor, polls) = started(true);
        monitor.devices_changed();
        std::thread::sleep(Duration::from_millis(1400));
        assert_eq!(polls.load(Ordering::SeqCst), 1);
        monitor.refresh_now();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(polls.load(Ordering::SeqCst), 2);
        monitor.stop();
    }

    /// Changes that never stop (a device that keeps reconnecting) bring no round until they pause.
    #[test]
    fn a_storm_of_changes_waits_for_quiet() {
        let (monitor, polls) = started(false);
        for _ in 0..12 {
            monitor.devices_changed();
            std::thread::sleep(Duration::from_millis(250));
        }
        assert_eq!(polls.load(Ordering::SeqCst), 1, "no round while the changes keep coming");
        std::thread::sleep(Duration::from_millis(1200));
        assert_eq!(polls.load(Ordering::SeqCst), 2);
        monitor.stop();
    }
}

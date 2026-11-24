//! Checks for a new release shortly after start and then every few hours. With automatic
//! updates on, a new version is installed and the app restarts into it - but only while no window of the app is
//! open, so it never vanishes from under the user. When installing needs an administrator (Program Files without
//! admin rights on Windows, an Applications folder the user can't write to on macOS), the update waits for the
//! user's click instead and is announced once.
//! Runs on its own thread (the HTTP calls block); what the UI needs to know arrives through `events`.

use crate::loc::t;
use crate::log;
use crate::updater::{self, ReleaseInfo, Version};
use chrono::{DateTime, Local};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const FIRST_CHECK: Duration = Duration::from_secs(30);
const PERIOD: Duration = Duration::from_secs(6 * 3600);
const RETRY: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    NotChecked,
    Checking,
    UpToDate,
    Available,
    Installing,
    Failed,
}

#[derive(Clone, Debug)]
pub struct Status {
    pub stage: Stage,
    pub latest: Option<ReleaseInfo>,
    pub last_check: Option<DateTime<Local>>,
    pub progress: i32,
    pub error: Option<String>,
}

impl Status {
    /// A newer version than the running one is known.
    pub fn newer_available(&self) -> bool {
        self.latest.as_ref().is_some_and(|l| l.version > Version::current())
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    /// The status changed (for a window that shows it).
    Changed,
    /// A new version that can't install by itself (admin prompt needed, or automatic updates off): announced once.
    Announce(ReleaseInfo),
    /// The new exe is in place: restart into it.
    Installed(ReleaseInfo),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Request {
    None,
    Check,
    Install,
    Stop,
}

struct Shared {
    status: Mutex<Status>,
    request: Mutex<Request>,
    signal: Condvar,
    auto_update: AtomicBool,
    window_open: AtomicBool,
    events: Box<dyn Fn(Event) + Send + Sync>,
}

pub struct UpdateService {
    shared: Arc<Shared>,
}

impl UpdateService {
    pub fn start(auto_update: bool, events: impl Fn(Event) + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared {
            status: Mutex::new(Status {
                stage: Stage::NotChecked,
                latest: None,
                last_check: None,
                progress: 0,
                error: None,
            }),
            request: Mutex::new(Request::None),
            signal: Condvar::new(),
            auto_update: AtomicBool::new(auto_update),
            window_open: AtomicBool::new(false),
            events: Box::new(events),
        });
        let s = shared.clone();
        let _ = std::thread::Builder::new().name("updates".into()).spawn(move || run(s));
        Self { shared }
    }

    pub fn status(&self) -> Status {
        self.shared.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_auto_update(&self, on: bool) {
        self.shared.auto_update.store(on, Ordering::Relaxed);
    }

    /// Whether a window of the app is open: an automatic update waits until none is.
    pub fn set_window_open(&self, open: bool) {
        self.shared.window_open.store(open, Ordering::Relaxed);
    }

    pub fn check_now(&self) {
        self.request(Request::Check);
    }

    pub fn install_now(&self) {
        self.request(Request::Install);
    }

    fn request(&self, r: Request) {
        *self.shared.request.lock().unwrap_or_else(|e| e.into_inner()) = r;
        self.shared.signal.notify_all();
    }
}

impl Drop for UpdateService {
    fn drop(&mut self) {
        self.request(Request::Stop);
    }
}

fn run(shared: Arc<Shared>) {
    let mut next = Instant::now() + FIRST_CHECK;
    let mut announced_tag: Option<String> = None;
    loop {
        // Waits for the next check or a request
        let request = {
            let mut r = shared.request.lock().unwrap_or_else(|e| e.into_inner());
            while *r == Request::None {
                let left = next.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    *r = Request::Check;
                    break;
                }
                r = shared.signal.wait_timeout(r, left).unwrap_or_else(|e| e.into_inner()).0;
            }
            std::mem::replace(&mut *r, Request::None)
        };
        let after = match request {
            Request::Stop => return,
            Request::Install => install(&shared),
            _ => {
                // The start's clean-up misses the old exe while the version that installed this one is still exiting
                // (always, after an update from the C# app); by the first check it is long gone
                crate::updater::clean_up();
                check(&shared, &mut announced_tag)
            }
        };
        // After an install the app restarts; until it has, nothing more to do
        next = Instant::now() + after.unwrap_or(PERIOD);
    }
}

fn set(shared: &Shared, change: impl FnOnce(&mut Status)) {
    change(&mut shared.status.lock().unwrap_or_else(|e| e.into_inner()));
    (shared.events)(Event::Changed);
}

/// When to check next; None after an install, which restarts the app.
fn check(shared: &Shared, announced_tag: &mut Option<String>) -> Option<Duration> {
    set(shared, |s| s.stage = Stage::Checking);
    let latest = match updater::get_latest() {
        Ok(latest) => latest,
        Err(e) => {
            log::write(&format!("update check failed: {e}"));
            set(shared, |s| {
                s.error = Some(e.message);
                s.stage = Stage::Failed;
            });
            return Some(RETRY);
        }
    };
    set(shared, |s| {
        s.latest = latest.clone();
        s.last_check = Some(Local::now());
        s.error = None;
    });
    let Some(latest) = latest.filter(|l| l.version > Version::current()) else {
        set(shared, |s| s.stage = Stage::UpToDate);
        return Some(PERIOD);
    };

    log::write(&format!("update available: {}", latest.tag));
    set(shared, |s| s.stage = Stage::Available);
    if shared.auto_update.load(Ordering::Relaxed) && updater::can_install_in_place() {
        if !shared.window_open.load(Ordering::Relaxed) {
            return install(shared);
        }
        return Some(RETRY); // the user is looking at a window: try again later
    }
    if announced_tag.as_deref() != Some(latest.tag.as_str()) {
        *announced_tag = Some(latest.tag.clone());
        (shared.events)(Event::Announce(latest));
    }
    Some(PERIOD)
}

fn install(shared: &Shared) -> Option<Duration> {
    let status = shared.status.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if !status.newer_available() || status.stage == Stage::Installing {
        return Some(PERIOD);
    }
    let release = status.latest.expect("a newer release is known");
    set(shared, |s| {
        s.progress = 0;
        s.stage = Stage::Installing;
    });
    let result = if updater::can_install_in_place() {
        updater::install(&release, &|p| set(shared, |s| s.progress = p))
    } else if updater::install_elevated() {
        // A folder only administrators can write to: a copy of this app with their rights did the same (a UAC prompt
        // on Windows, the password on macOS)
        Ok(())
    } else {
        Err(crate::sys::Error::other(t("upd.err.cancelled")))
    };
    match result {
        Ok(()) => {
            (shared.events)(Event::Installed(release));
            None
        }
        Err(e) => {
            log::write(&format!("update failed: {e}"));
            set(shared, |s| {
                s.error = Some(e.message);
                s.stage = Stage::Failed;
            });
            Some(RETRY)
        }
    }
}

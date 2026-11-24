//! HTTPS GET over NSURLSession, from the thread that asks: the request runs on the session's own queue and this waits
//! for its answer. Nothing is cached, so an update check always sees the release GitHub has now.

use crate::sys::{Error, Result};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2_foundation::{
    NSData, NSError, NSHTTPURLResponse, NSMutableURLRequest, NSString, NSURLRequestCachePolicy, NSURLResponse,
    NSURLSession, NSURLSessionConfiguration, NSURL,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// As WinHTTP is set up on Windows: 30 seconds for each step, a whole download may take 10 minutes.
const STEP: f64 = 30.0;
const WHOLE: Duration = Duration::from_secs(600);

/// An answer the session's queue hands to the waiting thread.
struct Answer<T> {
    value: Mutex<Option<Result<T>>>,
    arrived: Condvar,
}

impl<T> Answer<T> {
    fn new() -> Arc<Self> {
        Arc::new(Self { value: Mutex::new(None), arrived: Condvar::new() })
    }

    fn give(&self, value: Result<T>) {
        *self.value.lock().unwrap_or_else(|e| e.into_inner()) = Some(value);
        self.arrived.notify_all();
    }

    /// Waits up to `step` at a time, calling `between` after each wait; None once `deadline` has passed.
    fn wait(&self, deadline: Instant, step: Duration, between: impl Fn()) -> Option<Result<T>> {
        let mut value = self.value.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(v) = value.take() {
                return Some(v);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            value = self.arrived.wait_timeout(value, left.min(step)).unwrap_or_else(|e| e.into_inner()).0;
            between();
        }
    }
}

fn session() -> Retained<NSURLSession> {
    let config = NSURLSessionConfiguration::ephemeralSessionConfiguration();
    config.setTimeoutIntervalForRequest(STEP);
    config.setTimeoutIntervalForResource(WHOLE.as_secs_f64());
    config.setRequestCachePolicy(NSURLRequestCachePolicy::ReloadIgnoringLocalCacheData);
    NSURLSession::sessionWithConfiguration(&config)
}

fn request(url: &str, user_agent: &str, accept: Option<&str>) -> Result<Retained<NSMutableURLRequest>> {
    if !url.starts_with("https://") {
        return Err(Error::other(format!("not an https URL: {url}")));
    }
    let address =
        NSURL::URLWithString(&NSString::from_str(url)).ok_or_else(|| Error::other(format!("bad URL: {url}")))?;
    let request = NSMutableURLRequest::requestWithURL(&address);
    request.setValue_forHTTPHeaderField(Some(&NSString::from_str(user_agent)), &NSString::from_str("User-Agent"));
    if let Some(accept) = accept {
        request.setValue_forHTTPHeaderField(Some(&NSString::from_str(accept)), &NSString::from_str("Accept"));
    }
    Ok(request)
}

/// The error the session reports, or the HTTP status when it isn't 2xx.
fn outcome(response: *mut NSURLResponse, error: *mut NSError) -> Result<()> {
    if let Some(error) = unsafe { error.as_ref() } {
        return Err(Error::with_message(error.code() as u32, error.localizedDescription().to_string()));
    }
    let status = unsafe { response.as_ref() }
        .and_then(|r| r.downcast_ref::<NSHTTPURLResponse>())
        .map(|r| r.statusCode())
        .unwrap_or(0);
    if !(200..300).contains(&status) {
        return Err(Error::other(format!("Response status code does not indicate success: {status}.")));
    }
    Ok(())
}

fn timed_out() -> Error {
    Error::other("The request timed out.")
}

/// The body of a GET as text.
pub fn get_text(url: &str, user_agent: &str, accept: Option<&str>) -> Result<String> {
    let request = request(url, user_agent, accept)?;
    let answer = Answer::<Vec<u8>>::new();
    let given = answer.clone();
    let done = RcBlock::new(move |data: *mut NSData, response: *mut NSURLResponse, error: *mut NSError| {
        let body = unsafe { data.as_ref() }.map(|d| d.to_vec()).unwrap_or_default();
        given.give(outcome(response, error).map(|_| body));
    });
    let session = session();
    let task = unsafe { session.dataTaskWithRequest_completionHandler(&request, &done) };
    task.resume();
    let deadline = Instant::now() + Duration::from_secs_f64(STEP * 3.0);
    let body = answer.wait(deadline, Duration::from_secs(1), || {}).unwrap_or_else(|| {
        task.cancel();
        Err(timed_out())
    });
    // A session is only freed once it is invalidated
    session.finishTasksAndInvalidate();
    String::from_utf8(body?).map_err(|e| Error::other(e.to_string()))
}

/// Downloads a file of `size` bytes into `path`, telling `progress` each new percentage.
pub fn download(url: &str, user_agent: &str, path: &Path, size: u64, progress: &dyn Fn(i32)) -> Result<()> {
    let request = request(url, user_agent, None)?;
    let answer = Answer::<()>::new();
    let given = answer.clone();
    let target = path.to_path_buf();
    // The session deletes its file once this returns: it is moved where it belongs first
    let done = RcBlock::new(move |file: *mut NSURL, response: *mut NSURLResponse, error: *mut NSError| {
        let moved = outcome(response, error).and_then(|_| {
            let from = unsafe { file.as_ref() }
                .and_then(|f| f.path())
                .map(|p| PathBuf::from(p.to_string()))
                .ok_or_else(|| Error::other("the download left no file"))?;
            move_file(&from, &target)
        });
        given.give(moved);
    });
    let session = session();
    let task = unsafe { session.downloadTaskWithRequest_completionHandler(&request, &done) };
    task.resume();
    let last = std::cell::Cell::new(-1);
    let report = || {
        let percent = (task.countOfBytesReceived().max(0) as u64 * 100 / size.max(1)).min(100) as i32;
        if percent != last.get() {
            last.set(percent);
            progress(percent);
        }
    };
    let result = answer.wait(Instant::now() + WHOLE, Duration::from_millis(100), report).unwrap_or_else(|| {
        task.cancel();
        Err(timed_out())
    });
    session.finishTasksAndInvalidate();
    if result.is_ok() && last.get() != 100 {
        progress(100);
    }
    result
}

/// A rename, or a copy when the two are on different volumes.
fn move_file(from: &Path, to: &Path) -> Result<()> {
    let io = |e: std::io::Error| Error::other(e.to_string());
    let _ = std::fs::remove_file(to);
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map_err(io)?;
    let _ = std::fs::remove_file(from);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https() {
        assert!(get_text("http://example.com", "BatteryTray/test", None).is_err());
    }

    /// The real thing: GitHub's API answers, and a 404 is an error. `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn github_answers() {
        let text =
            get_text("https://api.github.com/repos/qu1ncky/BatteryTray/releases/latest", "BatteryTray/test", None)
                .unwrap();
        assert!(text.contains("tag_name"));
        let missing =
            get_text("https://api.github.com/repos/qu1ncky/BatteryTray/no-such-thing", "BatteryTray/test", None);
        assert!(missing.unwrap_err().message.contains("404"));
    }
}

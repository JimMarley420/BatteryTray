//! Console tool for checking the Rust core against the C# app on real devices:
//!   batterytray-core dump [--out FILE]   poll every provider once and print the readings, in the same format as
//!                                         `BatteryTray.exe --dump` writes them
//!   batterytray-core watch                keep polling as the app does and print every change
//!   batterytray-core list                 print every HID collection Windows lists
//! Options: --lang ru|en|zh (default: English for dump, as in the app's settings for watch), --trace (Barracuda
//! packets).
//! It reads the app's state.json but never writes it, and logs to %LOCALAPPDATA%\BatteryTray\rust-core, so it can
//! run next to the app.

use batterytray::{diag, hid, loc, log, monitor, providers, settings::AppSettings, state::StateStore, sys};
use std::sync::Arc;

fn usage() -> ! {
    eprintln!("usage: batterytray-core dump [--out FILE] | watch | list   [--lang ru|en|zh] [--trace]");
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().map(String::as_str) else { usage() };
    let option = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();

    log::set_folder(sys::app_data_folder().join("rust-core"));
    log::set_echo(true);
    // A dump is in English, as the C# app's (its --dump runs before the language is set), so the two compare line
    // by line; watch shows what the app would
    let language = option("--lang").or_else(|| {
        if command == "dump" {
            Some("en".into())
        } else {
            Some(AppSettings::load().language)
        }
    });
    loc::set(loc::resolve(language.as_deref()));
    if args.iter().any(|a| a == "--trace") {
        *providers::barracuda::TRACE.lock().unwrap() = Some(|what, bytes| {
            log::write(&format!(
                "Barracuda {what} {}",
                bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
            ));
        });
    }

    match command {
        "dump" => dump(option("--out")),
        "watch" => watch(),
        "list" => list(),
        _ => usage(),
    }
}

fn dump(out: Option<String>) {
    let text = diag::dump_lines().join("\r\n") + "\r\n";
    print!("{text}");
    if let Some(path) = out {
        if let Err(e) = std::fs::write(&path, &text) {
            eprintln!("cannot write {path}: {e}");
            std::process::exit(1);
        }
    }
}

fn watch() {
    let state = Arc::new(StateStore::load());
    println!("Polling as the app does; every change is printed. Ctrl+C to stop. elevated={}", sys::is_elevated());
    let _monitor = monitor::Monitor::start(providers::create(&state), state, || {});
    // The polling threads print through the log; this one just waits
    loop {
        std::thread::park();
    }
}

fn list() {
    let mut all = hid::enumerate(|_, _| true);
    all.sort_by(|a, b| (a.vendor_id, a.product_id, &a.path).cmp(&(b.vendor_id, b.product_id, &b.path)));
    for info in all {
        println!(
            "{info} mi={} product={}  {}",
            info.interface_number(),
            info.product.as_deref().unwrap_or("-"),
            info.path
        );
    }
}

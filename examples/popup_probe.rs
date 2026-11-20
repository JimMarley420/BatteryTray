//! Shows the reminder window and the overlay the way the app does - created, filled and shown from a callback the
//! event loop runs for a polling thread - and prints where their windows really are every 5 ms, for 2.5 s, with their
//! extended styles: `cargo run --example popup_probe`. For catching a popup that jumps when it opens, can be clicked
//! or takes focus. The overlay opens with seven named devices, enough for a second row, and is left with one after
//! 1.7 s: for catching a panel that doesn't follow its contents' size, either way.

use batterytray::alert_view::AlertView;
use batterytray::gui;
use batterytray::reading::DeviceKind;
use batterytray::rows::AlertItem;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::RECT;
use windows::Win32::UI::WindowsAndMessaging::{GetLayeredWindowAttributes, GetWindowRect, IsWindowVisible};

thread_local! {
    static VIEW: RefCell<Option<AlertView>> = const { RefCell::new(None) };
    static OVERLAY: RefCell<Option<batterytray::overlay::Overlay>> = const { RefCell::new(None) };
}

fn describe(window: &slint::Window) -> Option<String> {
    let h = gui::hwnd(window)?;
    let mut r = RECT::default();
    let mut alpha = 255u8;
    let (visible, ex) = unsafe {
        let _ = GetWindowRect(h, &mut r);
        let set = GetLayeredWindowAttributes(h, None, Some(&mut alpha), None).is_ok();
        if !set {
            alpha = 0; // a layered window without an opacity is not drawn at all
        }
        (
            IsWindowVisible(h).as_bool(),
            windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
                h,
                windows::Win32::UI::WindowsAndMessaging::GWL_EXSTYLE,
            ),
        )
    };
    // 0x20 transparent (click-through), 0x80 tool window, 0x80000 layered, 0x8000000 no-activate
    let flags: Vec<&str> = [(0x20, "transparent"), (0x80, "tool"), (0x80000, "layered"), (0x8000000, "noactivate")]
        .iter()
        .filter(|(bit, _)| ex & bit != 0)
        .map(|(_, name)| *name)
        .collect();
    Some(format!(
        "({}, {}) {}x{} visible={visible} drawn alpha={alpha} [{}]",
        r.left,
        r.top,
        r.right - r.left,
        r.bottom - r.top,
        flags.join(",")
    ))
}

/// The first `count` of seven devices on the air.
fn devices(count: usize) -> Vec<batterytray::rows::DeviceRow> {
    use batterytray::reading::DeviceReading;
    let names =
        ["Razer Barracuda X", "Attack Shark R3", "VXE R1 Pro Max", "AULA F75", "Xbox Controller", "MX Master 3S"];
    (0..count)
        .map(|i| {
            let (id, name) = (format!("d{i}"), names[i % names.len()]);
            let kind = if i % 2 == 0 { DeviceKind::Headset } else { DeviceKind::Mouse };
            let reading = DeviceReading {
                online: true,
                level: Some(37 + 9 * i as i32),
                ..DeviceReading::new(&id, name, kind, "2.4")
            };
            batterytray::rows::DeviceRow { id, current: Some(reading.clone()), last: Some(reading), name: name.into() }
        })
        .collect()
}

fn settings() -> batterytray::settings::AppSettings {
    batterytray::settings::AppSettings { overlay_corner: 3, overlay_show_names: true, ..Default::default() }
}

fn main() {
    gui::init().unwrap();
    batterytray::loc::set("ru");
    let (area, scale) = gui::primary_monitor();
    println!("work area {:?}, scale {scale}", (area.left, area.top, area.right, area.bottom));

    let start = Instant::now();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        let _ = slint::invoke_from_event_loop(move || {
            println!("{:>5} ms  created, filled and shown from invoke_from_event_loop", start.elapsed().as_millis());
            let mut view = AlertView::new(|| {}, || {}).unwrap();
            view.set_items(
                vec![AlertItem {
                    id: "m".into(),
                    name: "Razer Barracuda X".into(),
                    kind: DeviceKind::Headset,
                    level: 10,
                    critical: true,
                }],
                10,
            );
            view.show();
            VIEW.with(|v| *v.borrow_mut() = Some(view));

            // And the overlay's preview, the same way
            let mut overlay = batterytray::overlay::Overlay::new(Default::default());
            overlay.apply_settings(&settings());
            overlay.preview(&devices(7), &settings(), None);
            OVERLAY.with(|o| *o.borrow_mut() = Some(overlay));
        });
    });

    let sampler = slint::Timer::default();
    let last = RefCell::new(String::new());
    let shrunk = std::cell::Cell::new(false);
    sampler.start(slint::TimerMode::Repeated, Duration::from_millis(5), move || {
        let alert = VIEW.with(|v| v.borrow().as_ref().and_then(|v| describe(v.ui.window())));
        let overlay = OVERLAY.with(|o| o.borrow().as_ref().and_then(|o| o.window().and_then(describe)));
        let line = match (alert, overlay) {
            (None, None) => None,
            (a, o) => Some(format!("alert {}  |  overlay {}", a.unwrap_or_default(), o.unwrap_or_default())),
        };
        let Some(line) = line else { return };
        if *last.borrow() != line {
            println!("{:>5} ms  {line}", start.elapsed().as_millis());
            *last.borrow_mut() = line;
        }
        if start.elapsed() > Duration::from_millis(1700) && !shrunk.replace(true) {
            println!("{:>5} ms  six of the seven devices go off the air", start.elapsed().as_millis());
            OVERLAY.with(|o| {
                if let Some(o) = o.borrow_mut().as_mut() {
                    o.refresh(&devices(1), &settings(), None);
                }
            });
        }
        if start.elapsed() > Duration::from_millis(2600) {
            let _ = slint::quit_event_loop();
        }
    });
    slint::run_event_loop_until_quit().unwrap();
}

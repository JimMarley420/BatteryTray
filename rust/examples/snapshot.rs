//! Renders the windows with sample data to PNG files, off screen, with the same software renderer the app uses:
//! `cargo run --example snapshot -- <folder> [ru|en|zh] [scale]`. For checking the layout without running the app.

use batterytray::gui::{
    self, AlertRow, AlertWindow, DeviceCard, DeviceSettingsRow, MainWindow, OverlayEntry, OverlayWindow,
};
use batterytray::loc;
use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    static LAST: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
}

struct Offscreen;

impl Platform for Offscreen {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        LAST.with(|l| *l.borrow_mut() = Some(window.clone()));
        Ok(window)
    }
}

fn last() -> Rc<MinimalSoftwareWindow> {
    LAST.with(|l| l.borrow().clone().unwrap())
}

fn render(window: &MinimalSoftwareWindow, scale: f32, logical: (f32, f32), path: &std::path::Path) {
    window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: scale });
    let (w, h) = ((logical.0 * scale) as u32, (logical.1 * scale) as u32);
    window.set_size(slint::PhysicalSize::new(w, h));
    let mut buffer = vec![PremultipliedRgbaColor::default(); (w * h) as usize];
    window.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, w as usize);
    });
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    let bytes: Vec<u8> = buffer.iter().flat_map(|p| [p.red, p.green, p.blue, 255]).collect();
    writer.write_image_data(&bytes).unwrap();
    println!("{}", path.display());
}

fn rgb(r: u8, g: u8, b: u8) -> slint::Color {
    slint::Color::from_rgb_u8(r, g, b)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let folder = std::path::PathBuf::from(args.first().cloned().unwrap_or_else(|| ".".into()));
    loc::set(args.get(1).map(String::as_str).unwrap_or("ru"));
    let scale: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1.25);
    slint::platform::set_platform(Box::new(Offscreen)).unwrap();

    let main = MainWindow::new().unwrap();
    let window = last();
    gui::setup_texts(&main.global::<gui::T>());
    let card =
        |name: &str, glyph: &str, subtitle: &str, level: &str, color, charging, indeterminate, fill| DeviceCard {
            id: name.into(),
            name: name.into(),
            glyph: glyph.into(),
            subtitle: subtitle.into(),
            level_text: level.into(),
            color,
            charging,
            indeterminate,
            fill,
        };
    main.set_online(ModelRc::from(Rc::new(VecModel::from(vec![
        card("Razer Barracuda X", "\u{E7F6}", "2.4 ГГц", "37%", rgb(0x6C, 0xCB, 0x5F), false, false, 0.37),
        card(
            "Attack Shark R3",
            "\u{E962}",
            "Заряжается · кабель · процент будет виден без кабеля · до зарядки было 47%",
            "",
            rgb(0x60, 0xCD, 0xFF),
            true,
            true,
            0.0,
        ),
        card(
            "Rainy 75",
            "\u{E765}",
            "2.4 ГГц · примерно, по напряжению",
            "≈15%",
            rgb(0xF7, 0xC0, 0x4A),
            false,
            false,
            0.15,
        ),
    ]))));
    main.set_offline(ModelRc::from(Rc::new(VecModel::from(vec![card(
        "Xbox Wireless Controller",
        "\u{E7FC}",
        "Нет связи · последний раз 80%, 18.11 в 12:05",
        "—",
        rgb(0x7A, 0x7A, 0x7A),
        false,
        false,
        0.0,
    )]))));
    main.set_updated_text("Обновлено в 15:40:53".into());
    main.set_version_text("BatteryTray 1.99.0".into());
    main.set_banner_visible(true);
    main.set_banner_text("Доступна версия 2.0.0".into());
    main.set_banner_button_visible(true);
    render(&window, scale, (440.0, 640.0), &folder.join("main-devices.png"));

    main.set_tab(1);
    main.set_language("ru".into());
    main.set_overlay_enabled(true);
    main.set_shortcut_text("Ctrl+Alt+V".into());
    main.set_overlay_status("Горячая клавиша включена".into());
    main.set_overlay_corner(1);
    main.set_low(20);
    main.set_critical(10);
    main.set_repeat(10);
    main.set_repeat_text("10 мин".into());
    main.set_sound(true);
    main.set_notify_charged(true);
    main.set_auto_update(true);
    main.set_autostart(true);
    main.set_update_status("Установлена последняя версия 1.99.0 · проверено в 15:41".into());
    main.set_elevation_text(loc::t("elev.admin").into());
    main.set_device_settings(ModelRc::from(Rc::new(VecModel::from(vec![DeviceSettingsRow {
        id: "razer:barracuda-x".into(),
        glyph: "\u{E7F6}".into(),
        custom_name: "".into(),
        default_name: "Razer Barracuda X".into(),
        source: "2.4 ГГц · Razer Barracuda X".into(),
        visible: true,
        alerts: true,
        custom_thresholds: true,
        low: 30,
        critical: 15,
    }]))));
    render(&window, scale, (440.0, 2300.0), &folder.join("main-settings.png"));

    let alert = AlertWindow::new().unwrap();
    let window = last();
    gui::setup_texts(&alert.global::<gui::T>());
    alert.set_items(ModelRc::from(Rc::new(VecModel::from(vec![
        AlertRow { glyph: "\u{E7F6}".into(), name: "Razer Barracuda X".into(), level: "18%".into(), critical: false },
        AlertRow { glyph: "\u{E962}".into(), name: "Attack Shark R3".into(), level: "9%".into(), critical: true },
    ]))));
    alert.set_critical(true);
    alert.set_title_text(loc::t("alert.title.critical").into());
    alert.set_snooze_text("Напомнить через 10 мин".into());
    alert.set_hint_text(loc::t("alert.hint.critical").into());
    let height = alert.window().size().height as f32 / alert.window().scale_factor();
    render(&window, scale, (376.0, if height > 10.0 { height } else { 300.0 }), &folder.join("alert.png"));

    let overlay = OverlayWindow::new().unwrap();
    let window = last();
    gui::setup_texts(&overlay.global::<gui::T>());
    overlay.set_entries(ModelRc::from(Rc::new(VecModel::from(vec![
        OverlayEntry {
            glyph: "\u{E7F6}".into(),
            name: "Razer Barracuda X".into(),
            show_name: false,
            level: "37%".into(),
            color: rgb(0x6C, 0xCB, 0x5F),
            power: false,
        },
        OverlayEntry {
            glyph: "\u{E962}".into(),
            name: "Attack Shark R3".into(),
            show_name: true,
            level: "".into(),
            color: rgb(0x60, 0xCD, 0xFF),
            power: true,
        },
        OverlayEntry {
            glyph: "\u{E962}".into(),
            name: "VXE R1".into(),
            show_name: true,
            level: "82%".into(),
            color: rgb(0x6C, 0xCB, 0x5F),
            power: false,
        },
    ]))));
    render(&window, scale, (420.0, 44.0), &folder.join("overlay.png"));
}

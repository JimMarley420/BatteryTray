//! The main window's contents (UI/MainWindow.xaml.cs, UI/ViewModels.cs): what each card, row and setting shows,
//! built from the app's state. The window's callbacks go to the app (app.rs).

use crate::app::{level_color, with_app, App};
use crate::autostart;
use crate::gui::{self, DeviceCard, DeviceSettingsRow, Kind, MainWindow};
use crate::hotkey::{self, Gesture};
use crate::loc::{self, t, tf};
use crate::reading::DeviceKind;
use crate::rows::DeviceRow;
use crate::settings::AppSettings;
use crate::update_service::{Stage, Status};
use crate::updater::Version;
use crate::win;
use chrono::{DateTime, Local};
use slint::winit_030::{winit, EventResult, WinitWindowAccessor};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::rc::Rc;
use std::time::Duration;

/// Segoe Fluent Icons / MDL2 glyph for a device kind.
pub fn glyph(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Headset => "\u{E7F6}",
        DeviceKind::Mouse => "\u{E962}",
        DeviceKind::Keyboard => "\u{E765}",
        DeviceKind::Gamepad => "\u{E7FC}",
        DeviceKind::Other => "\u{E772}",
    }
}

fn color((r, g, b): (u8, u8, u8)) -> slint::Color {
    slint::Color::from_rgb_u8(r, g, b)
}

pub struct MainView {
    pub ui: MainWindow,
    online: Rc<VecModel<DeviceCard>>,
    offline: Rc<VecModel<DeviceCard>>,
    device_settings: Rc<VecModel<DeviceSettingsRow>>,
    /// Keeps "N min ago" fresh
    _clock: slint::Timer,
}

impl MainView {
    pub fn open(app: &App) -> Result<Self, slint::PlatformError> {
        let ui = gui::create(Kind::Normal, MainWindow::new)?;
        gui::setup_texts(&ui.global::<gui::T>());
        let view = Self {
            online: Rc::new(VecModel::default()),
            offline: Rc::new(VecModel::default()),
            device_settings: Rc::new(VecModel::default()),
            _clock: slint::Timer::default(),
            ui,
        };
        view.ui.set_online(ModelRc::from(view.online.clone()));
        view.ui.set_offline(ModelRc::from(view.offline.clone()));
        view.ui.set_device_settings(ModelRc::from(view.device_settings.clone()));
        view.ui.set_version_text(format!("BatteryTray {}", Version::current()).into());
        connect(&view.ui);
        view._clock.start(slint::TimerMode::Repeated, Duration::from_secs(15), || {
            with_app(|app| app.refresh_main());
        });

        view.load_settings(app);
        // Asked once here (and after a change): it means a call to the Task Scheduler
        view.ui.set_autostart(autostart::is_registered());
        view.refresh(&app.rows(), &app.settings);
        view.refresh_update(&app.update_status());

        // First shown in the bottom right corner, near the tray icon
        let area = gui::primary_work_area();
        let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForSystem() } as f32 / 96.0;
        let (w, h) = ((440.0 + 16.0) * scale, (640.0 + 40.0) * scale);
        view.ui.window().set_position(slint::PhysicalPosition::new(
            area.right - w as i32 - (12.0 * scale) as i32,
            area.bottom - h as i32 - (12.0 * scale) as i32,
        ));
        Ok(view)
    }

    /// Everything from the settings, as the window opens and after any change: the controls only ever show what the
    /// settings hold (a change the app refuses or adjusts is shown as it ends up).
    pub fn load_settings(&self, app: &App) {
        let s = &app.settings;
        let ui = &self.ui;
        ui.set_low(s.low_percent);
        ui.set_critical(s.critical_percent);
        ui.set_repeat(s.repeat_minutes);
        ui.set_repeat_text(tf("minutes", &[&s.repeat_minutes]).into());
        ui.set_sound(s.sound);
        ui.set_notify_charged(s.notify_charged);
        ui.set_auto_update(s.auto_update);
        ui.set_overlay_enabled(s.overlay_enabled);
        ui.set_overlay_mode(s.overlay_mode);
        ui.set_overlay_corner(s.overlay_corner);
        ui.set_overlay_names(s.overlay_show_names);
        ui.set_language(s.language.clone().into());
        ui.set_elevation_text(
            t(if win::is_elevated() {
                "elev.admin"
            } else if autostart::is_installed() {
                "elev.installed"
            } else {
                "elev.portable"
            })
            .into(),
        );
        self.refresh_overlay_status(s, app.overlay.shortcut_available());
    }

    pub fn refresh_overlay_status(&self, s: &AppSettings, available: bool) {
        self.ui.set_shortcut_text(
            hotkey::label(Gesture { key: s.overlay_hotkey_key, modifiers: s.overlay_hotkey_modifiers }).into(),
        );
        let error = s.overlay_enabled && !available;
        self.ui.set_overlay_status(
            t(if !s.overlay_enabled {
                "overlay.disabled"
            } else if error {
                "overlay.unavailable"
            } else {
                "overlay.ready"
            })
            .into(),
        );
        self.ui.set_overlay_status_error(error);
    }

    /// A shortcut that can't be taken: says why, in red, until the next change.
    pub fn shortcut_refused(&self, valid: bool) {
        self.ui.set_overlay_status(t(if valid { "overlay.conflict" } else { "overlay.invalid" }).into());
        self.ui.set_overlay_status_error(true);
    }

    pub fn refresh(&self, rows: &[DeviceRow], s: &AppSettings) {
        let now = Local::now();
        let shown: Vec<&DeviceRow> = rows.iter().filter(|r| !s.device(&r.id).hidden).collect();
        sync(&self.online, shown.iter().filter(|r| r.online()).map(|r| card(r, s, now)).collect());
        sync(&self.offline, shown.iter().filter(|r| !r.online()).map(|r| card(r, s, now)).collect());

        let newest = rows.iter().filter_map(|r| r.current.as_ref().map(|c| c.time)).max();
        self.ui.set_updated_text(
            match newest {
                Some(t) => tf("updated.at", &[&t.format("%H:%M:%S")]),
                None => t("polling"),
            }
            .into(),
        );

        // Rebuilt only when the set of devices changes, so a name being typed keeps its focus. Its order doesn't go
        // by that name, or every keystroke that changed it would be such a change
        let fresh: Vec<DeviceSettingsRow> =
            crate::rows::settings_order(rows).into_iter().map(|r| settings_row(r, s)).collect();
        let ids_now: Vec<SharedString> = self.device_settings.iter().map(|r| r.id).collect();
        if ids_now != fresh.iter().map(|r| r.id.clone()).collect::<Vec<_>>() {
            self.device_settings.set_vec(fresh);
        } else {
            for (i, row) in fresh.into_iter().enumerate() {
                if self.device_settings.row_data(i).as_ref() != Some(&row) {
                    self.device_settings.set_row_data(i, row);
                }
            }
        }
    }

    pub fn refresh_update(&self, u: &Status) {
        let version = Version::current().to_string();
        let latest = u.latest.as_ref().map(|l| l.version.to_string()).unwrap_or_default();
        let status = match u.stage {
            Stage::Checking => t("upd.checking"),
            Stage::UpToDate => tf(
                "upd.uptodate",
                &[&version, &u.last_check.map(|c| c.format("%H:%M").to_string()).unwrap_or_default()],
            ),
            Stage::Available => tf("upd.available", &[&latest, &version]),
            Stage::Installing if u.progress > 0 && u.progress < 100 => tf("upd.downloading", &[&latest, &u.progress]),
            Stage::Installing => tf("upd.installing", &[&latest]),
            Stage::Failed => tf("upd.failed", &[&u.error.clone().unwrap_or_default()]),
            Stage::NotChecked => tf("upd.idle", &[&version]),
        };
        let newer = u.newer_available();
        self.ui.set_install_visible(newer && matches!(u.stage, Stage::Available | Stage::Failed));
        self.ui.set_install_text(tf("upd.install", &[&latest]).into());
        self.ui.set_check_enabled(!matches!(u.stage, Stage::Checking | Stage::Installing));

        // Release notes are Markdown; shown as plain text
        let notes = if newer {
            u.latest.as_ref().map(|l| l.notes.replace("**", "").replace('`', "").trim().to_string())
        } else {
            None
        }
        .unwrap_or_default();
        let notes = if notes.chars().count() > 600 {
            notes.chars().take(600).collect::<String>().trim_end().to_string() + "…"
        } else {
            notes
        };
        self.ui.set_update_notes(notes.into());

        self.ui.set_banner_visible(newer && matches!(u.stage, Stage::Available | Stage::Installing));
        self.ui.set_banner_text(
            if u.stage == Stage::Installing { status.clone() } else { tf("upd.banner", &[&latest]) }.into(),
        );
        self.ui.set_banner_button_visible(u.stage == Stage::Available);
        self.ui.set_update_status(status.into());
    }
}

/// Replaces a model's rows only when something changed, so nothing redraws for nothing.
fn sync(model: &VecModel<DeviceCard>, cards: Vec<DeviceCard>) {
    if model.iter().collect::<Vec<_>>() != cards {
        model.set_vec(cards);
    }
}

/// One card on the devices tab, or one row under "Not connected".
fn card(row: &DeviceRow, s: &AppSettings, now: DateTime<Local>) -> DeviceCard {
    let mut parts = Vec::new();
    let (mut level, mut estimate, mut unknown, mut charging) = (None, false, false, false);
    let mut level_rgb = (0x7A, 0x7A, 0x7A);
    if row.online() {
        let cur = row.current.as_ref().expect("online rows have a reading");
        level = cur.level;
        estimate = cur.estimate;
        unknown = cur.charging && level.is_none();
        if cur.charging {
            parts.push(t("state.charging"));
        } else if cur.full_on_charger {
            parts.push(t("state.full"));
        }
        parts.push(cur.via.clone());
        if let Some(detail) = cur.detail.as_ref().filter(|d| !d.is_empty()) {
            parts.push(detail.clone());
        }
        charging = cur.charging;
        level_rgb = level_color(level, cur.charging, true, &row.id, s);
    } else {
        // The reason, when the provider knows it ("asleep or off"), reads better than a bare "Not connected"
        let why = row.current.as_ref().and_then(|c| c.detail.clone()).filter(|d| !d.is_empty());
        parts.push(match why {
            Some(why) => {
                let mut chars = why.chars();
                chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
            }
            None => t("state.offline"),
        });
        if let Some(last) = &row.last {
            if let Some(was) = last.level {
                parts.push(tf("last.seen", &[&was, &ago(last.time, now)]));
            }
        }
    }
    DeviceCard {
        id: row.id.clone().into(),
        name: row.name.clone().into(),
        glyph: glyph(row.kind()).into(),
        subtitle: parts.join(" · ").into(),
        // An estimate is marked as one; while charging with no readable level the lightning alone says it all
        level_text: match level {
            Some(l) => format!("{}{l}%", if estimate { "≈" } else { "" }),
            None if unknown => String::new(),
            None => "—".into(),
        }
        .into(),
        color: color(level_rgb),
        charging,
        indeterminate: unknown,
        fill: (level.unwrap_or(0) as f32 / 100.0).clamp(0.0, 1.0),
    }
}

fn ago(time: DateTime<Local>, now: DateTime<Local>) -> String {
    let d = now - time;
    if d.num_minutes() < 1 {
        t("ago.now")
    } else if d.num_hours() < 1 {
        tf("ago.min", &[&d.num_minutes()])
    } else if d.num_days() < 1 {
        tf("ago.hours", &[&d.num_hours()])
    } else {
        loc::date_time(&time)
    }
}

/// One device block on the settings tab.
fn settings_row(row: &DeviceRow, s: &AppSettings) -> DeviceSettingsRow {
    let ds = s.device(&row.id);
    let any = row.any();
    DeviceSettingsRow {
        id: row.id.clone().into(),
        glyph: glyph(row.kind()).into(),
        custom_name: ds.name.clone().unwrap_or_default().into(),
        default_name: any.name.clone().into(),
        source: format!("{} · {}", any.via, any.name).into(),
        visible: !ds.hidden,
        alerts: ds.alerts,
        custom_thresholds: ds.low_percent.is_some() || ds.critical_percent.is_some(),
        low: ds.low_percent.unwrap_or(s.low_percent),
        critical: ds.critical_percent.unwrap_or(s.critical_percent),
    }
}

/// The window's callbacks: each one changes the app and lets it refresh what depends on it.
fn connect(ui: &MainWindow) {
    ui.on_refresh(|| {
        with_app(|a| a.refresh_now());
    });
    ui.on_install_update(|| {
        with_app(|a| a.updates_install());
    });
    ui.on_check_updates(|| {
        with_app(|a| a.updates_check());
    });
    ui.on_language_chosen(|l| {
        with_app(|a| a.set_language(&l));
    });
    ui.on_test_alert(|| {
        with_app(|a| a.show_test_alert());
    });
    ui.on_open_folder(|| {
        let _ = std::process::Command::new("explorer.exe").arg(crate::log::folder()).spawn();
    });
    ui.on_autostart_changed(|on| {
        with_app(|a| a.set_autostart(on));
    });
    ui.on_overlay_reset(|| {
        with_app(|a| {
            a.set_overlay_shortcut(Gesture {
                key: crate::settings::DEFAULT_HOTKEY.0,
                modifiers: crate::settings::DEFAULT_HOTKEY.1,
            })
        });
    });
    ui.on_overlay_preview(|| {
        with_app(|a| a.overlay_preview());
    });

    ui.on_low_changed(|v| {
        with_app(|a| {
            a.edit(|s| {
                s.low_percent = v;
                // The critical level stays at least 5 below the reminder level
                s.critical_percent = s.critical_percent.min(v - 5);
            })
        });
    });
    ui.on_critical_changed(|v| {
        with_app(|a| {
            a.edit(|s| {
                s.critical_percent = v;
                s.low_percent = s.low_percent.max(v + 5);
            })
        });
    });
    ui.on_repeat_changed(|v| {
        with_app(|a| a.edit(|s| s.repeat_minutes = v));
    });
    ui.on_sound_changed(|on| {
        with_app(|a| a.edit(|s| s.sound = on));
    });
    ui.on_notify_charged_changed(|on| {
        with_app(|a| a.edit(|s| s.notify_charged = on));
    });
    ui.on_auto_update_changed(|on| {
        with_app(|a| a.edit(|s| s.auto_update = on));
    });
    ui.on_overlay_enabled_changed(|on| {
        with_app(|a| a.edit(|s| s.overlay_enabled = on));
    });
    ui.on_overlay_mode_chosen(|m| {
        with_app(|a| a.edit(|s| s.overlay_mode = m));
    });
    ui.on_overlay_corner_chosen(|c| {
        with_app(|a| a.edit(|s| s.overlay_corner = c));
    });
    ui.on_overlay_names_changed(|on| {
        with_app(|a| a.edit(|s| s.overlay_show_names = on));
    });

    ui.on_device_name_edited(|id, text| {
        with_app(|a| {
            a.edit(|s| {
                let name = text.trim();
                s.device_mut(&id).name = if name.is_empty() { None } else { Some(text.to_string()) };
            })
        });
    });
    ui.on_device_visible_changed(|id, on| {
        with_app(|a| a.edit(|s| s.device_mut(&id).hidden = !on));
    });
    ui.on_device_alerts_changed(|id, on| {
        with_app(|a| a.edit(|s| s.device_mut(&id).alerts = on));
    });
    ui.on_device_thresholds_changed(|id, on| {
        with_app(|a| {
            a.edit(|s| {
                // Turning it on starts from the global levels, turning it off goes back to following them
                let (low, critical) = (s.low_percent, s.critical_percent);
                let d = s.device_mut(&id);
                d.low_percent = on.then_some(low);
                d.critical_percent = on.then_some(critical);
            })
        });
    });
    ui.on_device_low_changed(|id, v| {
        with_app(|a| {
            a.edit(|s| {
                let d = s.device_mut(&id);
                if d.low_percent.is_none() && d.critical_percent.is_none() {
                    return; // only while this device's own levels are on
                }
                let low = v.clamp(10, 50);
                d.low_percent = Some(low);
                if d.critical_percent.is_some_and(|c| c > low - 5) {
                    d.critical_percent = Some(low - 5);
                }
            })
        });
    });
    ui.on_device_critical_changed(|id, v| {
        with_app(|a| {
            a.edit(|s| {
                let d = s.device_mut(&id);
                if d.low_percent.is_none() && d.critical_percent.is_none() {
                    return;
                }
                let critical = v.clamp(5, 45);
                d.critical_percent = Some(critical);
                if d.low_percent.is_some_and(|l| critical > l - 5) {
                    d.low_percent = Some((critical + 5).min(50));
                }
            })
        });
    });

    // Closing drops the window and everything it holds; it is built again the next time it opens
    ui.window().on_close_requested(|| {
        slint::Timer::single_shot(Duration::ZERO, || {
            with_app(|a| a.close_main());
        });
        slint::CloseRequestResponse::HideWindow
    });

    // The shortcut field records the next key combination from the raw key event: the physical key, so the same
    // shortcut on every keyboard layout
    let weak = ui.as_weak();
    ui.window().on_winit_window_event(move |_, event| {
        use winit::event::{ElementState, WindowEvent};
        use winit::platform::scancode::PhysicalKeyExtScancode;
        let WindowEvent::KeyboardInput { event, .. } = event else { return EventResult::Propagate };
        if !weak.upgrade().is_some_and(|ui| ui.get_recording_shortcut()) {
            return EventResult::Propagate;
        }
        let Some(scancode) = event.physical_key.to_scancode() else { return EventResult::Propagate };
        let vk = hotkey::virtual_key(scancode);
        if vk == 0x09 {
            return EventResult::Propagate; // keyboard users can leave the field
        }
        if event.state == ElementState::Pressed && !event.repeat && !hotkey::is_modifier(vk) {
            let gesture = Gesture { key: vk as i32, modifiers: hotkey::modifiers_down() };
            slint::Timer::single_shot(Duration::ZERO, move || {
                with_app(|a| a.set_overlay_shortcut(gesture));
            });
        }
        EventResult::PreventDefault
    });
}

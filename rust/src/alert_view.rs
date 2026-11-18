//! The low battery reminder window (UI/AlertWindow.xaml.cs): in the bottom right corner, on top, never taking focus,
//! until the user snoozes or mutes it.
//! It opens unseen and becomes visible only once it stands in its corner at its real size. Right after show() the
//! window still has the height Slint guessed before it existed (48 px short at 125%: text is measured exactly only
//! then), and gets its real one a moment later.

use crate::gui::{self, AlertRow, AlertWindow, Kind};
use crate::loc::{t, tf};
use crate::overlay::Rect;
use crate::rows::AlertItem;
use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;
use windows::Win32::Foundation::RECT;
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

pub struct AlertView {
    pub ui: AlertWindow,
    items: Vec<AlertItem>,
    rows: Rc<VecModel<AlertRow>>,
    /// Shown but not yet seen: waiting for its real size
    revealing: Rc<Cell<bool>>,
    reveal_timer: Rc<slint::Timer>,
    _keep_on_top: slint::Timer,
}

impl AlertView {
    /// A reminder window; its two answers call `snooze` and `mute`.
    pub fn new(snooze: impl Fn() + 'static, mute: impl Fn() + 'static) -> Result<Self, slint::PlatformError> {
        let ui = gui::create(Kind::Reminder, AlertWindow::new)?;
        gui::setup_texts(&ui.global::<gui::T>());
        let rows = Rc::new(VecModel::default());
        ui.set_items(ModelRc::from(rows.clone()));
        ui.on_snooze(snooze);
        ui.on_mute(mute);
        let revealing = Rc::new(Cell::new(false));
        let reveal_timer = Rc::new(slint::Timer::default());

        // Whenever its size changes (it gets its real one, a longer list, another scale): back into its corner. While
        // it is still unseen, it is shown once its size has stayed the same for a moment.
        let weak = ui.as_weak();
        let (r, timer) = (revealing.clone(), reveal_timer.clone());
        ui.window().on_winit_window_event(move |_, event| {
            use slint::winit_030::winit::event::WindowEvent;
            if matches!(event, WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }) {
                if let Some(ui) = weak.upgrade() {
                    place(&ui);
                    if r.get() {
                        reveal_after(&timer, &ui, &r, 40);
                    }
                }
            }
            slint::winit_030::EventResult::Propagate
        });
        let keep_on_top = slint::Timer::default();
        let weak = ui.as_weak();
        // Full-screen apps and other topmost windows can push it back; reasserted every few seconds
        keep_on_top.start(slint::TimerMode::Repeated, Duration::from_secs(3), move || {
            if let Some(ui) = weak.upgrade() {
                gui::keep_on_top(ui.window());
                place(&ui);
            }
        });
        Ok(Self { ui, items: Vec::new(), rows, revealing, reveal_timer, _keep_on_top: keep_on_top })
    }

    pub fn ids(&self) -> Vec<String> {
        self.items.iter().map(|i| i.id.clone()).collect()
    }

    /// True when the window already shows this device at this severity (no new sound needed).
    pub fn shows(&self, item: &AlertItem) -> bool {
        self.items.iter().any(|i| i.id == item.id && i.critical == item.critical)
    }

    pub fn set_items(&mut self, due: Vec<AlertItem>, repeat_minutes: i32) {
        let critical = due.iter().any(|d| d.critical);
        self.ui.set_critical(critical);
        self.ui.set_title_text(
            t(if critical {
                "alert.title.critical"
            } else if due.len() == 1 {
                "alert.title"
            } else {
                "alert.title.many"
            })
            .into(),
        );
        self.ui.set_snooze_text(tf("alert.snooze", &[&repeat_minutes]).into());
        self.ui.set_hint_text(t(if critical { "alert.hint.critical" } else { "alert.hint" }).into());
        self.rows.set_vec(
            due.iter()
                .map(|d| AlertRow {
                    glyph: crate::main_view::glyph(d.kind).into(),
                    name: d.name.clone().into(),
                    level: format!("{}%", d.level).into(),
                    critical: d.critical,
                })
                .collect::<Vec<_>>(),
        );
        self.items = due;
        if self.ui.window().is_visible() {
            self.style(); // the border follows the severity; a new height is placed by the resize handler
        }
    }

    pub fn show(&self) {
        if self.ui.window().is_visible() {
            return;
        }
        // Near its corner already (by the guessed size), unseen until it has its real size there
        let (area, scale) = gui::primary_monitor();
        let (w, h) = ((376.0 * scale).round() as i32, (self.ui.get_content_height() * scale).round() as i32);
        let margin = (16.0 * scale).round() as i32;
        self.ui.window().set_position(slint::PhysicalPosition::new(area.right - w - margin, area.bottom - h - margin));
        self.revealing.set(true);
        let _ = self.ui.show();
        // Seen once its size has settled (each resize starts this again; none may come if it was right already)
        reveal_after(&self.reveal_timer, &self.ui, &self.revealing, 80);
    }

    fn style(&self) {
        gui::set_border(self.ui.window(), Some(accent(&self.ui)));
    }

    /// Where it is on the screen, for the overlay to stay clear of it.
    pub fn bounds(&self) -> Option<Rect> {
        if !self.ui.window().is_visible() {
            return None;
        }
        let hwnd = gui::hwnd(self.ui.window())?;
        let mut r = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut r).ok()? };
        Some(Rect { x: r.left as f64, y: r.top as f64, w: (r.right - r.left) as f64, h: (r.bottom - r.top) as f64 })
    }

    pub fn close(&self) {
        let _ = self.ui.hide();
    }
}

/// The border's color: the severity's, as the accent bar on the left.
fn accent(ui: &AlertWindow) -> (u8, u8, u8) {
    if ui.get_critical() {
        (0xFF, 0x6B, 0x6B)
    } else {
        (0xF7, 0xC0, 0x4A)
    }
}

/// In ms, unless its size changes again first: into its corner, and seen. Windows creates the window only once the
/// event loop gets to it after show(); until then this waits for it.
fn reveal_after(timer: &Rc<slint::Timer>, ui: &AlertWindow, revealing: &Rc<Cell<bool>>, ms: u64) {
    let weak = ui.as_weak();
    let (again, revealing) = (timer.clone(), revealing.clone());
    timer.start(slint::TimerMode::SingleShot, Duration::from_millis(ms), move || {
        let Some(ui) = weak.upgrade() else { return };
        if gui::hwnd(ui.window()).is_none() {
            reveal_after(&again, &ui, &revealing, 20);
            return;
        }
        place(&ui);
        if revealing.replace(false) {
            gui::set_border(ui.window(), Some(accent(&ui)));
            gui::set_alpha(ui.window(), 255);
        }
    });
}

/// The bottom right corner of the primary screen's work area, 16 px in from its edges: by the window's real size.
fn place(ui: &AlertWindow) {
    let Some((r, scale)) = gui::window_rect(ui.window()) else { return };
    let area = gui::primary_work_area();
    let margin = (16.0 * scale).round() as i32;
    let (x, y) = (area.right - (r.right - r.left) - margin, area.bottom - (r.bottom - r.top) - margin);
    if (r.left, r.top) != (x, y) {
        gui::move_window(ui.window(), x, y);
    }
}

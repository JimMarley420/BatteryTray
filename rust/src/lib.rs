//! BatteryTray, ported from the C# app: the HID layer (Hid/), the battery protocols (Providers/), what they need
//! from Core/ (readings, state.json, texts, log, the polling loop), and the app around them (tray, settings,
//! autostart, updater). The windows come next.

pub mod alert_view;
pub mod app;
pub mod autostart;
pub mod diag;
pub mod gui;
pub mod hid;
pub mod hotkey;
pub mod http;
pub mod loc;
pub mod log;
pub mod main_view;
pub mod monitor;
pub mod overlay;
pub mod providers;
pub mod reading;
pub mod rows;
pub mod settings;
pub mod state;
pub mod tray;
pub mod update_service;
pub mod updater;
pub mod win;

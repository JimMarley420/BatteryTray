# BatteryTray in Rust

The Rust version of BatteryTray: the same app as the C# one in the parent folder, with the same protocols, settings,
logon task and self-update, in an exe a fraction of the C# one's size that needs a few MB of memory instead of ~200 MB.
It replaces the C# exe in place: installed C# copies update themselves to it (see "Moving from the C# app" below).

## Building

Needs Rust (stable, see `rust-version` in `Cargo.toml`) and the MSVC build tools with a Windows SDK (`rc.exe` compiles
the icon, manifest and version resource).

```
cargo build --release        # target\release\BatteryTray.exe and batterytray-core.exe
cargo test                   # unit tests, no devices needed
cargo test -- --ignored      # also downloads the latest release from GitHub and checks it as an update would
cargo fmt --check            # formatting (rustfmt.toml)
cargo clippy --all-targets -- -D warnings
```

The C runtime is linked into the exe (`.cargo/config.toml` in the parent folder), so it starts on a Windows without
the Visual C++ redistributable. CI runs the release exe on such a Windows, Server Core in a container
(`.github/scripts/clean-windows.ps1`), and `build-release.ps1` refuses an exe that names the runtime's DLLs.

The version in `Cargo.toml` is the release version: the updater (this one and the C# one) only installs an exe whose
FileVersion equals the release's, and `build.rs` writes it into the exe. `build-release.ps1` in the parent folder builds
the release exe and zip; it checks that version and writes the crates' licenses with
[cargo-about](https://github.com/EmbarkStudios/cargo-about) (`about.toml`, `about.hbs`).

## Layout

| | |
|---|---|
| `src/hid.rs`, `src/win.rs` | HID enumeration and I/O over hand-declared Win32 calls: overlapped reads with a timeout that cancel what they started, so a late reply is never read as the answer to the next request |
| `src/providers/` | one module per device family, ported from `Providers/*.cs` byte for byte; each says where its protocol facts come from |
| `src/monitor.rs` | polls every provider on its own thread and schedule; providers whose devices push changes cut their wait short |
| `src/state.rs`, `src/settings.rs` | `state.json` and `settings.json` in `%LOCALAPPDATA%\BatteryTray`, in the C# app's format |
| `src/app.rs` | startup, single instance, the tray, reminders, the overlay and updates; everything UI runs in Slint's event loop |
| `src/gui.rs`, `ui/*.slint` | the windows (Slint, software renderer), and the Win32 parts Slint has no API for |
| `src/updater.rs`, `src/update_service.rs`, `src/http.rs` | self-update from GitHub Releases over WinHTTP |
| `src/autostart.rs` | the logon task (elevated when installed in Program Files) |

## Tools

- `batterytray-core dump | watch | list` - a console tool for comparing readings with the C# app on real devices:
  `dump` prints what `BatteryTray.exe --dump` writes, `watch` polls as the app does and prints every change, `list`
  prints every HID collection. It reads the app's `state.json` but never writes it, and logs to
  `%LOCALAPPDATA%\BatteryTray\rust-core`.
- `cargo run --example snapshot -- <folder> [ru|en|zh] [scale]` - renders the windows with sample data to PNG files,
  off screen, to check a layout without running the app.
- `cargo run --example popup_probe` - opens the reminder and the overlay as the app does and prints where their windows
  are every 5 ms, with their extended styles: for catching a popup that jumps, can be clicked or takes focus.

## Moving from the C# app

The C# updater installs the "BatteryTray.exe" asset of the latest (non-prerelease) release when its size and SHA-256
match what GitHub publishes and its FileVersion equals the release version. The Rust exe has the same name and a
version resource, and it keeps everything the C# app relied on: the `BatteryTray.SingleInstance` mutex, the
`BatteryTray.Activator` window a second launch signals, the `BatteryTray` logon task, the `--background`, `--update`,
`--register-task`, `--unregister-task` and `--dump` switches, and the same `settings.json` and `state.json` (values it
doesn't know are kept). On its first start it removes what the C# app left behind (`BatteryTray.old.exe` and the
.NET single-file extraction folder in `%TEMP%`).

## Licenses

MIT, as the rest of the project. The licenses of the crates in the exe go into `THIRD_PARTY_LICENSES.md` in the release
zip (see above). The UI is built with [Slint](https://slint.dev) under its Royalty-free license 2.0,
which asks for an attribution: the Slint badge on a public page the app is downloaded from - the main README, which the
releases link to. Without it the app may not be distributed.

[![Made with Slint](https://raw.githubusercontent.com/slint-ui/slint/master/logo/MadeWithSlint-logo-whitebg.png)](https://slint.dev)

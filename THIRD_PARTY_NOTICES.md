# Third-party notices

## razer-barracuda-x-battery

The Razer Barracuda X protocol code (`rust/src/providers/barracuda.rs`), the HID helpers (`rust/src/hid.rs`) and the tray icon renderer (`rust/src/tray.rs`) are adapted from [razer-barracuda-x-battery](https://github.com/mehdibouchami/razer-barracuda-x-battery) by Mehdi Bouchami, used under the MIT License:

```
MIT License

Copyright (c) 2026 Mehdi Bouchami

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## rivalcfg

The SteelSeries battery commands (`rust/src/providers/steelseries.rs`) follow [rivalcfg](https://github.com/flozz/rivalcfg) by Fabien Loison, licensed under the WTFPL.

## ATTACK SHARK web driver

The Rongyuan receiver status command and the list of receiver IDs (`rust/src/providers/rongyuan.rs`) were found by reading the official ATTACK SHARK web driver (qmk.top) for interoperability; no code from it is included.

## Protocol references (facts only, no code copied)

The following projects document the device protocols that the providers below implement independently. Only protocol facts (command bytes, report layouts, device IDs) were taken from them; none of their code is included, so their licenses don't apply to this project's code.

- `rust/src/providers/razer_peripheral.rs` — report layout, commands, per-device transaction IDs and waits from [OpenRazer](https://github.com/openrazer/openrazer) (GPL-2.0).
- `rust/src/providers/logitech.rs` — HID++ requests, battery features and voltage curve from [Solaar](https://github.com/pwr-Solaar/Solaar) (GPL-2.0), the Linux driver `drivers/hid/hid-logitech-hidpp.c` (GPL-2.0) and [LGSTrayBattery](https://github.com/andyvorld/LGSTrayBattery); the 2012 Logitech HID++ 2.0 draft specification.
- `rust/src/providers/xenta.rs` — the battery frame of the Xenta receiver `1D57:FA60` (`03 MM 40 ST LL`), its states, the false 100% on link-up, the cable IDs and the model codes from [attackshark-battery-windows](https://github.com/daeoxide/attackshark-battery-windows) (MIT; `docs/PROTOCOL.md`), [Attack-Shark-Battery-KDE](https://gitlab.com/gabriel17166/Attack-Shark-Battery-KDE) (MIT) and the [HaloBattery community fork](https://github.com/ahmedkhursheed23/HaloBattery) (MIT; `providers/inphic.py`).
- `rust/src/providers/headset.rs` — headset requests and responses from [HeadsetControl](https://github.com/Sapd/HeadsetControl) (GPL-3.0); those of the HyperX Cloud III Wireless, and sending a request as a feature report when Windows doesn't take it as an output one, from [HyperHeadset](https://github.com/LennardKittner/HyperHeadset) (MIT); the IDs of the Corsair HS80 RGB Wireless and its Bragi requests, the same as the Virtuoso XT's, from [OpenLinkHub](https://github.com/jurkovic-nikola/OpenLinkHub) (GPL-3.0); the meaning of the Bragi charge states from [ckb-next](https://github.com/ckb-next/ckb-next) (GPL-2.0).
- `rust/src/providers/oem_mouse.rs` — the 17-byte frame, its `0x55` checksum and the `GetPower` command and reply of VXE mice, and the VXE product IDs, from [OpenMouse](https://github.com/OpenMouse-Project/mouse-protocol) (AGPL-3.0) and [vxe-battery-tray](https://github.com/ivancacic/vxe-battery-tray) (MIT); the Areson transport (feature report out, input report `09` in on another collection) and the reply of the `25A7:FA7C` receiver from [redragon-mouse-linux](https://github.com/rajchauhan28/redragon-mouse-linux) (`PROTOCOL.md`) and [Wireless-Device-Battery-Monitor](https://github.com/krisgfx9/Wireless-Device-Battery-Monitor) (GPL-3.0; `docs/areson.md`, `src/drivers/fantech.cpp`).
- `rust/src/providers/aula_mchose.rs` — the `3554:FA09` receiver's battery and model code requests, their replies, the MCHOSE model codes and a G75 Pro owner's captured replies from [myfinder-device-battery](https://github.com/wushuai0802/myfinder-device-battery) (AGPL-3.0; `engine/upstream-protocols.md`, its reading of the official M HUB web driver); the AULA charging flag, the top bit some firmware sets on the echoed command, and that a request may wake a dozing keyboard, from [Wireless-Device-Battery-Monitor](https://github.com/krisgfx9/Wireless-Device-Battery-Monitor) (GPL-3.0; `src/drivers/aula.cpp`, issues 57, 137 and 191).
- `rust/src/providers/playstation.rs` — DualSense / DualShock 4 battery bytes from the Linux kernel driver `drivers/hid/hid-playstation.c` (GPL-2.0).
- `rust/src/providers/xinput.rs` — Bluetooth product IDs of Xbox controllers as listed by [SDL](https://github.com/libsdl-org/SDL) (zlib).

## Rust crates and Slint

The exe is built from open-source Rust crates (MIT, Apache-2.0, Unicode-3.0 and other permissive licenses); their license texts are in `THIRD_PARTY_LICENSES.md` in the release zip, generated by `build-release.ps1` from `rust/Cargo.lock`. The windows are drawn with [Slint](https://slint.dev), used under the Slint Royalty-free Desktop, Mobile, and Web Applications License 2.0 (its attribution badge is in the README).

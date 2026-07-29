# l-lightning

Control a generic **ELK-BLEDOM** Bluetooth-LE RGB LED strip from this machine — a
replacement for the lost remote and the dead vendor app. **Rust core + Node UI**
(UI to come; see the spec).

## Status

Proof of concept **working**: a Rust binary (`poc/`, using [btleplug]) scans for
the controller, connects, and drives power / brightness / full RGB. Verified
on-device on a Surface Go 3 (Arch + BlueZ), ending on a user-confirmed purple.

## Quick start (POC)

```sh
cargo run --manifest-path poc/Cargo.toml
```

Power-cycle the strip when it prints `scanning`; keep your phone's Bluetooth off
(the controller accepts only one connection at a time).

## How it works

The controller speaks a simple BLE GATT protocol — 9-byte `0x7E…0xEF` packets
written to characteristic `fff3`, with a subscription on `fff4` to hold the link
open. Full protocol, the device's GATT map, and the connection quirks are in
[`CLAUDE.md`](./CLAUDE.md).

[btleplug]: https://github.com/deviceplug/btleplug

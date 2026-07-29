# l-lightning

On-device controller for a generic **ELK-BLEDOM** Bluetooth-LE RGB LED strip (the
vendor remote was lost and the app never worked). **Rust core + Node UI.**

Garden-side framing (why it exists, status, milestones) lives in the soft-fig
garden at `~/soft-fig_garden/projects/l-lightning/`. **This file is authoritative
for code.**

## Layout
- `poc/` — proof-of-concept binary crate (`l-lightning-poc`, [btleplug]). Scans for
  the controller, connects, and drives a power/brightness/colour sequence. A
  throwaway spike; the real workspace is defined in the spec phase.

Planned post-spec (layout TBD): a `core` crate (BLE + protocol + connection state
machine), a small daemon/bridge, and a Node UI.

## Build & run the POC
```sh
cargo run --manifest-path poc/Cargo.toml
```
Power-cycle the strip when it prints `scanning`, and keep it near the machine.
Requires BlueZ running and the phone's Bluetooth **off** (one central at a time).

## The device
- Name `ELK-BLEDOM06`, address `BE:67:00:A5:CC:56` (the POC matches any
  `ELK-BLEDOM*` name, with this address as a fallback).
- Write channel: characteristic `0000fff3-…` (`WRITE_WITHOUT_RESPONSE`).
- Keepalive: subscribe `0000fff4-…` (`NOTIFY`).

## Protocol — 9-byte packets (`0x7E … 0xEF`) written to `fff3`
| action | packet |
|---|---|
| power on | `7E 00 04 F0 00 01 FF 00 EF` |
| power off | `7E 00 04 00 00 00 FF 00 EF` |
| set colour | `7E 00 05 03 RR GG BB 00 EF` |
| brightness % | `7E 00 01 NN 00 00 00 00 EF` |

## Known quirks (detail in the garden note `notes/001-poc-elk-bledom.md`)
- Advertises reliably only right after power-on; idle adverts are weak.
- `connect()` may spuriously report `le-connection-abort-by-local` or a
  service-discovery timeout — retry and poll for the `fff3` characteristic.
- Drops the link after the first write(s); subscribe `fff4`, settle ~700 ms, and
  auto-reconnect on write failure.
- Don't `bluetoothctl remove` the device — it wipes BlueZ's cached GATT.

[btleplug]: https://github.com/deviceplug/btleplug

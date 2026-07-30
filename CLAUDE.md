# l-lightning

On-device controller for a generic **ELK-BLEDOM** Bluetooth-LE RGB LED strip (the
vendor remote was lost and the app never worked). **Rust core + Node UI.**

Garden-side framing (why it exists, status, milestones) lives in the soft-fig
garden at `~/soft-fig_garden/projects/l-lightning/`. **This file is authoritative
for code.**

## Layout
- `crates/core` — library crate (`l-lightning-core`): ELK-BLEDOM protocol encoder,
  connection state machine (scan → connect → reconnect with exponential backoff),
  command layer with coalescing throttle.
- `crates/daemon` — binary crate (`l-lightningd`): Unix-socket JSON-RPC 2.0 server
  (NDJSON framing) wrapping the core. Full method surface: `get_state`, `set_power`,
  `set_brightness`, `set_color`, `list_presets`/`save`/`delete`/`apply_preset`,
  `get_config`/`set_config`, `start_effect`/`stop_effect` (breathe / color_cycle /
  strobe / fade_to), `reconnect`/`rescan`. Notifications: `state`, `connection`.
  Config at `$XDG_CONFIG_HOME/l-lightning/config.toml`.
- `crates/cli` — binary crate (`l-lightning`): thin JSON-RPC client over the
  Unix socket. Auto-spawns the daemon if not running. Subcommands: `status`, `on`,
  `off`, `brightness`, `color`, `presets`, `preset save|delete|apply`, `effect
  start|stop`, `config`, `config color-order|device`, `reconnect`.
- `poc/` — proof-of-concept binary crate (`l-lightning-poc`, [btleplug]). Frozen.
- `crates/tui` — native GUI (egui + eframe + winit): power toggle, brightness/RGB
  sliders + color swatch, presets (list/save/apply/delete), effects panel.
  Touch-native via winit. Launched via `l-lightning tui`.
- `ui/` — spike: terminal-kit touch validation experiment (results: touch tap works,
  drag doesn't under Wayfire). Kept for reference.

## Build & run
```sh
cargo build --release --workspace
./target/release/l-lightningd
```
Power-cycle the strip when it prints `listening on ...`. Requires BlueZ running
and the phone's Bluetooth **off** (one central at a time).

The POC is still runnable via:
```sh
cargo run --manifest-path poc/Cargo.toml
```

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
[terminal-kit]: https://github.com/cronvel/terminal-kit

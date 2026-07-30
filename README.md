# l-lightning

Standalone on-device controller for a generic **ELK-BLEDOM** Bluetooth-LE RGB LED
strip. Replaces the lost remote and the dead vendor app. **Rust daemon + egui TUI**
over a local Unix socket.

## Status

**Complete** — POC → spec → core daemon → touch-native egui TUI → systemd packaging.
Running on a Surface Go 3 (Arch + BlueZ) as a user service.

## Quick start

```sh
l-lightning status          # check connection + state
l-lightning on              # power on
l-lightning color 255 0 128 # set colour
l-lightning brightness 50   # dim to 50%
l-lightning tui             # open the touch-native GUI

# or use presets and effects
l-lightning preset save "warm" 255 200 100
l-lightning preset apply warm
l-lightning effect start breathe
```

## Architecture

```
l-lightning tui (egui) ──► Unix socket ──► l-lightningd (daemon)
                                               │ btleplug + BlueZ
                                               ▼
                                        ELK-BLEDOM BLE controller
```

- **`l-lightning`** — CLI client and launcher; auto-spawns the daemon if not running.
- **`l-lightningd`** — JSON-RPC 2.0 daemon over `$XDG_RUNTIME_DIR/l-lightning/daemon.sock`;
  owns BLE, connection FSM, effects engine, presets, and config persistence.
- **`l-lightning-tui`** — egui + eframe native window (touch-native via winit):
  Control (power/brightness/colour sliders + swatch), Presets, Effects tabs.

## Install

```sh
cargo build --release --workspace
cp target/release/l-lightning target/release/l-lightningd target/release/l-lightning-tui ~/.local/bin/
```

A [bombadil](https://github.com/ukv/bombadil)-managed systemd user unit runs the daemon:
`systemctl --user enable --now l-lightningd`.

## How it works

The controller uses a simple BLE GATT protocol — 9-byte `0x7E…0xEF` packets written
to characteristic `fff3`, with a subscription on `fff4` to hold the link open. The
daemon wraps this behind a persistent, self-healing connection state machine with
exponential backoff. Full protocol and quirks are in [`CLAUDE.md`](./CLAUDE.md).

[btleplug]: https://github.com/deviceplug/btleplug

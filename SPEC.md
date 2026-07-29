# l-lightning — spec v1

Standalone on-device controller for a generic **ELK-BLEDOM** Bluetooth-LE RGB LED
strip. Replaces the lost remote and the dead vendor app. **Rust core daemon + Node
TUI**, talking over a local Unix socket.

Framing/status live in the garden (`~/soft-fig_garden/projects/l-lightning/`); this
file is the build contract. Protocol + device facts: repo `CLAUDE.md` and the garden
note `notes/001-poc-elk-bledom.md`.

## Goals

- Control power, brightness, and full RGB color from this machine, no app/cloud.
- A touch-friendly TUI with sliders, a color picker, presets/scenes, and effects.
- Rock-solid connection handling — the controller drops the link readily; the daemon
  hides that behind a persistent, self-healing connection.
- Standalone: one command brings up everything (daemon auto-started if needed).

## Architecture

```
┌──────────────────┐   JSON-RPC 2.0 (NDJSON)   ┌───────────────────────────┐
│  Node TUI (ui/)  │ ◄───── Unix socket ─────► │  l-lightningd (Rust)      │
│  terminal-kit    │  $XDG_RUNTIME_DIR/.sock    │  ┌─────────────────────┐  │
│  sliders/picker  │                            │  │ core: BLE+protocol  │  │
│  presets/effects │                            │  │ connection FSM      │  │
└──────────────────┘                            │  │ effects engine      │  │
                                                │  │ presets/config store│  │
                                                │  └─────────┬───────────┘  │
                                                └────────────┼──────────────┘
                                                     btleplug │ BlueZ/D-Bus
                                                              ▼
                                                     ELK-BLEDOM controller
```

- **`crates/core`** (`l-lightning-core`, lib) — btleplug BLE, ELK protocol encoder,
  connection state machine, effects engine, presets/config model. Reusable, no IPC.
- **`crates/daemon`** (`l-lightningd`, bin) — wraps core, serves the Unix-socket
  JSON-RPC API, owns persistence. Single writer to the device.
- **`crates/cli`** (`l-lightning`, bin, optional-but-early) — thin JSON-RPC client;
  scripting + a test harness before the TUI exists; also the launcher that spawns
  the daemon and opens the TUI.
- **`ui/`** — Node TUI (terminal-kit), JSON-RPC client over the socket.
- **`poc/`** — the existing spike, kept for reference (frozen).

## IPC — JSON-RPC 2.0, newline-delimited JSON

Socket: `$XDG_RUNTIME_DIR/l-lightning/daemon.sock` (fallback `/tmp/l-lightning-$UID.sock`).

**Methods (client → daemon)**
| method | params | result |
|---|---|---|
| `get_state` | — | `{power, brightness, rgb:[r,g,b], connection, effect}` |
| `set_power` | `{on:bool}` | `state` |
| `set_brightness` | `{pct:0..100}` | `state` |
| `set_color` | `{r,g,b}` | `state` |
| `list_presets` | — | `[{id,name,rgb,brightness}]` |
| `save_preset` | `{name,rgb,brightness}` | `{id}` |
| `delete_preset` | `{id}` | `ok` |
| `apply_preset` | `{id}` | `state` |
| `start_effect` | `{kind, speed, params}` | `state` |
| `stop_effect` | — | `state` |
| `get_config` / `set_config` | `{device?, color_order?, ...}` | `config` |
| `reconnect` / `rescan` | — | `connection` |

**Notifications (daemon → subscribed clients)**
- `state` — on any change (power/brightness/color/effect).
- `connection` — `Disconnected|Scanning|Connecting|Connected|Reconnecting`.
- `alert` — human-readable warnings (e.g. "controller not found").

Writes are throttled to a safe minimum inter-write gap (TBD by validation; the POC's
1100 ms is far too slow for a live slider — target ~50–80 ms with drag-debounce on
the UI side).

## Data model & persistence

`$XDG_CONFIG_HOME/l-lightning/config.toml`, owned by the daemon:
- `device`: address/name (default: match `ELK-BLEDOM*`, fallback `BE:67:00:A5:CC:56`).
- `color_order`: RGB swizzle (default `rgb`; POC rendered purple correctly).
- `presets`: list of `{id,name,rgb,brightness}`.
- `last_state`: restored on connect.

## Connection state machine (in the daemon)

`Idle → Scanning → Connecting → Connected` with:
- keepalive by subscribing to `fff4`;
- on connect, replay `last_state` (power/color/brightness);
- on write failure / disconnect → `Reconnecting` with exponential backoff, queue the
  latest desired state, resume when back;
- tolerate `le-connection-abort-by-local` / service-discovery timeouts (poll for the
  `fff3` characteristic), as learned in the POC.

## Effects engine (daemon-side, survives UI restarts)

Task that writes frames on a timer; stops on any manual command or `stop_effect`:
- `breathe` (sine brightness), `color_cycle` (hue rotation), `strobe` (on/off),
  `fade_to` (ramp to a target). Each takes a `speed` and optional params.

## Node TUI (ui/)

Framework: **terminal-kit** — native mouse (click/drag) support + 24-bit color for a
real color picker, actively maintained. (Ink is the fallback if we drop heavy touch;
its mouse support is weak.)

Screens/widgets:
- **Main**: power toggle, brightness slider, RGB sliders + color swatch/wheel, live
  state readout, connection indicator.
- **Presets**: list, apply, save current, delete.
- **Effects**: pick kind + speed, start/stop.
- **Settings**: device, color order, reconnect/rescan.

Touch/mouse: draggable sliders, tappable swatches, large hit targets.

## Risks & validation (do these EARLY)

1. **Touch in the terminal** (highest risk). Under Wayfire/wlroots, touch is not
   auto-delivered to terminals as pointer events. First UI slice is a spike: does a
   finger-drag move a terminal-kit slider in the user's terminal? If not, options:
   (a) enable touch→pointer emulation in Wayfire, (b) route through the existing
   touch-drawer infra, (c) revisit UI form. Decide before building the full TUI.
2. **Write throughput**: find the min reliable inter-write gap for smooth slider
   drags without dropping the link.
3. **Multiple clients**: daemon must fan out notifications and serialize device writes.

## Build plan (slices; commit per slice, on `main`)

**M2 — core + daemon**
- 2.1 Cargo workspace; extract POC scan/connect/protocol into `core` (lib).
- 2.2 Connection state machine: persistent link, keepalive, reconnect/backoff, state store.
- 2.3 Command layer: power/brightness/color with write throttling (+ find min gap).
- 2.4 `l-lightningd`: Unix-socket JSON-RPC server (methods + notifications).
- 2.5 Presets + config persistence.
- 2.6 Effects engine.
- 2.7 `l-lightning` CLI client (test harness + launcher).

**M3 — Node TUI**
- 3.0 Touch/mouse validation spike (see Risks #1).
- 3.1 Socket client + live state sync.
- 3.2 Main screen (power, brightness, color).
- 3.3 Presets screen.
- 3.4 Effects panel.
- 3.5 Settings; touch polish (big targets, drag sliders).

**M4 — packaging**
- 4.1 `l-lightning` launcher auto-spawns the daemon (standalone one-command start).
- 4.2 Optional systemd user service for the daemon.
- 4.3 Docs (README/CLAUDE refresh, garden status bump).

## Open questions

- Effect set for v1 — the four above, or a different starter set?
- Should the daemon autostart at login (systemd user service), or only on first UI launch?

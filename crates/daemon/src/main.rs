use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use l_lightning_core::command::CommandLayer;
use l_lightning_core::connection::{ConnState, Connection, DeviceState};
use l_lightning_core::rpc::{Notification, Request, Response};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;

mod config;
mod effect;

use config::{Config, Preset};
use effect::{EffectHandle, EffectKind};

const NOTIFY_BUF: usize = 32;

struct Daemon {
    cmd_layer: CommandLayer,
    notify_tx: broadcast::Sender<Notification>,
    current: Mutex<DeviceState>,
    presets: Mutex<Vec<Preset>>,
    next_preset_id: Mutex<u64>,
    effect_handle: Mutex<Option<EffectHandle>>,
    effect_label: Mutex<Option<String>>,
}

impl Daemon {
    fn new(cmd_layer: CommandLayer, config: &Config) -> Self {
        let (notify_tx, _) = broadcast::channel(NOTIFY_BUF);

        let current = config.last_state.clone().unwrap_or_default();
        let max_id = config
            .presets
            .iter()
            .map(|p| p.id)
            .max()
            .unwrap_or(0);

        Self {
            cmd_layer,
            notify_tx,
            current: Mutex::new(current),
            presets: Mutex::new(config.presets.clone()),
            next_preset_id: Mutex::new(max_id + 1),
            effect_handle: Mutex::new(None),
            effect_label: Mutex::new(None),
        }
    }

    fn get_state(&self) -> Value {
        {
            let mut handle = self.effect_handle.lock().unwrap();
            if let Some(ref h) = *handle {
                if h.is_finished() {
                    *handle = None;
                    *self.effect_label.lock().unwrap() = None;
                }
            }
        }

        let s = self.current.lock().unwrap().clone();
        let cs = self.cmd_layer.state();
        let conn_label = conn_state_label(&cs);
        let effect = self.effect_label.lock().unwrap().clone();
        json!({
            "power": s.power,
            "brightness": s.brightness,
            "rgb": [s.r, s.g, s.b],
            "connection": conn_label,
            "effect": effect
        })
    }

    async fn set_power(&self, on: bool) {
        self.stop_current_effect();
        let state = {
            let mut cur = self.current.lock().unwrap();
            cur.power = on;
            cur.clone()
        };
        self.cmd_layer.apply(state.clone()).await;
        let notif = Notification::new("state", self.get_state());
        let _ = self.notify_tx.send(notif);
        self.save_current_config(state);
    }

    async fn set_brightness(&self, pct: u8) {
        self.stop_current_effect();
        let state = {
            let mut cur = self.current.lock().unwrap();
            cur.brightness = pct.min(100);
            cur.clone()
        };
        self.cmd_layer.apply(state.clone()).await;
        let notif = Notification::new("state", self.get_state());
        let _ = self.notify_tx.send(notif);
        self.save_current_config(state);
    }

    async fn set_color(&self, r: u8, g: u8, b: u8) {
        self.stop_current_effect();
        let state = {
            let mut cur = self.current.lock().unwrap();
            cur.r = r;
            cur.g = g;
            cur.b = b;
            cur.clone()
        };
        self.cmd_layer.apply(state.clone()).await;
        let notif = Notification::new("state", self.get_state());
        let _ = self.notify_tx.send(notif);
        self.save_current_config(state);
    }

    fn stop_current_effect(&self) {
        let mut handle = self.effect_handle.lock().unwrap();
        if let Some(h) = handle.take() {
            h.stop();
        }
        *self.effect_label.lock().unwrap() = None;
    }

    fn start_effect(&self, kind: EffectKind, speed: u64, target: Option<DeviceState>) {
        self.stop_current_effect();
        let initial = self.current.lock().unwrap().clone();
        let label = effect::effect_label(&kind).to_string();

        let handle = effect::start(
            self.cmd_layer.clone(),
            kind,
            effect::EffectParams { speed, target },
            initial,
        );
        *self.effect_handle.lock().unwrap() = Some(handle);
        *self.effect_label.lock().unwrap() = Some(label);
    }

    fn list_presets(&self) -> Value {
        let presets = self.presets.lock().unwrap();
        json!(presets
            .iter()
            .map(|p| json!({
                "id": p.id,
                "name": p.name,
                "rgb": [p.r, p.g, p.b],
                "brightness": p.brightness,
            }))
            .collect::<Vec<_>>())
    }

    fn save_preset(&self, name: String, r: u8, g: u8, b: u8, brightness: u8) -> u64 {
        let id = {
            let mut next = self.next_preset_id.lock().unwrap();
            let id = *next;
            *next += 1;
            id
        };
        let preset = Preset {
            id,
            name,
            r,
            g,
            b,
            brightness,
        };
        let mut presets = self.presets.lock().unwrap();
        presets.push(preset);
        self.save_presets_config(&presets);
        id
    }

    fn delete_preset(&self, id: u64) -> bool {
        let mut presets = self.presets.lock().unwrap();
        let len_before = presets.len();
        presets.retain(|p| p.id != id);
        if presets.len() != len_before {
            self.save_presets_config(&presets);
            true
        } else {
            false
        }
    }

    async fn apply_preset(&self, id: u64) -> Option<Value> {
        let preset = {
            let presets = self.presets.lock().unwrap();
            presets.iter().find(|p| p.id == id).cloned()
        };
        match preset {
            Some(p) => {
                let state = {
                    let mut cur = self.current.lock().unwrap();
                    cur.power = true;
                    cur.brightness = p.brightness;
                    cur.r = p.r;
                    cur.g = p.g;
                    cur.b = p.b;
                    cur.clone()
                };
                self.cmd_layer.apply(state.clone()).await;
                let notif = Notification::new("state", self.get_state());
                let _ = self.notify_tx.send(notif);
                self.save_current_config(state);
                Some(self.get_state())
            }
            None => None,
        }
    }

    fn get_config(&self) -> Value {
        let presets = self.presets.lock().unwrap();
        let cur = self.current.lock().unwrap().clone();
        let dev = std::env::var("L_LIGHTNING_DEVICE").ok();
        json!({
            "device": dev,
            "color_order": "rgb",
            "presets": presets.iter().map(|p| json!({
                "id": p.id,
                "name": p.name,
                "rgb": [p.r, p.g, p.b],
                "brightness": p.brightness,
            })).collect::<Vec<_>>(),
            "last_state": {
                "power": cur.power,
                "brightness": cur.brightness,
                "rgb": [cur.r, cur.g, cur.b],
            }
        })
    }

    fn set_config(&self, params: &Value) -> Result<Value, String> {
        let mut changed = false;

        if let Some(order) = params.get("color_order").and_then(|v| v.as_str()) {
            let valid = ["rgb", "rbg", "grb", "gbr", "brg", "bgr"];
            if !valid.contains(&order) {
                return Err(format!(
                    "Invalid color_order '{}': must be one of {:?}",
                    order, valid
                ));
            }
            changed = true;
        }

        if let Some(addr) = params.get("device").and_then(|v| v.as_str()) {
            std::env::set_var("L_LIGHTNING_DEVICE", addr);
            changed = true;
        }

        if changed {
            let cur = self.current.lock().unwrap().clone();
            self.save_current_config(cur);
        }

        Ok(self.get_config())
    }

    fn save_current_config(&self, state: DeviceState) {
        let presets = self.presets.lock().unwrap().clone();
        config::save(&Config {
            device: std::env::var("L_LIGHTNING_DEVICE").ok(),
            color_order: None,
            presets,
            last_state: Some(state),
        });
    }

    fn save_presets_config(&self, presets: &[Preset]) {
        let cur = self.current.lock().unwrap().clone();
        config::save(&Config {
            device: std::env::var("L_LIGHTNING_DEVICE").ok(),
            color_order: None,
            presets: presets.to_vec(),
            last_state: Some(cur),
        });
    }
}

fn conn_state_label(s: &ConnState) -> &str {
    match s {
        ConnState::Idle => "Disconnected",
        ConnState::Scanning => "Scanning",
        ConnState::Connecting => "Connecting",
        ConnState::Connected => "Connected",
    }
}

fn socket_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        let mut p = PathBuf::from(dir);
        p.push("l-lightning");
        std::fs::create_dir_all(&p).ok();
        p.push("daemon.sock");
        p
    } else {
        PathBuf::from("/tmp/l-lightning.sock")
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = config::load();
    let conn = Connection::new().await?;
    let cmd_layer = CommandLayer::new(conn);

    let daemon = Arc::new(Daemon::new(cmd_layer, &config));

    if let Some(ref last_state) = config.last_state {
        if last_state.power || last_state.brightness > 0 {
            daemon.cmd_layer.apply(last_state.clone()).await;
        }
    }

    let sp = socket_path();
    if sp.exists() {
        std::fs::remove_file(&sp)?;
    }

    let listener = UnixListener::bind(&sp)?;
    eprintln!("[l-lightningd] listening on {}", sp.display());

    spawn_connection_watcher(daemon.clone());

    loop {
        let (stream, _) = listener.accept().await?;
        let d = daemon.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, d).await {
                eprintln!("[l-lightningd] client error: {e}");
            }
        });
    }
}

fn spawn_connection_watcher(daemon: Arc<Daemon>) {
    let mut watcher = daemon.cmd_layer.watch();
    let tx = daemon.notify_tx.clone();

    tokio::spawn(async move {
        let initial = watcher.borrow().clone();
        send_conn_notif(&tx, &initial);

        loop {
            if watcher.changed().await.is_err() {
                break;
            }
            let s = watcher.borrow().clone();
            send_conn_notif(&tx, &s);
        }
    });
}

fn send_conn_notif(tx: &broadcast::Sender<Notification>, s: &ConnState) {
    let notif = Notification::new("connection", json!({ "state": conn_state_label(s) }));
    let _ = tx.send(notif);
}

async fn handle_client(
    stream: UnixStream,
    daemon: Arc<Daemon>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (reader, mut writer) = stream.into_split();
    let buf = BufReader::new(reader);
    let mut lines = buf.lines();
    let mut notify_rx = daemon.notify_tx.subscribe();

    loop {
        tokio::select! {
            line = lines.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        let resp = dispatch(trimmed, &daemon).await;
                        let mut bytes = serde_json::to_vec(&resp).unwrap_or_default();
                        bytes.push(b'\n');
                        let _ = writer.write_all(&bytes).await;
                    }
                    Ok(None) | Err(_) => break,
                }
            }
            notif = notify_rx.recv() => {
                match notif {
                    Ok(n) => {
                        let mut bytes = serde_json::to_vec(&n).unwrap_or_default();
                        bytes.push(b'\n');
                        let _ = writer.write_all(&bytes).await;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        eprintln!("[l-lightningd] client lagged by {n} notifications");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    Ok(())
}

async fn dispatch(line: &str, daemon: &Daemon) -> Response {
    let req: Request = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => return Response::err(Value::Null, -32700, format!("Parse error: {e}")),
    };

    if req.jsonrpc != "2.0" {
        return Response::err(req.id, -32600, "Invalid Request: jsonrpc must be \"2.0\"");
    }

    match req.method.as_str() {
        "get_state" => Response::ok(req.id, daemon.get_state()),

        "set_power" => {
            let on = req.params.get("on").and_then(|v| v.as_bool());
            match on {
                Some(on) => {
                    daemon.set_power(on).await;
                    Response::ok(req.id, daemon.get_state())
                }
                None => Response::err(req.id, -32602, "Invalid params: 'on' (bool) required"),
            }
        }

        "set_brightness" => {
            let pct = req.params.get("pct").and_then(|v| v.as_u64());
            match pct {
                Some(p) if p <= 100 => {
                    daemon.set_brightness(p as u8).await;
                    Response::ok(req.id, daemon.get_state())
                }
                Some(_) => Response::err(req.id, -32602, "Invalid params: pct must be 0-100"),
                None => Response::err(req.id, -32602, "Invalid params: 'pct' (0-100) required"),
            }
        }

        "set_color" => {
            let r = req.params.get("r").and_then(|v| v.as_u64());
            let g = req.params.get("g").and_then(|v| v.as_u64());
            let b = req.params.get("b").and_then(|v| v.as_u64());
            match (r, g, b) {
                (Some(r), Some(g), Some(b)) if r <= 255 && g <= 255 && b <= 255 => {
                    daemon.set_color(r as u8, g as u8, b as u8).await;
                    Response::ok(req.id, daemon.get_state())
                }
                _ => Response::err(req.id, -32602, "Invalid params: 'r','g','b' (0-255) required"),
            }
        }

        "reconnect" | "rescan" => {
            let state = daemon.current.lock().unwrap().clone();
            daemon.cmd_layer.apply(state).await;
            Response::ok(req.id, daemon.get_state())
        }

        "list_presets" => Response::ok(req.id, daemon.list_presets()),

        "save_preset" => {
            let name = req.params.get("name").and_then(|v| v.as_str());
            let r = req.params.get("r").and_then(|v| v.as_u64());
            let g = req.params.get("g").and_then(|v| v.as_u64());
            let b = req.params.get("b").and_then(|v| v.as_u64());
            let brightness = req.params.get("brightness").and_then(|v| v.as_u64());

            match (name, r, g, b, brightness) {
                (Some(name), Some(r), Some(g), Some(b), Some(brightness))
                    if r <= 255 && g <= 255 && b <= 255 && brightness <= 100 =>
                {
                    let id = daemon.save_preset(
                        name.to_string(),
                        r as u8,
                        g as u8,
                        b as u8,
                        brightness as u8,
                    );
                    Response::ok(req.id, json!({ "id": id }))
                }
                _ => Response::err(
                    req.id,
                    -32602,
                    "Invalid params: 'name' (string), 'r','g','b' (0-255), 'brightness' (0-100) required",
                ),
            }
        }

        "delete_preset" => {
            let id = req.params.get("id").and_then(|v| v.as_u64());
            match id {
                Some(id) => {
                    if daemon.delete_preset(id) {
                        Response::ok(req.id, json!("ok"))
                    } else {
                        Response::err(req.id, -32602, format!("Preset '{}' not found", id))
                    }
                }
                None => Response::err(req.id, -32602, "Invalid params: 'id' required"),
            }
        }

        "apply_preset" => {
            let id = req.params.get("id").and_then(|v| v.as_u64());
            match id {
                Some(id) => match daemon.apply_preset(id).await {
                    Some(state) => Response::ok(req.id, state),
                    None => Response::err(req.id, -32602, format!("Preset '{}' not found", id)),
                },
                None => Response::err(req.id, -32602, "Invalid params: 'id' required"),
            }
        }

        "get_config" => Response::ok(req.id, daemon.get_config()),

        "set_config" => match daemon.set_config(&req.params) {
            Ok(config) => Response::ok(req.id, config),
            Err(msg) => Response::err(req.id, -32602, msg),
        },

        "start_effect" => {
            let kind_str = req.params.get("kind").and_then(|v| v.as_str());
            let speed = req.params.get("speed").and_then(|v| v.as_u64());

            match (kind_str, speed) {
                (Some(kind_str), Some(speed)) if speed > 0 => {
                    let kind: EffectKind = match serde_json::from_value(json!(kind_str)) {
                        Ok(k) => k,
                        Err(_) => {
                            return Response::err(
                                req.id,
                                -32602,
                                format!(
                                    "Invalid effect kind '{}': must be one of breathe, color_cycle, strobe, fade_to",
                                    kind_str
                                ),
                            );
                        }
                    };

                    let target = if matches!(kind, EffectKind::FadeTo) {
                        let r = req.params.get("r").and_then(|v| v.as_u64());
                        let g = req.params.get("g").and_then(|v| v.as_u64());
                        let b = req.params.get("b").and_then(|v| v.as_u64());
                        let brightness =
                            req.params.get("brightness").and_then(|v| v.as_u64());

                        match (r, g, b, brightness) {
                            (Some(r), Some(g), Some(b), Some(brightness))
                                if r <= 255 && g <= 255 && b <= 255 && brightness <= 100 =>
                            {
                                Some(DeviceState {
                                    power: true,
                                    brightness: brightness as u8,
                                    r: r as u8,
                                    g: g as u8,
                                    b: b as u8,
                                })
                            }
                            _ => {
                                return Response::err(
                                    req.id,
                                    -32602,
                                    "fade_to requires params: 'r','g','b' (0-255), 'brightness' (0-100)",
                                );
                            }
                        }
                    } else {
                        None
                    };

                    daemon.start_effect(kind, speed, target);
                    Response::ok(req.id, daemon.get_state())
                }
                _ => Response::err(
                    req.id,
                    -32602,
                    "Invalid params: 'kind' (breathe|color_cycle|strobe|fade_to) and 'speed' (ms, >0) required",
                ),
            }
        }

        "stop_effect" => {
            daemon.stop_current_effect();
            Response::ok(req.id, daemon.get_state())
        }

        _ => Response::err(req.id, -32601, format!("Method not found: {}", req.method)),
    }
}

fn main() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    if let Err(e) = rt.block_on(run()) {
        eprintln!("[l-lightningd] fatal: {e}");
        std::process::exit(1);
    }
}

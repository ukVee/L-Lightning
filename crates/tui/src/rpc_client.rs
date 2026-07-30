use l_lightning_core::rpc::{Notification, Request, Response};
use serde_json::Value;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

pub fn socket_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        let mut p = PathBuf::from(dir);
        p.push("l-lightning");
        p.push("daemon.sock");
        p
    } else {
        PathBuf::from("/tmp/l-lightning.sock")
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DaemonState {
    pub power: bool,
    pub brightness: u8,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub connection: String,
    pub effect: Option<String>,
}

#[derive(Debug, Clone)]
pub enum DaemonEvent {
    State(DaemonState),
    Connection(String),
    #[allow(dead_code)]
    Response { id: Value, result: Value },
    #[allow(dead_code)]
    Error { id: Value, message: String },
    Disconnected,
}

fn parse_state(params: &Value) -> Option<DaemonState> {
    Some(DaemonState {
        power: params.get("power")?.as_bool()?,
        brightness: params.get("brightness")?.as_u64()? as u8,
        r: params.get("rgb")?.get(0)?.as_u64()? as u8,
        g: params.get("rgb")?.get(1)?.as_u64()? as u8,
        b: params.get("rgb")?.get(2)?.as_u64()? as u8,
        connection: params.get("connection")?.as_str()?.to_string(),
        effect: params.get("effect").and_then(|v| v.as_str()).map(String::from),
    })
}

pub struct RpcClient {
    tx: mpsc::UnboundedSender<RpcAction>,
}

enum RpcAction {
    Request { id: Value, method: String, params: Value },
}

impl RpcClient {
    pub async fn connect(event_tx: mpsc::UnboundedSender<DaemonEvent>) -> Result<Self, Box<dyn std::error::Error>> {
        let path = socket_path();
        let stream = UnixStream::connect(&path).await?;

        let (reader, mut writer) = stream.into_split();
        let buf = BufReader::new(reader);
        let mut lines = buf.lines();

        let (action_tx, mut action_rx) = mpsc::unbounded_channel::<RpcAction>();

        tokio::spawn(async move {

            loop {
                tokio::select! {
                    line = lines.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                let trimmed = line.trim();
                                if trimmed.is_empty() {
                                    continue;
                                }

                                if let Ok(notif) = serde_json::from_str::<Notification>(trimmed) {
                                    match notif.method.as_str() {
                                        "state" => {
                                            if let Some(s) = parse_state(&notif.params) {
                                                let _ = event_tx.send(DaemonEvent::State(s));
                                            }
                                        }
                                        "connection" => {
                                            if let Some(s) = notif.params.get("state").and_then(|v| v.as_str()) {
                                                let _ = event_tx.send(DaemonEvent::Connection(s.to_string()));
                                            }
                                        }
                                        _ => {}
                                    }
                                } else if let Ok(resp) = serde_json::from_str::<Response>(trimmed) {
                                    match resp.result {
                                        Some(result) => {
                                            let _ = event_tx.send(DaemonEvent::Response {
                                                id: resp.id,
                                                result,
                                            });
                                        }
                                        None => {
                                            if let Some(err) = resp.error {
                                                let _ = event_tx.send(DaemonEvent::Error {
                                                    id: resp.id,
                                                    message: err.message,
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                            Ok(None) | Err(_) => {
                                let _ = event_tx.send(DaemonEvent::Disconnected);
                                break;
                            }
                        }
                    }
                    action = action_rx.recv() => {
                        match action {
                            Some(RpcAction::Request { id, method, params }) => {
                                let req = Request {
                                    jsonrpc: "2.0".to_string(),
                                    method,
                                    params,
                                    id,
                                };
                                let mut bytes = serde_json::to_vec(&req).unwrap_or_default();
                                bytes.push(b'\n');
                                let _ = writer.write_all(&bytes).await;
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        Ok(Self { tx: action_tx })
    }

    pub fn request(&self, method: &str, params: Value) -> Value {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = Value::Number(COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed).into());
        let id2 = id.clone();
        let _ = self.tx.send(RpcAction::Request {
            id: id2,
            method: method.to_string(),
            params,
        });
        id
    }
}

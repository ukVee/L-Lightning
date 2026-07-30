use std::path::PathBuf;
use std::process::{exit, Command};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

fn socket_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        let mut p = PathBuf::from(dir);
        p.push("l-lightning");
        p.push("daemon.sock");
        p
    } else {
        PathBuf::from("/tmp/l-lightning.sock")
    }
}

fn sibling_bin(name: &str) -> PathBuf {
    let mut p = std::env::current_exe().unwrap_or_default();
    p.pop();
    p.push(name);
    p
}

async fn ensure_daemon() -> Result<UnixStream, String> {
    let path = socket_path();
    if let Ok(stream) = UnixStream::connect(&path).await {
        return Ok(stream);
    }

    let daemon_bin = sibling_bin("l-lightningd");
    eprintln!("daemon not running, starting {}...", daemon_bin.display());
    Command::new(&daemon_bin)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to start daemon: {e}"))?;

    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(stream) = UnixStream::connect(&path).await {
            return Ok(stream);
        }
    }

    Err("daemon did not start within 5s (is l-lightningd installed and on PATH?)".into())
}

async fn rpc(method: &str, params: Value) -> Result<Value, String> {
    let mut stream = ensure_daemon().await?;

    let request = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
        "id": 1
    });

    let (reader, mut writer) = stream.split();
    let mut buf = serde_json::to_vec(&request).map_err(|e| format!("json: {e}"))?;
    buf.push(b'\n');
    writer
        .write_all(&buf)
        .await
        .map_err(|e| format!("write: {e}"))?;

    let mut line = String::new();
    let mut buf_reader = BufReader::new(reader);
    buf_reader
        .read_line(&mut line)
        .await
        .map_err(|e| format!("read: {e}"))?;

    let response: Value =
        serde_json::from_str(&line).map_err(|e| format!("parse: {e}"))?;

    if let Some(err) = response.get("error") {
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        return Err(msg.to_string());
    }

    Ok(response["result"].clone())
}

fn usage() {
    eprintln!(
        "\
l-lightning — ELK-BLEDOM BLE LED controller

USAGE:
  l-lightning tui
  l-lightning status
  l-lightning on | off
  l-lightning brightness <0-100>
  l-lightning color <r> <g> <b>
  l-lightning presets
  l-lightning preset save <name> <r> <g> <b> <brightness>
  l-lightning preset delete <id>
  l-lightning preset apply <id>
  l-lightning effect start <kind> <speed> [r g b brightness]
  l-lightning effect stop
  l-lightning config
  l-lightning config color-order <order>
  l-lightning config device <addr>
  l-lightning reconnect

Effect kinds:  breathe  color_cycle  strobe  fade_to
fade_to requires target: r g b brightness"
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
        exit(1);
    }

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    if let Err(e) = rt.block_on(run(&args)) {
        eprintln!("[l-lightning] {e}");
        exit(1);
    }
}

async fn run(args: &[String]) -> Result<(), String> {
    let cmd = args[1].as_str();
    match cmd {
        "tui" => {
            rpc("get_state", json!({})).await?;
            let tui_bin = sibling_bin("l-lightning-tui");
            let status = Command::new(&tui_bin)
                .status()
                .map_err(|e| format!("failed to launch tui: {e}"))?;
            if !status.success() {
                eprintln!("tui exited with error");
            }
        }

        "status" => {
            let state = rpc("get_state", json!({})).await?;
            println!("{}", serde_json::to_string_pretty(&state).unwrap());
        }

        "on" => {
            rpc("set_power", json!({ "on": true })).await?;
            print_state().await?;
        }

        "off" => {
            rpc("set_power", json!({ "on": false })).await?;
            print_state().await?;
        }

        "brightness" => {
            let pct = parse_arg::<u8>(args, 2, "brightness (0-100)")?;
            rpc("set_brightness", json!({ "pct": pct })).await?;
            print_state().await?;
        }

        "color" => {
            let r = parse_arg::<u8>(args, 2, "r (0-255)")?;
            let g = parse_arg::<u8>(args, 3, "g (0-255)")?;
            let b = parse_arg::<u8>(args, 4, "b (0-255)")?;
            rpc("set_color", json!({ "r": r, "g": g, "b": b })).await?;
            print_state().await?;
        }

        "presets" => {
            let presets = rpc("list_presets", json!({})).await?;
            if presets.as_array().is_none_or(|a| a.is_empty()) {
                println!("no presets saved");
            } else {
                println!("{}", serde_json::to_string_pretty(&presets).unwrap());
            }
        }

        "preset" => {
            let sub = args.get(2).map(String::as_str).unwrap_or("");
            match sub {
                "save" => {
                    let name = parse_str(args, 3, "name")?;
                    let r = parse_arg::<u8>(args, 4, "r (0-255)")?;
                    let g = parse_arg::<u8>(args, 5, "g (0-255)")?;
                    let b = parse_arg::<u8>(args, 6, "b (0-255)")?;
                    let brightness = parse_arg::<u8>(args, 7, "brightness (0-100)")?;
                    let result = rpc(
                        "save_preset",
                        json!({ "name": name, "r": r, "g": g, "b": b, "brightness": brightness }),
                    )
                    .await?;
                    let id = result["id"].as_u64().unwrap_or(0);
                    println!("saved preset '{name}' as #{id}");
                }
                "delete" => {
                    let id = parse_arg::<u64>(args, 3, "id")?;
                    rpc("delete_preset", json!({ "id": id })).await?;
                    println!("deleted preset #{id}");
                }
                "apply" => {
                    let id = parse_arg::<u64>(args, 3, "id")?;
                    rpc("apply_preset", json!({ "id": id })).await?;
                    print_state().await?;
                }
                _ => {
                    eprintln!("preset subcommand: save | delete | apply");
                    exit(1);
                }
            }
        }

        "effect" => {
            let sub = args.get(2).map(String::as_str).unwrap_or("");
            match sub {
                "start" => {
                    let kind = parse_str(args, 3, "kind (breathe|color_cycle|strobe|fade_to)")?;
                    let speed =
                        parse_arg::<u64>(args, 4, "speed (ms between frames, >0)")?;

                    let mut params = json!({ "kind": kind, "speed": speed });

                    if kind == "fade_to" {
                        let r = parse_arg::<u8>(args, 5, "r (0-255) for fade_to")?;
                        let g = parse_arg::<u8>(args, 6, "g (0-255) for fade_to")?;
                        let b = parse_arg::<u8>(args, 7, "b (0-255) for fade_to")?;
                        let brightness =
                            parse_arg::<u8>(args, 8, "brightness (0-100) for fade_to")?;
                        params["r"] = json!(r);
                        params["g"] = json!(g);
                        params["b"] = json!(b);
                        params["brightness"] = json!(brightness);
                    }

                    rpc("start_effect", params).await?;
                    print_state().await?;
                }
                "stop" => {
                    rpc("stop_effect", json!({})).await?;
                    print_state().await?;
                }
                _ => {
                    eprintln!("effect subcommand: start | stop");
                    exit(1);
                }
            }
        }

        "config" => {
            let sub = args.get(2).map(String::as_str);
            match sub {
                Some("color-order") => {
                    let order = parse_str(args, 3, "order (rgb|rbg|grb|gbr|brg|bgr)")?;
                    let result = rpc("set_config", json!({ "color_order": order })).await?;
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                }
                Some("device") => {
                    let addr = parse_str(args, 3, "device address")?;
                    let result = rpc("set_config", json!({ "device": addr })).await?;
                    println!("{}", serde_json::to_string_pretty(&result).unwrap());
                }
                Some(other) => {
                    eprintln!("unknown config subcommand: {other}");
                    eprintln!("config subcommands: color-order | device");
                    exit(1);
                }
                None => {
                    let config = rpc("get_config", json!({})).await?;
                    println!("{}", serde_json::to_string_pretty(&config).unwrap());
                }
            }
        }

        "reconnect" => {
            rpc("reconnect", json!({})).await?;
            println!("reconnect sent");
        }

        _ => {
            eprintln!("unknown command: {cmd}");
            usage();
            exit(1);
        }
    }

    Ok(())
}

async fn print_state() -> Result<(), String> {
    let state = rpc("get_state", json!({})).await?;
    let power = state["power"].as_bool().unwrap_or(false);
    let brightness = state["brightness"].as_u64().unwrap_or(0);
    let rgb = &state["rgb"];
    let conn = state["connection"].as_str().unwrap_or("?");
    let effect = state["effect"].as_str();

    print!(
        "{}  brightness: {}%  rgb: {}  connection: {}",
        if power { "ON " } else { "OFF" },
        brightness,
        if let Some(arr) = rgb.as_array() {
            format!(
                "[{}, {}, {}]",
                arr.first().and_then(|v| v.as_u64()).unwrap_or(0),
                arr.get(1).and_then(|v| v.as_u64()).unwrap_or(0),
                arr.get(2).and_then(|v| v.as_u64()).unwrap_or(0)
            )
        } else {
            "?".into()
        },
        conn,
    );

    if let Some(eff) = effect {
        print!("  effect: {eff}");
    }

    println!();
    Ok(())
}

fn parse_arg<T: std::str::FromStr>(args: &[String], idx: usize, desc: &str) -> Result<T, String> {
    let val = args
        .get(idx)
        .ok_or_else(|| format!("missing argument #{idx}: {desc}"))?;
    val.parse::<T>()
        .map_err(|_| format!("invalid {desc}: '{val}'"))
}

fn parse_str<'a>(args: &'a [String], idx: usize, desc: &str) -> Result<&'a str, String> {
    args.get(idx)
        .map(String::as_str)
        .ok_or_else(|| format!("missing argument #{idx}: {desc}"))
}

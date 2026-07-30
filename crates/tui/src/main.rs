mod app;
mod rpc_client;

use app::App;
use rpc_client::RpcClient;
use std::path::PathBuf;
use std::process::{exit, Command};
use std::sync::mpsc as std_mpsc;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::sync::mpsc;

fn sibling_bin(name: &str) -> PathBuf {
    let mut p = std::env::current_exe().unwrap_or_default();
    p.pop();
    p.push(name);
    p
}

async fn ensure_daemon() {
    let path = rpc_client::socket_path();
    match UnixStream::connect(&path).await {
        Ok(_) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let daemon_bin = sibling_bin("l-lightningd");
            eprintln!("daemon not running, starting {}...", daemon_bin.display());
            if let Err(e) = Command::new(&daemon_bin)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                eprintln!("failed to start daemon: {e}");
                exit(1);
            }

            for _ in 0..50 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if UnixStream::connect(&path).await.is_ok() {
                    return;
                }
            }
            eprintln!("daemon did not start within 5s (is l-lightningd installed?)");
            exit(1);
        }
        Err(e) => {
            eprintln!("waiting for daemon (socket exists, not accepting): {e}");
            for _ in 0..50 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if UnixStream::connect(&path).await.is_ok() {
                    return;
                }
            }
            eprintln!("daemon not responding within 5s");
            exit(1);
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (event_tx, mut event_rx_tokio) = mpsc::unbounded_channel();
    let (event_tx_std, event_rx_std) = std_mpsc::channel();

    let rt = tokio::runtime::Runtime::new()?;

    rt.block_on(ensure_daemon());

    let rpc_client = rt.block_on(async {
        let client = RpcClient::connect(event_tx).await?;
        Ok::<_, Box<dyn std::error::Error>>(client)
    })?;

    let _rt_guard = rt.enter();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([480.0, 640.0])
            .with_title("l-lightning"),
        ..Default::default()
    };

    eframe::run_native(
        "l-lightning",
        native_options,
        Box::new(move |_cc| {
            let app = App::new(rpc_client, event_rx_std);

            // bridge events from tokio mpsc → std mpsc
            std::thread::spawn(move || {
                while let Some(event) = event_rx_tokio.blocking_recv() {
                    if event_tx_std.send(event).is_err() {
                        break;
                    }
                }
            });

            Ok(Box::new(app))
        }),
    )?;

    Ok(())
}

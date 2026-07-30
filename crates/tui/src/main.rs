mod app;
mod rpc_client;

use app::App;
use rpc_client::RpcClient;
use std::sync::mpsc as std_mpsc;
use tokio::sync::mpsc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (event_tx, mut event_rx_tokio) = mpsc::unbounded_channel();
    let (event_tx_std, event_rx_std) = std_mpsc::channel();

    let rt = tokio::runtime::Runtime::new()?;

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

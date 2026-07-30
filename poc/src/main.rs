// Proof of concept: control an ELK-BLEDOM BLE RGB LED controller from Rust.
//
// Uses l-lightning-core for protocol + device primitives. The POC is a
// frozen reference; the real daemon will build on the same core lib.
//
// Flow: scan -> match by name ("ELK-BLEDOM*") or address -> connect ->
// subscribe to the notify channel (fff4) to keep the cheap link alive ->
// write command packets to char 0xFFF3.

use std::error::Error;
use std::time::Duration;

use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::Manager;
use l_lightning_core::device::{self, ensure_connected, establish_gatt, find_elk};
use l_lightning_core::protocol;
use tokio::time;

/// Write a command with reconnect-on-failure + inter-write delay, logging each step.
async fn write_command(
    dev: &btleplug::platform::Peripheral,
    ch: &btleplug::api::Characteristic,
    wtype: btleplug::api::WriteType,
    bytes: &[u8],
) -> Result<(), Box<dyn Error>> {
    if let Err(e) = dev.write(ch, bytes, wtype).await {
        println!("    !! write failed ({e}); reconnecting...");
        ensure_connected(dev).await?;
        dev.write(ch, bytes, wtype).await?;
        println!("    -> (after reconnect)");
    }
    time::sleep(Duration::from_millis(1100)).await;
    Ok(())
}

macro_rules! macrobat {
    ($dev:expr, $ch:expr, $wtype:expr, $($label:ident => $cmd:expr),+ $(,)?) => {{
        $(
            write_command($dev, $ch, $wtype, &$cmd).await?;
            println!("    -> {}", stringify!($label));
        )+
    }};
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let manager = Manager::new().await?;
    let central = manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .ok_or("no Bluetooth adapter found")?;

    println!("[*] scanning up to 60s for ELK-BLEDOM — power-cycle the strip now if it isn't seen...");
    central.start_scan(ScanFilter::default()).await?;

    let mut dev = None;
    for i in 0..60 {
        time::sleep(Duration::from_secs(1)).await;
        if let Some(p) = find_elk(&central).await? {
            dev = Some(p);
            break;
        }
        if i % 5 == 4 {
            println!("    still scanning ({}s)...", i + 1);
        }
    }
    let _ = central.stop_scan().await;
    let dev = dev.ok_or("ELK-BLEDOM not found (powered? phone Bluetooth off?)")?;

    time::sleep(Duration::from_millis(600)).await;

    let props = dev.properties().await?.unwrap_or_default();
    println!(
        "[+] found '{}' [{}] — connecting...",
        props.local_name.unwrap_or_default(),
        dev.address()
    );
    establish_gatt(&dev).await?;
    println!("[+] connected, services discovered.");

    println!("[*] characteristics:");
    for c in dev.characteristics() {
        println!("      {}  {:?}", c.uuid, c.properties);
    }

    if let Some(nch) = dev.characteristics().into_iter().find(|c| c.uuid == device::NOTIFY_UUID) {
        match dev.subscribe(&nch).await {
            Ok(_) => println!("[+] subscribed to notifications (fff4)"),
            Err(e) => println!("[!] notify subscribe failed (continuing): {e}"),
        }
    }

    let chars: Vec<_> = dev.characteristics().into_iter().collect();
    let ch = device::find_write_characteristic(&chars)
        .ok_or("no writable characteristic found")?
        .clone();
    let wtype = device::write_type_for(&ch);

    println!("[+] writing to {} using {:?}", ch.uuid, wtype);

    time::sleep(Duration::from_millis(700)).await;

    println!("[*] LIGHT SHOW — watch the strip:");

    macrobat!(
        &dev, &ch, wtype,
        power_on => protocol::power_on(),
        brightness_100 => protocol::set_brightness(100),
        RED => protocol::set_color(255, 0, 0),
        GREEN => protocol::set_color(0, 255, 0),
        BLUE => protocol::set_color(0, 0, 255),
        WHITE => protocol::set_color(255, 255, 255),
        dim_15 => protocol::set_brightness(15),
        back_100 => protocol::set_brightness(100),
        purple => protocol::set_color(128, 0, 255),
    );

    println!("[+] POC complete — leaving strip on purple, disconnecting.");
    let _ = dev.disconnect().await;
    Ok(())
}

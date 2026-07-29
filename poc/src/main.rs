// Proof of concept: control an ELK-BLEDOM BLE RGB LED controller from Rust.
//
// Flow: scan -> match by name ("ELK-BLEDOM*") or address -> connect ->
// subscribe to the notify channel (fff4) to keep the cheap link alive ->
// write the documented 9-byte command packets to char 0xFFF3.
//
// ELK-BLEDOM command packets (each 9 bytes, framed 0x7E ... 0xEF):
//   power on   : 7E 00 04 F0 00 01 FF 00 EF
//   power off  : 7E 00 04 00 00 00 FF 00 EF
//   set color  : 7E 00 05 03 <R> <G> <B> 00 EF
//   brightness : 7E 00 01 <0-100> 00 00 00 00 EF   (percent)

use std::error::Error;
use std::time::Duration;

use btleplug::api::{
    Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use tokio::time;
use uuid::Uuid;

const TARGET_ADDR: &str = "BE:67:00:A5:CC:56";
const WRITE_UUID: Uuid = Uuid::from_u128(0x0000fff3_0000_1000_8000_00805f9b34fb);
const NOTIFY_UUID: Uuid = Uuid::from_u128(0x0000fff4_0000_1000_8000_00805f9b34fb);

async fn find_elk(central: &Adapter) -> Result<Option<Peripheral>, btleplug::Error> {
    for p in central.peripherals().await? {
        let name = p
            .properties()
            .await?
            .and_then(|pr| pr.local_name)
            .unwrap_or_default();
        let addr = p.address().to_string();
        if name.starts_with("ELK-BLEDOM") || addr.eq_ignore_ascii_case(TARGET_ADDR) {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

/// Make sure we hold a live GATT link, re-establishing + re-subscribing if the
/// controller dropped us (these clones close the link readily).
async fn ensure_connected(dev: &Peripheral) -> Result<(), Box<dyn Error>> {
    if !dev.is_connected().await? {
        dev.connect().await?;
        dev.discover_services().await?;
        if let Some(nch) = dev.characteristics().into_iter().find(|c| c.uuid == NOTIFY_UUID) {
            let _ = dev.subscribe(&nch).await;
        }
    }
    Ok(())
}

/// Write one command; on a dropped link, reconnect once and retry.
async fn send(
    dev: &Peripheral,
    ch: &Characteristic,
    wtype: WriteType,
    bytes: &[u8],
    label: &str,
) -> Result<(), Box<dyn Error>> {
    if let Err(e) = dev.write(ch, bytes, wtype).await {
        println!("    !! {label} write failed ({e}); reconnecting...");
        ensure_connected(dev).await?;
        dev.write(ch, bytes, wtype).await?;
        println!("    -> {label} (after reconnect)");
    } else {
        println!("    -> {label}");
    }
    time::sleep(Duration::from_millis(1100)).await;
    Ok(())
}

/// Establish a usable GATT session. These clones frequently return
/// "service discovery timed out" (or `le-connection-abort-by-local`) on connect
/// even when the ACL link is actually up, so we tolerate the connect error and
/// poll for the write characteristic to appear before giving up.
async fn establish_gatt(dev: &Peripheral) -> Result<(), Box<dyn Error>> {
    for attempt in 1..=3u32 {
        if let Err(e) = dev.connect().await {
            println!("    attempt {attempt}/3: connect returned '{e}' (checking link anyway)...");
        }
        // Poll ~12s for services to resolve; connect() may have errored spuriously.
        for _ in 0..24 {
            if dev.is_connected().await.unwrap_or(false) {
                let _ = dev.discover_services().await;
                if dev.characteristics().iter().any(|c| c.uuid == WRITE_UUID) {
                    return Ok(());
                }
            }
            time::sleep(Duration::from_millis(500)).await;
        }
        println!("    attempt {attempt}/3: services not ready, resetting link...");
        let _ = dev.disconnect().await;
        time::sleep(Duration::from_millis(1000)).await;
    }
    Err("could not establish a GATT session (link too weak — move the controller closer?)".into())
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

    let mut dev: Option<Peripheral> = None;
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

    // Let the adapter settle after scanning before we initiate a connection.
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

    // Keep the link alive: subscribe to the notify channel before we start writing.
    if let Some(nch) = dev.characteristics().into_iter().find(|c| c.uuid == NOTIFY_UUID) {
        match dev.subscribe(&nch).await {
            Ok(_) => println!("[+] subscribed to notifications (fff4)"),
            Err(e) => println!("[!] notify subscribe failed (continuing): {e}"),
        }
    }

    let chars = dev.characteristics();
    let ch = chars
        .iter()
        .find(|c| c.uuid == WRITE_UUID)
        .or_else(|| {
            chars.iter().find(|c| {
                c.properties
                    .intersects(CharPropFlags::WRITE | CharPropFlags::WRITE_WITHOUT_RESPONSE)
            })
        })
        .ok_or("no writable characteristic found")?
        .clone();

    let wtype = if ch.properties.contains(CharPropFlags::WRITE_WITHOUT_RESPONSE) {
        WriteType::WithoutResponse
    } else {
        WriteType::WithResponse
    };
    println!("[+] writing to {} using {:?}", ch.uuid, wtype);

    // Let the connection settle before the first command.
    time::sleep(Duration::from_millis(700)).await;

    let power_on = [0x7e, 0x00, 0x04, 0xf0, 0x00, 0x01, 0xff, 0x00, 0xef];
    let bright = |pct: u8| [0x7e, 0x00, 0x01, pct, 0x00, 0x00, 0x00, 0x00, 0xef];
    let color = |r: u8, g: u8, b: u8| [0x7e, 0x00, 0x05, 0x03, r, g, b, 0x00, 0xef];

    println!("[*] LIGHT SHOW — watch the strip:");
    send(&dev, &ch, wtype, &power_on, "POWER ON").await?;
    send(&dev, &ch, wtype, &bright(100), "brightness 100%").await?;
    send(&dev, &ch, wtype, &color(255, 0, 0), "RED").await?;
    send(&dev, &ch, wtype, &color(0, 255, 0), "GREEN").await?;
    send(&dev, &ch, wtype, &color(0, 0, 255), "BLUE").await?;
    send(&dev, &ch, wtype, &color(255, 255, 255), "WHITE").await?;
    send(&dev, &ch, wtype, &bright(15), "dim to 15%").await?;
    send(&dev, &ch, wtype, &bright(100), "back to 100%").await?;
    send(&dev, &ch, wtype, &color(128, 0, 255), "purple (final)").await?;

    println!("[+] POC complete — leaving strip on purple, disconnecting.");
    let _ = dev.disconnect().await;
    Ok(())
}

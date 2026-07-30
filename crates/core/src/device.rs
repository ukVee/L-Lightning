use std::error::Error;
use std::time::Duration;

use btleplug::api::{Central, CharPropFlags, Characteristic, Peripheral as _, WriteType};
use btleplug::platform::{Adapter, Peripheral};
use tokio::time;
use uuid::Uuid;

pub const TARGET_ADDR: &str = "BE:67:00:A5:CC:56";
pub const WRITE_UUID: Uuid = Uuid::from_u128(0x0000fff3_0000_1000_8000_00805f9b34fb);
pub const NOTIFY_UUID: Uuid = Uuid::from_u128(0x0000fff4_0000_1000_8000_00805f9b34fb);

const INTER_WRITE_DELAY_MS: u64 = 1100;
const BLE_WRITE_TIMEOUT_MS: u64 = 5000;

pub async fn find_elk(central: &Adapter) -> Result<Option<Peripheral>, btleplug::Error> {
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

pub async fn ensure_connected(dev: &Peripheral) -> Result<(), Box<dyn Error>> {
    if !dev.is_connected().await? {
        dev.connect().await?;
        dev.discover_services().await?;
        if let Some(nch) = dev.characteristics().into_iter().find(|c| c.uuid == NOTIFY_UUID) {
            let _ = dev.subscribe(&nch).await;
        }
    }
    Ok(())
}

pub async fn establish_gatt(dev: &Peripheral) -> Result<(), Box<dyn Error>> {
    time::sleep(Duration::from_millis(1000)).await;

    for attempt in 1..=4u32 {
        match time::timeout(
            Duration::from_millis(BLE_WRITE_TIMEOUT_MS),
            dev.connect(),
        )
        .await
        {
            Ok(Err(e)) => {
                eprintln!(
                    "    attempt {attempt}/4: connect returned '{e}' (checking link anyway)..."
                );
            }
            Err(e) => {
                eprintln!(
                    "    attempt {attempt}/4: connect timed out: {e} (checking link anyway)..."
                );
            }
            _ => {}
        }

        time::sleep(Duration::from_millis(2000)).await;

        for _ in 0..40 {
            if dev.is_connected().await.unwrap_or(false) {
                let _ = time::timeout(
                    Duration::from_millis(BLE_WRITE_TIMEOUT_MS),
                    dev.discover_services(),
                )
                .await;
                if dev.characteristics().iter().any(|c| c.uuid == WRITE_UUID) {
                    return Ok(());
                }
            }
            time::sleep(Duration::from_millis(500)).await;
        }

        eprintln!("    attempt {attempt}/4: services not ready, resetting...");
        let _ = dev.disconnect().await;
        time::sleep(Duration::from_millis(2000)).await;
    }

    Err("could not establish a GATT session (link too weak — move the controller closer?)".into())
}

pub fn find_write_characteristic(chars: &[Characteristic]) -> Option<&Characteristic> {
    chars.iter().find(|c| c.uuid == WRITE_UUID).or_else(|| {
        chars.iter().find(|c| {
            c.properties
                .intersects(CharPropFlags::WRITE | CharPropFlags::WRITE_WITHOUT_RESPONSE)
        })
    })
}

pub fn write_type_for(ch: &Characteristic) -> WriteType {
    if ch.properties.contains(CharPropFlags::WRITE_WITHOUT_RESPONSE) {
        WriteType::WithoutResponse
    } else {
        WriteType::WithResponse
    }
}

pub async fn write_command(
    dev: &Peripheral,
    ch: &Characteristic,
    wtype: WriteType,
    bytes: &[u8],
) -> Result<(), Box<dyn Error>> {
    let first = time::timeout(
        Duration::from_millis(BLE_WRITE_TIMEOUT_MS),
        dev.write(ch, bytes, wtype),
    )
    .await;
    match first {
        Ok(Ok(())) => {}
        _ => {
            ensure_connected(dev).await?;
            time::timeout(
                Duration::from_millis(BLE_WRITE_TIMEOUT_MS),
                dev.write(ch, bytes, wtype),
            )
            .await
            .map_err(|e| Box::new(e) as Box<dyn Error>)?
            .map_err(|e| Box::new(e) as Box<dyn Error>)?;
        }
    }
    time::sleep(Duration::from_millis(INTER_WRITE_DELAY_MS)).await;
    Ok(())
}

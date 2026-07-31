use std::sync::Arc;
use std::time::Duration;

use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager, Peripheral};
use tokio::sync::{mpsc, watch, Mutex};

use crate::device;
use crate::protocol;

const INTER_WRITE_MS: u64 = 1100;
const SCAN_TIMEOUT_SECS: u64 = 60;
const IDLE_DISCONNECT_SECS: u64 = 30;
const BLE_WRITE_TIMEOUT_MS: u64 = 5000;
const RECONNECT_WAIT_SECS: u64 = 2;

fn is_transient_ble_error(e: &str) -> bool {
    let msg = e.to_lowercase();
    msg.contains("in progress")
        || msg.contains("le-connection-abort-by-local")
        || msg.contains("software caused connection abort")
        || msg.contains("device or resource busy")
        || msg.contains("connection refused")
        || msg.contains("already")
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConnState {
    Idle,
    Scanning,
    Connecting,
    Connected,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct DeviceState {
    pub power: bool,
    pub brightness: u8,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[allow(dead_code)]
enum Cmd {
    ApplyState(DeviceState),
    Stop,
}

#[derive(Clone)]
pub struct Connection {
    cmd_tx: mpsc::Sender<Cmd>,
    state_rx: watch::Receiver<ConnState>,
    last_state: Arc<Mutex<DeviceState>>,
    fsm_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl Connection {
    pub async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let manager = Manager::new().await?;
        let adapter = manager
            .adapters()
            .await?
            .into_iter()
            .next()
            .ok_or("no Bluetooth adapter found")?;

        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>(8);
        let (state_tx, state_rx) = watch::channel(ConnState::Idle);
        let last_state = Arc::new(Mutex::new(DeviceState::default()));
        let fsm_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>> = Arc::new(Mutex::new(None));

        let fsm = Fsm {
            adapter,
            peripheral: None,
            write_char: None,
            write_type: None,
            cmd_rx,
            state_tx,
            last_state: last_state.clone(),
        };

        let handle = tokio::spawn(fsm.run());
        *fsm_handle.lock().await = Some(handle);

        Ok(Self {
            cmd_tx,
            state_rx,
            last_state,
            fsm_handle,
        })
    }

    pub fn set(&self, state: DeviceState) {
        let _ = self.cmd_tx.try_send(Cmd::ApplyState(state));
    }

    pub fn state(&self) -> ConnState {
        self.state_rx.borrow().clone()
    }

    pub fn watch(&self) -> watch::Receiver<ConnState> {
        self.state_rx.clone()
    }

    pub async fn device_state(&self) -> DeviceState {
        self.last_state.lock().await.clone()
    }

    pub async fn shutdown(self) {
        let _ = self.cmd_tx.try_send(Cmd::Stop);
        if let Some(h) = self.fsm_handle.lock().await.take() {
            let _ = h.await;
        }
    }
}

struct Fsm {
    adapter: Adapter,
    peripheral: Option<Peripheral>,
    write_char: Option<btleplug::api::Characteristic>,
    write_type: Option<btleplug::api::WriteType>,
    cmd_rx: mpsc::Receiver<Cmd>,
    state_tx: watch::Sender<ConnState>,
    last_state: Arc<Mutex<DeviceState>>,
}

impl Fsm {
    async fn run(mut self) {
        loop {
            let current = self.state_tx.borrow().clone();
            match current {
                ConnState::Idle => {
                    match tokio::time::timeout(
                        Duration::from_secs(30),
                        self.cmd_rx.recv(),
                    )
                    .await
                    {
                        Ok(Some(Cmd::ApplyState(s))) => {
                            *self.last_state.lock().await = s;
                            self.transition(ConnState::Scanning);
                        }
                        Ok(Some(Cmd::Stop)) | Ok(None) => break,
                        Err(_elapsed) => {}
                    }
                }

                ConnState::Scanning => {
                    match self.scan_for_device().await {
                        Ok(Some(peripheral)) => {
                            self.peripheral = Some(peripheral);
                            self.transition(ConnState::Connecting);
                        }
                        Ok(None) => {
                            eprintln!("[l-lightning] device not found, returning to idle");
                            self.transition(ConnState::Idle);
                        }
                        Err(e) => {
                            if is_transient_ble_error(&e.to_string()) {
                                eprintln!("[l-lightning] scan transient error: {e}, retrying");
                                tokio::time::sleep(Duration::from_secs(RECONNECT_WAIT_SECS)).await;
                            } else {
                                eprintln!("[l-lightning] scan error: {e}");
                                tokio::time::sleep(Duration::from_secs(5)).await;
                                self.transition(ConnState::Idle);
                            }
                        }
                    }
                }

                ConnState::Connecting => {
                    let dev = match self.peripheral.clone() {
                        Some(d) => d,
                        None => {
                            self.transition(ConnState::Idle);
                            continue;
                        }
                    };

                    tokio::time::sleep(Duration::from_millis(600)).await;

                    let gatt_result: Result<(), String> = device::establish_gatt(&dev)
                        .await
                        .map_err(|e| e.to_string());
                    if let Err(e) = gatt_result {
                        if is_transient_ble_error(&e.to_string()) {
                            eprintln!("[l-lightning] connect transient: {e}, rescanning");
                            self.peripheral = None;
                            self.transition(ConnState::Scanning);
                        } else {
                            eprintln!("[l-lightning] connect/GATT failed: {e}");
                            self.peripheral = None;
                            self.transition(ConnState::Idle);
                        }
                    } else {
                        self.finish_connect(&dev).await;
                    }
                }

                ConnState::Connected => {
                    tokio::select! {
                        cmd = self.cmd_rx.recv() => {
                            match cmd {
                                Some(Cmd::ApplyState(s)) => {
                                    match self.write_delta(&s).await {
                                        Ok(()) => {
                                            *self.last_state.lock().await = s;
                                        }
                                        Err(e) => {
                                            if is_transient_ble_error(&e.to_string()) {
                                                eprintln!("[l-lightning] write transient: {e}, keeping link");
                                            } else {
                                                eprintln!("[l-lightning] write failed, disconnecting");
                                                let _ = self.disconnect().await;
                                                self.transition(ConnState::Idle);
                                            }
                                        }
                                    }
                                }
                                Some(Cmd::Stop) | None => {
                                    let _ = self.disconnect().await;
                                    break;
                                }
                            }
                        }
                        _ = tokio::time::sleep(Duration::from_secs(IDLE_DISCONNECT_SECS)) => {
                            eprintln!("[l-lightning] idle timeout, disconnecting");
                            let _ = self.disconnect().await;
                            self.transition(ConnState::Idle);
                        }
                    }
                }
            }
        }
    }

    fn transition(&self, new_state: ConnState) {
        let _ = self.state_tx.send(new_state);
    }

    async fn disconnect(&mut self) -> Result<(), ()> {
        if let Some(ref dev) = self.peripheral {
            let _ = dev.disconnect().await;
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        self.peripheral = None;
        self.write_char = None;
        self.write_type = None;
        Ok(())
    }

    async fn scan_for_device(&self) -> Result<Option<Peripheral>, btleplug::Error> {
        let _ = self.adapter.stop_scan().await;
        self.adapter.start_scan(ScanFilter::default()).await?;

        let mut elapsed = 0u64;
        while elapsed < SCAN_TIMEOUT_SECS {
            tokio::time::sleep(Duration::from_secs(2)).await;
            elapsed += 2;
            if self.cmd_rx.is_closed() {
                let _ = self.adapter.stop_scan().await;
                return Ok(None);
            }
            if let Some(p) = device::find_elk(&self.adapter).await? {
                let _ = self.adapter.stop_scan().await;
                return Ok(Some(p));
            }
        }

        let _ = self.adapter.stop_scan().await;
        Ok(None)
    }

    async fn finish_connect(&mut self, dev: &Peripheral) {
        let chars: Vec<_> = dev.characteristics().into_iter().collect();

        if let Some(nch) = chars.iter().find(|c| c.uuid == device::NOTIFY_UUID) {
            let _ = dev.subscribe(nch).await;
        }

        match (
            device::find_write_characteristic(&chars).cloned(),
            device::find_write_characteristic(&chars).map(device::write_type_for),
        ) {
            (Some(ch), Some(wt)) => {
                tokio::time::sleep(Duration::from_millis(700)).await;

                self.write_char = Some(ch);
                self.write_type = Some(wt);

                let ls = self.last_state.lock().await.clone();
                if self.replay_state(dev, &ls).await.is_err() {
                    eprintln!("[l-lightning] replay write failed, disconnecting");
                    let _ = dev.disconnect().await;
                    self.peripheral = None;
                    self.transition(ConnState::Idle);
                    return;
                }

                tokio::time::sleep(Duration::from_millis(1500)).await;

                self.transition(ConnState::Connected);
            }
            _ => {
                eprintln!("[l-lightning] no writable characteristic after GATT discovery");
                let _ = dev.disconnect().await;
                self.peripheral = None;
                self.transition(ConnState::Idle);
            }
        }
    }

    async fn write_delta(&self, state: &DeviceState) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let dev = self.peripheral.as_ref().ok_or("no peripheral")?;
        let ch = self.write_char.as_ref().ok_or("no write characteristic")?;
        let wtype = self.write_type.ok_or("no write type")?;

        let old = self.last_state.lock().await.clone();

        if state.power != old.power {
            if state.power {
                tokio::time::timeout(
                    Duration::from_millis(BLE_WRITE_TIMEOUT_MS),
                    dev.write(ch, &protocol::power_on(), wtype),
                )
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)??
                ;
                tokio::time::sleep(Duration::from_millis(INTER_WRITE_MS)).await;
            } else {
                tokio::time::timeout(
                    Duration::from_millis(BLE_WRITE_TIMEOUT_MS),
                    dev.write(ch, &protocol::power_off(), wtype),
                )
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)??
                ;
                tokio::time::sleep(Duration::from_millis(INTER_WRITE_MS)).await;
                return Ok(());
            }
        }

        if state.brightness != old.brightness {
            tokio::time::timeout(
                Duration::from_millis(BLE_WRITE_TIMEOUT_MS),
                dev.write(ch, &protocol::set_brightness(state.brightness), wtype),
            )
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)??
            ;
            tokio::time::sleep(Duration::from_millis(INTER_WRITE_MS)).await;
        }

        if state.r != old.r || state.g != old.g || state.b != old.b {
            tokio::time::timeout(
                Duration::from_millis(BLE_WRITE_TIMEOUT_MS),
                dev.write(ch, &protocol::set_color(state.r, state.g, state.b), wtype),
            )
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)??
            ;
            tokio::time::sleep(Duration::from_millis(INTER_WRITE_MS)).await;
        }

        Ok(())
    }

    async fn replay_state(&self, dev: &Peripheral, state: &DeviceState) -> Result<(), ()> {
        let ch = self.write_char.as_ref().ok_or(())?;
        let wtype = self.write_type.ok_or(())?;

        if state.power && state.brightness > 0 {
            tokio::time::timeout(
                Duration::from_millis(BLE_WRITE_TIMEOUT_MS),
                dev.write(ch, &protocol::power_on(), wtype),
            )
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
            tokio::time::sleep(Duration::from_millis(700)).await;
            tokio::time::timeout(
                Duration::from_millis(BLE_WRITE_TIMEOUT_MS),
                dev.write(ch, &protocol::set_brightness(state.brightness), wtype),
            )
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
            tokio::time::sleep(Duration::from_millis(700)).await;
            tokio::time::timeout(
                Duration::from_millis(BLE_WRITE_TIMEOUT_MS),
                dev.write(ch, &protocol::set_color(state.r, state.g, state.b), wtype),
            )
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
            tokio::time::sleep(Duration::from_millis(700)).await;
        }
        Ok(())
    }
}

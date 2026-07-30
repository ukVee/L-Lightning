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
    cmd_tx: mpsc::UnboundedSender<Cmd>,
    state_rx: watch::Receiver<ConnState>,
    last_state: Arc<Mutex<DeviceState>>,
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

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Cmd>();
        let (state_tx, state_rx) = watch::channel(ConnState::Idle);
        let last_state = Arc::new(Mutex::new(DeviceState::default()));

        let fsm = Fsm {
            adapter,
            peripheral: None,
            write_char: None,
            write_type: None,
            cmd_rx,
            state_tx,
            last_state: last_state.clone(),
        };

        tokio::spawn(fsm.run());

        Ok(Self {
            cmd_tx,
            state_rx,
            last_state,
        })
    }

    pub fn set(&self, state: DeviceState) {
        let _ = self.cmd_tx.send(Cmd::ApplyState(state));
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
}

struct Fsm {
    adapter: Adapter,
    peripheral: Option<Peripheral>,
    write_char: Option<btleplug::api::Characteristic>,
    write_type: Option<btleplug::api::WriteType>,
    cmd_rx: mpsc::UnboundedReceiver<Cmd>,
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
                            eprintln!("[l-lightning] scan error: {e}");
                            tokio::time::sleep(Duration::from_secs(5)).await;
                            self.transition(ConnState::Idle);
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
                        eprintln!("[l-lightning] connect/GATT failed: {e}");
                        self.peripheral = None;
                        self.transition(ConnState::Idle);
                    } else {
                        self.finish_connect(&dev).await;
                    }
                }

                ConnState::Connected => {
                    tokio::select! {
                        cmd = self.cmd_rx.recv() => {
                            match cmd {
                                Some(Cmd::ApplyState(s)) => {
                                    if self.write_delta(&s).await.is_err() {
                                        eprintln!("[l-lightning] write failed, disconnecting");
                                        let _ = self.disconnect().await;
                                        self.transition(ConnState::Idle);
                                    } else {
                                        *self.last_state.lock().await = s;
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

    async fn disconnect(&self) -> Result<(), ()> {
        if let Some(ref dev) = self.peripheral {
            let _ = dev.disconnect().await;
        }
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
                self.replay_state(dev, &ls).await;

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

    async fn write_delta(&self, state: &DeviceState) -> Result<(), ()> {
        let dev = self.peripheral.as_ref().ok_or(())?;
        let ch = self.write_char.as_ref().ok_or(())?;
        let wtype = self.write_type.ok_or(())?;

        let old = self.last_state.lock().await.clone();

        if state.power != old.power {
            if state.power {
                dev.write(ch, &protocol::power_on(), wtype).await.map_err(|_| ())?;
                tokio::time::sleep(Duration::from_millis(INTER_WRITE_MS)).await;
            } else {
                dev.write(ch, &protocol::power_off(), wtype).await.map_err(|_| ())?;
                tokio::time::sleep(Duration::from_millis(INTER_WRITE_MS)).await;
                return Ok(());
            }
        }

        if state.brightness != old.brightness {
            dev.write(ch, &protocol::set_brightness(state.brightness), wtype)
                .await
                .map_err(|_| ())?;
            tokio::time::sleep(Duration::from_millis(INTER_WRITE_MS)).await;
        }

        if state.r != old.r || state.g != old.g || state.b != old.b {
            dev.write(ch, &protocol::set_color(state.r, state.g, state.b), wtype)
                .await
                .map_err(|_| ())?;
            tokio::time::sleep(Duration::from_millis(INTER_WRITE_MS)).await;
        }

        Ok(())
    }

    async fn replay_state(&self, dev: &Peripheral, state: &DeviceState) {
        let ch = match self.write_char.as_ref() {
            Some(c) => c,
            None => return,
        };
        let wtype = match self.write_type {
            Some(wt) => wt,
            None => return,
        };

        if state.power && state.brightness > 0 {
            let _ = dev.write(ch, &protocol::power_on(), wtype).await;
            tokio::time::sleep(Duration::from_millis(700)).await;
            let _ = dev.write(ch, &protocol::set_brightness(state.brightness), wtype).await;
            tokio::time::sleep(Duration::from_millis(700)).await;
            let _ = dev.write(ch, &protocol::set_color(state.r, state.g, state.b), wtype).await;
            tokio::time::sleep(Duration::from_millis(700)).await;
        }
    }
}

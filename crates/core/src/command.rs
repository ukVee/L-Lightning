use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::connection::{ConnState, Connection, DeviceState};

#[derive(Clone)]
pub struct CommandLayer {
    conn: Connection,
    min_gap: Arc<Mutex<Duration>>,
    pending: Arc<Mutex<Option<DeviceState>>>,
    last_sent: Arc<Mutex<Instant>>,
}

impl CommandLayer {
    pub fn new(conn: Connection) -> Self {
        let min_gap = Arc::new(Mutex::new(Duration::from_millis(1100)));
        let pending = Arc::new(Mutex::new(None));
        let last_sent = Arc::new(Mutex::new(Instant::now()));

        let conn_c = conn.clone();
        let pending_c = pending.clone();
        let last_sent_c = last_sent.clone();
        let min_gap_c = min_gap.clone();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(10)).await;
                let gap = *min_gap_c.lock().unwrap();
                let mut last = last_sent_c.lock().unwrap();
                if last.elapsed() >= gap {
                    let mut p = pending_c.lock().unwrap();
                    if let Some(s) = p.take() {
                        conn_c.set(s);
                        *last = Instant::now();
                    }
                }
            }
        });

        Self {
            conn,
            min_gap,
            pending,
            last_sent,
        }
    }

    pub fn set_gap(&self, gap: Duration) {
        *self.min_gap.lock().unwrap() = gap;
    }

    pub fn gap(&self) -> Duration {
        *self.min_gap.lock().unwrap()
    }

    pub fn apply(&self, state: DeviceState) {
        let now = Instant::now();
        let gap = *self.min_gap.lock().unwrap();

        let mut pending = self.pending.lock().unwrap();
        *pending = Some(state);

        let mut last = self.last_sent.lock().unwrap();
        if now.duration_since(*last) >= gap {
            if let Some(s) = pending.take() {
                self.conn.set(s);
                *last = now;
            }
        }
    }

    pub fn state(&self) -> ConnState {
        self.conn.state()
    }

    pub fn watch(&self) -> tokio::sync::watch::Receiver<ConnState> {
        self.conn.watch()
    }

    pub async fn device_state(&self) -> DeviceState {
        self.conn.device_state().await
    }

    pub async fn discover_min_gap(&self) -> Duration {
        let candidates: [u64; 7] = [1100, 800, 500, 200, 100, 80, 50];
        let mut last_good = Duration::from_millis(candidates[0]);
        let test_states = [
            DeviceState {
                power: true,
                brightness: 100,
                r: 255,
                g: 0,
                b: 0,
            },
            DeviceState {
                power: true,
                brightness: 100,
                r: 0,
                g: 255,
                b: 0,
            },
            DeviceState {
                power: true,
                brightness: 100,
                r: 0,
                g: 0,
                b: 255,
            },
        ];

        for &gap_ms in &candidates {
            let gap = Duration::from_millis(gap_ms);
            self.set_gap(gap);

            for state in &test_states {
                self.apply(state.clone());
                tokio::time::sleep(gap).await;
            }

            tokio::time::sleep(Duration::from_secs(2)).await;

            match self.conn.state() {
                ConnState::Connected => last_good = gap,
                _ => return last_good,
            }
        }

        last_good
    }
}

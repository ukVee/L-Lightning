use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::connection::{ConnState, Connection, DeviceState};

#[derive(Clone)]
pub struct CommandLayer {
    conn: Connection,
    min_gap: Arc<Mutex<Duration>>,
    pending: Arc<Mutex<Option<DeviceState>>>,
    last_sent: Arc<Mutex<Instant>>,
    shutdown_flag: Arc<AtomicBool>,
    flush_handle: Arc<StdMutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl CommandLayer {
    pub fn new(conn: Connection) -> Self {
        let min_gap = Arc::new(Mutex::new(Duration::from_millis(1100)));
        let pending = Arc::new(Mutex::new(None));
        let last_sent = Arc::new(Mutex::new(Instant::now() - Duration::from_millis(1100)));
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let flush_handle: Arc<StdMutex<Option<tokio::task::JoinHandle<()>>>> =
            Arc::new(StdMutex::new(None));

        let conn_c = conn.clone();
        let pending_c = pending.clone();
        let last_sent_c = last_sent.clone();
        let min_gap_c = min_gap.clone();
        let shutdown_flag_c = shutdown_flag.clone();
        let flush_handle_c = flush_handle.clone();

        let handle = tokio::spawn(async move {
            loop {
                if shutdown_flag_c.load(Ordering::SeqCst) {
                    if let Some(s) = pending_c.lock().await.take() {
                        conn_c.set(s);
                    }
                    break;
                }
                let gap = *min_gap_c.lock().await;
                let elapsed = last_sent_c.lock().await.elapsed();
                if elapsed >= gap {
                    let mut p = pending_c.lock().await;
                    if let Some(s) = p.take() {
                        conn_c.set(s);
                        *last_sent_c.lock().await = Instant::now();
                        tokio::time::sleep(gap).await;
                        continue;
                    }
                }
                tokio::time::sleep(gap.saturating_sub(elapsed)).await;
            }
        });

        *flush_handle_c.lock().unwrap() = Some(handle);

        Self {
            conn,
            min_gap,
            pending,
            last_sent,
            shutdown_flag,
            flush_handle,
        }
    }

    pub async fn set_gap(&self, gap: Duration) {
        *self.min_gap.lock().await = gap;
    }

    pub async fn gap(&self) -> Duration {
        *self.min_gap.lock().await
    }

    pub async fn apply(&self, state: DeviceState) {
        let now = Instant::now();
        let gap = *self.min_gap.lock().await;

        let mut pending = self.pending.lock().await;
        *pending = Some(state);

        let mut last = self.last_sent.lock().await;
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

    pub async fn shutdown(self) {
        self.shutdown_flag.store(true, Ordering::SeqCst);
        let handle = self.flush_handle.lock().unwrap().take();
        if let Some(h) = handle {
            let _ = h.await;
        }
        self.conn.shutdown().await;
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

        let mut attempts = 0;
        loop {
            if attempts >= 3 {
                eprintln!("[l-lightning] discover_min_gap: could not reach Connected, giving up");
                return last_good;
            }
            match self.conn.state() {
                ConnState::Connected => break,
                ConnState::Idle => {
                    eprintln!("[l-lightning] discover_min_gap: idle, sending wake command (attempt {})", attempts + 1);
                    self.apply(test_states[0].clone()).await;
                    tokio::time::sleep(Duration::from_secs(12)).await;
                }
                other => {
                    eprintln!("[l-lightning] discover_min_gap: state is {:?}, waiting (attempt {})", other, attempts + 1);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
            attempts += 1;
        }

        for &gap_ms in &candidates {
            let gap = Duration::from_millis(gap_ms);
            self.set_gap(gap).await;

            for state in &test_states {
                self.apply(state.clone()).await;
                tokio::time::sleep(gap).await;
            }

            tokio::time::sleep(Duration::from_secs(2)).await;

            let cs = self.conn.state();
            if cs == ConnState::Connected {
                last_good = gap;
            } else {
                eprintln!(
                    "[l-lightning] discover_min_gap: gap {}ms lost link (state: {:?}), best was {}ms",
                    gap_ms, cs, last_good.as_millis()
                );
                if cs == ConnState::Idle {
                    eprintln!(
                        "[l-lightning] discover_min_gap: went to Idle — likely write failure, not idle timeout"
                    );
                }
                return last_good;
            }
        }

        last_good
    }
}

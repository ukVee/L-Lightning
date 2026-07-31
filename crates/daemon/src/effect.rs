use std::sync::Arc;
use std::time::Duration;

use l_lightning_core::command::CommandLayer;
use l_lightning_core::connection::DeviceState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    Breathe,
    ColorCycle,
    Strobe,
    FadeTo,
}

#[derive(Debug, Clone)]
pub struct EffectParams {
    pub speed: u64,
    pub target: Option<DeviceState>,
}

pub struct EffectHandle {
    cancel: Arc<tokio::sync::Notify>,
    task: tokio::task::JoinHandle<()>,
}

impl EffectHandle {
    pub fn stop(&self) {
        self.cancel.notify_one();
        self.task.abort();
    }

    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

pub fn start(
    cmd_layer: CommandLayer,
    kind: EffectKind,
    params: EffectParams,
    initial_state: DeviceState,
) -> EffectHandle {
    let cancel = Arc::new(tokio::sync::Notify::new());
    let cancel_c = cancel.clone();

    let task = tokio::spawn(async move {
        run_effect(cmd_layer, kind, params, initial_state, cancel_c).await;
    });

    EffectHandle { cancel, task }
}

async fn run_effect(
    cmd_layer: CommandLayer,
    kind: EffectKind,
    params: EffectParams,
    mut state: DeviceState,
    cancel: Arc<tokio::sync::Notify>,
) {
    let period = Duration::from_millis(params.speed);
    let start = tokio::time::Instant::now();

    let initial_hsv = rgb_to_hsv(state.r, state.g, state.b);

    loop {
        tokio::select! {
            _ = cancel.notified() => break,
            _ = tokio::time::sleep(period) => {},
        }

        let elapsed = start.elapsed();

        state = match kind {
            EffectKind::Breathe => frame_breathe(&state, elapsed, params.speed),
            EffectKind::ColorCycle => {
                frame_color_cycle(&state, &initial_hsv, elapsed, params.speed)
            }
            EffectKind::Strobe => frame_strobe(&state, elapsed, params.speed),
            EffectKind::FadeTo => {
                match frame_fade_to(&state, params.target.as_ref()) {
                    Some(next) => next,
                    None => break,
                }
            }
        };

        cmd_layer.apply(state.clone()).await;
    }
}

pub fn effect_label(kind: &EffectKind) -> &str {
    match kind {
        EffectKind::Breathe => "breathe",
        EffectKind::ColorCycle => "color_cycle",
        EffectKind::Strobe => "strobe",
        EffectKind::FadeTo => "fade_to",
    }
}

fn frame_breathe(state: &DeviceState, elapsed: Duration, speed: u64) -> DeviceState {
    let period_ms = (speed.saturating_mul(100)) as f64;
    let t_secs = elapsed.as_millis() as f64 / 1000.0;
    let cycle = (t_secs * std::f64::consts::TAU / (period_ms / 1000.0)).sin();
    let brightness = (((cycle + 1.0) / 2.0) * 100.0).round() as u8;

    DeviceState {
        brightness,
        power: brightness > 0,
        ..*state
    }
}

fn frame_color_cycle(
    state: &DeviceState,
    initial_hsv: &(f64, f64, f64),
    elapsed: Duration,
    speed: u64,
) -> DeviceState {
    let (_, s, v) = *initial_hsv;
    let rotation_ms = (speed.saturating_mul(200)) as f64;
    let t_secs = elapsed.as_millis() as f64 / 1000.0;
    let hue = (t_secs * 360.0 / (rotation_ms / 1000.0)) % 360.0;

    let (r, g, b) = hsv_to_rgb(hue, s, v);
    DeviceState {
        r,
        g,
        b,
        power: true,
        ..*state
    }
}

fn frame_strobe(state: &DeviceState, elapsed: Duration, speed: u64) -> DeviceState {
    let half_period_ms = speed.max(50);
    let power = (elapsed.as_millis() as u64 / half_period_ms).is_multiple_of(2);
    DeviceState { power, ..*state }
}

fn frame_fade_to(
    state: &DeviceState,
    target: Option<&DeviceState>,
) -> Option<DeviceState> {
    let target = target?;

    if state.r == target.r
        && state.g == target.g
        && state.b == target.b
        && state.brightness == target.brightness
        && state.power == target.power
    {
        return None;
    }

    fn step(current: u8, target: u8) -> u8 {
        match current.cmp(&target) {
            std::cmp::Ordering::Less => current + 1,
            std::cmp::Ordering::Greater => current - 1,
            std::cmp::Ordering::Equal => current,
        }
    }

    Some(DeviceState {
        power: target.power,
        brightness: step(state.brightness, target.brightness),
        r: step(state.r, target.r),
        g: step(state.g, target.g),
        b: step(state.b, target.b),
    })
}

fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f64, f64, f64) {
    let rf = r as f64 / 255.0;
    let gf = g as f64 / 255.0;
    let bf = b as f64 / 255.0;

    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let delta = max - min;

    let h = if delta == 0.0 {
        0.0
    } else if max == rf {
        60.0 * (((gf - bf) / delta) % 6.0)
    } else if max == gf {
        60.0 * ((bf - rf) / delta + 2.0)
    } else {
        60.0 * ((rf - gf) / delta + 4.0)
    }
    .rem_euclid(360.0);

    let s = if max == 0.0 { 0.0 } else { delta / max };
    let v = max;

    (h, s, v)
}

fn hsv_to_rgb(h: f64, s: f64, v: f64) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;

    let (rp, gp, bp) = match (h as u32 / 60) % 6 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };

    (
        ((rp + m) * 255.0).round() as u8,
        ((gp + m) * 255.0).round() as u8,
        ((bp + m) * 255.0).round() as u8,
    )
}

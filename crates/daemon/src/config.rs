use std::fs;
use std::path::PathBuf;

use l_lightning_core::connection::DeviceState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    pub id: u64,
    pub name: String,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub brightness: u8,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub color_order: Option<String>,
    #[serde(default)]
    pub presets: Vec<Preset>,
    #[serde(default)]
    pub last_state: Option<DeviceState>,
}

pub fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        let mut p = PathBuf::from(dir);
        p.push("l-lightning");
        p
    } else if let Some(dir) = dirs_fallback() {
        let mut p = dir;
        p.push(".config");
        p.push("l-lightning");
        p
    } else {
        PathBuf::from("/tmp/l-lightning-config")
    }
}

pub fn config_path() -> PathBuf {
    let mut p = config_dir();
    p.push("config.toml");
    p
}

pub fn load() -> Config {
    let path = config_path();
    match fs::read_to_string(&path) {
        Ok(raw) => match toml::from_str(&raw) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("[l-lightningd] config parse error: {e}, backing up and using defaults");
                let _ = fs::rename(&path, path.with_extension("toml.bak"));
                Config::default()
            }
        },
        Err(_) => Config::default(),
    }
}

pub fn save(config: &Config) {
    let dir = config_dir();
    let _ = fs::create_dir_all(&dir);
    let path = config_path();
    if let Ok(raw) = toml::to_string_pretty(config) {
        let tmp = path.with_extension("tmp");
        let _ = fs::write(&tmp, &raw);
        let _ = fs::rename(&tmp, &path);
    }
}

fn dirs_fallback() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

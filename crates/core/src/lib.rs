pub mod command;
pub mod connection;
pub mod device;
pub mod protocol;
pub mod rpc;

use std::path::PathBuf;

pub fn socket_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        let mut p = PathBuf::from(dir);
        p.push("l-lightning");
        std::fs::create_dir_all(&p).ok();
        p.push("daemon.sock");
        p
    } else {
        PathBuf::from("/tmp/l-lightning.sock")
    }
}

//! Thin async client of `cosmic-fabricd`'s unix socket (line-delimited JSON).
//! Mirrors the daemon's socket path logic; the GUIs never talk to fabric/ollama
//! directly — only to the daemon.

use serde::Deserialize;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

pub fn sock_path() -> PathBuf {
    let base = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        format!(
            "{}/.cache/cosmic-fabric",
            std::env::var("HOME").unwrap_or_default()
        )
    });
    PathBuf::from(base).join("cosmic-fabric.sock")
}

async fn call(req: serde_json::Value) -> Result<serde_json::Value, String> {
    let mut stream = UnixStream::connect(sock_path())
        .await
        .map_err(|e| format!("daemon not reachable: {e}"))?;
    let line = serde_json::to_string(&req).map_err(|e| e.to_string())? + "\n";
    stream
        .write_all(line.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut resp = String::new();
    reader
        .read_line(&mut resp)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&resp).map_err(|e| format!("bad response: {e}"))
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Loaded {
    pub model: Option<String>,
    pub gpu_pct: Option<f64>,
    pub ctx: Option<u64>,
    pub vram_mib: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Vram {
    pub used: u64,
    pub free: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Status {
    pub serve: bool,
    #[serde(default)]
    pub loaded: Vec<Loaded>,
    pub vram: Option<Vram>,
    pub default_model: Option<String>,
    pub default_vendor: Option<String>,
}

pub async fn status() -> Result<Status, String> {
    let v = call(serde_json::json!({ "op": "status" })).await?;
    serde_json::from_value(v).map_err(|e| e.to_string())
}

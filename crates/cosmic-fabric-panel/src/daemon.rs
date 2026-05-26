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

pub async fn patterns() -> Result<Vec<String>, String> {
    let v = call(serde_json::json!({ "op": "patterns" })).await?;
    let arr = v.get("patterns").cloned().unwrap_or_default();
    serde_json::from_value(arr).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RunResult {
    pub output: Option<String>,
    pub model: Option<String>,
    pub placement: Option<f64>,
    pub error: Option<String>,
}

pub async fn run(pattern: String, input: String) -> Result<RunResult, String> {
    let v = call(serde_json::json!({ "op": "run", "pattern": pattern, "input": input })).await?;
    serde_json::from_value(v).map_err(|e| e.to_string())
}

#[derive(Debug, Clone)]
pub enum RunEvent {
    Chunk(String),
    Done(RunResult),
    Error(String),
}

/// Stream a pattern run from the daemon: yields a `Chunk` per fragment, then a
/// `Done` (or `Error`). For use with `Subscription::run_with`.
pub fn run_stream(
    pattern: String,
    input: String,
) -> impl cosmic::iced::futures::Stream<Item = RunEvent> {
    use cosmic::iced::futures::SinkExt;
    cosmic::iced::stream::channel(64, move |mut output: cosmic::iced::futures::channel::mpsc::Sender<RunEvent>| async move {
        let conn = match UnixStream::connect(sock_path()).await {
            Ok(c) => c,
            Err(e) => {
                let _ = output.send(RunEvent::Error(format!("daemon: {e}"))).await;
                return;
            }
        };
        let (rd, mut wr) = conn.into_split();
        let req = serde_json::json!({
            "op": "run", "stream": true, "pattern": pattern, "input": input
        })
        .to_string()
            + "\n";
        if let Err(e) = wr.write_all(req.as_bytes()).await {
            let _ = output.send(RunEvent::Error(e.to_string())).await;
            return;
        }
        let mut reader = BufReader::new(rd);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let l = line.trim();
            if l.is_empty() {
                continue;
            }
            let v: serde_json::Value = match serde_json::from_str(l) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Some(c) = v.get("chunk").and_then(|x| x.as_str()) {
                if output.send(RunEvent::Chunk(c.to_string())).await.is_err() {
                    break;
                }
            } else if v.get("done").is_some() {
                let rr: RunResult = serde_json::from_value(v).unwrap_or_default();
                let _ = output.send(RunEvent::Done(rr)).await;
                break;
            } else if let Some(e) = v.get("error").and_then(|x| x.as_str()) {
                let _ = output.send(RunEvent::Error(e.to_string())).await;
                break;
            }
        }
    })
}

/// Current clipboard text (the panel's quick-run input source).
pub fn clipboard() -> String {
    std::process::Command::new("wl-paste")
        .arg("-n")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

pub fn set_clipboard(text: &str) {
    use std::io::Write;
    if let Ok(mut c) = std::process::Command::new("wl-copy")
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        if let Some(si) = c.stdin.as_mut() {
            let _ = si.write_all(text.as_bytes());
        }
        let _ = c.wait();
    }
}

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
    // Bound the whole round-trip: a hung/wedged daemon must surface as an error,
    // not an Task that never resolves. 30s covers the slowest non-stream op
    // (a URL fetch, ~10s server-side) with margin.
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
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
        serde_json::from_str::<serde_json::Value>(&resp).map_err(|e| format!("bad response: {e}"))
    })
    .await
    .map_err(|_| "daemon timed out".to_string())?
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

/// Vendor → models catalog (from `/models/names`'s `vendors` map), for the
/// per-pattern model picker.
pub async fn catalog() -> Result<std::collections::BTreeMap<String, Vec<String>>, String> {
    let v = call(serde_json::json!({ "op": "models" })).await?;
    let vendors = v.get("vendors").cloned().unwrap_or_default();
    serde_json::from_value(vendors).map_err(|e| e.to_string())
}

/// Render a pattern's prompt without running it (system + input, `{{vars}}`
/// substituted). For the workspace's prompt-first view and agent hand-off.
pub async fn assemble(pattern: String, input: String) -> Result<String, String> {
    let v = call(serde_json::json!({ "op": "assemble", "pattern": pattern, "input": input })).await?;
    if let Some(e) = v.get("error").and_then(|x| x.as_str()) {
        return Err(e.to_string());
    }
    v.get("prompt")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "no prompt in response".to_string())
}

/// Fetch a web page as text via the daemon (`scrape` = Jina markdown, or
/// `readability`). Returns (text, char_count).
pub async fn fetch_url(url: String, mode: String) -> Result<(String, usize), String> {
    let v = call(serde_json::json!({ "op": "fetch", "url": url, "mode": mode })).await?;
    if let Some(e) = v.get("error").and_then(|x| x.as_str()) {
        return Err(e.to_string());
    }
    let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let chars = v
        .get("chars")
        .and_then(|x| x.as_u64())
        .map(|n| n as usize)
        .unwrap_or_else(|| text.chars().count());
    Ok((text, chars))
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
    stream_request(serde_json::json!({
        "op": "run", "stream": true, "pattern": pattern, "input": input
    }))
}

/// Stream a chat turn into a fabric session (multi-turn history server-side).
/// No pattern → the daemon uses `raw_query` (the universal relay).
pub fn chat_stream(
    session: String,
    input: String,
) -> impl cosmic::iced::futures::Stream<Item = RunEvent> {
    stream_request(serde_json::json!({
        "op": "run", "stream": true, "session": session, "input": input
    }))
}

/// Shared streaming-run transport: write the request, yield `Chunk`s then `Done`/`Error`.
fn stream_request(req: serde_json::Value) -> impl cosmic::iced::futures::Stream<Item = RunEvent> {
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
        let req = req.to_string() + "\n";
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

#[derive(Debug, Clone)]
pub enum BrokerEvent {
    Start(String), // pattern
    Chunk(String),
    Done(RunResult),
    Error(String),
}

/// Long-lived subscription to the daemon's broadcast channel: runs dispatched
/// elsewhere (e.g. the launcher with output=panel) arrive here. Reconnects if
/// the daemon isn't up yet or restarts.
pub fn subscribe() -> impl cosmic::iced::futures::Stream<Item = BrokerEvent> {
    use cosmic::iced::futures::SinkExt;
    cosmic::iced::stream::channel(64, |mut output: cosmic::iced::futures::channel::mpsc::Sender<BrokerEvent>| async move {
        loop {
            if let Ok(conn) = UnixStream::connect(sock_path()).await {
                let (rd, mut wr) = conn.into_split();
                if wr.write_all(b"{\"op\":\"subscribe\"}\n").await.is_ok() {
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
                        let ev = match v.get("event").and_then(|x| x.as_str()) {
                            Some("start") => Some(BrokerEvent::Start(
                                v.get("pattern").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                            )),
                            Some("chunk") => v
                                .get("text")
                                .and_then(|x| x.as_str())
                                .map(|t| BrokerEvent::Chunk(t.to_string())),
                            Some("done") => {
                                Some(BrokerEvent::Done(serde_json::from_value(v.clone()).unwrap_or_default()))
                            }
                            Some("error") => Some(BrokerEvent::Error(
                                v.get("error").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                            )),
                            _ => None, // {"subscribed":true}, etc.
                        };
                        if let Some(ev) = ev {
                            if output.send(ev).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(3)).await; // reconnect
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

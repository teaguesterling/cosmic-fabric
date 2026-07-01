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

/// The woollama inference-seam snapshot from the `status` op: whether routing is
/// enabled, whether a router is reachable, the resolved endpoint, and which
/// backend a plain run uses right now (`active_backend`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Woollama {
    pub enabled: bool,
    pub reachable: bool,
    pub endpoint: Option<String>,
    pub active_backend: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Status {
    pub serve: bool,
    #[serde(default)]
    pub loaded: Vec<Loaded>,
    pub vram: Option<Vram>,
    pub default_model: Option<String>,
    pub default_vendor: Option<String>,
    #[serde(default)]
    pub woollama: Woollama,
}

impl Status {
    /// A short routing badge for the status line, shown only when woollama
    /// routing is enabled: `◆ woollama` when the router is reachable (plain runs
    /// route through it), `◇ woollama down` when it isn't (runs fall back to
    /// fabric). `None` when routing is off — fabric is the default, no clutter.
    pub fn woollama_badge(&self) -> Option<String> {
        if !self.woollama.enabled {
            return None;
        }
        Some(if self.woollama.reachable {
            "\u{25c6} woollama".to_string()
        } else {
            "\u{25c7} woollama down".to_string()
        })
    }

    /// The single backend-health chip for the status line: woollama's badge when
    /// routing is enabled (`◆ woollama` up / `◇ woollama down`), else `○ woollama
    /// off`. woollama is the backend now, so this replaces the old fabric `serve`
    /// display (they'd otherwise both report woollama — see the daemon's `serve`).
    pub fn backend_pill(&self) -> String {
        self.woollama_badge()
            .unwrap_or_else(|| "\u{25cb} woollama off".to_string())
    }
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

/// woollama's addressable model ids (`provider/model`), for the Run-tab per-run
/// model picker. Empty when woollama isn't reachable.
pub async fn woollama_models() -> Result<Vec<String>, String> {
    let v = call(serde_json::json!({ "op": "woollama_models" })).await?;
    let arr = v.get("models").cloned().unwrap_or_default();
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

/// Send a `tool_confirm` ack to the daemon (separate connection from the run
/// stream, so the run-loop thread can keep blocking on its own event). The id
/// must match what came on a `RunEvent::ToolConfirmRequired`.
pub async fn send_tool_confirm(id: String, approved: bool) -> Result<(), String> {
    let v = call(serde_json::json!({
        "op": "tool_confirm", "id": id, "approved": approved
    })).await?;
    if let Some(e) = v.get("error").and_then(|x| x.as_str()) {
        return Err(e.to_string());
    }
    Ok(())
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

/// A vision run: send an image (path) + a question; the daemon applies the
/// capability rule (auto-picks a vision model) and shells out to `fabric -a`.
/// Non-streaming — one request → one result.
pub async fn run_image(
    image: String,
    input: String,
    pattern: Option<String>,
) -> Result<RunResult, String> {
    let mut req = serde_json::json!({ "op": "run", "image": image, "input": input });
    if let Some(p) = pattern {
        req["pattern"] = serde_json::Value::String(p);
    }
    let v = call(req).await?;
    let rr: RunResult = serde_json::from_value(v).map_err(|e| e.to_string())?;
    match rr.error {
        Some(e) => Err(e),
        None => Ok(rr),
    }
}

#[derive(Debug, Clone)]
pub enum RunEvent {
    Chunk(String),
    /// A tool was invoked. `args` is the model-supplied JSON value (typically
    /// an object). `id` correlates with the matching `ToolResult`.
    ToolCall { name: String, args: serde_json::Value, id: String },
    /// A tool's execution completed. `summary` is a short, panel-displayable
    /// preview (truncated by the daemon).
    ToolResult { name: String, id: String, summary: String },
    /// A `panel-confirm`-mode tool needs user approval before execution.
    /// Surfaces fire a `tool_confirm` op back to the daemon (separate
    /// connection) with `id` + an `approved: bool`. Phase 2 work — daemon
    /// currently auto-denies if no confirm hook is wired (Phase 1 default).
    ToolConfirmRequired {
        name: String,
        args: serde_json::Value,
        id: String,
        command_preview: String,
    },
    Done(RunResult),
    Error(String),
}

/// Parse a tool-event payload (the inner object from `{"tool": …}` or the
/// `{"event":"tool", …}` envelope) into the matching `RunEvent` variant.
/// Returns None for unknown / malformed shapes (forward-compat).
fn tool_event_from_obj(obj: &serde_json::Value) -> Option<RunEvent> {
    let typ = obj.get("type")?.as_str()?;
    let name = obj.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let id = obj.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let args = obj.get("args").cloned().unwrap_or(serde_json::Value::Null);
    match typ {
        "tool_call" => Some(RunEvent::ToolCall { name, args, id }),
        "tool_result" => {
            let summary = obj.get("summary").and_then(|x| x.as_str()).unwrap_or("").to_string();
            Some(RunEvent::ToolResult { name, id, summary })
        }
        "tool_confirm_required" => {
            let command_preview = obj.get("command_preview")
                .and_then(|x| x.as_str()).unwrap_or("").to_string();
            Some(RunEvent::ToolConfirmRequired { name, args, id, command_preview })
        }
        _ => None,
    }
}

/// Stream a pattern run from the daemon: yields a `Chunk` per fragment, then a
/// `Done` (or `Error`). For use with `Subscription::run_with`.
pub fn run_stream(
    pattern: String,
    input: String,
    model_id: Option<String>,
) -> impl cosmic::iced::futures::Stream<Item = RunEvent> {
    let mut req = serde_json::json!({
        "op": "run", "stream": true, "pattern": pattern, "input": input
    });
    // A per-run override: `provider/model` (woollama's id) → the daemon's
    // model+vendor override (split on the FIRST '/').
    if let Some((vendor, model)) = model_id.as_deref().and_then(|s| s.split_once('/')) {
        req["vendor"] = vendor.into();
        req["model"] = model.into();
    }
    stream_request(req)
}

/// Stream a chat turn into a session. With no `model_id`, the daemon uses fabric
/// (`raw_query`, history server-side). With a `claude-code`/`claude-agent`
/// `provider/model` id, the daemon routes the session through woollama's stateful
/// `/v1/responses` (the backend owns the transcript).
pub fn chat_stream(
    session: String,
    input: String,
    model_id: Option<String>,
) -> impl cosmic::iced::futures::Stream<Item = RunEvent> {
    let mut req = serde_json::json!({
        "op": "run", "stream": true, "session": session, "input": input
    });
    if let Some((vendor, model)) = model_id.as_deref().and_then(|s| s.split_once('/')) {
        req["vendor"] = vendor.into();
        req["model"] = model.into();
    }
    stream_request(req)
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
            } else if let Some(tool_obj) = v.get("tool") {
                // {"tool": {type, name, args, id, ...}} from the daemon's
                // tool-loop branch — translate into a typed RunEvent.
                if let Some(ev) = tool_event_from_obj(tool_obj) {
                    if output.send(ev).await.is_err() {
                        break;
                    }
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

/// Grab **text** from the clipboard (or primary selection). Requests text MIME
/// types explicitly, so an image-only clipboard yields nothing rather than raw
/// bytes — feeding image bytes into a text widget panics cosmic-text's shaper.
fn paste_text(primary: bool) -> Option<String> {
    for t in ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "TEXT", "STRING"] {
        let mut c = std::process::Command::new("wl-paste");
        c.args(["-n", "-t", t]);
        if primary {
            c.arg("-p");
        }
        if let Ok(o) = c.output() {
            if o.status.success() && !o.stdout.is_empty() {
                return Some(String::from_utf8_lossy(&o.stdout).into_owned());
            }
        }
    }
    None
}

/// The primary selection (highlighted text), falling back to the clipboard —
/// the quick-action's input source. Text only (see `paste_text`).
pub fn selection() -> String {
    paste_text(true).or_else(|| paste_text(false)).unwrap_or_default()
}

/// If the clipboard holds an image, write it to a temp file and return the path
/// (for a vision run). Picks the first `image/*` type offered.
pub fn clipboard_image() -> Option<String> {
    let types = std::process::Command::new("wl-paste").arg("--list-types").output().ok()?;
    let types = String::from_utf8_lossy(&types.stdout);
    let mime = types.lines().map(|l| l.trim()).find(|l| l.starts_with("image/"))?.to_string();
    let ext = mime.rsplit('/').next().unwrap_or("png");
    let out = std::process::Command::new("wl-paste").arg("-t").arg(&mime).output().ok()?;
    if out.stdout.is_empty() {
        return None;
    }
    let path = std::env::temp_dir().join(format!("cosmic-fabric-clip.{ext}"));
    std::fs::write(&path, &out.stdout).ok()?;
    Some(path.to_string_lossy().into_owned())
}

/// Current clipboard **text** (the panel/workspace quick-run input source).
/// Text only — an image clipboard returns "" (see `paste_text`).
pub fn clipboard() -> String {
    paste_text(false).unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn wool(enabled: bool, reachable: bool) -> Status {
        Status {
            woollama: Woollama { enabled, reachable, ..Default::default() },
            ..Default::default()
        }
    }

    #[test]
    fn badge_hidden_when_disabled() {
        assert_eq!(Status::default().woollama_badge(), None);
        assert_eq!(wool(false, true).woollama_badge(), None);
    }

    #[test]
    fn badge_active_when_enabled_and_reachable() {
        assert_eq!(wool(true, true).woollama_badge().as_deref(), Some("\u{25c6} woollama"));
    }

    #[test]
    fn badge_down_when_enabled_but_unreachable() {
        assert_eq!(wool(true, false).woollama_badge().as_deref(), Some("\u{25c7} woollama down"));
    }

    #[test]
    fn backend_pill_reflects_woollama() {
        assert_eq!(wool(true, true).backend_pill(), "\u{25c6} woollama");
        assert_eq!(wool(true, false).backend_pill(), "\u{25c7} woollama down");
        // routing disabled → an explicit "off" chip (no fabric backend to show)
        assert_eq!(Status::default().backend_pill(), "\u{25cb} woollama off");
    }

    #[test]
    fn status_parses_woollama_and_back_compat() {
        let v = serde_json::json!({
            "serve": true,
            "woollama": {"enabled": true, "reachable": true,
                         "endpoint": "unix:/run/woollama.sock", "active_backend": "woollama"}
        });
        let s: Status = serde_json::from_value(v).unwrap();
        assert!(s.woollama.enabled && s.woollama.reachable);
        assert_eq!(s.woollama.active_backend.as_deref(), Some("woollama"));
        // An older daemon omits the field entirely → defaults, no badge.
        let s2: Status = serde_json::from_value(serde_json::json!({"serve": true})).unwrap();
        assert!(!s2.woollama.enabled);
        assert_eq!(s2.woollama_badge(), None);
    }
}

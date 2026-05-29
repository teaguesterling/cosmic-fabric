# Design plan — Decision 1: smart URL origin (YouTube transcript)

Status: **drafted 2026-05-29** · sequenced second per [integration-plan.md](../integration-plan.md).
Settled UX: **make the existing URL origin smart — don't add buttons.** YouTube
URLs auto-ingest their transcript via `fabric -y`; every other URL stays the
generic Jina scrape. This plan turns that into a concrete daemon-only delta with
one open UX fork at the end.

## Scope

When the daemon's `fetch` op gets a YouTube URL, it shells out to `fabric -y`
(transcript) instead of the generic Jina scrape. Detection lives **in the
daemon** (Python `core.fetch_url`), so the Rust panel stays identical — it still
calls `daemon::fetch_url(url, "scrape")` and gets text back. Out of scope:
Spotify, arbitrary `scrape_*` patterns, fabric's `--comments`/`--metadata`
extras for `-y` (transcript only by default; extras can be a later flag), and
any change to the Origin enum or its UI affordance.

## Why detection in the daemon, not the panel

The architectural invariant from `review-and-fabric-integration.md`: "the daemon
owns all fabric integration." `fabric -y` is a CLI shell-out (REST has no
multimodal/scrape fields), and putting hostname detection in the Rust panel
would split the integration across two languages and force a second
update-and-deploy whenever the detection table grows. The daemon already owns
the Jina path; the YouTube path slots in next to it.

## Data delta

### `core.fetch_url` — turn `mode="scrape"` into "scrape, smart"

Today:
```python
def fetch_url(url, mode="scrape", timeout=10):
    if mode == "readability": ...   # naive HTML strip
    # else: Jina r.jina.ai/<url>
```

After:
```python
_SMART_HANDLERS = {
    # hostname (exact, lowercase) → handler(url, timeout) → text
    "youtube.com":     _fetch_youtube,
    "www.youtube.com": _fetch_youtube,
    "m.youtube.com":   _fetch_youtube,
    "youtu.be":        _fetch_youtube,
}

def fetch_url(url, mode="scrape", timeout=10):
    if mode == "readability": ...  # unchanged
    if mode in ("scrape", "smart"):
        h = urllib.parse.urlparse(url).hostname or ""
        handler = _SMART_HANDLERS.get(h.lower())
        if handler:
            return handler(url, timeout=max(timeout, 60))  # transcripts are slow
    # fall through: Jina (unchanged)
    ...

def _fetch_youtube(url, timeout=60):
    """fabric -y URL → transcript on stdout. </dev/null so fabric doesn't
    block on stdin (the gotcha noted in earlier sessions)."""
    r = subprocess.run(["fabric", "-y", url],
                       stdin=subprocess.DEVNULL,
                       capture_output=True, text=True, timeout=timeout)
    if r.returncode != 0:
        raise RuntimeError(f"fabric -y failed: {r.stderr.strip()[:400]}")
    return r.stdout.strip()
```

- **Hostname allow-list, not regex.** Allow-list is the smallest correct
  predicate: no false positives on URLs that merely contain `youtube` in the
  path. Extension is one dict entry per future host.
- **`urllib.parse.urlparse(...).hostname.lower()`** — handles scheme/port/auth
  consistently; safer than string-prefix matching.
- **Per-handler timeout escalation.** Jina is ~5–10s; `fabric -y` can take
  20–60s for a long video. The handler raises the timeout floor without
  bothering callers.
- **`mode="scrape"` keeps its meaning** for callers; we just made it smarter.
  Add `mode="page"` as the explicit escape hatch (forces Jina even for
  YouTube) — see the open question.

### `cosmic-fabric` CLI — expose the escape hatch

```
cosmic-fabric fetch <url>             # smart (today's default)
cosmic-fabric fetch <url> --mode page # force generic Jina scrape
```

Implementation: extend the existing `fetch` branch in `src/cosmic-fabric` to
pass `--mode page` through to the daemon's `fetch` op (which already accepts
`mode`). One line in the CLI, no daemon change beyond accepting the new mode
value (and `core.fetch_url` already needs to recognize `"page"` per the snippet
above).

## What does NOT change

- `Origin` enum in `workspace.rs` — same 5 variants.
- The kit and session surfaces — they don't touch URLs directly; they get the
  result text from `fetch_url`.
- The fabric REST flow — `fabric -y` is a separate process, not a `/chat` call.

## Panel-side toggle (confirmed option b, 2026-05-29)

Original recommendation was (a) CLI-only — the user picked (b) **conditional
loom toggle**: when the URL the user typed/pasted is detected as
YouTube-eligible, a small "Transcript / Page" segmented control appears next to
the URL input (defaults to Transcript). On any other URL, no toggle (no
clutter on non-YouTube URLs). This means **the detection allow-list lives in
both the daemon (decides the actual handler) and the panel (decides whether
to show the toggle)**.

### Rust delta (`workspace.rs`)

```rust
const YOUTUBE_HOSTS: &[&str] =
    &["youtube.com", "www.youtube.com", "m.youtube.com", "youtu.be"];

fn is_youtube_url(s: &str) -> bool {
    url::Url::parse(s).ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
        .map(|h| YOUTUBE_HOSTS.contains(&h.as_str()))
        .unwrap_or(false)
}
```

State on `Workspace`:
```rust
url_force_page: bool,   // false = Transcript (default), true = Page
```

In the URL origin block (around `workspace.rs:1057`):
```rust
if is_youtube_url(&self.url_input) {
    row = row.push(segmented_control(
        ["Transcript", "Page"],
        if self.url_force_page { 1 } else { 0 },
        Message::SetUrlForcePage,
    ));
}
```

The fetch call site (`workspace.rs:321`) passes the chosen mode:
```rust
let mode = if self.url_force_page { "page" } else { "scrape" };  // "scrape" = smart
daemon::fetch_url(url, mode.into())
```

(`url` crate already in the dep tree? — if not, hand-roll the host extract;
five lines, matches the Python `urlparse` path.)

### Allow-list duplication (the cost)

The same set of hostnames exists in `core.py::_SMART_HANDLERS` and
`workspace.rs::YOUTUBE_HOSTS`. The cost is real but bounded (one set per
language, ≤10 entries each). To keep them honest, **the daemon is the source of
truth** — a new daemon op `{op: "smart_url_hosts"}` returns the daemon's host
list, and the panel fetches it on startup and uses *that* for `is_youtube_url`
(falling back to a small built-in list if the daemon call fails). The Rust
constant becomes a fallback, not a parallel source of truth.

```python
# core.py
def smart_url_hosts():
    return sorted(_SMART_HANDLERS.keys())
```

This is one daemon op + one panel-side fetch — small price for keeping the
allow-list maintained in one place while preserving (b)'s discoverability win.

## Failure modes + handling

| failure | behavior |
|---|---|
| no `fabric` on PATH | `_fetch_youtube` raises; daemon's `fetch` op returns `{"error": ...}`; the loom shows the error in its existing error slot — same as a Jina timeout today |
| fabric exits non-zero (private/unavailable video) | stderr → error message visible to the user |
| transcripts disabled on the video | `fabric -y` exits with a clear message; surface it verbatim |
| timeout exceeded (>60s) | `subprocess.run` raises `TimeoutExpired`; daemon's fetch handler catches and reports |

No partial-output handling: transcripts are short enough that streaming would
buy nothing.

## Settled — escape-hatch surface

**Confirmed 2026-05-29:** option (b) **conditional loom toggle** (panel-side
"Transcript / Page" control, visible only on detected YouTube URLs; default
Transcript). The cross-language allow-list duplication is real but bounded
and mitigated by treating the daemon as the source of truth and exposing a
`smart_url_hosts` op so the panel reads the list rather than redeclaring it.
The CLI `--mode page` escape hatch still ships — both surfaces gain the
override.

## Tests

Add to `src/test_core.py`:

- `test_fetch_url_smart_youtube_dispatch`: monkey-patch `_fetch_youtube` to a
  stub; assert `fetch_url("https://www.youtube.com/watch?v=X")` calls the stub,
  and `fetch_url("https://example.com")` does not.
- `test_fetch_url_hostname_case_insensitive`: `https://YouTube.com/...` still
  matches.
- `test_fetch_url_mode_page_forces_jina`: monkey-patch `_fetch_youtube` to a
  stub that fails the test if called; with `mode="page"`, a YouTube URL falls
  through to the (mocked) Jina path.
- `test_fetch_url_rejects_non_http`: unchanged behavior — `ftp://...` still
  raises `ValueError`.

No live `fabric -y` call in tests (network + Ollama-independent: still
external). Live sanity check during deploy: one YouTube URL, one non-YouTube
URL.

## Migration / rollout

- No file-format break (no schema delta).
- No daemon protocol break (`mode` already a field).
- Deploy same as `d12cda9`: `cp src/core.py src/cosmic-fabricd
  ~/.local/share/cosmic-fabric/`, `install -m755 src/cosmic-fabric
  ~/.local/bin/`, restart daemon by PID.

## Definition of done

1. `core.fetch_url` dispatches YouTube hostnames to `_fetch_youtube` (which
   shells `fabric -y` with `</dev/null`, timeout ≥ 60s).
2. `mode="page"` forces the generic Jina path even on YouTube URLs.
3. `cosmic-fabric fetch <url> --mode page` is wired.
4. New unit tests pass.
5. Live: `cosmic-fabric fetch https://www.youtube.com/watch?v=...` returns a
   transcript; the loom URL origin on the same URL feeds the transcript into
   the prompt; `--mode page` returns Jina-style markdown for the same URL.

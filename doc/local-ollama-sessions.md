# Runbook: stateful Local (ollama) chat through woollama

This is the operational setup for routing **Local (ollama) session chat** through
woollama's stateful conversations instead of fabric's server-side sessions. The
*code* shipped in cosmic-fabric (PR #5); this runbook covers the *operational*
pieces that live outside the repo and must be running for the feature to be live.

> **If this setup isn't running, nothing breaks.** Local chat transparently falls
> back to fabric's server-side session (`backend=fabric`). The feature is
> opt-in and degrades cleanly — see [Troubleshooting](#troubleshooting).

## What it does

```
cosmic-fabric panel (Local chat)
        │  daemon: stream_run, session set, vendor=Ollama
        ▼
   cosmic-fabricd ──▶ woollama  POST /v1/responses  key="cosmic-fabric:<session>"
        (attach-by-key)              │  backend = store-backed (conv-7)
                                     ▼
                          conversation store (this runbook)
                          one JSON file per thread; the store owns the transcript
```

woollama routes the conversation *handle*; an **external store owns the bytes**.
woollama never becomes a transcript database. Continuity survives daemon **and**
woollama restarts (the store persists). See `doc/woollama-coordination.md` §2 for
the design rationale.

## Components

1. **A conversation store** — here, woollama's file-backed REST reference store
   (`examples/rest-convstore` in the woollama repo). Persists one JSON file per
   thread; survives restarts.
2. **woollama** wired to that store via `~/.config/woollama/mcp.json`
   (`conversationStore`), restarted to pick it up.
3. **cosmic-fabric** with `[woollama] enabled = true` in `policy.toml` (the panel's
   Local backend then routes ollama sessions through woollama).

## Setup

### 1. Run the store (durable, via systemd --user)

Example units are in [`examples/systemd/`](../examples/systemd/):

```sh
cp examples/systemd/woollama-convstore.service ~/.config/systemd/user/
# Optional — only if you also want woollama itself reboot-durable:
cp examples/systemd/woollama.service           ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now woollama-convstore.service
# systemctl --user enable --now woollama.service
```

Edit the paths in the units if your woollama checkout isn't at `~/Projects/woollama`.

> **Manual alternative** (not reboot-durable):
> ```sh
> CONVSTORE_DIR=~/.local/state/woollama-convstore \
>   ~/Projects/woollama/.venv/bin/python \
>   ~/Projects/woollama/examples/rest-convstore/server.py --port 9000 &
> ```

### 2. Wire woollama to the store

Create `~/.config/woollama/mcp.json`. **woollama replaces (does not merge) its
bundled default**, so carry the default servers forward and add `conversationStore`:

```json
{
  "conversationStore": { "type": "http", "url": "http://127.0.0.1:9000" },
  "mcpServers": {
    "hello":   { "command": "/home/YOU/Projects/woollama/.venv/bin/python",
                 "args": ["${WOOLLAMA_EXAMPLES_DIR}/mcp-hello/server.py"] },
    "textops": { "command": "/home/YOU/Projects/woollama/.venv/bin/python",
                 "args": ["${WOOLLAMA_EXAMPLES_DIR}/mcp-textops/server.py"] }
  }
}
```

> **Pin the interpreter.** The bundled default uses bare `"command": "python"`,
> which resolves to whatever `python` is on `PATH` at spawn time — if that venv
> lacks `fastmcp`, the MCP server fails and woollama **aborts startup** (a failed
> downstream server is fatal). Pinning to woollama's own venv python makes startup
> independent of ambient `PATH`.

### 3. Restart woollama and enable it in cosmic-fabric

`conversationStore` is read once at startup, so woollama must restart:

```sh
systemctl --user restart woollama.service   # or restart however you run it
```

In `~/.config/cosmic-fabric/policy.toml`:

```toml
[woollama]
enabled = true
```

## Verification

```sh
# 1. Store reachable?
curl -s http://127.0.0.1:9000/threads/_probe -o /dev/null -w "%{http_code}\n"   # 200

# 2. woollama wired the store? (in its startup log)
#    "conversation store wired: http http://127.0.0.1:9000 backs non-claude models"

# 3. Two-turn ollama continuity through woollama (same key continues the thread):
SOCK="$XDG_RUNTIME_DIR/woollama.sock"
curl -s --unix-socket "$SOCK" http://x/v1/responses -H 'Content-Type: application/json' \
  -d '{"model":"ollama/qwen3:14b-iq4xs","input":"Remember the codeword TIGER. Reply ACK.","key":"demo"}'
curl -s --unix-socket "$SOCK" http://x/v1/responses -H 'Content-Type: application/json' \
  -d '{"model":"ollama/qwen3:14b-iq4xs","input":"What codeword? Reply with just it.","key":"demo"}'
# second reply should contain TIGER

# 4. The store owns the bytes — a thread file appears:
ls ~/.local/state/woollama-convstore/
```

In the panel: open a **Local** chat, send a turn, then a follow-up that depends on
the first ("now make it shorter"). The daemon log shows
`run-stream ollama session via woollama (attach-by-key)` and the run reports
`backend=woollama`.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| Local chat starts fresh every turn; runs report `backend=fabric` | store not running, or `conversationStore` not wired | `curl :9000/threads/_probe`; check the woollama log for "conversation store wired"; restart woollama after editing `mcp.json` |
| woollama won't start; log shows `No module named 'fastmcp'` / "server 'hello' failed" | `mcp.json` `"command": "python"` resolved a venv without `fastmcp`; a failed MCP server aborts startup | pin `command` to woollama's `.venv/bin/python` (see step 2) |
| woollama `--user` service can't find `claude` / `ollama` | minimal service `PATH` | add their dirs to `Environment=PATH=` in `woollama.service` |
| Cloud (anthropic/openai) chat needs a key | the units are **keyless** by design | out of scope here — keyless protects the project budget; claude-code works keyless via the subscription CLI |

## Caveats

- **Reference store, not production infra.** `rest-convstore` is woollama's
  reference implementation. It's serviceable (file-backed, persists), but for a
  hardened deployment you'd point `conversationStore` at a real store.
- **The woollama Rust port may reshape this.** The conversation-store seam is the
  port's slice 6, so the `conversationStore` config shape (and where the store
  lives) may change. Treat this as proving the path.
- **Keep woollama keyless.** claude-code uses the Claude subscription CLI — do not
  inject `ANTHROPIC_API_KEY` into the long-lived process (project budget).

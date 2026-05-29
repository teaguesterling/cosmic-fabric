# Design plan — Decision 3: session picker in the session surface

Status: **drafted 2026-05-29** · sequenced third per [integration-plan.md](../integration-plan.md).
Settled UX: **list/resume/wipe lives in the session surface (a session dropdown).
Export = the existing per-artifact "Copy Conversation" control.** The loom stays
out of session management; `cosmic-fabric` gets session subcommands for scripting.

## Scope

Add a session dropdown to `SessionApp` (the chat surface) backed by fabric's
existing REST endpoints — `GET /sessions/names` (list) and `GET /sessions/:name`
(structured history) — plus a Wipe affordance for the current session. The
existing **New chat** button stays. Add a small `cosmic-fabric session`
subcommand group (`list`/`resume`/`wipe`) for scripting. No work in the loom; no
new fabric integration beyond REST calls the daemon already knows how to make.

## What fabric actually exposes (probed live)

| op | how | response |
|---|---|---|
| list sessions | `GET /sessions/names` | `["cf-1779933959","cfraw1",…]` |
| read a session | `GET /sessions/:name` | `{"Name":"…","Messages":[{"role":"system"\|"user"\|"assistant","content":"…"},…]}` |
| wipe a session | `fabric --wipesession=<name>` (CLI; REST has no DELETE) | exit 0 / stderr on error |
| resume (write) | `POST /chat` with `sessionName:"<name>"` (already used) | SSE stream as today |

The `system` role is fabric's pattern preamble — the client filters it out for
display, just like a fresh chat does today.

## Daemon delta (`src/cosmic-fabricd` + `src/core.py`)

Three new ops, all thin wrappers over `FAB`:

```python
# core.py
def sessions_list(self):
    with urllib.request.urlopen(f"{self.url}/sessions/names", timeout=5) as r:
        return json.load(r) or []

def session_messages(self, name):
    with urllib.request.urlopen(f"{self.url}/sessions/{name}", timeout=10) as r:
        data = json.load(r)
    # Drop the system preamble; keep user/assistant in order.
    return [m for m in (data.get("Messages") or []) if m.get("role") in ("user","assistant")]

def session_wipe(self, name):
    r = subprocess.run(["fabric", f"--wipesession={name}"],
                       stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=10)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.strip()[:400] or "wipe failed")
```

Daemon ops: `sessions_list`, `session_get`, `session_wipe` — each ≤5 lines in
`handle()`, returning the expected JSON shape.

## Rust panel delta (`session.rs`)

State additions on `SessionApp`:

```rust
sessions: Vec<String>,        // names, refreshed on app start + after Wipe
loading: bool,                // suppresses Send while a Resume is in flight
// existing: session, messages, input, pending, run_seq, streaming, error
```

New messages:

```rust
Message::RefreshSessions          // → daemon::sessions_list
Message::SessionsLoaded(Vec<String>)
Message::Resume(String)           // → daemon::session_get(name) → load messages
Message::Resumed(String, Vec<(Role, String)>)
Message::Wipe                     // → daemon::session_wipe(self.session) then NewSession
```

UI delta in `view()`: the header gains a dropdown between the title and the
"New chat" button, listing `self.sessions` (current session preselected, "New
chat (chat-…)" entry at top for the live unsaved one). Selecting an entry fires
`Resume(name)`; `Resumed` replaces `self.messages` and `self.session`. A trash-
icon button next to it fires `Wipe`.

`Message::Send` is unchanged: it already POSTs with `sessionName = self.session`,
so a resumed session continues server-side history naturally.

## CLI delta (`src/cosmic-fabric`)

A `session` subcommand group, matching the daemon ops:

```
cosmic-fabric session list                # GET /sessions/names → newline list
cosmic-fabric session show <name>         # GET /sessions/:name → role: content lines
cosmic-fabric session wipe <name>         # fabric --wipesession=<name>
```

`resume` isn't a CLI verb — resuming = passing `--session <name>` to a `run`.
Add `--session <name>` to the existing `cosmic-fabric run` flags (one more entry
in `_parse_flags`, one more `req["session"] = ...` line).

## What does NOT change

- The `run` op already accepts `session` (the workspace already uses it for
  conversation continuity). The chat surface already sends turns through
  `chat_stream(session, input)`. The plumbing is already there — we're just
  adding *picking* to the existing *running*.
- Export stays the per-artifact **Copy Conversation** control already built. No
  new exporter, no second source of truth for transcripts.
- The loom does not gain session UI. The session surface owns it.

## Open implementation question — session labels in the dropdown

**Q: how does a session appear in the picker dropdown?**

The raw names are auto-generated timestamps (`cf-1779933959`, `chat-1748…`).
Three options:

- **(a) Raw names** — `chat-1748…` etc. Trivial; ugly; relies on the user to
  remember what each session was about. Cheapest code (one fetch, render
  strings).
- **(b) First user turn as label** — fetch each session, derive label =
  `truncate(first user turn, 60)`. Common chat-UI pattern. Cost: **N round-trips
  on dropdown open** (one `GET /sessions/:name` per session). For fewer than ~20
  sessions on loopback this is fine; beyond that, noticeable.
- **(c) User-renamable** — keep auto names internally, let the user attach a
  display name client-side (in `policy.toml` or a sidecar `~/.config/
  cosmic-fabric/session-labels.toml`). No new fabric op; adds a small edit
  affordance per row in the dropdown.

**Recommendation: (b) first-user-turn label.** It's the common pattern users
already expect from chat surfaces, costs N small REST calls on loopback
(milliseconds aggregate for normal session counts), and **needs no new state
or storage**. The N+1 cost only happens when the dropdown opens, and the
results are cached in `self.sessions_labeled` until the next refresh — single
fetch per session per app lifetime. (a) is hostile-by-default; (c) adds
storage and a rename UI for a problem (b) already solves automatically.

(c) is a low-cost *future* addition — the label cache and the rename map can
coexist (rename overrides derived label) — so picking (b) now doesn't preclude
(c) later.

**Confirm:** option (b) — label = truncated first user turn, derived per-
session at dropdown-open time and cached?

## Tests

Add to `src/test_core.py` (using mocked `urlopen`):

- `test_sessions_list_returns_names`: mock `/sessions/names` → assert list.
- `test_session_messages_drops_system`: mock `/sessions/X` with
  `[system, user, assistant]` → only user+assistant returned, order preserved.
- `test_session_wipe_shells_fabric`: monkey-patch `subprocess.run` to assert
  the right argv; non-zero exit raises.

Rust side: a small `session.rs` unit test for the message-state transitions
(Resumed replaces both `session` and `messages`).

## Migration / rollout

- Existing sessions in fabric's store are picked up automatically by the new
  list endpoint — no data conversion needed.
- The "New chat" timestamp scheme stays; new sessions just become resumable on
  the next launch.
- Deploy same as the prior decisions; the Rust panel needs a rebuild
  (`cd crates && just install`).

## Definition of done

1. Daemon: `sessions_list`, `session_get`, `session_wipe` ops, each handled.
2. `cosmic-fabric session list/show/wipe` subcommands work; `run --session
   <name>` flag works.
3. `SessionApp` has the dropdown + Wipe button; selecting an entry loads its
   history; Wipe wipes the current session then starts a new one.
4. Tests pass.
5. Live: open two sessions across launches, switch between them via the
   dropdown, send a turn in each — server-side history is preserved per
   session; the dropdown shows them with their first-user-turn label.

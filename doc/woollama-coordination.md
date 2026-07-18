# woollama coordination items — from the cosmic-fabric integration

cosmic-fabric routes inference through the **woollama** router (see the
`[woollama]` policy block and `src/core.py:WoollamaClient`). Shipped so far:
slice 1 (stateless plain runs, no woollama changes), the UI wiring (Run-row
model picker, Local|Claude chat toggle), and **attach-by-key stateful chat**
(PR #3). This file tracks the remaining cross-project items.

Target architecture (agreed 2026-06-07): woollama is the inference backbone;
fabric sits *behind* woollama as a pattern source; cosmic-fabricd thins toward a
desktop-session daemon.

> **Status (2026-06-11).** The two items below that were "blocked on woollama"
> are now **unblocked** — woollama shipped both the native-ctx fix and the
> conversation backends. What remains is cosmic-fabric-side wiring (plus, for
> local-model sessions, a one-time woollama *config* step). Re-scoped below.
>
> **Timing caveat:** woollama is mid **Rust port** (Python → `woollama-core`
> rlib + `woollama-py` cdylib wheel + `woollama-server` bin). The conversation-
> store seam is the port's slice 6, so its config shape may shift — coordinate
> before building #2. The public `/v1/...` surface is preserved, so the
> *already-shipped* cosmic-fabric paths are safe. (See the integration memory.)

---

## 1. Context-window control for the `ollama/` provider (`num_ctx`) — ✅ woollama side RESOLVED

**Was:** ollama's OpenAI-compatible `/v1/chat/completions` ignores `num_ctx`, so a
client couldn't size the context window through woollama's passthrough — long
inputs silently truncated at ollama's default 4096.

**Resolved (woollama):** woollama honors `num_ctx` for the `ollama` provider by
translating to ollama's **native** `/api/chat` (`ollama_native`, now under
`woollama/core/`), including in stateful turns.

**Remaining (cosmic-fabric, small):** the woollama path doesn't yet *send* a
context hint. `core.to_openai_options` forwards sampling knobs but **not**
`core.pick_ctx()`'s sizing (it still returns `modelContextLength` only for the
fabric/Ollama run). To match the fabric path, forward the per-input ctx tier
(2048/8192/16384/32768) as `options.num_ctx` (or woollama's accepted field) on the
woollama chat/respond calls.
**Acceptance:** a woollama run asking for num_ctx=16384 shows `ollama ps`
CONTEXT 16384 for that run.

---

## 2. Stateful sessions for local (ollama) models — ✅ DONE (code); operational setup is a runbook

**Was:** woollama had no state-owning backend for ollama models (the duckdb
`stored` backend was shipped 2026-06-05 and reverted 2026-06-06 — "woollama must
never be the store"), so local-model session chat couldn't route through
`/v1/responses`; it stayed on fabric's server-side `sessionName`.

**Resolved (woollama)** — backends added without woollama becoming the store:
- **conv-6 managed-agents** (`managed_agents.py`): Anthropic-hosted state.
- **conv-7 store-backed** (`conversations.py` + `Mcp`/`HttpStoreProvider`): a
  **BYO external store** owns the transcript. Once a `conversationStore` is
  configured, EVERY non-claude model (ollama, cloud, recipes) becomes stateful on
  `/v1/responses` + `/v1/conversations`. The principle holds: woollama routes
  conversation *handles*; the external store owns the *bytes*.

**Done (cosmic-fabric, PR #5):** `cosmic-fabricd:stream_run` routes `Local`/ollama
sessions through woollama via `wool.respond(..., key="cosmic-fabric:{session}")`
(attach-by-key), with a clean fallback to fabric's server-side session when
woollama is down or has no store wired (HTTP 501). `core.WoollamaClient.respond()`
now raises on non-2xx so that fallback can fire. No new daemon state.

**Done (cosmic-fabric, 2026-07-18) — the conversations surface consumed** (the
discovery/read/teardown half; turns were already attach-by-key above):
- `WoollamaClient` grew `conversations(key_prefix=…)` (`GET /v1/conversations`,
  filtered to our `cosmic-fabric:` namespace via the echoed `key`),
  `conversation_items(id)` (`GET …/items` → `(role, text)` pairs — the
  resume-on-open read), and `delete_conversation(id)`.
- Daemon ops `sessions_list` / `session_transcript` / `session_delete`: the panel
  drives everything by session NAME; conversation ids never cross the daemon
  boundary. A transcript read is list+filter-by-key, **never** an attach-POST (a
  read must not create). Unknown session ⇒ empty transcript, not an error.
- Gated integration coverage (`test_integration.RealWoollama`): the full journey
  live against a real `woollamad` + store — attach-by-key create → server-side
  recall on turn 2 → discovery (key echoed) → transcript → delete — plus the
  daemon ops end-to-end. Skips cleanly when no store answers.

Remaining (UI, next slice): the panel session picker / resume-on-open in
`crates/cosmic-fabric-panel/src/session.rs` consuming `sessions_list` +
`session_transcript`.

**Operational setup (a runbook, not code) — see
[`local-ollama-sessions.md`](local-ollama-sessions.md):** the feature is live only
while a `conversationStore` is wired in woollama's `mcp.json` and a store is
running (reference: `examples/rest-convstore`; example systemd `--user` units in
[`examples/systemd/`](../examples/systemd/)). Until then, Local sessions
transparently use fabric (`backend=fabric`) — no regression. *The woollama Rust
port's slice 6 (conversation-store seam) may reshape the store config — treat the
reference store as proving the path, not production infra.*

---

## Already resolved (no further woollama work)

- **Attach-by-key** (2026-06-11, PR #3): the daemon no longer holds a
  session→conv map (`_WOOLLAMA_CONVS` removed); it drives turns by
  `key="cosmic-fabric:{session}"` and woollama owns the durable, cross-client
  key→conversation map. Continuity survives daemon **and** woollama restarts.
  *Requires woollama ≥ commit `5e79769`; an older process silently ignores `key`
  and restarts the conversation every turn.*
- **Sampling knobs** (temperature, top_p, frequency/presence penalty): OpenAI
  equivalents, forwarded by `core.to_openai_options`.
- **Thinking/`<think>` blocks** don't leak: ollama's OpenAI endpoint isolates
  reasoning in a separate `reasoning` field; `content` is clean.
- **Transport** uses woollama's owner-only Unix socket
  (`$XDG_RUNTIME_DIR/woollama.sock`, 0600), falling back to the loopback `.addr`.

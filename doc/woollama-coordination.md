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

## 2. Stateful sessions for local (ollama) models — woollama backends SHIPPED; cosmic-fabric + one config step remain

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

**Remaining — the entry point for a future session:**
1. **woollama config (one-time):** wire a `conversationStore` provider in woollama's
   `mcp.json` (an MCP store server, or an HTTP file-store URL — both reference
   providers exist). Until wired, `backend_for_model("ollama/…")` is still `None`
   (stateless). *Decide which store first* (reference MCP store vs. a real one).
2. **cosmic-fabric (small, thanks to attach-by-key):** broaden the stateful guard
   in `cosmic-fabricd:stream_run` — currently
   `vendor in ("claude-code", "claude-agent")` — to also include `Local`/ollama
   sessions, routing them through the **same**
   `wool.respond(..., key="cosmic-fabric:{session}")` path. No new daemon state;
   attach-by-key already gives durable, namespaced continuity. The `session.rs`
   `Local` backend's chat then becomes stateful instead of one-shot.

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

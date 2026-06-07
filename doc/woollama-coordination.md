# woollama coordination items — from the cosmic-fabric integration

cosmic-fabric now routes plain inference through the **woollama** router (see the
`[woollama]` policy block and `src/core.py:WoollamaClient`). Slice 1 (stateless
plain runs) needed **no** woollama changes. The next two integration steps are
blocked on the woollama side — both verified from cosmic-fabric, written up here
so the woollama project can pick them up. **These are woollama work, not ours.**

Target architecture (agreed 2026-06-07): woollama is the inference backbone;
fabric sits *behind* woollama as a pattern source; cosmic-fabricd thins toward a
desktop-session daemon.

---

## 1. Context-window control for the `ollama/` provider (`num_ctx`)

**Problem.** ollama's OpenAI-compatible endpoint (`POST /v1/chat/completions`)
**ignores `num_ctx`**, including when passed as `{"options": {"num_ctx": N}}` in
the body. woollama's `ollama/<model>` passthrough forwards the client body
verbatim to that endpoint, so a client (cosmic-fabric) **cannot** size the
context window through woollama. Long inputs silently load at ollama's default
context and truncate.

**Verified (2026-06-07, ollama on localhost:11434, qwen3:14b-iq4xs):**
- Unload, then `POST /v1/chat/completions` with `options.num_ctx=16384` →
  `ollama ps` shows **CONTEXT 4096** (default; ignored).
- ollama only honors `num_ctx` on its **native** `POST /api/chat` (and
  `/api/generate`) via `options.num_ctx`.

**What cosmic-fabric does today (fabric path):** sends `modelContextLength`, and
`core.pick_ctx()` right-sizes per input (2048 / 8192 / 16384 / 32768 tiers).
The woollama path currently drops this.

**Asked of woollama:** for the `ollama` provider, translate an OpenAI request
into ollama's **native** `/api/chat` when a context size is requested (or always),
mapping a `num_ctx`/context field → `options.num_ctx`. Equivalently, accept a
context hint on the passthrough and route it natively. Other providers are
unaffected (no `num_ctx` concept).

**Acceptance:** a chat-completions request through woollama that asks for
num_ctx=16384 results in `ollama ps` showing CONTEXT 16384 for that run.

---

## 2. A state-owning conversation backend for non-claude (ollama/recipe) models

**Problem.** woollama deliberately has **no state-owning backend for ollama
models** — the duckdb `stored` backend was shipped 2026-06-05 and **reverted
2026-06-06** ("woollama must never be the store"). At HEAD only
`ClaudeResumeBackend` is stateful; `backend_for_model("ollama/…")` → `None` →
stateless (the caller owns history). So cosmic-fabric's session chat for local
models cannot route through woollama statefully — it stays on fabric's
server-side `sessionName` for now.

**The principle (woollama's, respected here):** woollama routes conversation
*handles*; backends own the *state*. The reverted duckdb store violated that.

**Asked of woollama (design-level; woollama's call):** a backend that owns (or
defers to an external owner of) conversation state for non-claude models, so
`/v1/responses` + `/v1/conversations` work for `ollama/<model>`. Per
`docs/conversations-api-design.md §8`, the leading candidate is **Managed
Agents** (Anthropic owns the session); another option is a state owner that
isn't woollama's own embedded DB.

**cosmic-fabric side, once a backend exists:** map a cosmic session name → a
woollama `conversation_id`, send turns via `/v1/responses` with
`store:true`/`conversation`, and route the streaming chat path (currently guarded
to fabric in `cosmic-fabricd:stream_run` when `session` is set) through woollama.

---

## Already resolved (no woollama work needed)

- **Sampling knobs** (temperature, top_p, frequency/presence penalty) have OpenAI
  equivalents and route through fine — `core.to_openai_options` forwards them.
- **Thinking/`<think>` blocks** don't leak: ollama's OpenAI endpoint puts
  reasoning in a separate `reasoning` field; `content` is clean, matching fabric's
  stripped output.
- **Transport** uses woollama's owner-only Unix socket
  (`$XDG_RUNTIME_DIR/woollama.sock`, 0600), falling back to the loopback `.addr`.

# Code review + fabric integration study (2026-05-27)

Done at the "we want this to be *the* fabric frontend" checkpoint. Two parts:
a concrete code-review pass over what's built, and a study of fabric's real
surface to find where the integration is shallow.

## The framing question (decide first)

fabric already has a CLI, an Obsidian plugin, a Raycast extension, a web UI.
"*The* fabric frontend" is broad and partly already taken. The `scribe-*` pattern
pack + the usage so far point at a sharper product:

- **(A) A writing/thinking workbench powered by fabric** — curated patterns (with
  discovery), multi-turn editing via sessions, model-tier comparison, paste-an-
  image-then-critique. "The best fabric-powered writing workbench on Linux."
- **(B) A generic fabric GUI** — breadth: every pattern/context/strategy/vendor
  exposed, transcription as a feature, a faithful GUI over the whole CLI.

These are different products; the roadmap below is annotated for which is which.
The evidence leans (A). **This is a user decision, not settled here.**

## What fabric actually exposes (and what we use)

REST (`fabric --serve`, verified live on the box):

| capability | endpoint / field | we use it? |
|---|---|---|
| patterns | `GET /patterns/names`, `/patterns/:name` | ✅ (but see below) |
| models | `GET /models/names` | partial (status only) |
| vendors | ~30 (`--listvendors`): Ollama, Anthropic, OpenAI, Gemini, Groq, Bedrock… | ❌ no picker |
| run | `POST /chat` (SSE) | ✅ run + assemble |
| **contexts** | `contextName` in `/chat`; `GET /contexts/names` | ❌ |
| **sessions** (multi-turn) | `sessionName` in `/chat`; `GET /sessions/names` | ❌ |
| **strategies** (CoT etc.) | `strategyName` in `/chat`; `/strategies` | ❌ (need `fabric --setup`) |
| **language** | `language` in `/chat` | ❌ |
| **modelContextLength** | `/chat` field — request a ctx window | ❌ ← fixes URL-spill |
| ChatOptions | `temperature`, `topP`, penalties, `thinking`, **`search`** | partial (thinking via policy) |

`POST /chat` request shape (authoritative, from upstream `internal/server/chat.go`):
`ChatRequest{ prompts:[ PromptRequest{ userInput, vendor, model, contextName,
patternName, strategyName, sessionName, variables } ], language,
modelContextLength, ...ChatOptions{ temperature, topP, frequencyPenalty,
presencePenalty, thinking, search, searchLocation } }`. SSE events:
`{type: content|usage|error|complete, format, content}`.

**Multimodal is CLI-only — NOT in REST.** No attachment/url/scrape/youtube/
transcribe fields on `/chat`. So:
- Our **daemon-does-Jina-fetch for URL was the right call** (REST can't scrape).
- youtube (`-y`), audio (`--transcribe-file`), image attachment (`-a`), TTS, image
  gen → the **daemon shells out to the `fabric` CLI** (or fetches itself). They
  do not become REST calls. *(Confirms the "daemon owns integration" architecture.)*

### Pattern reality
- **265 patterns exist; we expose 11** (`scribe-*` filter).
- **`Description` is empty for every pattern** (scribe and non-scribe alike) — so
  discovery cannot use subtitles. It needs **name search + favorites + recents**
  (and, for (A), a curated set), not the "verb + description" rows in the mockup.

## Code review (concrete)

| # | where | issue | fix |
|---|---|---|---|
| 1 | `daemon.rs:20` `call()` | no timeout on connect/read — a hung daemon hangs the UI Task forever (silent, not just slow) | wrap in `tokio::time::timeout` (e.g. 20s run, 5s status); map elapsed → `Err` |
| 2 | `settings.rs:235`, `workspace.rs:206`, `window.rs:291` | `starts_with("scribe-")` hardcoded in 3 places — bakes the scribe pack into the app | centralize: a `patterns(filter)` notion; for (A) a curated/favorites list in config; for (B) all patterns + discovery |
| 3 | `workspace.rs` (×10 `self.error = Some`) | single `error: Option<String>`; each setter clobbers — a fetch error then a run error loses the first | acceptable for now; if it bites, a small Vec or toast queue |
| 4 | `workspace.rs:217` | `matches!(action, Action::Edit(_))` for debounce — correct (only `Edit` mutates text); just noting it's intentional | none |
| 5 | `workspace.rs` Claude dest | "Claude Desktop" == Copy + nudge → **glorified Copy**. Conflates *destination* (sink) with *model* (run-with-claude). Two menu items that do the same thing read as broken | the real "use Claude" is a **model/vendor picker** (run with `claude-*`), not a sink. Fold the dest, add a model picker |
| 6 | daemon `fetch` | synchronous on the per-connection thread (`ThreadingUnixStreamServer`) | fine — isolated; 10s timeout already |

## Opportunities, ranked (no-regret → identity-dependent)

1. **`modelContextLength` through the `run` op** — *no-regret, do next.* Size from
   input length (+ margin); directly fixes the qwen3 URL-spill (81% GPU). Pure
   daemon change, no UI risk. Pairs with the existing context-tier-sizing note.
2. **`call()` timeout** (#1 above) — *no-regret.* Small, removes a hang class.
3. **Model/vendor picker in the workspace** — high value either way; `/chat` takes
   per-prompt `model`+`vendor`; 30 vendors available. Subsumes the Claude stub.
4. **Pattern discovery** (search + favorites + recents) — required before
   un-hiding patterns. (A): curated + search. (B): all 265 + search.
5. **Sessions = multi-turn.** Turns the static "conversation" into a real chat.
   **A data-model shift, not a param:** today the workspace is stateless-per-run
   (Clear wipes all). Sessions need persistent conversation state — commit to
   *where* it lives (per-pattern? global? switchable) before building.
6. **Contexts** — reusable background prepended to prompts. Cheap once sessions'
   state model exists.
7. **Strategies** — run `fabric --setup` to fetch them, *look at what arrives*,
   then decide whether to expose. Don't design UI blind.
8. **Multimodal via CLI shell-out** — youtube/transcribe/attachment in the daemon;
   (A) gates audio to "your voice notes," (B) treats it as a feature.

## Architectural discipline (restate)

The **daemon owns all fabric integration** (the "one channel"). As we add CLI
shell-out for multimodal, session/context CRUD, strategy listing, etc., they all
go in the **Python daemon**; the Rust panel stays a thin socket client. Don't let
fabric calls leak into the Rust side.

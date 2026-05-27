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

## Product model — three surfaces (settled with the user, 2026-05-27)

> **goo is out of scope for now** — an aspiration. Near-term handoffs go via
> clipboard / a staging layer, not a goo route.

One daemon (the one channel) + one **profile** (active-set config) feed three
distinct UI surfaces:

1. **Workbench — "the loom"** *(seed: `cosmic-fabric-panel window`)*. The power +
   config surface: full access to fabric's whole surface (all patterns, vendors,
   contexts, strategies, multimodal), **not** optimized for speed. Its defining
   job is to **configure the other two** — curate the active-set, set per-pattern
   model/vendor/variables.
2. **One-offs — "the kit"** *(launcher + panel, built)*. The COSMIC tie-ins:
   quick, context-aware access to your common/active features. Flow:
   **select → inference → review → close.** Future: context-menu, selection
   actions, widgets. All read the profile the loom configures.
3. **Session** *(new)*. A lightweight IM-style chat dialog for CoT / multi-step,
   backed by fabric **sessions** (`sessionName`). Lighter than Alpaca, not the
   loom. Opened from the loom **or escalated from a one-off** ("turn this result
   into a conversation") — the kit→session escalation is the connective tissue.

Consequence: the hardcoded `scribe-` filter isn't a wart — it's a stand-in for a
**configurable active-set** the loom edits and the kit reads. "The current
everything is too much" → curation is the spine. (Sessions are **not** dropped —
they're surface 3, just lightweight and separate from the loom.)

## The spine: a personalization profile (active-set) in `policy.toml`

One source of truth the daemon serves and every surface reads:

```toml
[surface]
active = ["scribe-summarize", "scribe-explain", …]  # curated working set
#   surfaces read this; absent → fall back to scribe-* (back-comat)

[patterns.scribe-summarize]
model = "…"; vendor = "…"          # model/provider selection (config-time)
favorite = true
variables = { depth = "medium" }   # default {{vars}}
# (later) surfaces = ["launcher","panel"], strategy = "...", context = "..."
```

- New daemon op `{"op":"surface"}` → `{active:[…], patterns:{name:{…}}}`.
- The three hardcoded `starts_with("scribe-")` sites (settings/workspace/window)
  all switch to reading `surface.active`. Build curation once; every surface —
  and future context-menu/widget/selection surfaces — benefits.

## Proposed next slice: **Pattern Library + per-pattern config** (the Workbench's reason to exist)

Merges the user's two leanings (pattern discovery/selection/configuration **and**
model/provider selection) into one coherent slice, and directly attacks "too much":

1. **Library** — browse/search all 265 patterns by name (descriptions are empty;
   name search + maybe folder/tag from the pattern dir). Toggle each into the
   **active set** (★).
2. **Per-pattern config** — for active patterns: **model + vendor** picker (the 30
   vendors), default variables. Writes `[patterns.X]` + `[surface].active`.
3. **Surfaces read the profile** — replace the 3 scribe- literals with the
   `surface` op. The popup/launcher now show *your* curated set, configured.

Workbench stays small (config + the existing run/inspect console); not Alpaca.
Per-run model override and the quick OS surfaces (context menu / selection /
widget) come after, riding the same profile.

**Open question for the slice:** does the active-set live entirely in
`policy.toml` (simple, one file) or split (curation vs model-policy)? Lean: one
file, `[surface]` + `[patterns.*]` — the daemon already owns it.

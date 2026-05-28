# Fabric on the COSMIC desktop — feature map & vision

What cosmic-fabric is: a COSMIC-native frontend bundle over a single local
**fabric** deployment. This doc maps **fabric's whole surface** to **desktop
usefulness**, **how we envision each piece working**, and **what's in place**.
It's both the vision and the roadmap.

> Companion docs: `design.md` (architecture), `review-and-fabric-integration.md`
> (code review + product model), `panel-mockup.html` (UI mockups),
> `cosmic-goo-integration.md` (goo routing — *aspiration, out of scope for now*).

## The shape (recap)

One daemon (`cosmic-fabricd`) owns the fabric deployment — the **one channel**;
everything else is a thin socket client. One **profile** (`policy.toml`) is the
shared config every surface reads. Three UI surfaces:

- **Loom** 🧵 — the Workbench (`cosmic-fabric-panel window`): the power + config
  surface. Run/inspect, **Library** (curate which patterns are yours), **Models**
  (define model instantiations). *Not optimized for speed; it configures the rest.*
- **Kit** 🧰 — the quick OS tie-ins: launcher (Super → type), panel popup, and the
  **quick-action** (a global hotkey on the selection). **select → inference →
  review → close.** Reads the curated set the loom defines.
- **Session** 💬 — a lightweight chat dialog (multi-turn, fabric sessions).

Legend: ✅ built · 🔶 partial · ⬜ planned · 🔬 needs on-box model prototyping.

## Feature catalog

### 1 · Patterns — the verbs

| fabric | desktop usefulness | how we envision it | status |
|---|---|---|---|
| patterns (265 of them) — a system prompt that transforms input | the core action: summarize/explain/critique/extract/… on whatever you're looking at | run from any surface; you **curate** a working set (the rest stay searchable) | ✅ run everywhere |
| pattern **variables** (`{{lang}}`, `{{depth}}`) | adverbs — "summarize **deeply**", "translate to **fr**" | per-pattern default variables in the profile; per-run override later | 🔶 daemon passes them; UI editing ⬜ |
| `--input-has-vars` | templated inputs | niche; expose only if needed | ⬜ |

Curation: which patterns are "yours" is **include/exclude globs** over names
(`scribe-*` is just our pack — the code is pack-agnostic). ✅

### 2 · Models & vendors — where/how it runs

| fabric | desktop usefulness | how we envision it | status |
|---|---|---|---|
| ~30 **vendors** (Ollama, Anthropic, OpenAI, Gemini, Groq, …) | local for cheap/private, cloud for hard tasks | only configured vendors surface (what you can actually run) | ✅ catalog from `/models/names` |
| per-pattern **model/vendor** | the right tool per verb (local summarize, cloud visualize) | assign each pattern a **named instantiation** | ✅ Models editor |
| **deployment params** (ctx, thinking, temperature) | right-size the KV cache; toggle reasoning; tune creativity | **model → variants** (`qwen3/fast` @2048, `qwen3/deep` @16384), default variant | ✅ two-level instantiations + editor |
| **`modelContextLength`** | stop short inputs over-allocating; let long inputs (web pages) grow | auto-size by input (`pick_ctx`) when a variant has no explicit ctx | ✅ |
| **capabilities** (text/vision/…) | a vision pattern *can't* run on a text-only model — a hard constraint | tag instantiations; a **capability rule** auto-picks a capable one | 🔶 stored; rule ⬜ 🔬 |
| ChatOptions **`search`** (model-side web search) | "answer using the web" without scraping | a per-run/instantiation toggle | ⬜ |

### 3 · Inputs / sources — what you act on

| fabric | desktop usefulness | how we envision it | status |
|---|---|---|---|
| text / clipboard | the everyday case | clipboard is the kit's default; editable in the loom | ✅ |
| **primary selection** | act on highlighted text with zero copy-paste | the **quick-action** hotkey + the launcher | ✅ |
| file | run a pattern over a document | path field now; native picker later | 🔶 path-paste; picker ⬜ |
| **URL / web** (`--scrape_url` Jina, `--readability`) | "summarize this page" from a link | daemon fetches → markdown → feeds the prompt (keyless Jina, no REST dep) | ✅ |
| **YouTube** (`-y` transcript/comments) | "summarize this talk" | a URL-source variant (transcript → prompt) | ⬜ |
| **audio** (`--transcribe-file`, STT) | voice notes; "what did this meeting cover" | source → transcribe → prompt; front half of the voice loop | ⬜ 🔬 |
| **image** (`-a` attachment, vision) | "describe / extract text from this image" | image source; **requires a vision model** (capability rule) | ⬜ 🔬 |
| Spotify (`--spotify`) | podcast metadata | niche; later | ⬜ |

The source is **polymorphic**: the input pane adapts per origin (URL gets a
fetch + scrape/readability toggle; image gets a thumbnail), and non-text origins
show the **transform** that feeds the prompt (`→ N chars markdown`). Mocked in
`panel-mockup.html`.

### 4 · Outputs / destinations — where the result goes

| fabric / desktop | usefulness | how we envision it | status |
|---|---|---|---|
| clipboard | the universal backstop | always copied; the default send-to | ✅ |
| save to file | keep the artifact | per-artifact "Save…" | ✅ |
| the panel | review a result without a window | launcher → broadcast → panel pane | ✅ broker |
| **hand off to an agent** (Claude Desktop/Code) | assemble a prompt, continue in a full agent | render prompt (no run) → stage → open the agent | 🔶 clipboard stub; real route = goo (deferred) |
| **TTS** (`--list-gemini-voices`) | read the result aloud; back half of the voice loop | a "speak" destination | ⬜ 🔬 |
| **image generation** (`--image-file`) | "make a diagram/illustration" | image-gen pattern → image result (polymorphic response) | ⬜ 🔬 |

Send-to is a **customizable destination registry** (Copy default + Save now;
Claude/Alpaca shown disabled until a real route exists). ✅ per-artifact dropdowns.

### 5 · Conversation — sessions

| fabric | desktop usefulness | how we envision it | status |
|---|---|---|---|
| **sessions** (`sessionName`, server-side history) | multi-turn / chain-of-thought without rebuilding context | the **Session** surface — a light chat dialog; **escalate from a one-off** | ✅ surface; escalate ⬜ |
| session list (`--listsessions`) | resume a past conversation | a session picker | ⬜ |

### 6 · Context & strategy — advanced shaping

| fabric | desktop usefulness | how we envision it | status |
|---|---|---|---|
| **contexts** (`contextName`) | reusable background prepended to prompts ("my writing style", "this project") | named contexts in the loom; attach per-pattern/session | ⬜ |
| **strategies** (`strategyName`, CoT etc.) | prompt-engineering strategies layered on a pattern | pick a strategy per run; needs `fabric --setup` to fetch them | ⬜ (look after setup) |
| **extensions** (`--addextension`) | custom tools fabric can call | surface registered extensions; advanced | ⬜ |

### 7 · Classification & curation — making it legible

| concept | usefulness | status |
|---|---|---|
| **active-set** (include/exclude globs) | your working subset of 265 patterns drives the quick surfaces | ✅ |
| **categories** (on models *and* variants) | group/reason about your setup (local/cloud/fast/deep) | ✅ stored + editable; filtering UI ⬜ |
| **usage index** (who uses each model) | see where each instantiation is applied | ✅ Models view |

## Surfaces — how features land

**Loom 🧵** (`window`): Run (source → pattern → auto-assembled prompt → run →
response → send-to) · **Library** (search all 265, ★ curate, route each to an
instantiation) · **Models** (define models + variants, categories, defaults,
usage). The home for everything in §2, §6, §7 and the polymorphic source/response
of §3/§4.

**Kit 🧰**: launcher (pop-launcher, type to filter) · panel popup (status +
quick-run on clipboard + Open workspace/Chat) · **quick-action** (`quick`,
`Super+Shift+F`: grid of active patterns on the selection, inline review). The
fast path for §1 on §3's text sources. *Cross-app context menus are not possible
on Wayland — the hotkey+selection path is the feasible equivalent.*

**Session 💬** (`session`): a chat dialog over §5.

## Roadmap — by value × feasibility

1. **Live shakedown** of what's built (no new code; needs a monitor). 🔶
2. **Escalate-from-one-off** → open a session seeded with a result. Small.
3. **Capability rule + image source** (§2 capabilities + §3 image) — the rule that
   justifies instantiations; needs a vision model prototyped. 🔬
4. **Audio in / TTS out** (§3 audio + §4 TTS) — the voice loop; fabric does STT
   (`--transcribe-file`) + TTS natively. 🔬
5. **Contexts & strategies** (§6) — reusable background + CoT strategies.
6. **goo routing** (real agent hand-off, §4) — when goo's route layer exists.

## Mockups

`panel-mockup.html` covers the popup + workspace + polymorphic source/response +
Models + send-to. Inline sketches of the new surfaces:

```
quick-action (Super+Shift+F)            session (chat)              Models (loom)
┌─ Run on selection ──────┐   ┌─ Fabric chat · chat-1730 ─┐   ┌ Models ─────────────┐
│ "Volcanoes form when…"  │   │ You: explain monads        │   │ Default model [qwen3/fast▾]
│ ───────────────────────  │   │ Fabric: A monad is a…      │   │ [new name][Ollama▾][model▾][Add]
│  Summarize              │   │ You: simpler?              │   │ ╭ qwen3  qwen3:14b·Ollama [Edit][✕]
│  Explain                │   │ Fabric: Think of a box…    │   │ │ caps[text] cats[local]
│  Critique               │   │                            │   │ │ used by: default
│  Draft response         │   │ ┌────────────────────────┐ │   │ │ ★ fast: ctx 2048, think off
│ ───────────────────────  │   │ │ Message…        [Send] │ │   │ │   deep: ctx 16384  ← scribe-think
│              [Close]    │   │ └────────────────────────┘ │   │ ╰ [Categories: local,…][+variant]
└─────────────────────────┘   └────────────────────────────┘   └─────────────────────┘
```

**Mockups worth building next** (HTML, no monitor needed): the **polymorphic
source** states (URL fetch, image+vision-required, audio); the **image-gen
response**; a **contexts/strategies** config panel. (Several already exist in
`panel-mockup.html`.)

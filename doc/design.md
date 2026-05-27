# cosmic-fabric — design

**One sentence:** a COSMIC-native, *widget-level* frontend bundle over a **single
local fabric deployment** — standalone from goo (a sibling that talks fabric's
REST) — where the components don't just *run* patterns, they keep one deployment's
**configuration** coherent across surfaces.

## The shared substrate (what "a single fabric deployment" means)

The center of gravity is one config trio every component agrees on:

- one `fabric --serve` instance (the engine),
- `~/.config/fabric/.env` (vendors: Ollama URL, Anthropic key) + the `scribe-*` pattern pack,
- **`~/.config/cosmic-fabric/policy.toml`** — the per-pattern **model/vendor** map
  (the relocated `scribe.pack.toml` manifest; its real owner).

## The four components — daemon + three thin frontends

| component | surface | job | fabric REST |
|---|---|---|---|
| `cosmic-fabric-daemon` | — | owns fabric lifecycle (spawn/health) + holds `policy.toml`; *optionally* a policy-injecting proxy | `GET /config`, proxy `POST /chat` |
| `cosmic-fabric-launcher` | pop-launcher plugin (hotkey) | pick pattern → run on selection → result | `GET /patterns/names`, `POST /chat` |
| `cosmic-fabric-panel` | COSMIC panel applet | status, quick-run, recent | ping `/config`, `POST /chat` |
| `cosmic-fabric-settings` | libcosmic window | edit patterns, model mappings, vendors | `GET/POST /patterns/:name`, `GET /models/names` |

REST routes verified live: `/patterns/names`, `/patterns/:name`, `/chat` (run+stream),
`/models/names`, `/config`.

## Output-by-length rule (the launcher constraint, per surface)

Launchers can't render paragraphs, so route output by length:

- **launcher:** short → inline result row; long → `wl-copy` + "✓ copied (N chars)".
- **panel:** small scrolling popover (medium).
- **settings:** full output (it's a window).

## Phasing

- **Phase 0 (prototype):** launcher only. A script that reads `policy.toml`, captures
  `wl-paste -p`, runs the pattern, applies the output rule. **No daemon** — config
  file + the `fabric` CLI directly (not REST: avoids requiring a running `--serve`
  and reverse-engineering `/chat`). Proves `selection → fabric → result`.
- **Phase 1:** factor the shared bits (policy loader, fabric client, output rule) into
  a lib; switch to REST `/chat` (streaming); the launcher becomes a thin client. Seed
  of the daemon.
- **Phase 2:** stand up `cosmic-fabric-daemon` (lifecycle + policy, optional proxy);
  add `-panel` and `-settings` as daemon clients.

## Layout

Standalone repo, eventually a Cargo workspace (`crates/{launcher,panel,settings,daemon,lib}`).
The Phase-0 launcher lives in `prototype/` as a Python script (pop-launcher plugins are
language-agnostic), rewritten to a Rust crate at Phase 1.

## Future: context-tier sizing (Phase-2, daemon)

Right-size the KV cache per request: keep 2–4 variants of the local model baked at
different `num_ctx` (e.g. 2048 / 8192 / 16384), and pick the smallest tier whose
context fits `input + generation + margin`. Short inputs → small tier (100% GPU,
fast, max app headroom); long docs → big tier (fits, but may spill to CPU — accepted
over truncation).

Realities that shape it:
- **q8 KV moved the spill point out** — at q8, footprint ≈ 7.9 GiB @2048 → ~8.4 GiB
  @8192 (likely still 100% GPU) → ~9.1 GiB @16384 (spills). So the *default* tier can
  probably grow to ~8192; the ladder mainly matters at the extremes.
- **One tier fits at a time** on 11 GB (two ~8 GiB tiers don't co-fit, even with
  `OLLAMA_MAX_LOADED_MODELS=2`), so a tier switch = a full model reload (~seconds +
  GPU). Keep tiers coarse and tune keep-alive to avoid thrash.
- Variants share the weight blob (only the `num_ctx` param differs) → disk-cheap.
- Selection: token-count via `len/4` or `ollama /api/tokenize`.
- The daemon owns residency/lifecycle → natural home; the launcher's placement check
  (`/api/ps`) already measures each tier's real GPU%, so the daemon can self-calibrate.

Near-term cheap experiment (no ladder yet): bake/test `iq4xs` at `num_ctx 8192` with
q8 KV — if it still loads 100% GPU, just raise the single default and most inputs are
covered without a ladder.

## Launcher → panel result handoff (built; panel auto-open pending live check)

The launcher dispatches a run and the **panel** shows the streaming result
(retiring the launcher's `notify-send --wait` hang); the launcher's
clipboard/notify path stays as the standalone fallback. This is also the
**cosmic-goo-ready shared channel** — goo's fabric route would `broadcast` the
same way.

- **Daemon broker:** `{"op":"subscribe"}` holds a connection; a run with
  `"broadcast":true` streams its `{"event":start|chunk|done}` to all
  subscribers, while the requester gets `{"dispatched":true,"subscribers":n}`.
  If `n==0` it returns `dispatched:false` (no run) so the caller falls back.
- **Panel:** an always-on `daemon::subscribe()` Subscription → `BrokerEvent`;
  `Start` auto-opens the popup (`get_popup`), chunks stream into the result.
- **Launcher:** `[output] mode = "panel"` (opt-in) → broadcast, else normal
  run + local deliver (no regression, no lost results).

Socket-tested end-to-end (launcher worker → broker → subscriber: start + ~39
chunks + done, 100% GPU). **Unverified:** the panel applet's popup auto-opening
from the background `Start` event (compositors may want a user gesture) — needs
an in-panel check.

## Standalone-from-goo

The launcher speaks pop-launcher's line-JSON — the *same surface* a future goo
meta-plugin would target — with zero goo dependency. They coexist on the launcher;
neither needs the other.

## Workspace window + polymorphic source/response (designed; mockup: `panel-mockup.html`)

Beyond the popup's fast path, a real **workspace window** (`cosmic-fabric-panel
window`, same mechanism as Settings) is the "response window" — openable,
persistent, the home for inspecting and routing. Visual reference:
`doc/panel-mockup.html` (open in a browser).

**Settled UX decisions:**

- **Triad = source → verb (pattern) → destination.** The same object/verb/
  indirect-object grammar designed for goo (see `cosmic-goo-integration.md`),
  made concrete and single-channel. Not the headline; just the shape.
- **Prompt-first, auto-assembled.** No "Assemble" button: the Prompt card
  re-renders live as source/pattern/variables change (`assemble` op). **Run** is
  the only action; the **Response** card is always present (placeholder until Run).
- **Source is editable and multi-origin**, shared between popup and workspace:
  segmented origin picker (clipboard · file · text · url · audio · image). Clipboard
  is a *seed, not a binding* (no auto-resample per run). Popup default = clipboard
  preloaded, so the fast path stays one-click → notify.
- **Send-to = a customizable destination registry** (the goo `To:`). Compact
  copy-icon+dropdown per artifact: one on the Prompt card, one on the Response
  card, **conversation** promoted to the window header bar (COSMIC standard for
  whole-doc actions). Ships with Copy (default) + Save-to-file; Claude/Alpaca
  shown **disabled** until goo's route layer lands (no `claude://` wired early).
  Registry managed in Settings.
- **Source includes the run it produced**: each card carries a `⊹ on "…"`
  source-ref; the conversation control bundles source + prompt + response.
- Footer: Retry · Stats (undesigned) · Clear · Settings.

**Polymorphic source & response** (fabric supports all of this natively — see
the fabric-multimodal note; CLI flags, REST exposes a subset):

- **Source pane adapts per origin**, not just multi-origin: url → field + Fetch +
  scrape/readability toggle; image → thumbnail (no editable text, attaches as
  vision input); audio → record/upload + duration. Non-text origins (url/audio/
  youtube) show the **transform** that feeds the prompt (`→ N chars markdown`),
  so prompt-first stays honest.
- **Response dispatches on MIME**: text box · image viewer (image-gen) · audio
  player (TTS). The Response card is not a fixed `<pre>`.

**Model-by-capability (the crux).** Once patterns need different model *types*
(text/vision/image/audio) across local + several APIs, the preferred-models list
can't stay flat. Settings → Models gets **capability tabs** (Text/Vision/Image/
Audio), models grouped by **vendor**, capability **inferred per model** (vendor+
family) with a manual tag override. A pattern offers only models from the
category it needs; that same data drives the workspace's **active model badge**
("⚠ qwen3 can't see images · switch ▾", Run disabled until a capable model is
picked). Reality check: categories are real but **unevenly populated** on this box
— Text fully, Vision via a couple of API models + llava, Image mostly API, Audio
via fabric `--transcribe-file`. Design for that, don't pretend the box does
everything locally.

### Build slices (independently shippable)

1. **Popup polish** — ✅ *built.* Verb labels (pretty `scribe-*`), "Open
   workspace…" button in the popup footer (spawns `cosmic-fabric-panel window`).
   (status pill / result-glance-opens-workspace-with-state deferred — the latter
   needs a daemon `last_result` op, see below.)
2. **Workspace window** — ✅ *built (v1).* `cosmic-fabric-panel window`: a
   `cosmic::app` window (like Settings). Multi-origin **Source** (Clipboard /
   Text / File-by-path / **URL**; Audio/Image shown disabled), editable via
   `text_editor`; **auto-assembled** prompt card (debounced 400ms on edit,
   immediate on pattern/source-load — no Assemble button); always-present
   **Response** card streaming via the run broker; Copy prompt / Copy response /
   Copy conversation + Save result to file. Smoke-tested (renders, no panic).
3. **Send-to registry + Settings** — *not built.* v1 ships **Copy** + **Save**
   only (buttons, not the dropdown registry). Destination management +
   model-by-capability categorization land here.
4. **Audio** — separate design (voice loop: STT via fabric `--transcribe-file`
   → meta-prompt pattern-pick → fabric → TTS). Much later.

**v1 deferrals (explicit, so the build doesn't read as broken):** send-to is Copy
buttons not the dropdown registry; file source is path-paste not a native
picker; Audio/Image origins are disabled; the model-capability badge is absent
(text/URL only); popup→workspace handoff opens an *empty* workspace (no
`last_result` carry yet). The mockup (`panel-mockup.html`) shows the full target.

### Daemon ops added this pass

- `{"op":"fetch","url":U,"mode":"scrape"|"readability"}` → `{"text":…,"chars":N}`.
  `scrape` = keyless Jina (`r.jina.ai/<url>`) markdown; `readability` = direct
  fetch + naive tag-strip (no Jina). Tight 10s timeout, no retry (the connection
  is the caller's own thread; the UI re-issues). Socket-tested.
- Rust clients: `daemon::assemble(pattern,input)` and
  `daemon::fetch_url(url,mode)` in `crates/.../daemon.rs`.

### LOCKED features

- **URL / web source** — *locked.* Validated **live end-to-end** (2026-05-26,
  scrape + `scribe-summarize` on `teaguesterling.github.io/judgementalmonad.com`):
  faithful summary, exit 0. Findings:
  - **Keyless Jina works.** Both `curl https://r.jina.ai/<url>` and fabric's own
    `-u/--scrape_url` returned clean markdown with **no `JINA_AI_API_KEY`** set.
  - **fabric `-u` gotcha (CLI only):** it still **blocks on stdin** even with a
    scrape URL — must run with stdin closed (`</dev/null`) or it hangs forever.
    Our planned daemon path sidesteps this entirely (daemon does the Jina/curl
    fetch itself → markdown → existing `input` of `run`/`assemble`; **no
    fabric-CLI shell-out, no REST change**).
  - **VRAM caveat:** a full web page as context **spilled** `qwen3:14b-iq4xs` to
    **81% GPU** (~19% on CPU). Web/URL sources produce large inputs — exactly the
    `context-tier sizing` case above; web sources may want a bigger ctx tier or a
    cloud model. The active-model / tier logic should account for transformed
    source length, not just the raw subject.
  - Readability is the alternate toggle. Builds with slice 2 (workspace) — no
    model-capability dependency, so it's the clean first non-text source.

### Designed but NOT locked (deferred)

- **Image source (vision)** — recommended *second* lock: it's what forces the
  model-by-capability categorization (and the active badge) to be real. Needs a
  vision model (llava local, or gpt-4o / claude via API).
- **Image-gen response** — thin locally; mostly API (gpt-image-1). Demonstrates
  polymorphic response.
- **Audio source / voice loop** — slice 4.

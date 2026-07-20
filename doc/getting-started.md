# Getting started with cosmic-fabric

A complete walkthrough: install the backend (woollama, which owns the managed
fabric), run `fabric --setup` properly (vendors, API keys, patterns, strategies),
install the COSMIC surfaces, configure your profile, and confirm each capability.
Budget ~20 minutes.

> Already know fabric? Jump to [3 · Install the surfaces](#3--install-the-cosmic-fabric-surfaces).
> Reference: [`fabric-on-the-desktop.md`](fabric-on-the-desktop.md) (what each feature
> is + status), [`manual.md`](manual.md) (day-to-day usage).

## The pieces (what you're standing up)

```
  you ──▶ surfaces (loom · kit · session)
              │  unix socket (line-JSON)
              ▼
        cosmic-fabricd  ──/w1──▶  woollama  ──▶  Ollama (local) / Anthropic / …
        (desktop glue)      (model routing + templating;    (the models)
                             owns the managed fabric --serve)

        vision only:  cosmic-fabricd ──▶ fabric CLI (fabric -a <image>)
```

- **woollama** — the inference router: model routing **and** pattern templating.
  It owns and supervises the managed `fabric --serve` and serves fabric's pattern
  library on `/w1/patterns`. It holds the provider API keys.
- **fabric** — the pattern library. You configure its vendors/keys once with
  `fabric --setup`; **woollama** then manages its server. cosmic-fabric also calls
  the `fabric` CLI directly for **vision** (image runs).
- **Ollama** — local models (text + vision), no API cost.
- **cosmic-fabricd** — the desktop-glue daemon; every surface is a thin client of
  its socket. It routes text runs to woollama and shells out to `fabric` for vision.
- **surfaces** — the panel/launcher/quick-action (kit), the workbench (loom), the
  chat (session).

## 1 · Prerequisites

- **COSMIC desktop** (this is a COSMIC-native bundle).
- **fabric** on `PATH` — `go install github.com/danielmiessler/fabric/cmd/fabric@latest`
  (or a release binary). Check: `fabric --version`. (woollama manages fabric's
  server; cosmic-fabric only calls the `fabric` CLI directly for vision.)
- **woollama** — the inference router (`woollamad`), the text backend for every
  run. cosmic-fabric auto-spawns a **keyless** one if none is running, so a
  standing instance is optional, but the `woollamad` binary must be installed.
- **Ollama** running, with at least one text model (e.g. `qwen3:14b-iq4xs`) and,
  for vision, `ollama pull llama3.2-vision`.
- **wl-clipboard** (`wl-copy`/`wl-paste`) — clipboard + selection.
- **Rust** (stable) to build the panel, and `just`.

## 2 · Configure fabric — `fabric --setup`

This is the part people skip and regret. fabric's vendors/keys/patterns are what
**woollama** drives when it runs a pattern, so you still configure fabric here —
woollama just owns its server afterward. `fabric --setup` (alias `fabric -S`) is
an **interactive** walk through *every reconfigurable part* of fabric. Run it:

```sh
fabric --setup
```

Work through these (you can re-run `--setup` any time to add more):

### a. Vendors & API keys
For each provider you want, select it and paste its key. What each unlocks:

| vendor | key needed? | unlocks |
|---|---|---|
| **Ollama** | no (local) | local text **and vision** (`llama3.2-vision`), zero cost — set the server URL (default `http://localhost:11434`) |
| **Anthropic** | yes | Claude (text **+ vision**) |
| **OpenAI** | yes | **STT/transcription** (`whisper-1`, `gpt-4o-transcribe`) + **image generation** |
| **Gemini** | yes (free at `aistudio.google.com/apikey`) | **TTS** + **image generation** + Google models |
| OpenRouter / Groq / Mistral / … | yes | more models (optional) |

> **"Antigravity" is not a fabric vendor** — it's Google's agentic IDE, not a
> model API. For Google models, add **Gemini**.

### b. Defaults
Set your **default model + vendor** (e.g. Ollama / `qwen3:14b-iq4xs` for cheap,
private, local). Change later with `fabric -d` (`--changeDefaultModel`, interactive)
or per-run with `-m <model> -V <vendor>`.

### c. Patterns & strategies
`--setup` offers to fetch the **patterns** repo (the verbs) and the **strategies**
repo (CoT-style prompt modifiers). Accept both. Update patterns later with
`fabric -U` (`--updatepatterns`).

### d. Optional extras
YouTube data API (richer `-y`), Jina (`-u` web scrape works **keyless**, but a key
raises rate limits), etc.

### Where it all lands
- `~/.config/fabric/.env` — vendor keys + defaults *(never commit this)*.
- `~/.config/fabric/patterns/` — the pattern library.
- `~/.config/fabric/strategies/` — strategies (after setup).

### Verify
```sh
fabric --listvendors      # vendors fabric knows
fabric -L                 # models actually reachable (i.e. keyed)
fabric --liststrategies   # should be non-empty after setup
fabric --serve            # you don't run this — woollama owns/supervises the managed fabric
```

If a vendor isn't in `fabric -L`, its key isn't set — re-run `fabric --setup`.

## 3 · Install the cosmic-fabric surfaces

```sh
# the panel binary (loom + kit popup + session + quick + settings)
cd crates && just install          # release → ~/.local/bin + .desktop + icon
#   or: just install-debug         # faster build, larger binary

# the daemon + launcher plugin (Python; language-agnostic)
#   the daemon lives at ~/.local/share/cosmic-fabric/cosmic-fabricd
#   the launcher auto-spawns it; or run it once: cosmic-fabricd &
```

Then:
- **Panel applet** — cosmic-settings → Panel (or Dock) → Configure applets → add
  **Fabric**. (After a rebuild: `pkill cosmic-fabric-panel`; the panel respawns it.)
- **Launcher** — the pop-launcher plugin is picked up automatically; Super → type a
  verb.
- **Quick-action shortcut** — bind a key to the selection grid:
  cosmic-settings → Keyboard → Shortcuts → **Custom** → add command
  `cosmic-fabric-panel quick` (e.g. **Super+Shift+F**). *(May need a logout/login
  to take effect.)*

## 4 · Configure your profile

Two equivalent ways — the file, or the GUI.

**The file:** `~/.config/cosmic-fabric/policy.toml`
```toml
[surface]                          # which patterns the quick surfaces show
include = ["scribe-*"]             # globs (* / ?) or exact names; empty = all
exclude = []

[models.qwen3]                     # a named model instantiation
vendor = "Ollama"
model  = "qwen3:14b-iq4xs"
capabilities = ["text"]
categories = ["local"]
default = "fast"
  [models.qwen3.variants.fast]  ctx = 2048 ; thinking = "off"
  [models.qwen3.variants.deep]  ctx = 16384

[models.llama-vision]              # a vision model → the capability rule uses it
vendor = "Ollama" ; model = "llama3.2-vision:latest"
capabilities = ["text", "vision"]

[default] use = "qwen3/fast"       # or legacy inline: model = "…", vendor = "…"
[patterns.scribe-visualize] use = "sonnet"

[output] mode = "notify"           # notify | dialog | edit | clipboard | panel
[ollama] url = "http://localhost:11434" ; warn_below_gpu = 99
```

**The GUI (recommended):** open the workbench — `cosmic-fabric-panel window` —
- **Library**: search all patterns, ★ the ones you want (writes `[surface]`),
  route each to a model.
- **Models**: define instantiations + variants, categories, default variant.

The daemon re-reads `policy.toml` per run, so edits take effect immediately.

## 5 · Use it

- **Kit (fast):** panel popup (quick-run on the clipboard) · launcher (Super →
  type) · **quick-action** (Super+Shift+F on highlighted text → grid → result).
- **Loom (power/config):** `cosmic-fabric-panel window` — Run a source→pattern→
  result, curate in Library, configure in Models.
- **Session (chat):** `cosmic-fabric-panel session` — multi-turn.
- **Scripts (CLI):** `cosmic-fabric` — a thin client of the daemon socket, so you
  get the *orchestrated* deployment: `echo text | cosmic-fabric run scribe-summarize`,
  `cosmic-fabric assemble <pattern>`, `cosmic-fabric fetch <url>`,
  `cosmic-fabric patterns`. For raw fabric, `FABRIC_API="$(cosmic-fabric fabric-url)"`.

> **New here?** Walk through the hands-on [`tutorial.md`](tutorial.md) — your first
> 30 minutes, doing one real task per surface.

## 6 · Capability cheat-sheet (what works, what needs a key)

| capability | needs | how it surfaces |
|---|---|---|
| text patterns | Ollama or any keyed vendor | everywhere |
| **vision** (describe/OCR an image) | **nothing extra** — local `llama3.2-vision` or Anthropic | image run; the **capability rule** auto-picks a vision model |
| **web / URL** (summarize a page) | nothing (keyless Jina) | URL source |
| **audio / STT** | **OpenAI key** | (after key) `--transcribe-file` |
| **image generation** | **OpenAI or Gemini key** | (after key) image-gen pattern |
| **TTS** (read aloud) | **Gemini key** | (after key) speak destination |

## 7 · Troubleshooting

- **"daemon not reachable"** — start it: `cosmic-fabricd &` (check
  `~/.cache/cosmic-fabric/daemon.log`). Only one should run; it refuses to steal a
  live socket.
- **No patterns in the popup** — your `[surface].include` matched nothing; widen it
  or curate in the Library. `!` in the launcher force-shows all.
- **Result spilled to CPU** (GPU% < 100, ⚠ badge) — the model + context exceeded
  VRAM; use a smaller variant (lower `ctx`) or a cloud instantiation. The daemon
  auto-sizes context for Ollama runs.
- **Quick-action key does nothing** — log out/in; verify under Settings → Keyboard
  → Shortcuts → Custom.
- **A vendor's models are missing** — its key isn't set; `fabric --setup`.
- **Is fabric/woollama exposed on the network?** cosmic-fabric no longer runs its
  own `fabric --serve`, so there's no cosmic-fabric-owned network binding to secure.
  Inference goes to **woollama**, which holds the provider API keys and by default
  binds **loopback + an owner-only (0600) unix socket**
  (`$XDG_RUNTIME_DIR/woollama.sock`). As of woollama 0.8.0 it also supports optional
  surface auth (`WOOLLAMA_TOKEN`) and **refuses a non-loopback bind without a token**.
  If none is running, cosmic-fabric auto-spawns a **keyless** woollamad (it strips
  `ANTHROPIC_API_KEY` from the child's environment). The only fabric process
  cosmic-fabric starts directly is the short-lived `fabric` CLI for a vision run.
- **Settings/model changes not taking** — the daemon re-reads per run; for the
  panel's curated list, reopen the popup.

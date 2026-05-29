# Getting started with cosmic-fabric

A complete walkthrough: install the engine, run `fabric --setup` properly
(vendors, API keys, patterns, strategies), install the COSMIC surfaces, configure
your profile, and confirm each capability. Budget ~20 minutes.

> Already know fabric? Jump to [3 · Install the surfaces](#3--install-the-cosmic-fabric-surfaces).
> Reference: [`fabric-on-the-desktop.md`](fabric-on-the-desktop.md) (what each feature
> is + status), [`manual.md`](manual.md) (day-to-day usage).

## The pieces (what you're standing up)

```
  you ──▶ surfaces (loom · kit · session)
              │  unix socket (line-JSON)
              ▼
        cosmic-fabricd  ──REST──▶  fabric --serve  ──▶  Ollama (local) / Anthropic / …
        (owns the deployment)        (the engine)        (the models)
```

- **fabric** — the engine (patterns + model routing). You configure it once.
- **Ollama** — local models (text + vision), no API cost.
- **cosmic-fabricd** — the daemon that owns the fabric deployment; every surface
  is a thin client of its socket.
- **surfaces** — the panel/launcher/quick-action (kit), the workbench (loom), the
  chat (session).

## 1 · Prerequisites

- **COSMIC desktop** (this is a COSMIC-native bundle).
- **fabric** on `PATH` — `go install github.com/danielmiessler/fabric/cmd/fabric@latest`
  (or a release binary). Check: `fabric --version`.
- **Ollama** running, with at least one text model (e.g. `qwen3:14b-iq4xs`) and,
  for vision, `ollama pull llama3.2-vision`.
- **wl-clipboard** (`wl-copy`/`wl-paste`) — clipboard + selection.
- **Rust** (stable) to build the panel, and `just`.

## 2 · Configure fabric — `fabric --setup`

This is the part people skip and regret. `fabric --setup` (alias `fabric -S`) is
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
fabric --serve            # start the REST API the daemon talks to (port 8080)
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
- **Is fabric exposed on the network?** The daemon binds `fabric --serve` to
  **`127.0.0.1:8080`** (loopback only) — its REST API holds your API keys, so it
  must not be LAN-reachable. Check: `ss -tlnp | grep :8080` should show
  `127.0.0.1:8080`, not `*`/`0.0.0.0`. (fabric's own default is `:8080` = all
  interfaces; we override it.) If you *want* LAN access, do it deliberately —
  `--address 0.0.0.0:8080` **with** `--api-key`.
- **Settings/model changes not taking** — the daemon re-reads per run; for the
  panel's curated list, reopen the popup.

# cosmic-fabric — Getting Started

A step-by-step setup, from a bare fabric install to all four surfaces working.
~20 minutes. cosmic-fabric is a frontend bundle over **one local fabric
deployment**, so we set fabric up first, then cosmic-fabric on top.

> Companion docs: `fabric-on-the-desktop.md` (what every feature does + status),
> `manual.md` (reference), `design.md` (architecture).
> **Security:** API keys go into `~/.config/fabric/.env` via `fabric --setup` —
> never paste a key into a chat or a shell command that gets logged.

---

## 0 · Prerequisites

- **COSMIC desktop** (Wayland).
- **`fabric`** (danielmiessler) on your `PATH` (e.g. `~/.local/bin/fabric`).
- **`ollama`** for local models, with at least one model pulled:
  `ollama pull qwen3:14b` (text workhorse) and, for vision, `ollama pull llama3.2-vision`.
- **`wl-clipboard`** (`wl-copy` / `wl-paste`).
- To build the panel: **Rust** toolchain + **`just`**. The daemon needs **Python 3.11+**.
- Optional: `nvidia-smi` (enables the GPU-placement readout).

---

## 1 · Set up fabric (`fabric --setup`)

This is the foundation. Everything cosmic-fabric does runs through your fabric config.

### 1a · Run setup
```sh
fabric --setup
```
Interactive: it walks **every vendor** — enter a key to enable one, leave blank to
skip — then sets a default vendor/model and downloads the pattern library and
strategies. To (re)configure one vendor later:
```sh
fabric --setup-vendor=Gemini      # or Anthropic, OpenAI, Ollama, …
fabric --changeDefaultModel=qwen3:14b-iq4xs
```
Keys land in `~/.config/fabric/.env` (written by setup — don't hand-paste keys
elsewhere).

### 1b · Which vendors to enable — and what each unlocks

| vendor | get it from | unlocks |
|---|---|---|
| **Ollama** (local, free) | install + `ollama pull` | local **text** (qwen3) and **vision** (llama3.2-vision); set `OLLAMA_API_URL` |
| **Anthropic** | console.anthropic.com | claude — strong **text + vision**, cloud quality |
| **OpenAI** | platform.openai.com/api-keys | **STT / transcription** (whisper) + **image generation** |
| **Gemini** | aistudio.google.com/apikey (free tier, no card) | **TTS**, **image generation**, Google models |
| others (Groq, Mistral, OpenRouter, …) | each provider | more model choices (optional) |

> **Note:** "Antigravity" is Google's agentic IDE, **not** a fabric vendor — there's
> nothing to key for it. For Google models, enable **Gemini**.

Minimum to start: **Ollama** (local, free). Add **Anthropic** for cloud quality,
**OpenAI** for audio/image-gen, **Gemini** for TTS/image-gen.

### 1c · Patterns & strategies
```sh
fabric -U                 # download / update the pattern library (~265 patterns)
fabric --liststrategies   # confirm strategies were fetched by --setup
```
A **custom pack** (e.g. `scribe-*`): set `PATTERNS_LOADER_GIT_REPO_URL` in
`.env`, or drop your pattern folders into `~/.config/fabric/patterns/`.

### 1d · Verify fabric
```sh
fabric --listvendors                 # configured vendors
fabric -L                            # available models
fabric --listpatterns | head
echo "the mitochondria is the powerhouse of the cell" | fabric --pattern summarize
```

---

## 2 · Install cosmic-fabric

### 2a · Build + install
```sh
cd cosmic-fabric/crates
just install         # release build → ~/.local/bin + daemon + .desktop + icon
#   or: just install-debug   (faster build, larger binary)
```

### 2b · The daemon
`cosmic-fabricd` owns the fabric deployment (the "one channel"). It's auto-spawned
by the launcher/panel, or run it once:
```sh
cosmic-fabricd &     # logs: ~/.cache/cosmic-fabric/daemon.log
```

### 2c · Add the panel applet
**cosmic-settings → Desktop → Panel** (or **Dock**) **→ Configure applets → add
"Fabric".** After a rebuild, reload it: `pkill cosmic-fabric-panel` (the panel
respawns the new binary).

---

## 3 · Configure cosmic-fabric

Your config is `~/.config/cosmic-fabric/policy.toml` (the daemon re-reads it per
run, so edits take effect immediately). Edit by hand **or**, recommended, in the
**Workbench** (`cosmic-fabric-panel window`).

### 3a · The profile (your active set)
`[surface]` picks which patterns surface in the quick UIs, as include/exclude
**globs** over pattern names:
```toml
[surface]
include = ["scribe-*"]   # your working set; empty = all patterns
exclude = []
```
Curate it visually in the loom's **Library** tab (★ to add/remove; search all 265).

### 3b · Model instantiations
A named model + deployment params, referenced by `use`:
```toml
[models.qwen3]
vendor = "Ollama"; model = "qwen3:14b-iq4xs"
capabilities = ["text"]; categories = ["local"]; default = "fast"
  [models.qwen3.variants.fast]  ctx = 2048; thinking = "off"
  [models.qwen3.variants.deep]  ctx = 16384

[models.llama-vision]            # lets the capability rule auto-pick vision
vendor = "Ollama"; model = "llama3.2-vision:latest"
capabilities = ["text", "vision"]; categories = ["local", "vision"]

[default] use = "qwen3/fast"
[patterns.scribe-visualize] use = "sonnet"
```
Edit all of this visually in the loom's **Models** tab (add/edit models + variants,
set the default variant, tag categories, see who uses each). An image run then
**auto-selects** a `capabilities = ["vision"]` instantiation.

### 3c · Result delivery & globals
`cosmic-fabric-panel settings` — result-delivery mode (notify / dialog / editor /
clipboard / panel), the Ollama URL, and the GPU-warn threshold.

---

## 4 · The surfaces — how to use each

### Kit (the quick OS tie-ins)
- **Launcher** — `Super`, type a verb, Enter → runs on your **selection**.
- **Panel popup** — click the Fabric applet → status + quick-run on the **clipboard**.
- **Quick-action** — bind a hotkey: **Settings → Keyboard → Shortcuts → Custom →
  add a command** running `cosmic-fabric-panel quick` (suggested: `Super+Shift+F`).
  Highlight text, press it, pick a pattern, review inline. *(May need a logout/in.)*

### Loom — the Workbench (`cosmic-fabric-panel window`)
- **Run** — pick a source (clipboard / text / file / **URL**) → a pattern → the
  prompt auto-assembles → **Run** → route the result (Copy / Save / …).
- **Library** — curate your active set.
- **Models** — define/edit model instantiations.

### Session (`cosmic-fabric-panel session`)
- A light multi-turn chat (fabric sessions keep the history). "New chat" resets.

### (4th, programmatic) the socket
`$XDG_RUNTIME_DIR/cosmic-fabric.sock` — line-delimited JSON; the API all the UIs
are built on, and the seam for any future automation. Not user-facing.

---

## 5 · Verify (quick checklist)

- [ ] Panel shows **serve ● up** + your default model.
- [ ] Copy some text → panel popup → pick a pattern → a result streams in.
- [ ] `Super+Shift+F` on a selection → grid → run → inline result.
- [ ] *(if Anthropic/Gemini keyed)* a cloud pattern runs.
- [ ] *(if llama3.2-vision pulled)* an image run auto-selects the vision model.

---

## Troubleshooting

- **"serve ○ down"** — `fabric --serve` isn't up; the daemon starts it, or run it
  yourself. Check `~/.cache/cosmic-fabric/daemon.log`.
- **No patterns in the popup** — your `[surface].include` glob doesn't match your
  pattern names. `fabric --listpatterns` to see the real names.
- **Quick-action key doesn't fire** — log out/in; verify under Settings → Keyboard
  → Shortcuts → Custom.
- **Low GPU% / spilling** — a large input pushed the KV cache off-GPU; the daemon
  auto-sizes context, or pick a smaller model/variant in Models.
- **A capability is blocked** (audio/image-gen/TTS) — that vendor isn't keyed:
  `fabric --setup-vendor=OpenAI` (STT/image-gen) or `=Gemini` (TTS/image-gen).
- **Stale daemon after editing it** — `pkill -f cosmic-fabricd` and rerun (it
  refuses to steal a live socket, so kill the old one first).

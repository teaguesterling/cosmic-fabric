# Getting started with cosmic-fabric

A complete walkthrough: install [fabric](https://github.com/danielmiessler/fabric),
configure its vendors/keys with `fabric --setup`, install cosmic-fabric, wire up the
surfaces, and make it yours. No sudo — everything is user-level (`~/.local`).

> Already know fabric? Jump to [§4 Install cosmic-fabric](#4-install-cosmic-fabric).
> What each capability needs is in [§8 Capabilities & keys](#8-capabilities--keys-cheat-sheet).

## 0 · The pieces

```
fabric (--serve)        the engine: patterns, models, sessions, multimodal
  ├── Ollama            local models (private, free)
  └── Anthropic/OpenAI/Gemini/…   cloud models (keys)
cosmic-fabricd          the daemon — owns fabric, serves one unix socket ("one channel")
  ├── panel applet      kit: status + quick-run            ┐
  ├── launcher plugin   kit: Super → "fab <pattern>"        │ thin socket clients
  ├── quick-action      kit: hotkey on the selection        │ sharing one profile
  ├── workspace (loom)  power + config (Library, Models)     │ (policy.toml)
  └── session           multi-turn chat                     ┘
```

One config file — `~/.config/cosmic-fabric/policy.toml` (the **profile**) — is read
by every surface. fabric's own config (vendors/keys) lives in `~/.config/fabric/.env`.

## 1 · Prerequisites

- **COSMIC desktop** (Pop!_OS COSMIC or another COSMIC session).
- **fabric** — `go install github.com/danielmiessler/fabric/cmd/fabric@latest`
  (or a release binary). Confirm: `fabric --version`.
- **Ollama** (for local models; recommended) — https://ollama.com . Confirm:
  `ollama --version`.
- **Rust + just** (to build the panel applet) — `rustup`, then `cargo install just`.
- **wl-clipboard** (`wl-copy`/`wl-paste`) and **libnotify** (`notify-send`) —
  usually present on COSMIC; `apt install wl-clipboard libnotify-bin` if not.

## 2 · Configure fabric — `fabric --setup` (the important part)

`fabric --setup` is an interactive menu. Run it and work through:

### a. Vendors & API keys
Pick each vendor you want and paste its key. What each unlocks:

| vendor | get a key at | unlocks |
|---|---|---|
| **Ollama** (local) | — (no key; just the server) | local text **and vision** (e.g. `llama3.2-vision`), private + free |
| **Anthropic** | console.anthropic.com | claude (strong text; **vision**-capable) |
| **OpenAI** | platform.openai.com/api-keys | GPT models; **STT/transcription** (whisper); **image generation** |
| **Gemini** | aistudio.google.com/apikey (free tier) | Gemini models; **TTS**; image generation |

> **"Antigravity" is not a fabric vendor** — it's Google's agentic IDE, not a model
> API. For Google models, choose **Gemini**.

Keys are written to `~/.config/fabric/.env`. **Never commit that file**; never paste a
key into a chat or a shell command that gets logged.

### b. Default vendor & model
Set a default (e.g. `Ollama` / `qwen3:14b-iq4xs`). Pull it first (see §3).

### c. Patterns (the verbs)
fabric fetches the **upstream pattern set** from a git repo via the pattern loader
(the default, ~254 patterns):

```ini
# ~/.config/fabric/.env
PATTERNS_LOADER_GIT_REPO_URL=https://github.com/danielmiessler/fabric.git
PATTERNS_LOADER_GIT_REPO_PATTERNS_FOLDER=data/patterns
```

`fabric --setup` fetches them; confirm with `fabric --listpatterns`. **Custom packs**
(like the `scribe-*` set) are just folders you drop into `~/.config/fabric/patterns/`
— each pattern is a directory containing a `system.md`. They appear alongside the
upstream ones (here: 11 `scribe-*` + ~254 upstream = 265 total). You then **curate**
which of those you actually surface, in the Library (§6).

### d. Strategies (optional)
`fabric --setup` also fetches prompt **strategies** (CoT etc.) via
`PROMPT_STRATEGIES_GIT_REPO_URL`. List them: `fabric --liststrategies`.

### e. Verify
```sh
fabric --serve &                       # REST on :8080 (cosmic-fabricd starts this for you too)
curl -s localhost:8080/patterns/names | head -c 200
curl -s localhost:8080/models/names    | python3 -m json.tool | head
fabric --listvendors                   # vendors you've configured show models
```

## 3 · Local models (Ollama)

```sh
ollama pull qwen3:14b-iq4xs       # text workhorse (~8 GB on an 11 GB GPU)
ollama pull llama3.2-vision       # local vision (~7.8 GB) — powers image runs
```

Tips for an 11 GB GPU (e.g. RTX 2080 Ti):
- A q8 KV cache lowers footprint — set `OLLAMA_KV_CACHE_TYPE=q8_0` (systemd override).
- cosmic-fabric **auto-sizes context** per input (`pick_ctx`), so short inputs stay
  fully on the GPU and long ones (web pages) grow instead of truncating.
- Under desktop memory pressure a model may spill to CPU (the panel shows a
  `⚠ GPU%` badge); it still runs.

## 4 · Install cosmic-fabric

```sh
git clone https://github.com/teaguesterling/cosmic-fabric
cd cosmic-fabric

cd src && ./install.sh && cd ..  # daemon + shared core → ~/.local/share/cosmic-fabric/,
                                 # launcher plugin → pop-launcher (+ symlinked onto PATH),
                                 # seeds ~/.config/cosmic-fabric/policy.toml if absent.
                                 # The daemon is COPIED (stable across edits) — re-run
                                 # install.sh after a `git pull` to update it.
pkill cosmic-launcher            # reload pop-launcher so it sees the plugin

cd crates && just install        # the panel applet (release build). Or: just install-debug
```

The daemon auto-starts the first time you use the launcher/panel (and it ensures
`fabric --serve` is up). To start it by hand:

```sh
python3 ~/.local/share/cosmic-fabric/cosmic-fabricd &   # idempotent — won't steal a live socket
```

## 5 · Wire the surfaces

- **Panel applet** (status + quick-run): `cosmic-settings` → **Panel** (or Dock) →
  **Configure applets** → add **"Fabric"**. (`just hint` prints this.) After a
  rebuild, reload with `pkill cosmic-fabric-panel` (the panel respawns it).
- **Launcher**: already registered. Super → type `fab summarize` (acts on the current
  selection). `!` in the launcher shows *all* patterns, not just your active set.
- **Quick-action** (hotkey on the selection): `cosmic-settings` → **Keyboard** →
  **Shortcuts** → **Custom** → add a command shortcut running
  `cosmic-fabric-panel quick`, bound to e.g. **Super+Shift+F**. (May need a
  logout/login to take effect.)
- **Workspace (loom)**: `cosmic-fabric-panel window`, or **"Open workspace…"** in the
  popup.
- **Session (chat)**: `cosmic-fabric-panel session`, or **"Chat…"** in the popup.

## 6 · Make it yours (the profile)

Everything below writes `~/.config/cosmic-fabric/policy.toml`, which every surface reads.

- **Curate patterns** — Workspace → **Library**: search all patterns, ★ the ones you
  want. Your active set is what the popup/launcher/quick-action show. (Under the hood:
  `[surface]` include/exclude globs — `scribe-*` is just one pack.)
- **Define models** — Workspace → **Models**: create **instantiations** (a model +
  deployment params), with **variants** (`qwen3/fast` @ctx 2048, `qwen3/deep` @16384)
  and a default variant. Tag them with **categories** and **capabilities**. Set the
  global default and per-pattern model. Example:

  ```toml
  [models.qwen3]
  vendor = "Ollama"; model = "qwen3:14b-iq4xs"
  capabilities = ["text"]; categories = ["local"]; default = "fast"
    [models.qwen3.variants.fast]  # ctx 2048, quick
    ctx = 2048; thinking = "off"
    [models.qwen3.variants.deep]  # ctx 16384, thorough
    ctx = 16384

  [models.llama-vision]
  vendor = "Ollama"; model = "llama3.2-vision:latest"
  capabilities = ["text", "vision"]   # the capability rule picks this for image runs

  [default] use = "qwen3/fast"
  [patterns.scribe-visualize] use = "sonnet"
  ```

- **Capability rule**: tag a model `capabilities = ["vision"]` and image runs
  auto-pick it (prefers local). No manual model-switching for vision.

## 7 · First runs

- **Kit · popup** — copy some text, click the Fabric applet, pick a pattern → result
  streams + a notification (and it's on your clipboard).
- **Kit · launcher** — highlight text, Super → `fab summarize` → Enter.
- **Kit · quick-action** — highlight text, **Super+Shift+F** → pick a pattern → inline
  result, copied.
- **Loom · workspace** — pick a Source (clipboard / type / file / **URL** → Fetch),
  pick a pattern, the prompt assembles, **Run**, then route with the send-to menus.
- **Session** — open Chat, talk; fabric keeps the conversation history.
- **Vision** — once a vision model is configured (e.g. `llama3.2-vision`), image runs
  auto-select it (UI image-source pane is on the roadmap; the daemon path works today).

## 8 · Capabilities & keys cheat-sheet

| capability | works with |
|---|---|
| text patterns | **Ollama** (local) or any cloud vendor |
| **vision** (describe/OCR an image) | **local** `llama3.2-vision` (Ollama) — *or* Anthropic/OpenAI/Gemini |
| **web/URL** (summarize a page) | built in (keyless Jina reader; no key needed) |
| **STT** (transcribe audio) | **OpenAI** key (whisper / gpt-4o-transcribe) |
| **image generation** | **OpenAI** or **Gemini** key |
| **TTS** (read aloud) | **Gemini** key |

## 9 · Troubleshooting

- **Daemon not reachable** — `cosmic-fabricd &`; check `~/.cache/cosmic-fabric/daemon.log`.
- **fabric not serving** — the daemon starts `fabric --serve`; or run it yourself and
  `curl localhost:8080/patterns/names`.
- **No patterns in the popup** — curate an active set in the Library, or confirm the
  pattern loader (`fabric --listpatterns`).
- **Quick-action key doesn't fire** — log out/in; verify under Settings → Keyboard →
  Shortcuts → Custom.
- **Result spilled to CPU** (`⚠ GPU%`) — desktop memory pressure; close VRAM hogs, or
  use a smaller ctx variant / a cloud model for big inputs.
- **Applet changes not showing** — `pkill cosmic-fabric-panel` to reload.

## 10 · Uninstall

```sh
cd crates && just uninstall                            # panel binary + .desktop + icon
rm -rf ~/.local/share/cosmic-fabric                    # daemon + core
rm -rf ~/.local/share/pop-launcher/plugins/cosmic-fabric
rm -f  ~/.local/bin/cosmic-fabric-launcher             # the PATH symlink
# optional: rm -rf ~/.config/cosmic-fabric ~/.cache/cosmic-fabric
```

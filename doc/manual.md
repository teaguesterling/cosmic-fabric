# cosmic-fabric — user manual

Run [fabric](https://github.com/danielmiessler/fabric) AI patterns on your current
selection, from the COSMIC launcher. Type `fab <pattern>`, pick one, and the result
lands on your clipboard (with a View/Edit notification).

- [Requirements](#requirements)
- [Install](#install)
- [Using it](#using-it)
- [Configuration (`policy.toml`)](#configuration)
- [How it works](#how-it-works)
- [GPU / model placement](#gpu--model-placement)
- [Troubleshooting](#troubleshooting)
- [Uninstall](#uninstall)

---

## Requirements

| Need | For |
|---|---|
| **COSMIC desktop** (pop-launcher) | the launcher surface |
| **woollama** (`woollamad`) | the inference router — text backend for every run (routing + pattern templating; owns the managed fabric). Auto-spawned keyless if not already running |
| **`fabric`** on `PATH` (`~/.local/bin`) | the pattern library (woollama manages its server); also the vision path (`fabric -a`) |
| **`wl-clipboard`** (`wl-paste`, `wl-copy`) | capture selection / deliver result |
| **`python3`** ≥ 3.11 | the launcher + daemon (uses stdlib `tomllib`) |
| `notify-send` (libnotify) | notifications (View/Edit buttons) |
| `zenity` *(optional)* | the result dialog (`dialog` mode / the View button) |
| `cosmic-edit` *(optional)* | open the result in an editor (Edit button / `edit` mode) |
| **ollama** *(for local models)* / `ANTHROPIC_API_KEY` in `~/.config/fabric/.env` *(for cloud)* | wherever your models run |

The `scribe-*` pattern pack should be installed in `~/.config/fabric/patterns/`
(that's what the launcher surfaces first).

## Install

```sh
cd ~/Projects/cosmic-fabric/src
./install.sh          # user-dir only, no sudo
pkill cosmic-launcher # reload the launcher so it picks up the plugin
```

`install.sh` puts:
- the daemon (`cosmic-fabricd` + `core.py`) in `~/.local/share/cosmic-fabric/`,
- the launcher plugin in `~/.local/share/pop-launcher/plugins/cosmic-fabric/`,
- a starter `~/.config/cosmic-fabric/policy.toml` (only if you don't already have one).

The daemon (`cosmic-fabricd`) starts automatically the first time you use the
launcher; it also makes sure the **woollama** backend is up — discovering a live
woollama, or auto-spawning a keyless one — and woollama in turn owns the managed
`fabric --serve`.

## Using it

1. **Select text** in any app.
2. Open the COSMIC launcher and type **`fab `** followed by a filter, e.g. `fab summ`.
3. Pick a pattern and press **Enter**.
4. The result is copied to your clipboard, and a notification appears with **View**
   (scrollable dialog) and **Edit** (open in cosmic-edit) buttons.

**Pattern filtering:**

| You type | You get |
|---|---|
| `fab ` | the curated `scribe-*` patterns |
| `fab summ` | `scribe-*` matches (falls back to built-ins if none match) |
| `fab !youtube` | the **full** ~250 built-in library (the `!` prefix opens it) |

## Panel applet & settings

Build + install the libcosmic panel applet (Rust):

```sh
cd crates && just install      # release build → ~/.local/bin + .desktop + icon
#   (or: just install-debug for a faster, larger debug binary)
```
Then add it: **cosmic-settings → Panel (or Dock) → Configure applets → add "Fabric"**.
After a rebuild, reload it with `pkill cosmic-fabric-panel` (the panel respawns it).

The applet (it talks to `cosmic-fabricd`, which the launcher auto-spawns — or run
`cosmic-fabricd` once):
- **Status** — fabric serve health, default model, loaded model + GPU% (with a
  `⚠` when it has spilled to CPU), VRAM free.
- **Quick-run** — pick a `scribe-*` pattern → it runs on the **clipboard** and the
  result **streams** into a scrollable pane with a Copy button.
- **Settings…** — opens the settings window.

**Settings window** (`cosmic-fabric-panel settings`) edits `~/.config/cosmic-fabric/policy.toml`
(auto-saves; the daemon re-reads per run):
- default model + per-pattern model as tiers (Default / Local / Haiku / Sonnet),
- result-delivery mode,
- ollama URL + warn-below-GPU% threshold.

For models beyond those tiers, edit `policy.toml` directly (below).

## Workspace window

A fuller, prompt-first console — open it from the applet popup (**Open
workspace…**) or run `cosmic-fabric-panel window`. Unlike the popup's one-click
clipboard run, the workspace lets you **pick a source, see the assembled prompt,
then run**:

1. **Source** — choose an origin and edit the text:
   - **Clipboard** — load the current clipboard (a seed; edit freely after).
   - **Text** — type/paste directly.
   - **File** — paste a path (`~/…` ok) → **Load**.
   - **URL** — paste a link → **Fetch**: the page is scraped to markdown (keyless
     Jina) and becomes the source. A note shows the transformed length.
   - *(Audio / Image are shown but disabled — they arrive with vision/model work.)*
2. **Pattern** — pick a `scribe-*` verb. The **Prompt** card assembles
   automatically (the system prompt + your source, no model run) and re-renders as
   you edit.
3. **Run** — executes the prompt; the **Response** card streams in.
4. **Route** — each of the **Prompt**, **Response**, and **Conversation**
   (source + prompt + response) has a send-to control: click it to **Copy**, or
   the **▾** to choose a destination — **Save to file…**, or (disabled until
   goo's route layer lands) **Claude Desktop** / **Alpaca**, plus **Manage
   destinations…**.

The destination list will become editable in Settings; Claude/Alpaca light up
once goo's route layer exists (see `doc/design.md`).

## Configuration

Everything lives in **`~/.config/cosmic-fabric/policy.toml`**. It's re-read on every
run, so edits to models/output take effect immediately (no reload). Only *code*
changes need `pkill cosmic-launcher` / `pkill cosmic-fabricd`.

```toml
# Which model/vendor runs a pattern. A pattern not listed uses [default].
[default]
model = "qwen3:14b-iq4xs"
vendor = "Ollama"
extra = ["--thinking=off", "--suppress-think"]   # qwen3: no reasoning + strip <think>

[patterns]
scribe-visualize = { model = "claude-sonnet-4-6", vendor = "Anthropic", extra = [] }
# scribe-summarize = { model = "claude-haiku-4-5", vendor = "Anthropic", extra = [] }

# How the result is surfaced. The clipboard is ALWAYS set first as a backstop.
[output]
mode = "notify"   # notify | dialog | edit | clipboard

# The local model server. `url` powers the post-run GPU-placement check.
[ollama]
bin = "/opt/ollama/bin/ollama"
url = "http://localhost:11434"
warn_below_gpu = 99   # notify if an Ollama run lands below this % on the GPU

# The woollama backend. woollama is the text backend (routing + templating) and is
# enabled by default; set enabled=false only to hard-disable it (no text runs).
[woollama]
enabled = true         # false → hard-disable the woollama backend
# address = "host:port" # override discovery (default: $XDG_RUNTIME_DIR/woollama.sock)
```

**`[default]` / `[patterns]`** — `model`, `vendor` (`Ollama` or `Anthropic`), and
`extra` (CLI-style flags translated to fabric API options: `--thinking=off`,
`--suppress-think`, `--raw`, `--temperature=…`).

**`[output].mode`:**

| mode | behavior |
|---|---|
| `notify` *(default)* | notification with **View**/**Edit** buttons; non-intrusive |
| `dialog` | pop a scrollable `zenity` window every run |
| `edit` | open the result in `cosmic-edit` every run |
| `clipboard` | copy + a plain notification (no buttons) |
| `panel` | dispatch the result to the **Fabric panel applet** (it streams there); falls back to a notification if the panel isn't running |

**`[ollama]`** — `url` is used now for the GPU-placement check; `bin` is reserved for
the daemon's future model-lifecycle role; `warn_below_gpu` sets the spill-warning
threshold.

**`[woollama]`** — woollama is the **text backend**, enabled by default: every
plain run renders its pattern on woollama's `/w1` and infers there, and **Local**
chat sessions run through woollama too (its `/v1` responses surface, attach-by-key).
woollama owns both model routing and pattern templating, backed by the managed
[fabric](https://github.com/danielmiessler/fabric) server it supervises (see
[woollama](https://github.com/teaguesterling/woollama)). There is
**no fabric fallback**: if woollama is unreachable a text run errors — which is why
`cosmic-fabricd` auto-spawns a keyless woollamad when it can't find a live one. Set
`enabled = false` only to hard-disable the backend. **Local** chat sessions are
*stateful* when a woollama conversation store is wired (otherwise one-shot).
`address` overrides discovery (default: woollama's owner-only Unix socket at
`$XDG_RUNTIME_DIR/woollama.sock`). Standing up stateful Local sessions — the
conversation store plus the `systemd` units — is covered in the runbook,
[`local-ollama-sessions.md`](local-ollama-sessions.md).

## How it works

```
COSMIC launcher ──"fab"──▶ cosmic-fabric-launcher (thin client)
                               │  selection (wl-paste -p)
                               ▼  unix socket (JSON)
                          cosmic-fabricd ──/w1 run──▶ woollama ──▶ fabric --serve ──▶ Ollama / Anthropic
                               │        (routing + templating; owns managed fabric)     │
                               └── /api/ps placement check ◀──────────────────────────────┘
                               ▼
                          result → launcher → clipboard + View/Edit notification
```

- **`cosmic-fabricd`** is desktop glue: it holds the policy, routes text runs to
  **woollama** (which renders the pattern on `/w1` and infers via its managed
  `fabric --serve`), checks GPU placement, and shells out to the `fabric` CLI for
  vision. Socket: `$XDG_RUNTIME_DIR/cosmic-fabric.sock`.
- **The launcher** is a thin client: list patterns, capture selection, ask the daemon
  to run, deliver the result. It auto-spawns the daemon on first use.
- **Logs:** `~/.cache/cosmic-fabric/daemon.log` and `~/.cache/cosmic-fabric/launcher.log`.

See [design.md](design.md) for the architecture and roadmap (panel, settings,
context-tier sizing).

## GPU / model placement

On an 11 GB card the default `qwen3:14b-iq4xs` (IQ4_XS weights) is ~8 GiB — right at
the edge of fitting alongside the desktop. After each Ollama run the daemon checks
`/api/ps` and the notification warns **`⚠ NN% GPU — spilled to CPU`** if layers fell
back to the CPU.

To keep it 100% GPU-resident, enable **q8 KV cache** on the ollama service (near-
lossless, keeps the full context, frees ~0.5 GB) — a systemd drop-in
(`/etc/systemd/system/ollama.service.d/override.conf`, needs sudo):

```ini
[Service]
Environment="OLLAMA_FLASH_ATTENTION=1"
Environment="OLLAMA_KV_CACHE_TYPE=q8_0"
```
then `sudo systemctl daemon-reload && sudo systemctl restart ollama`.

Under heavy GPU pressure (e.g. a running game), route the heavy verbs to cloud
(`vendor = "Anthropic"`) in `policy.toml` instead of fighting for VRAM.

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| No `Fabric` entry when typing `fab ` | launcher not reloaded → `pkill cosmic-launcher` (or log out/in) |
| "No text selected" / nothing runs | the app didn't expose its selection to Wayland PRIMARY. **Terminals (Alacritty):** set `[selection] save_to_clipboard = true` in `~/.config/alacritty/alacritty.toml` — the launcher falls back to the clipboard |
| Notification says *empty result* / error | check `~/.cache/cosmic-fabric/daemon.log` (it logs the run + first SSE events + errors) |
| `⚠ NN% GPU — spilled to CPU` | VRAM pressure — close apps, enable q8 KV (above), or route the pattern to cloud |
| View/Edit notification seems to hang | `notify-send --wait` blocks until you click and COSMIC notifications persist; set `[output] mode = "dialog"` or `"clipboard"` to avoid the wait |
| Daemon won't start | confirm `~/.local/share/cosmic-fabric/cosmic-fabricd` exists and `fabric` is on `PATH`; check `daemon.log`; `pkill cosmic-fabricd` and retry |

Reset the daemon any time: `pkill cosmic-fabricd` (the launcher respawns it).

## Uninstall

```sh
rm -rf ~/.local/share/cosmic-fabric \
       ~/.local/share/pop-launcher/plugins/cosmic-fabric
pkill cosmic-fabricd ; pkill cosmic-launcher
# optional: rm -rf ~/.config/cosmic-fabric ~/.cache/cosmic-fabric
```

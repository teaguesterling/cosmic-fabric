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
| **`fabric`** on `PATH` (`~/.local/bin`) | the engine (patterns + model routing) |
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
launcher; it also makes sure `fabric --serve` is running.

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

**`[ollama]`** — `url` is used now for the GPU-placement check; `bin` is reserved for
the daemon's future model-lifecycle role; `warn_below_gpu` sets the spill-warning
threshold.

## How it works

```
COSMIC launcher ──"fab"──▶ cosmic-fabric-launcher (thin client)
                               │  selection (wl-paste -p)
                               ▼  unix socket (JSON)
                          cosmic-fabricd  ──REST /chat (SSE)──▶ fabric --serve ──▶ Ollama / Anthropic
                               │                                                     │
                               └── /api/ps placement check ◀────────────────────────┘
                               ▼
                          result → launcher → clipboard + View/Edit notification
```

- **`cosmic-fabricd`** owns the deployment: ensures `fabric --serve` is up, holds the
  policy, runs patterns over `POST /chat`, checks GPU placement. Socket:
  `$XDG_RUNTIME_DIR/cosmic-fabric.sock`.
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

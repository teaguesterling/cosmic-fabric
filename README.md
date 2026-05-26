# cosmic-fabric

COSMIC-native, **widget-level** frontends over a single local [fabric](https://github.com/danielmiessler/fabric)
deployment. Not one app — a small bundle of thin surfaces that keep one fabric
deployment's *configuration* coherent: a launcher plugin, a panel applet, a
settings window, and a daemon that ties them together.

**Standalone from [cosmic-goo](https://github.com/…/cosmic-goo).** cosmic-fabric and
goo are siblings: both speak to fabric, neither depends on the other. The launcher
plugin uses pop-launcher's protocol — the same surface a future goo meta-plugin
would target — with zero goo dependency.

📖 **[User manual](doc/manual.md)** · 🏗 **[Design & roadmap](doc/design.md)**

## Quickstart

```sh
cd src && ./install.sh     # user-dir only, no sudo
pkill cosmic-launcher      # reload the launcher
```
Then: **select text** anywhere → open the launcher → type **`fab summ`** → pick
`scribe-summarize` → Enter. The result lands on your clipboard with a View/Edit
notification. Per-pattern model/vendor + output mode live in
`~/.config/cosmic-fabric/policy.toml` (see the [manual](doc/manual.md)).

Requires `fabric` (in `~/.local/bin`), `wl-clipboard`, `python3` ≥ 3.11, `notify-send`;
`zenity`/`cosmic-edit` optional for the result dialog/editor.

## Status

| Phase | What | State |
|---|---|---|
| **0** | pop-launcher plugin prototype (fabric CLI, clipboard/dialog delivery) | ✅ `prototype/` (kept as reference) |
| **1** | `cosmic-fabricd` daemon (REST/SSE, policy, placement) + thin socket-client launcher | ✅ `src/` (current) |
| **2** | `cosmic-fabric-panel` libcosmic applet — status, quick-run with a **streaming** result pane, GPU-spill badge; + a **settings window** (`cosmic-fabric-panel settings`) editing `policy.toml` | ✅ `crates/` (current) |
| **2b** | launcher→panel result handoff (daemon pub/sub broker); context-tier sizing | ⬜ planned |

## License

MIT — see [LICENSE](LICENSE).

## Components (target)

| component | surface | role |
|---|---|---|
| `cosmic-fabric-launcher` | pop-launcher plugin (hotkey) | pick pattern → run on selection → result |
| `cosmic-fabric-panel` | COSMIC panel applet | status · quick-run · recent |
| `cosmic-fabric-settings` | libcosmic window | edit patterns, model mappings, vendors |
| `cosmic-fabric-daemon` | — | fabric lifecycle + the shared `policy.toml` |

# cosmic-fabric

COSMIC-native, **widget-level** frontends over a single local [fabric](https://github.com/danielmiessler/fabric)
deployment. Not one app — a small bundle of thin surfaces that keep one fabric
deployment's *configuration* coherent: a launcher plugin, a panel applet, a
settings window, and a daemon that ties them together.

**Standalone from [cosmic-goo](https://github.com/…/cosmic-goo).** cosmic-fabric and
goo are siblings: both speak to fabric, neither depends on the other. The launcher
plugin uses pop-launcher's protocol — the same surface a future goo meta-plugin
would target — with zero goo dependency.

See [doc/design.md](doc/design.md) for the architecture and phasing.

## Status: Phase 0 — prototype

A working pop-launcher plugin: in the COSMIC launcher, type `fab <pattern>`, pick a
fabric pattern, and it runs that pattern on your current selection (delivered to the
clipboard with a notification preview).

```sh
cd prototype && ./install.sh
# open the COSMIC launcher → type:  fab summ   → pick a pattern (text selected first)
```

Requires `fabric` (in `~/.local/bin`), `wl-clipboard`, and `notify-send`.
Per-pattern model/vendor lives in `~/.config/cosmic-fabric/policy.toml`.

## Components (target)

| component | surface | role |
|---|---|---|
| `cosmic-fabric-launcher` | pop-launcher plugin (hotkey) | pick pattern → run on selection → result |
| `cosmic-fabric-panel` | COSMIC panel applet | status · quick-run · recent |
| `cosmic-fabric-settings` | libcosmic window | edit patterns, model mappings, vendors |
| `cosmic-fabric-daemon` | — | fabric lifecycle + the shared `policy.toml` |

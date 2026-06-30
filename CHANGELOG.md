# Changelog

All notable changes to cosmic-fabric are documented here. This project adheres to
[Semantic Versioning](https://semver.org/).

## [0.2.0] - 2026-06-29

First tagged release. Its defining feature is the **woollama integration**: the
[woollama](https://github.com/teaguesterling/woollama) router becomes
cosmic-fabric's inference *and* prompt-templating backbone, with fabric moving
behind woollama (and retained as an automatic fallback).

### Added
- **woollama inference seam** — plain runs route inference through woollama's
  OpenAI-compatible `/v1/chat/completions`, gated by a `[woollama]` policy block
  (off by default). Transport prefers woollama's owner-only Unix socket, falling
  back to loopback TCP.
- **The daemon owns a woollama instance** — `ensure_serve` discovers a running
  `woollama.service` or spawns a keyless `woollamad`, mirroring how it supervises
  fabric.
- **`/w1` pattern templating** — `WoollamaClient.list_patterns`/`render`/
  `run_pattern[_stream]` against woollama's `/w1` namespace.
- **Stateful chat via woollama** — Claude (claude-code/claude-agent via
  claude-resume) and Local/ollama (conv-store-backed) sessions, driven by
  attach-by-key so woollama owns the durable key→conversation map.
- **Per-run model picker** sourced from woollama's `/v1/models`, plus a Settings
  woollama toggle and a reachability badge. Run UI overhaul (combobox pickers,
  collapse flow, Copy/Send menus).

### Changed
- **Pattern templating now belongs to woollama** (Phase B). The daemon's run
  paths hand woollama `(pattern, variables, model)` and let it render + infer on
  `/w1`; `FAB.assemble_prompt` is gone from the run paths. `assemble` →
  `wool.render`; `patterns`/`surface` → `wool.list_patterns`. Fabric remains the
  automatic fallback (disabled/unreachable/`/w1` error, or a fabric-only request
  feature such as context/strategy/language/search).
- Panel/CLI response shapes are unchanged across the migration.

### Notes
- fabric is still required as the fallback backend; removing it is future work
  (Phase C). With woollama 0.6.0+, woollama can run its own managed `fabric
  --serve` and serve the full pattern library on `/w1`.

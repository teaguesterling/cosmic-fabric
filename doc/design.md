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

## Deferred: launcher → panel result handoff

Goal: the launcher dispatches a run and the **panel** shows the streaming result
(retiring the launcher's `notify-send --wait` hang), with the launcher's
clipboard/notify path kept as the standalone fallback.

Design (build with live eyes — the panel auto-open is the unverifiable part):
- **Daemon broker:** a `{"op":"subscribe"}` op holds a persistent connection;
  a run sent with `"broadcast": true` is run as a stream *and* its
  chunk/done lines are pushed to every subscriber. (Subscriber list + a lock;
  the stream op already exists.)
- **Panel:** a long-lived `Subscription` on `subscribe`; an incoming run opens
  the popup (reuse the `get_popup` path) and feeds the existing
  `RunEvent` handling.
- **Launcher:** a `policy.toml` `[output] mode = "panel"` (opt-in, default
  unchanged so the launcher never regresses) that sends `broadcast` instead of
  running + delivering itself.

Building blocks in place: the daemon `run`-stream op and the panel's `RunEvent`
streaming. Remaining: the broker (socket-testable) + the panel auto-open
(needs an in-panel check).

## Standalone-from-goo

The launcher speaks pop-launcher's line-JSON — the *same surface* a future goo
meta-plugin would target — with zero goo dependency. They coexist on the launcher;
neither needs the other.

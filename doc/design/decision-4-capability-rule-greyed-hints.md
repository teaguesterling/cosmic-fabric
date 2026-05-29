# Design plan — Decision 4: generalize capability rule + greyed "needs key" hints

Status: **drafted 2026-05-29** · sequenced last per [integration-plan.md](../integration-plan.md).
Settled UX: **generalize the capability rule. When a capable instantiation exists,
auto-resolve to it (as vision does today). When none does, the surface shows the
affordance *greyed with a "needs <vendor> key" hint* — not hidden, not silently
failing.** STT = an input source, TTS = a per-artifact "Speak" destination,
image-gen = a response type. All via daemon CLI shell-out (REST has no
multimodal). This is the largest of the four decisions and **factors cleanly
into two phases**: (i) the detection/greyed-hint *framework* (key-free, fully
testable on this box), and (ii) the per-modality plumbing (each needs a real
key to exercise). This plan covers (i) end-to-end and stubs out (ii).

## Why split

The framework is the contribution: it's the reusable mechanism for "this
capability exists / doesn't / is greyed-with-hint" that all three modalities
slot into. Building any one modality end-to-end without the framework gets us
one feature; building the framework first gets us a clean place for all three
and we can land STT/TTS/image-gen incrementally as keys appear, each as a
~50-line slice. On this box (no OpenAI key, no Gemini key), the framework is
deliverable and verifiable; the modality slices are not. Splitting honors the
"deliver what's testable" discipline.

## Phase (i) — the framework

### Vendor → capabilities (the mapping decision)

**Q: where does the mapping "OpenAI provides STT + image-gen, Gemini provides
TTS + image-gen, Anthropic provides text + vision, Ollama provides text +
(vision via model)" come from?**

- **(a) Hardcoded in `core.py`.** A constant dict `_VENDOR_CAPS` pinned to
  fabric's current vendor architecture. Audited when we bump fabric. One line
  to add a vendor.
- **(b) Config-driven.** `policy.toml [vendors.OpenAI] capabilities = [...]`
  per-user. Flexible but the data is fabric's, not the user's — no user has
  reason to deviate.
- **(c) Runtime probe.** Try each cloud endpoint at startup, record what
  responds. Most accurate, but the most code and the most flake (rate-limits,
  network hangs at startup).

**Recommendation: (a) hardcoded constant.** The vendor-capability mapping is a
fixed property of a fabric build, not a per-user configuration; pinning it as
a constant is the honest representation. Auditing on fabric upgrades is one
diff to a small dict. (b) adds config no user benefits from editing; (c) makes
the daemon flaky for a fact that isn't actually dynamic.

```python
# core.py — fabric v1.4.x mapping (audit on upgrade)
_VENDOR_CAPS = {
    "OpenAI":    {"text", "stt", "image_gen"},
    "Anthropic": {"text", "vision"},
    "Gemini":    {"text", "tts", "image_gen", "vision"},
    "Ollama":    {"text"},  # vision is per-model on Ollama
    # … (add as we audit; "text" assumed everywhere; absence of a key = no key)
}
```

**Confirm:** option (a) — hardcoded constant audited on fabric upgrade?

### Detecting *presence* — which vendors are keyed right now

`fabric -L` (`--listmodels`) is the authoritative "what's reachable now" — its
lines are `<vendor>|<model>`. A vendor appears in `fabric -L` iff its key is set
(Ollama appears whenever the server is up). This is also what the user runs to
verify their setup; using the same source means our notion of "available"
matches theirs.

```python
def vendors_present(timeout=5):
    """The set of vendor names that fabric -L lists — i.e. vendors with a key
    set + reachable. Cached for the daemon's lifetime; explicit `refresh` op
    clears it."""
    r = subprocess.run(["fabric", "-L"], stdin=subprocess.DEVNULL,
                       capture_output=True, text=True, timeout=timeout)
    if r.returncode != 0:
        return set()
    seen = set()
    for line in r.stdout.splitlines():
        # lines look like:  "       \t[14]\tAnthropic|claude-sonnet-4-6"
        if "|" in line:
            vendor = line.rsplit("\t", 1)[-1].split("|", 1)[0].strip()
            if vendor:
                seen.add(vendor)
    return seen
```

The model-side bug noted in passing: `core.model_catalog()` currently calls
`self._get("/models/names")` and expects `{"vendors": {...}}`, but the live
endpoint returns `{"models": [...]}` (flat). Fixing that is a related cleanup —
the same `vendors_present` parse over `fabric -L` is a more honest catalog
anyway. Out of scope for *this* plan but linked: log it as a follow-up.

### The capability ledger — one source of truth, served by the daemon

```python
def capability_status():
    """{capability: {"available": bool, "via": [vendor,...] or [],
                     "hint": "needs an OpenAI key — run `fabric --setup`"}}
    Combines vendor presence with the vendor→caps map; also walks the user's
    declared models for `vision` (Ollama vision is model-level, not vendor-
    level). Cached behind `vendors_present`."""
    have = vendors_present()
    out = {}
    for cap in ("vision", "stt", "tts", "image_gen"):
        via = [v for v, caps in _VENDOR_CAPS.items() if cap in caps and v in have]
        # vision: also count a configured Ollama model with `capabilities=["vision"]`
        if cap == "vision":
            via += _ollama_models_with_capability("vision", policy)
        out[cap] = {
            "available": bool(via),
            "via": via,
            "hint": _hint_for(cap, have),  # "needs an OpenAI key — run `fabric --setup`"
        }
    return out
```

New daemon op: `{"op": "capabilities"}` → that dict. Cheap; cached for the
daemon's lifetime; a `refresh: true` arg clears the cache.

### The greyed-hint widget

A small reusable Rust helper (`crates/cosmic-fabric-panel/src/cap.rs`):

```rust
pub struct CapStatus { pub available: bool, pub hint: String, pub via: Vec<String> }

/// A button-ish affordance that's enabled when the capability is available,
/// greyed with a tooltip+caption when it isn't. Wraps button::standard.
pub fn cap_btn<'a, M: 'a + Clone>(label: &str, status: &CapStatus, on: M) -> Element<'a, M> {
    if status.available {
        button::standard(label).on_press(on).into()
    } else {
        button::standard(format!("{label}  ({})", status.hint))   // visible hint
            // no .on_press → disabled
            .into()
    }
}
```

Where it slots:
- **Loom source picker** — STT entry uses `cap_btn("🎤 Audio", caps.stt, …)`.
- **Per-artifact send-to** — `cap_btn("Speak", caps.tts, …)` as a destination.
- **Pattern/response area** — when a pattern is an image-gen pattern, the
  response area uses `cap_btn` (or a banner equivalent) gated on `caps.image_gen`.

### Generalize the resolver to use the ledger

`core.resolve(pattern, pol, need_capability=…)` today calls `_first_capable` if
the resolved instantiation can't satisfy the capability. Extend `_first_capable`
to consider **vendor-level capabilities** in addition to model-declared ones —
if `need_capability == "stt"` and any keyed vendor lists `stt`, the daemon
shells out via a vendor-appropriate model from that vendor instead of needing
the user to have declared an STT model in `policy.toml`. That eliminates the
"you have to declare a vision model in policy" friction for cloud-only
capabilities the user hasn't pre-configured.

For vision it stays model-level (user's Ollama vision model is the *preferred*
choice; vendor-level fallback to Anthropic is a backup).

### Tests (framework, key-free)

- `test_vendors_present_parses_listmodels`: feed a captured `fabric -L` stdout
  to a `_parse_listmodels(stdout)` helper; assert vendor set.
- `test_capability_status_with_no_keys`: `vendors_present` returns `{"Ollama"}`
  → `caps.stt.available is False`, `caps.tts.available is False`, hints
  populated.
- `test_capability_status_with_openai_keyed`: `vendors_present` returns
  `{"Ollama","OpenAI"}` → `caps.stt.available is True`, `via == ["OpenAI"]`.
- `test_vision_still_prefers_local`: an Ollama-vision model declared in
  policy + Anthropic keyed → vision `via` lists Ollama first (`_first_capable`
  preserves the local-first ordering for vision).

Live sanity: on this box (no cloud keys), open the loom — STT/TTS/image-gen
affordances should be visible and greyed with hints; vision should be enabled
(local Ollama vision model declared).

## Phase (ii) — the per-modality plumbing (sketch)

Each is a ~50-line slice once Phase (i) is in. None deliverable without a key
on this box; included here so the framework's shape is honest about its uses.

### STT — input source

- New `Origin::Audio` (file picker → mic record later).
- Daemon op: `{op: "transcribe", path}` → shells `fabric --transcribe-file
  <path>` with `</dev/null`, captures stdout. Capability rule picks the keyed
  vendor (OpenAI's `whisper-1`). Output text feeds back as the prompt input.
- Loom UI: a "🎤 Audio" entry in the source picker, greyed when `caps.stt.available
  is False`. Same widget as URL — pick a file, fetch, replace input.

### TTS — per-artifact "Speak" destination

- Send-to dropdown gains a "Speak" entry per artifact (prompt / response /
  conversation).
- Daemon op: `{op: "speak", text}` → shells `fabric` with the TTS pattern +
  Gemini vendor; writes audio to a tmp file; daemon plays it (`paplay`/`aplay`)
  or returns the path for the panel to play. *Decision deferred to the slice.*
- Greyed when `caps.tts.available is False`.

### Image-gen — response type

- A pattern declared `category: "image-gen"` (or a heuristic on the pattern
  name — TBD in the slice) routes the response into an image renderer instead
  of a text view.
- Daemon op: extends `run` with `output_kind: "image"` — when set, daemon
  shells `fabric -m <vendor-image-model> …` with image-gen output mode; writes
  PNG to a tmp file; returns path.
- Response area: a `cap_btn` banner when `caps.image_gen.available is False`
  explaining what would happen with a key.

These are sketches. They are not "do them now" — they are "the framework leaves
room for each, sized."

## What does NOT change

- The 3-surface model. Capabilities surface where they're already shaped to —
  STT into the loom's existing source picker, TTS into the existing send-to,
  image-gen into the existing response area.
- The vision capability rule for existing image runs. The user-declared
  `llama-vision` model still wins via `_first_capable`'s Ollama-first ordering.
- Settings UI for keys. Keys remain a fabric concern (`fabric --setup`); the
  hint *teaches* that command rather than wrapping it. (We do not become a
  key-management UI.)

## Open implementation question (the only Phase-(i) fork)

The vendor → capability mapping source (a/b/c above); see the
**Recommendation** there.

## Migration / rollout

- No file-format break: `_VENDOR_CAPS` is code, `capabilities` op is additive,
  the greyed-button widget reuses existing button::standard.
- Daemon protocol additive: `{op: "capabilities"}` new; nothing else changes.
- Deploy same as the other decisions. The Rust panel needs a rebuild.

## Definition of done — Phase (i)

1. `core._VENDOR_CAPS` constant + `vendors_present()` parser of `fabric -L`.
2. Daemon `capabilities` op returns the ledger; cached, with a `refresh` flag.
3. `cap_btn` helper in the Rust panel; greyed + hint when unavailable.
4. Existing vision behavior unchanged (still local-first via `_first_capable`).
5. Tests pass (the four listed above).
6. Live: on this box, STT/TTS/image-gen affordances render greyed with hints;
   vision affordance is enabled; `cosmic-fabric` CLI gains a `capabilities`
   subcommand printing the ledger.

## Definition of done — Phase (ii)

Tracked as separate slices, one per modality, each landing when a key for the
relevant vendor is available to test against. Not blocking Phase (i).

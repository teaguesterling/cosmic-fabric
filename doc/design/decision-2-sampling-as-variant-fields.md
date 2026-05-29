# Design plan — Decision 2: sampling knobs as variant fields

Status: **drafted 2026-05-29** · sequenced first per [integration-plan.md](../integration-plan.md)
("no-regret first"). Settled UX: **variant fields only, no per-run sliders.**
This plan goes from "settled UX" to a concrete code/schema/UI delta. One open
implementation question at the end (the only real fork).

## Scope (one paragraph)

Extend the existing two-level `Model → Variant` schema with three more numeric
deployment knobs — `topP`, `frequencyPenalty`, `presencePenalty` — sitting next to
the `ctx`, `thinking`, `temperature` already on `Variant`. Wire them through the
Python instantiation resolver, the daemon-side options builder, and the Rust
Models editor in the loom. Out of scope: per-run sliders (rejected), `seed`
(future CLI flag, not a variant field), and any other ChatOptions field.

## Why these three, why no others

REST `ChatOptions` (verified shape) lists `temperature, topP, frequencyPenalty,
presencePenalty, thinking, search, searchLocation`. Of these:
- `temperature`, `thinking` — **already** variant fields.
- `search`, `searchLocation` — *behavior selector*, not a deployment knob; lives
  as a per-run flag (`--search` shipped in `d12cda9`), not on a variant.
- `topP`, `frequencyPenalty`, `presencePenalty` — **the gap**: pure sampling
  knobs that, like `temperature`, characterize *how this deployment behaves*.

So the field set this plan adds is exactly the gap. Nothing more.

## Schema delta

### Rust (`crates/cosmic-fabric-panel/src/policy.rs`)

```rust
pub struct Variant {
    pub ctx: Option<u32>,
    pub thinking: Option<String>,
    pub temperature: Option<f32>,
    // ↓ new — same Option<f32> + skip_serializing_if shape as `temperature`
    pub top_p: Option<f32>,
    pub frequency_penalty: Option<f32>,
    pub presence_penalty: Option<f32>,
    pub extra: Vec<String>,
    pub categories: Vec<String>,
}
```

- **TOML key style: snake_case** (`top_p`, `frequency_penalty`, `presence_penalty`)
  — matches TOML convention and the existing `warn_below_gpu` in `OllamaCfg`. No
  serde rename needed; field name *is* the TOML key.
- **`Option<f32>` with `skip_serializing_if = "Option::is_none"`** — `None` = "use
  the model default, don't send the field." Mirrors the existing `temperature`.
- **Backward compat:** old policies parse fine — `#[serde(default)]` on each
  Option means missing keys deserialize as `None`. Saved policies that *don't*
  set the fields stay byte-identical (skip-serializing).

### Python (`src/core.py`)

```python
# was:
_VARIANT_PARAMS = ("ctx", "thinking", "temperature", "extra")
# now:
_VARIANT_PARAMS = ("ctx", "thinking", "temperature", "top_p",
                   "frequency_penalty", "presence_penalty", "extra")
```

That single line change feeds the new fields through `_resolve_use` →
`_inst_of` → `resolve` automatically. Then in `resolve()`'s normalized return
dict, add the three keys (mirroring the existing `"temperature": inst.get(...)`
line). In `inst_to_options()`, emit them in fabric's JSON casing:

```python
# in inst_to_options(inst), after the temperature branch:
for src, dst in (("top_p", "topP"),
                 ("frequency_penalty", "frequencyPenalty"),
                 ("presence_penalty", "presencePenalty")):
    v = inst.get(src)
    if v is not None:
        try: opt[dst] = float(v)
        except (TypeError, ValueError): pass
```

The case asymmetry (snake_case in policy/TOML/Python, camelCase to fabric REST)
is intentional and isolated to this one mapping — same shape as `suppressThink`
(emitted camelCase, set internally on the `thinking="off"` branch).

### Backward-compat: `extra` CLI passthrough

`core.extra_to_options` translates legacy `extra = ["--temperature=0.4"]` flags
into REST fields. Extend it to also parse `--topp=`, `--presencepenalty=`,
`--frequencypenalty=` — fabric's own CLI flag names (verified from `fabric -h`).
Existing policies using `extra` keep working; new policies use the typed fields.

## UI delta — Models editor (`crates/cosmic-fabric-panel/src/workspace.rs`)

The Models editor today is **add-only** for variants: a single row with
`av_name` / `av_ctx` / `av_thinking` inputs and an "Add variant" button. The
existing variant *display* (line ~940) renders parts like `ctx 2048 · think off
· temp 0.4` from the Variant struct, but there's no way to **edit** the values
on an existing variant — `temperature` today can only be set by hand-editing
`policy.toml`.

Adding three more knobs forces the question this design plan has to answer:

### Open implementation question — variant edit affordance

**Q: how does the user *change* a variant's knobs after creation?**

- **(a) Inline editable row.** Each variant row becomes 4 small `text_input`s
  (`ctx`, `temp`, `topP`, `freqPen` — or expand to all six) that commit on
  Enter/blur; thinking stays a small dropdown. Compact, no modal, mirrors how the
  active-set ★ toggles directly. Adds ~6 fields per row of state to track in
  `Workspace`.
- **(b) "Edit" expander.** Each row gets a chevron that toggles a sub-panel with
  full inputs; the row itself stays a compact summary chip. Visually quieter,
  but adds a new interaction primitive (expansion state) the editor doesn't
  currently use.
- **(c) Extend the add row to six inputs, keep variant display read-only.**
  Add-only stays add-only; to change a knob you delete + re-add. Cheapest code,
  but treats variants as immutable for a mutable concept — fails "configure
  behavior" because the affordance to *configure* the existing thing is missing.

**Recommendation: (a) inline editable row.** Variants are small (≤6 numeric
fields, all `Option<f32>`/`Option<u32>` except thinking); the row is naturally
wide enough; "edit in place" matches the directness of the active-set ★ toggle
and the per-pattern model dropdown elsewhere in the loom. (b) is over-built for
six fields; (c) makes the new knobs unusable.

(a) does mean the add-variant row collapses into "+ new variant" → name input
only, and configuration moves to the row itself. Cleaner mental model: a
variant is a row, the row has its knobs; "add" is just "make a new empty row."

**Confirm:** option (a) — inline-editable variant rows, with "+ new variant"
becoming name-only?

## Wire format summary (where the names live)

| layer | name | example |
|---|---|---|
| Rust struct field | `top_p` | `Option<f32>` |
| `policy.toml` key | `top_p` | `top_p = 0.9` |
| Python instantiation dict | `"top_p"` | matches TOML |
| fabric REST `ChatOptions` JSON | `topP` | emitted by `inst_to_options` |
| fabric CLI flag (legacy `extra`) | `--topp=` | parsed by `extra_to_options` |

## Tests

Add to `src/test_core.py`:

- `test_inst_to_options_emits_sampling_knobs`: a synthetic inst with
  `{top_p: 0.9, frequency_penalty: 0.1, presence_penalty: 0.2}` yields exactly
  `{topP: 0.9, frequencyPenalty: 0.1, presencePenalty: 0.2}` (plus whatever the
  thinking/temperature branches add).
- `test_inst_to_options_skips_none`: same inst with all three `None` ⇒ none of
  the three keys appear (so a user who hasn't set them gets the model's defaults).
- `test_extra_to_options_legacy_flags`: `extra=["--topp=0.85","--presencepenalty=0.1"]`
  yields `{topP: 0.85, presencePenalty: 0.1}`.
- `test_resolve_carries_sampling_through_variant`: a policy with a variant
  declaring `top_p = 0.8` and a pattern using `model/v` → `resolve(...)["top_p"]
  == 0.8`.

Rust side: extend `real_policy_loads_and_round_trips` (already in
`policy.rs::tests`) with a model+variant that sets all three knobs and confirm
TOML round-trip.

## Migration / rollout

- No file-format break: new fields are optional.
- No daemon protocol break: the daemon already forwards `options` from the
  resolved instantiation into `body`.
- Deploy: same path as `d12cda9` — `cp src/core.py src/cosmic-fabricd
  ~/.local/share/cosmic-fabric/`, restart daemon by PID, rebuild + reinstall the
  panel (`cd crates && just install`).

## Definition of done

1. Rust `Variant` has the three new fields; `policy.toml` round-trips them.
2. Python `_VARIANT_PARAMS` / `resolve` / `inst_to_options` carry them and emit
   the camelCase ChatOptions.
3. `extra_to_options` recognizes the three legacy `--topp=` / `--presencepenalty=`
   / `--frequencypenalty=` flags.
4. Models editor: variant rows are inline-editable for the six knobs (assuming
   option (a) above is confirmed).
5. New unit tests pass; existing tests still pass.
6. Live sanity: a variant declaring `top_p = 0.5` on a local Ollama model runs
   without error; an `assemble` of the same pattern is unchanged (sampling
   doesn't affect the assembled prompt).

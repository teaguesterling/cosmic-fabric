# cosmic-goo ↔ cosmic-fabric integration (draft)

> Status: draft. The cosmic-fabric side (the daemon ops) is built + tested; the
> cosmic-goo side (the `fabric` route) lands when goo's route layer is built
> (goo:// is designed-not-built — see cosmic-goo `doc/design/`).

## One channel

`cosmic-fabricd` owns the one fabric deployment. cosmic-goo does **not** talk to
fabric directly — it's a **client of the daemon socket**, exactly like
cosmic-fabric's own launcher/panel. goo's `Using: goo://channel/fabric` maps onto
the daemon (a protocol match, not a custom bridge). Dependency is one-way
(goo → daemon, optional); cosmic-fabric stays goo-independent.

## fabric makes prompts; goo routes (including to Claude)

The division that resolves "can we send to Claude Desktop?":

- **fabric / `cosmic-fabricd`** does two things with a `scribe-*` pattern:
  - **run** it → execute against Ollama/Anthropic, return/stream the *result*.
  - **assemble** it → render the prompt (system + input, `{{vars}}` substituted)
    **without executing** — `GET /patterns/:name` gives the system prompt.
- **cosmic-goo** is the router. It already owns the `claude://` hand-off routes
  (Claude Desktop / Code). "Send to Claude" = **fabric assembles the prompt →
  goo opens Claude seeded with it.** cosmic-fabric never sends to Claude itself.

So the `scribe-*` patterns are **dual-use**: run locally on iq4xs, *or* launch
into a full Claude conversation — same pattern, different destination.

## Two fabric sub-channels: `inference` and `assemble`

fabric exposes **two capability sub-channels** under its domain — `Using:` picks
*what fabric produces*, `To:` picks *where it goes* (orthogonal, any combination):

- **`goo://channel/fabric/inference`** *(default for bare `goo://channel/fabric`)*
  → daemon **`run`** (+ stream / broadcast). Produces a **result**.
- **`goo://channel/fabric/assemble`** → daemon **`assemble`**. Produces a
  rendered **prompt** (no model run).

```
SUMMARIZE      goo://sel/  Using: fabric/inference                 # run → result back
SUMMARIZE      goo://sel/  Using: fabric/inference  To: goo://chat/notes   # result → a sink
DRAFT-RESPONSE goo://sel/  Using: fabric/assemble   To: claude://desktop   # prompt → open Claude
DRAFT-RESPONSE goo://sel/  Using: fabric/assemble   To: goo://clip/        # prompt → clipboard
SUMMARIZE      goo://sel/  Using: fabric/inference  To: claude://desktop   # result → seed Claude as context
```

| `Using:` | produces | `To:` (any sink/agent) |
|---|---|---|
| `fabric/inference` *(default)* | a **result** | return · clipboard · panel · chat · an agent (as context) |
| `fabric/assemble` | a **prompt** | claude://desktop/code · clipboard · file |

`OPTIONS goo://channel/fabric` lists the sub-channels (and their ops). The
verb may declare a *default* sub-channel (e.g. `draft-response` → `assemble`,
`summarize` → `inference`); otherwise bare `fabric` = `inference`.

**No daemon change:** the sub-channels are pure goo addressing —
`inference`→`run`, `assemble`→`assemble` — over the ops cosmic-fabricd already has.

## Daemon ops goo uses (all on the unix socket, line-JSON)

| op | for | status |
|---|---|---|
| `{"op":"run","pattern","input"[,"stream","broadcast"]}` | execute → result (optionally stream / broadcast to the panel) | ✅ built, tested |
| `{"op":"assemble","pattern","input","variables"}` → `{"prompt":...}` | render the prompt for an agent hand-off, **no model run** | ✅ built, tested |
| `{"op":"patterns"}` / `{"op":"models"}` / `{"op":"status"}` | discovery for goo's verb/adverb surfacing | ✅ built |

## What cosmic-goo needs to build (its side)

1. A **`fabric` channel handler** (the `Using: goo://channel/fabric` target) — a
   thin client of the daemon socket.
2. **Map the two sub-channels to daemon ops** — `fabric/inference` → `run`
   (+ stream / `broadcast` to the panel); `fabric/assemble` → `assemble`. Then
   send the produced output to `To:` **uniformly** (claude:// hand-off,
   clipboard, file, panel) — `To:` is just the destination, not a mode switch.
3. Surface the `scribe-*` verbs (from `{"op":"patterns"}`) + their adverbs
   (`{{lang}}`/`{{depth}}`/… → `With:`/`-v`) in goo's grammar.

goo already has: the `claude://` hand-off routes, the verb/object grammar. It
*stops* reimplementing fabric access — the daemon is the channel.

## The route shape

A thin daemon-socket client. v1 (bash routes) ships it as a helper; v2
(goo-engine, Rust) makes the same socket calls natively:

```
goo-fabric <inference|assemble> <pattern> [--var k=v …]   < input(stdin)   → output(stdout)
            inference → daemon {"op":"run", ...}        assemble → daemon {"op":"assemble", ...}
```

Per invocation, goo maps:

| goo | → fabric / daemon |
|---|---|
| verb (`summarize`) | `pattern` = the verb's `fabric_pattern` (`scribe-summarize`) |
| `Using: …/fabric/{inference\|assemble}` | which op |
| adverbs (`depth=ultra`) → `With:` | `variables = {depth: ultra}` → `{{depth}}` in the pattern |
| subject (`goo://sel/`) | `input` (text) |
| `To:` | where `output` goes (clipboard / `claude://` / panel) |

## Friction / open questions (surfaced while designing the route)

Ordered by how much they could force design changes.

1. **"References, not data" vs fabric wants *text* (foundational).** Subjects are
   locators, but `run`/`assemble` take a string `input`. `sel`/`clip`/`text` are
   already text; **`file`/`url`/`pdf`/`image` are not** (extract / fetch / OCR).
   Either goo resolves to `.text` first, or the daemon grows typed-input handling
   (url → fabric `-u`, file → read, `-a` attachment). *Lean: v1 fabric verbs take
   text subjects only; url/pdf/image later.*
2. **`claude://` can't carry a big assembled prompt** — *resolved by design
   (deferred).* `?q=` would seed a user message and hit URL-length limits. The
   fix: a **content-staging layer** (internal buffer / tmpfile / upload) holds the
   payload, and the agent launch (`claude://`, etc.) carries a **reference** to
   the staged content, not the inline prompt — i.e. "references, not data" applied
   to hand-offs. Future work; the *same* staging layer also covers #1's
   large/non-text inputs.
3. **A channel-agnostic verb is carrying fabric-specific config (architectural).**
   `fabric_pattern` + default sub-channel + adverb-names-that-match-`{{vars}}` are
   fabric coupling on a neutral verb. *Lean: a `[verb.channels.fabric]` block,
   separate from the verb's neutral identity.*
4. **Two sub-channels don't map onto v1's flat `via` adverb (v1/v2 impedance).**
   The `goo://channel/fabric/{inference|assemble}` path has no home in
   `--via=fabric`; v1 needs `--via=fabric --mode=assemble` or `via` values
   `fabric`/`fabric-assemble`. Clean only in v2.
5. **"Produce then route" is a two-step the v1 single-command model resists.**
   `To:` routing output (result → clipboard; prompt → `claude://`) is a pipeline;
   v1 routes were single-step. It's the spec's `Destination:` / two-step verb,
   expressed as a pipe in v1.
6. **Streaming / broadcast don't fit a synchronous route.** `run`-stream and
   `broadcast`-to-panel are async; a v1 template is synchronous. `To: panel`
   (broadcast) vs `To: clipboard` (run) take *different* ops — the route branches
   on `To:`, it isn't one uniform template.
7. **Implicit adverb-name ↔ pattern-variable-name contract.** `depth=ultra` works
   only because `scribe-think` uses `{{depth}}`; a rename silently breaks.
   *Needs an explicit adverb→var declaration on the verb.*

**Load-bearing:** #2 is resolved by design (the deferred staging layer), which
also absorbs #1's hard cases — so the remaining *foundational* call is just how
much of subject→text goo does before that layer exists (lean: text subjects in
v1). #3 (verb/channel coupling) is the architectural one to get right early.
#4–7 are mechanical (encoding choices, not blockers).

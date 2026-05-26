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

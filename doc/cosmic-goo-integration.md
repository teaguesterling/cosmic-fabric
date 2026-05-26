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

## The `To:` destination decides run-vs-assemble

```
SUMMARIZE      goo://sel/  Using: fabric                          # run → result back
SUMMARIZE      goo://sel/  Using: fabric  To: goo://chat/notes    # run → result to a sink
DRAFT-RESPONSE goo://sel/  Using: fabric  To: claude://desktop    # assemble → open Claude with the prompt
```

| `To:` is a… | examples | daemon op | goo then |
|---|---|---|---|
| **sink** (passive) | return, clipboard, panel, a notes chat | **`run`** / `run`-stream | deliver the *result* |
| **agent** (executes) | claude-desktop, claude-code, alpaca | **`assemble`** | open it seeded with the *prompt* |

sink-vs-agent is a property the `To:` handler/domain declares.

## Daemon ops goo uses (all on the unix socket, line-JSON)

| op | for | status |
|---|---|---|
| `{"op":"run","pattern","input"[,"stream","broadcast"]}` | execute → result (optionally stream / broadcast to the panel) | ✅ built, tested |
| `{"op":"assemble","pattern","input","variables"}` → `{"prompt":...}` | render the prompt for an agent hand-off, **no model run** | ✅ built, tested |
| `{"op":"patterns"}` / `{"op":"models"}` / `{"op":"status"}` | discovery for goo's verb/adverb surfacing | ✅ built |

## What cosmic-goo needs to build (its side)

1. A **`fabric` channel handler** (the `Using: goo://channel/fabric` target) — a
   thin client of the daemon socket.
2. **Route by `To:`**: agent destinations → `assemble` then the existing
   `claude://` (and clipboard/code) hand-off, seeding `q=<prompt>`; sink / no
   destination → `run` (or `run`-stream / `broadcast` to the panel).
3. Surface the `scribe-*` verbs (from `{"op":"patterns"}`) + their adverbs
   (`{{lang}}`/`{{depth}}`/… → `With:`/`-v`) in goo's grammar.

goo already has: the `claude://` hand-off routes, the verb/object grammar. It
*stops* reimplementing fabric access — the daemon is the channel.

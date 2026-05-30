# Tool-calling plan — three axes + a config-callbacks architecture

Status: **proposal drafted 2026-05-29.** Follow-on to
[`tool-calling-backends.md`](tool-calling-backends.md) — that doc mapped the
option space; this doc commits the architectural shape on the user-supplied
framing. **Not a settled `decision-N` doc yet** — it ends with an explicit
confirmation question on the three axes.

## What's already settled (re-stating so this doc stands alone)

- *Local execution* (user-confirmed): tools run on this box; vendor
  server-side execution paths (Anthropic Remote MCP) are out of scope.
- *fabric stays unadulterated*: the daemon owns the tool loop, bypassing
  fabric for tool-using runs. fabric continues to handle plain text patterns
  via REST as today, with zero changes.
- *Ollama is phase 1's model backend.* Live-tested across nine scenarios on
  qwen3:14b-iq4xs; one failure mode found (empty tool result →
  hallucination — addressed in Axis 2 below).
- *Anthropic-in-the-loop is a phase 2 question, not in scope here* (user said
  "I want to know what Ollama end to end looks like first"). The architecture
  below should not preclude it, but doesn't require it.

## The three axes (the user's framing)

> 1. Where do tool *definitions* come from?
> 2. Where do tools get *executed*?
> 3. What is the *feedback* process to the model?
>
> "*The entire thing could be done through config and callbacks while
> keeping fabric largely unadulterated.*"

The user's hint is the through-line. The proposal: **definitions in config,
behavior in callbacks, loop in the daemon, fabric untouched.** Each axis
below resolves with that pattern in mind.

---

## Axis 1 — Where do tool definitions come from?

**Recommendation: three layers, additive, resolved in this priority order:**

1. **Code-defined built-ins** (lowest layer, always present). A small
   registry in `core.py`:

   ```python
   _BUILTIN_TOOLS = {
       "http_get":           ToolSpec(schema=…, run=_http_get_safe,         mode="daemon"),
       "read_file":          ToolSpec(schema=…, run=_read_file_cwd_jailed,   mode="daemon"),
       "run_shell_confirmed":ToolSpec(schema=…, run=_run_shell_via_panel,    mode="panel-confirm"),
   }
   ```

   These are the three baseline tools the design has been written around.
   New built-ins land as code review just like any feature; they're under our
   security model and ship with the daemon.

2. **Config-defined tools** (`policy.toml` `[tools.<name>]` sections,
   user-editable). Two flavors:

   ```toml
   # (a) Parameterizing a built-in
   [tools.http_get]
   allow_domains = ["en.wikipedia.org", "duckduckgo.com"]

   # (b) Declaring a new tool that points at a shell command. The daemon
   #     handles arg substitution + result capture; the user owns the
   #     security of the command line.
   [tools.git_log]
   schema = """
   { "name":"git_log",
     "description":"Show recent git commits in the current repository.",
     "parameters":{"type":"object","properties":{
       "n":{"type":"integer","description":"How many commits"}},"required":["n"]} }
   """
   exec   = "git log -n {n} --oneline"
   mode   = "panel-confirm"   # or "daemon" if obviously safe
   ```

   Flavor (a) is the common case for built-ins: configure their behavior.
   Flavor (b) is the extension hatch — declarative additions without code
   changes, security via mode-choice. The user can grow their tool surface
   by editing `policy.toml` and restarting the daemon.

3. **MCP-discovered tools** (future, Phase 3 of the backends doc — kept off
   the critical path here). The daemon connects to a configured MCP server
   at startup, asks for its tool list, and registers each as a third source.
   Same `ToolSpec` shape internally; the `run` callback becomes "invoke this
   MCP server's tool over JSON-RPC."

**Pattern → tools binding** lives in pattern frontmatter (which the daemon
already reads):

```yaml
tool_use: true
tools: [http_get, git_log]      # which registered tools this pattern may use
# (absent `tools` = any registered tool; explicit list = least-privilege)
```

The daemon resolves names against the layered registry; an unknown name is
an error surfaced at pattern-load time, not mid-run.

### Why this answers Axis 1 well

- Built-ins are auditable code; config extends without code; MCP scales
  through ecosystem. Each layer pays its own way.
- The pattern-level allow-list is least-privilege — `scribe-look-it-up` only
  needs `http_get`; it doesn't get `run_shell_confirmed` for free.
- Config errors fail at startup with a clear message, not at the first run.

---

## Axis 2 — Where do tools get executed?

**Recommendation: three execution modes, declared per tool:**

| mode | who runs it | for what kind of tool |
|---|---|---|
| `daemon` | in-process Python callback | safe built-ins: `http_get` (allow-listed), `read_file` (cwd-jailed), JSON/text manipulation, computation |
| `panel-confirm` | daemon emits a confirm event → panel shows command + Approve/Deny → daemon executes on Approve | side-effect tools: `run_shell_confirmed`, file writes, anything mutating |
| `mcp` (future) | MCP server (separate process) | Phase 3; same daemon orchestration, separate execution boundary |

**The daemon owns the executor.** It enforces the mode declared in
`ToolSpec`, gates on the panel for `panel-confirm`, and applies four
hygiene rules to **every** result before feeding back to the model — these
came directly from the live tests:

```python
def _sanitize_tool_result(name, raw, schema_ok) -> str:
    """Daemon-side hygiene before feeding a tool result back to the model.
    The H failure (empty result → hallucinated email) lives here."""
    if raw is None or (isinstance(raw, str) and not raw.strip()):
        return f"TOOL {name} RETURNED NO DATA"                # ← H fix
    s = str(raw)
    if len(s) > 50_000:                                       # truncation cap
        return s[:50_000] + f"\n[truncated; original {len(s)} chars]"
    return s

def _execute_tool(name, args, spec) -> str:
    if not _validate_args(spec.schema, args):                 # ← E branch
        return f"SCHEMA ERROR: args don't match {name}'s schema: {args}"
    try:
        raw = spec.run(args)                                  # daemon callback
    except Exception as e:                                    # ← D branch
        return f"ERROR: {type(e).__name__}: {e}"
    return _sanitize_tool_result(name, raw, True)
```

Each branch matches a tested failure mode:
- **D** (tool raises): caught, wrapped, sent back as a recoverable string.
  Live test confirmed qwen3 surfaces this gracefully.
- **E** (schema mismatch): daemon validates against the tool's JSON Schema
  before executing; rejects with an explicit error the model can correct
  against. Not exercised by qwen3 in tests (it got args right) but the path
  has to exist for weaker models / harder schemas.
- **H** (empty result → hallucination): substituted with an explicit
  sentinel. **This is the most important hygiene rule** — the model can't
  recover from an empty string but does fine with `TOOL X RETURNED NO DATA`.

### `panel-confirm`: the UI handshake

Daemon, mid-loop, receives a `tool_call` for a `panel-confirm` tool. It:

1. Pauses the loop (doesn't call Ollama again yet).
2. Emits a `tool_confirm_required` event over the socket:
   `{name, args, command_preview, request_id}`.
3. The panel renders a modal/popover: "**Run shell command?** `git log -n 5
   --oneline`" + Approve/Deny.
4. Panel sends back `{op:"tool_confirm", request_id, approved: true/false}`.
5. Daemon executes if approved, returns `"USER DENIED EXECUTION"` if not.

No "trusted patterns" bypass. The confirm flow is the security boundary;
making it skippable per pattern would turn the bypass flag into the
injection target. Approve/Deny is the only way through.

### Why this answers Axis 2 well

- Three modes is the smallest set that covers the cases honestly. Adding
  `subprocess` mode (forking a child process for isolation) is a small
  later step if some tool warrants it; not yet motivated.
- Tool-result hygiene is centralized, not scattered. The four rules above
  apply to every callback's output.
- The security model is *declared* per-tool, not inferred. A `panel-confirm`
  tool whose author forgot to mark it as such will run silently — that's
  the kind of bug the declared mode prevents.

---

## Axis 3 — What is the process for feeding back to the model?

**Recommendation: daemon owns the loop end-to-end for tool-using runs;
fabric is bypassed entirely on that path.**

```
                       ┌─ pattern frontmatter ───────┐
                       │  tool_use: true              │
                       │  tools: [http_get, git_log]  │
                       └──────────────┬───────────────┘
                                      │
       cosmic-fabricd run op ─────────┤
       branches on tool_use           │
                                      ▼
              ┌──────────────────────────────────────────────┐
              │ DAEMON TOOL-LOOP                             │
              │                                              │
              │ 1) core.assemble_prompt(pattern, input, vars)│ ← already exists
              │ 2) messages = [system, user]                 │
              │ 3) loop until N turns or no tool_calls:      │
              │      POST localhost:11434/api/chat           │
              │        {model, messages, tools=[…]}          │
              │      ← message (content, tool_calls)         │
              │      stream `content` chunks → panel         │
              │      for each tool_call:                     │
              │        emit tool_call event → panel          │
              │        result = _execute_tool(name, args)    │ ← Axis 2
              │        emit tool_result event → panel        │
              │        messages.append(assistant, tool)      │
              │ 4) return final assistant text               │
              └──────────────────────────────────────────────┘

   fabric: not in this diagram. (Plain text patterns still go through
   fabric's REST /chat unchanged. The branch is a pattern-frontmatter check.)
```

**This is what "config and callbacks, fabric unadulterated" cashes out as:**
fabric is the path for plain patterns; the daemon is the path for tool
patterns. The `run` op's signature doesn't change, but its body now has two
branches. Surfaces don't have to know — they get back the same final text
plus richer streaming events.

### Loop control

- **Hard cap on turns**: default `N=8`. Configurable per pattern via
  frontmatter (`tool_max_turns: 12`) or per-policy in `[tools] max_turns`.
- **No wall-clock cap by default.** Long tools are legitimate (a 60s
  `http_get` is fine). User can set one if they want.
- **Termination = model emits zero `tool_calls`.** Live testing confirmed
  qwen3 reliably terminates; the cap is a safety net, not the normal path.
- **Token budget cap (phase 2 polish, not phase 1).** When we add Anthropic,
  the daemon should track cumulative input/output tokens and abort with a
  clear message if a configurable budget is hit.

### Streaming + observability

The Rust panel already receives `RunEvent::Chunk(s)` / `Done` / `Error`. We
add:

```rust
pub enum RunEvent {
    Chunk(String),
    ToolCall { name: String, args: serde_json::Value, id: String },  // NEW
    ToolResult { name: String, id: String, summary: String },         // NEW
    ToolConfirmRequired { name: String, args: serde_json::Value,      // NEW
                           command_preview: Option<String>, id: String },
    Done(RunMeta),
    Error(String),
}
```

The `session.rs` surface renders `ToolCall` / `ToolResult` as inline pills;
`workspace.rs` renders them as a collapsed trace section. The daemon log
gets the same trace for debugging.

### Why this answers Axis 3 well

- fabric's role is *strictly* "we re-use its pattern-assembly helper" —
  `core.assemble_prompt` (which we already own a Python equivalent of).
  Fabric the binary is not in the call graph for tool-using runs.
- Streaming is honest: content streams as it arrives; tool calls appear
  as discrete events (matches Ollama's chunk granularity, no fake deltas).
- All loop control is in one place. No "tools also work in the panel"
  drift; if a pattern uses tools, it goes through this loop, period.

---

## What this proposal collapses (cleanups it enables)

- **No fabric fork or upstream PR.** Settled by the maintainer's stance and
  reinforced here. The daemon path is the path.
- **No parallel pattern-assembly logic.** We re-use `core.assemble_prompt`
  on both branches.
- **No magic "tool inference" from pattern content.** Patterns *declare*
  `tool_use: true` and an explicit `tools:` list. If you didn't ask for
  tools, you won't get them — and you won't get bills you didn't expect.
- **No bypass for security boundaries.** `panel-confirm` is the only path
  for side-effect tools; no `--trusted-pattern` flag.

## Sequencing reminder (from `tool-calling-backends.md`)

The proposal above is the *architecture*. Phase 1 (~2 weeks) implements it
for Ollama with the three built-in tools and 1-shot loop; later phases add
Anthropic (vendor adapter), bounded multi-turn at scale, then MCP as a
third tool source. No phase changes the Axes 1–3 answers; later phases add
contents, not restructure.

## Confirm

Three things to confirm before this becomes a `decision-N` doc:

1. **Axis 1: layered (built-in code + policy.toml config + future MCP),
   pattern frontmatter declares which tools the pattern may use.** ✓?
2. **Axis 2: three modes (`daemon`, `panel-confirm`, future `mcp`), declared
   per tool, with the four hygiene rules centralized in the executor.** ✓?
3. **Axis 3: daemon owns the loop end-to-end on the tool path; fabric stays
   unchanged for plain patterns; new streaming events for the panel.** ✓?

If yes, the next step is implementation; this becomes `decision-5-local-tool-execution.md`.
If anything wants adjustment, we adjust before committing.

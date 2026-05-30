# Tool calling backends — a design round

Status: **exploration drafted 2026-05-29.** No decision yet. The work below
maps the option space, weighs each candidate honestly, identifies the
cross-cutting concerns, and ends with a recommendation + sequencing. Sits
alongside [`integration-plan.md`](../integration-plan.md) as a *forward
question*; not yet a `decision-N` doc.

## Framing — what's already settled

1. **fabric upstream won't carry tool calling.** Verified via #1387 (closed)
   and #1438 (open, stale): the maintainer's explicit stance is that MCP /
   A2A / function calling are "outside the scope of this project" and belong
   in standalone server projects. There is no expectation that fabric's
   REST `/chat` will ever carry `tools=[...]`.
2. **Ollama 0.24.0 supports tool calling natively** on `/api/chat`. Verified
   live: `qwen3:14b-iq4xs` + a `get_weather` tool returns a clean
   `tool_calls` response. `llama3.1:70b` and `gpt-oss:20b` also declare the
   capability.
3. **Anthropic, OpenAI, and Gemini all support tool use natively** at the API
   layer (each with its own schema). On this box, Anthropic and Ollama are
   keyed.
4. **"The daemon owns integration"** is already the architectural invariant
   from the umbrella plan and `review-and-fabric-integration.md`. We've
   already absorbed fabric scope-restrictions for multimodal via CLI
   shell-outs in `cosmic-fabricd`; tool calling is the same architectural
   shape — fabric won't carry it, so the daemon does the workaround.

So the question is **not** "should cosmic-fabric have tool calling" (the
ecosystem decision has been made — MCP is here, Ollama/Anthropic ship it).
The question is **how the daemon brings it in**, given fabric won't.

## Axes of choice (before the options)

Four orthogonal questions shape any design here:

1. **Where do *tools* come from?** Built-in registry hardcoded in the
   daemon (`read_file`, `run_shell`, `http_get`, etc.) · MCP servers
   discovered at runtime · user-declared in config · some mix.
2. **How does the model *receive* tools?** Native vendor APIs (Ollama
   `tools`, Anthropic `tool_use`, OpenAI `tools`/`functions`,
   Gemini function declarations) · prompt-injection ("you have these tools,
   respond with this JSON shape") · hybrid.
3. **Who *executes* tools?** The daemon · the model's vendor (e.g.,
   Anthropic Remote MCP, where Anthropic itself calls the tool servers
   server-side) · the panel-with-confirm (UI mediation).
4. **Loop shape?** 1-shot (one tool call max, then summary) · bounded
   multi-turn (N iterations until done or budget) · stream-with-interruption
   (tools fire during a streaming response).

The options below differ on (1) and (2) primarily; (3) and (4) are sub-
decisions each option resolves.

## Model-vendor comparison (the providers)

Before the architectural options, the vendors themselves differ enough as
*tool-calling backends* that the choice matters at the model layer too. This
is what's available on each, today, judged narrowly as a place to plug a
tool-using run.

| | **Ollama** | **Anthropic** | **OpenAI** | **Gemini** |
|---|---|---|---|---|
| keyed on this box | yes (local) | yes | no | no |
| native tool-call API | `tools` + `tool_calls` on `/api/chat` (0.3+) | `tool_use` / `tool_result` content blocks on `/v1/messages` | `tools` / `tool_calls` on `/v1/chat/completions` (and Responses API) | function declarations on `generateContent` |
| schema validation | client-side only — the JSON-Schema you pass is hint-shaped | server enforces JSON-Schema you pass | server enforces JSON-Schema; structured outputs compose with tools | server enforces; OpenAPI-flavored declarations |
| streaming tool deltas | per-call complete object in the stream; deltas weaker than cloud | `content_block_start` / `content_block_delta` / `content_block_stop` — cleanest streaming story | tool-call deltas in `choices[0].delta.tool_calls` — workable | streaming exists; less commonly used |
| parallel tool calls | yes (single response can emit multiple `tool_calls`) | yes (multiple `tool_use` blocks per turn) | yes | yes |
| **server-side tool execution** | no — we always run tools locally | **yes — Remote MCP**: pass MCP server URL, Anthropic invokes it server-side | partial — Assistants API runs some tools server-side; Responses API more flexible | partial |
| cost per tool call | $0 | input + output tokens for each turn; loops add up fast | similar to Anthropic | similar |
| rate limits | none (local) | per-account TPM/RPM; tight on bursty loops | per-tier; tight | generous free tier |
| right for… | local privacy, free experimentation, cheap loops | strategic / agentic loops where Remote MCP eliminates our loop entirely | OpenAI-shop integrations, structured outputs | Google-shop / free tier work |
| risk on this box | none (offline) | bill creep on multi-turn loops | not applicable (no key) | not applicable (no key) |

**Reading this table for cosmic-fabric:**
- **Ollama is the development backend.** Free, local, fast to iterate, no
  bill for a buggy loop. Smallest possible blast radius while you're getting
  the loop semantics right.
- **Anthropic is the shipping backend for strategic tool use.** Cleanest
  streaming story, server-enforced schemas, and Remote MCP — the
  user-visible fact that *Anthropic runs the tool loop server-side* changes
  the calculus entirely (see option F below).
- **OpenAI and Gemini are nice to support but not needed first.** Neither is
  keyed here, and Anthropic + Ollama cover both the cheap-local and
  cloud-quality use cases the design needs.

The architectural options that follow are how the daemon talks to *whichever*
vendor backend a run uses; they cut across the table above.

## Backend option A — Direct vendor APIs from the daemon

**Shape:** The daemon implements a thin client per vendor:
`POST localhost:11434/api/chat` (Ollama), `POST api.anthropic.com/v1/messages`
(Anthropic), `POST api.openai.com/v1/chat/completions` (OpenAI). For
tool-using runs, the daemon bypasses fabric, sends the request with
`tools=[...]` in the vendor's native shape, parses `tool_calls` /
`tool_use` blocks back, executes them locally, sends the result back in the
same conversation, repeats until `stop_reason != tool_use` or a loop cap.
Tools come from a small **built-in registry** in `core.py`
(`read_file`, `list_dir`, `http_get`, `run_shell_confirmed`, etc.).

**Pros**
- *Works today on this box*: Ollama is keyless and tested; Anthropic key
  already set.
- *Honest mental model*: vendor-by-vendor translation is small and explicit;
  we own the shape.
- *No new infra*: no MCP server processes to spawn, supervise, discover.
- *Streams cleanly*: each vendor's streaming API is well-understood; tool
  call deltas slot in.

**Cons**
- *Parallel code path per vendor*: schema differences (`tool_use` vs
  `tool_calls` vs `function_call`) mean ≥2 adapters plus a normalization
  layer. ~200 LoC per vendor is a fair estimate.
- *We re-assemble prompts ourselves*: fabric's pattern assembly
  (`assemble_prompt`) does the {{var}} substitution and appends input;
  we'd call that helper but skip fabric's `/chat` for the tool path. That's
  one extra place to keep in sync if fabric's assembly logic changes.
- *Tools bounded to what we ship*: extensible only by code, not config.
  Users can't BYO a "filesystem MCP" without us coding it.
- *Loss of fabric's session/context plumbing*: we'd need to re-handle
  `contextName` / `sessionName` ourselves on the tool path, or restrict the
  tool path to stateless runs.

**Fit:** Best as a *first step* — gets tool calling working end-to-end with
minimum architectural commitment.

## Backend option B — Daemon as MCP client

**Shape:** The daemon implements the MCP client protocol (stdio or HTTP/SSE
transport). It connects to one or more MCP servers — local (filesystem,
git, sqlite, shell) or remote (the user's homelab, a hosted service). On a
tool-using run, the daemon: discovers tools from connected servers,
translates the MCP tool schemas into each vendor's native format
(option A's adapters still apply for the model side), runs the loop. Tool
*execution* delegates to the MCP server (this is the whole point of MCP —
the server runs the tool).

**Pros**
- *Plugs into the ecosystem*: any of the ~hundreds of existing MCP servers
  works the day it's added. Filesystem, git, Postgres, Brave search, Notion,
  Linear — all available without us writing adapters.
- *Right side of the protocol drift*: Microsoft + Google both adopted MCP
  (per the #1387 thread); cosmic-fabric supports the standard the ecosystem
  is consolidating on.
- *Separation of concerns*: tool execution lives in the MCP server, not
  the daemon. Our security boundary is just "what servers do we trust?"
- *Anthropic's API supports remote MCP natively*: for Anthropic runs, we
  can pass an MCP server URL and Anthropic invokes it *server-side*
  (skipping our loop for that vendor). Big win for cloud runs.

**Cons**
- *MCP client is real work*: bidirectional JSON-RPC, transports (stdio +
  HTTP+SSE), capability negotiation, lifecycle (start/stop the server
  processes), error handling. ~500–800 LoC, vs option A's ~200 per vendor.
- *Still need vendor adapters*: MCP solves "where do tools come from,"
  not "how does the model invoke them." Each vendor's tool-call schema is
  still its own. So this option is *additive* to A's vendor adapters, not
  a replacement.
- *Server lifecycle*: who starts/stops the MCP servers? Per-daemon-launch
  spawn? User runs them externally? Each answer has cost.
- *No Ollama-side MCP*: Ollama doesn't speak MCP at the model layer; even
  on an Ollama run, the daemon would still translate MCP tools → Ollama
  `tools` schema and execute locally.

**Fit:** Best as a *phase 2* — once option A has shaken out the loop
mechanics, MCP becomes the *tool source* on top of the same vendor
adapters. Defer until either (i) we want a specific MCP server, or (ii)
ecosystem reasons (Anthropic Remote MCP, Windows 11 MCP) make a compelling
case for *the same week we ship*.

## Backend option C — Pattern-as-loop via prompt-injection + shell shell-out

**Shape:** No vendor tool-call APIs at all. The pattern's system prompt
declares "you have these tools: …; respond with `<TOOL>name args</TOOL>`
when you need one." The daemon parses the model's text output, extracts
tool invocations, executes them as shell commands, re-prompts the model with
the result. Loop until no `<TOOL>` block appears, then return the final
text. Works on **any model**, including ones that don't support native
tool calls (qwen2.5, gemma, smaller llamas).

**Pros**
- *Vendor-agnostic by construction*: works on every model regardless of
  tool-capability flags. No vendor adapters needed.
- *Fits fabric's pipe philosophy*: it's still text-in / text-out at every
  step; the daemon just reads stderr and re-pipes.
- *Trivially compatible with fabric*: no daemon path-divergence; fabric's
  REST `/chat` carries the whole loop because the model is just emitting
  text.
- *Useful fallback*: even after A/B land, this is the answer for models
  without native tool support.

**Cons**
- *Brittle*: model has to emit the exact envelope; any drift breaks the
  parse. Tool schemas are unstructured (no formal types), so the model
  guesses argument shapes. *Materially worse reliability than native APIs.*
- *No streaming during tools*: by the time we've parsed the text, the
  stream is done. Multi-turn tool loops feel laggy.
- *Security headache is worst here*: shell shell-out from model-emitted
  strings, even structured ones, is the highest blast radius for prompt
  injection. Sandboxing is mandatory and hard.
- *Ergonomically poor for the model*: native tool APIs include parameter
  schemas; prompt-injection doesn't. The model writes less reliable calls.

**Fit:** *Not the primary path.* A useful fallback for non-tool-capable
models if someone wants to use them in tool-using flows. Below the bar to
build first; reasonable to add later as a width-of-coverage feature.

## Backend option D — Hybrid: per-pattern routing

**Shape:** A pattern declares (in its frontmatter or a sidecar) whether it
wants tool calling: `tool_use = true` (or `tools = ["read_file", …]`). The
daemon's `run` op routes:
- `tool_use = false` (the default, every existing pattern) → unchanged
  fabric path. fabric's REST `/chat` does what it does today.
- `tool_use = true` → daemon takes the wheel: assembles the prompt itself
  (calls `core.assemble_prompt`), goes direct to the vendor API (option A's
  adapters), runs the tool loop, returns the final text via the same daemon
  `run` op.

**Pros**
- *Minimal change to the 99% of runs that don't use tools*: existing
  patterns and surfaces are bit-for-bit untouched.
- *Single daemon op, two paths*: `run`'s API doesn't change; the
  branch is internal. Surfaces don't have to know.
- *Composable with B*: when MCP-client lands, the tool-use branch can
  source tools from MCP without changing the routing logic.
- *Honest about scope*: fabric still owns *patterns*; the daemon owns *the
  tool loop*. Each owns what they're good at.

**Cons**
- *Two assembly paths to keep in sync*: when fabric's `assemble_prompt`
  changes upstream, we have to mirror the change on the tool path. Mitigated
  by the fact that `assemble_prompt` is already a Python helper we own
  (it's our re-implementation; fabric does its own server-side, but we have
  a working equivalent).
- *Pattern frontmatter is fabric's territory*: declaring `tool_use = true`
  in a pattern file means we're adding a non-standard field. Fabric ignores
  unknown fields, but users might wonder why their pattern works
  differently. (Sidecar `tools.toml` next to the pattern dir is an
  alternative.)

**Fit:** *The shipping shape.* This is what the daemon would actually do.
The other options (A, B, C) are *what fills the tool-use branch*.

## Backend option F — Anthropic Remote MCP (vendor-side loop execution)

**Shape:** For Anthropic runs, the daemon passes the request to
`/v1/messages` with `mcp_servers: [{type: "url", url: "…"}]`. Anthropic
itself connects to those MCP servers, invokes their tools server-side, and
returns the final assistant text *with the tool-use trace embedded as
content blocks*. **The daemon does not run any loop**: no parsing of
`tool_use`, no executing tools, no re-prompting. We get the trace back as
content for observability.

This is **orthogonal** to options A and B. A and B answer "how does the
daemon run the loop"; F answers "do we run the loop at all on cloud paths,
or does the vendor do it for us?" For vendors that offer this (Anthropic
today; OpenAI Assistants partially; others moving this direction), it's a
fundamentally different operational shape.

**Pros**
- *No tool-loop code on cloud paths.* The most complex, most error-prone,
  most security-sensitive part of the design (loop control, sandboxing,
  budget caps, error recovery) is *not in our daemon* for Anthropic runs.
- *Tools run wherever the MCP server lives.* Remote services (Linear, Notion,
  hosted Postgres) get reached by Anthropic over the network, not via our
  box; local services we'd need to expose anyway.
- *Vendor-shaped streaming.* Anthropic's `content_block` events already
  encode the tool-use trace; surfaces render it natively.
- *Composes with B.* If we later run a local MCP server (e.g., filesystem,
  jailed shell), Anthropic can target it via a publicly-reachable URL — but
  for purely-local tools, we still want option A or B's local execution.

**Cons**
- *Anthropic-only* (today). Ollama has no equivalent; OpenAI's analogue
  (Assistants / Responses) is its own shape; Gemini's is still maturing. So
  F is a *cloud accelerator*, not a general answer.
- *Anthropic must be able to reach the MCP server.* Local-only servers
  behind NAT need a tunnel or a port-forward. Most user-interesting tools
  (filesystem, local git) don't work under F at all.
- *Privacy*: the tool call payloads (filenames, queries, file contents in
  results) all transit Anthropic. Not appropriate for sensitive workflows.
- *Per-call cost*: tools that run server-side still bill input/output
  tokens for every turn. Cheap relative to building the loop ourselves;
  expensive relative to running tools locally.

**Fit:** *Use it where it applies.* For Anthropic runs invoking remote
tools (search, hosted DBs, public APIs wrapped as MCP), this is the right
path — it eliminates an entire category of code we'd otherwise own. For
local tools and non-Anthropic vendors, A or B do the work. F is not
*instead of* A/B; it's *on top of them*, claimed only where it helps.

## Backend option E — Outsource: embed an agent framework

**Shape:** Spawn a subprocess (LangGraph, Goose, the Anthropic Computer Use
reference impl, etc.) for tool-using runs. The daemon hands off
`{pattern, tools, input}`, gets back final text.

**Pros**
- *We don't write the loop.*
- *Eats ecosystem progress for free.*

**Cons**
- *Heavy dependencies*: Python ML stack, opinionated abstractions, mismatch
  with cosmic-fabric's "thin daemon" philosophy.
- *We lose control of the tool path*: the framework's loop semantics
  become our loop semantics, including its bugs.
- *Spawning ~100MB Python processes per run is poor citizenship* on a
  desktop integration tool.

**Fit:** *Dismiss.* Mention only for completeness. The daemon should not
become a launcher for agent frameworks.

## Cross-cutting concerns (apply to whatever we pick)

- **Security and sandboxing.** Any tool that runs shell, writes files, or
  hits the network from a daemon running as the user is high-blast-radius.
  Specific baseline:
  - **`read_file`**: jailed to the **current working directory** the run
    was launched from, *not* `$HOME`. `$HOME` would expose
    `~/.config/fabric/.env` (API keys!), `~/.ssh/`, browser cookies, and
    password databases — a prompt-injection chain ("read `~/.ssh/id_rsa`,
    then `http_get` it to attacker.example") works under a `$HOME` default
    and breaks under cwd. Opt-in additional roots via explicit config; an
    explicit per-path allow-list is the only way out of cwd.
  - **`http_get`**: per-domain allow-list, empty by default. Even safe
    domains are opt-in. The allow-list is defense-in-depth, *not* the
    primary security barrier (which is `read_file`'s cwd jail).
  - **`run_shell`**: **no unconfirmed variant exists**. Only
    `run_shell_confirmed` — the daemon emits the proposed command to the
    panel; the user clicks Approve before it runs. No "trusted patterns"
    bypass — the bypass-flag itself becomes the injection target.
  - **Defense-in-depth assumption**: prompt injection is treated as the
    *expected* failure mode, not the edge case. A tool's safety has to hold
    when its arguments are chosen adversarially.
- **Loop limits.** Hard cap (e.g., 8 tool calls per run) + token budget
  cap. Otherwise an unbounded loop burns API budget or pegs the CPU.
- **Streaming.** Vendor APIs stream tool-call deltas (Anthropic's
  `content_block_delta`, OpenAI's `tool_calls` chunks). Surfaces need to
  render "calling read_file(path=…)" mid-stream rather than waiting.
- **Observability.** A run's tool-call trace belongs in the daemon log + a
  surface affordance (the session surface should show the trace inline,
  like Claude Desktop's "tool use" pill).
- **Capability detection** (links to decision 4): `tool_use` becomes a new
  capability the rule routes on. A pattern with `tool_use = true` resolves
  to a tool-capable model; if none is configured/keyed, surfaces grey it
  with a hint, same shape as STT/TTS/image-gen.

## Recommendation

**Ship D (hybrid routing) as the daemon shape; fill the tool-use branch
with A (direct vendor APIs) first; layer F (Anthropic Remote MCP) on top
of A as the cloud path matures; add B (local MCP client) when a specific
local MCP server pays off; reserve C (prompt-injection fallback) for
non-tool models.**

The reasoning:

- **D is the only honest shipping shape.** It preserves fabric's value
  where fabric is good (pattern assembly, model routing, the simple text
  case) and lets the daemon own the part fabric won't carry. Anything else
  either breaks the 99% of runs that don't need tools, or adds infra we
  don't yet have a use for.
- **A is the right phase-1 because we can test it on this box today.**
  Ollama tools work; the Anthropic key is set. A 1-shot loop with three
  built-in tools (`read_file` jailed to **cwd, not $HOME**; `http_get`
  with allow-list; `run_shell_confirmed` requiring panel approval) is
  buildable in a focused slice and unblocks the quartermaster pattern's
  *downstream*.
- **F belongs on the Anthropic path the moment we add it**, not as a
  separate later phase. Remote MCP means *we don't write the loop at all*
  for Anthropic+remote-tool cases — that's a strict reduction in code we
  own, not an addition. The day the Anthropic adapter ships, F is two
  request-field changes away. Pure win on the cloud path.
- **B (local MCP client) is high-value but defer.** Right time is when a
  specific local MCP server we want to run becomes the motivating use case
  (filesystem, jailed git, local Postgres) — not now, when we'd be
  implementing protocol plumbing speculatively.
- **C is a width-feature.** Coverage of non-tool-capable models; low
  priority next to depth on the tool-capable path.

## Sequencing (if this direction lands)

Slices below are sized realistically — each is **~2 calendar weeks** of
focused work, not "a long afternoon." The 1-week estimates in the first
draft of this doc were under-scoped (the advisor caught it).

1. **Slice 1 — Ollama-local tool path (~2 weeks).** Hybrid routing in `run`
   op (D), Ollama vendor adapter (A), 1-shot loop, three built-in tools
   (`read_file` cwd-jailed; `http_get` allow-listed; `run_shell_confirmed`
   panel-mediated), `tool_use` capability + greyed-hint UI when no
   tool-capable model is configured, pattern frontmatter parsing for
   `tool_use = true`, observability (tool trace in the run output and
   daemon log). Test pattern: a `scribe-look-it-up` that runs `http_get`
   + summarizes.
2. **Slice 2 — Anthropic + bounded multi-turn + Remote MCP (~2 weeks).**
   Anthropic adapter (`tool_use` / `tool_result` content blocks), bounded
   multi-turn loop (`N=8` cap, token budget), streaming with `content_block`
   deltas surfaced to the session UI, **Remote MCP enablement on the
   Anthropic path** (F) — pass-through of `mcp_servers` configured in
   policy, no local loop for those calls.
3. **Slice 3 — local MCP client (~3 weeks, defer until motivated).** First
   `stdio` MCP client connection (filesystem MCP server as the simplest
   case). Adapter layer that translates MCP `Tool` definitions into Ollama
   `tools` and Anthropic `tool_use` shapes. Server lifecycle in the daemon
   (start/stop/restart). Not blocked on A/B's loops — *the daemon-local
   loop already exists*; B just adds an alternative tool *source*.
4. **Slice 4 — prompt-injection fallback (~1 week, optional).** For
   models without native tool support; brittle by construction, only worth
   it if a specific use case appears.

Each slice ships a real user-facing capability and is independently testable.

Each slice ships a real user-facing capability and is independently testable.

## What this does *not* do

- We are **not** building a fabric fork or upstreaming tool calls into
  fabric. That's settled by the maintainer's stance and isn't worth the
  fight.
- We are **not** using `fabric-mcp`. That project exposes fabric *as* an
  MCP server (external tools → fabric); it's the wrong direction for our
  need (fabric runs → tools).
- We are **not** building a general-purpose agent framework. The
  ambition is bounded: tool calls inside a pattern's run, with sane
  limits and explicit security. Anything more belongs in a separate
  project (e.g., goo, the aspirational layer).

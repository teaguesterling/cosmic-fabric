# fabric integration plan — the hard features (UX to confirm) — 2026-05-29

Picks up where [`review-and-fabric-integration.md`](review-and-fabric-integration.md)
left off. The **cheap, REST-native knobs are done** (commit `d12cda9`): variables,
contexts, strategies, language, and model-side web-search are now plumbed through
`core.run()` → daemon `run`/`stream_run` → the `cosmic-fabric` CLI, because they
are just fields on fabric's `ChatRequest` — the same "add a field" move as
`sessionName`/`modelContextLength`.

What's left are the features that need a **UX decision**, not just wiring. This
doc is organized by *decision shape*, not by feature: the hard features cluster
into five decisions, and each one, once made, answers several features at once.
Each section is **recommendation → why → the question to confirm**. The two
settled constraints throughout: the **3-surface model** (loom / kit / session) and
**"configure behavior, not aesthetics"** (a control earns its place only if it
changes *what you can do*, not *how it looks*).

> **Confirmed 2026-05-29:** all four recommendations below (Decisions 1–4) were
> accepted as the recommended option, and Decision 5 stands as recommended
> (CLI-only). The "Confirm" line in each section is now the **settled decision**;
> the sequencing section is the build order to follow.

> Architectural invariant (unchanged): the **daemon owns all fabric integration**.
> REST has no multimodal/scrape/youtube fields, so those become **CLI shell-outs
> inside the Python daemon**; the Rust panel stays a thin socket client.

---

## Decision 1 — Rich source ingestion (YouTube · scrape · Spotify · readability)

Covers: `-y` YouTube (transcript/comments/metadata), arbitrary page scrape
(`-u`, already done keyless via the daemon's Jina fetch), Spotify, the
`scrape_*` patterns. **Discriminator: does it extend the loom's `Origin` enum,
get auto-detected, or stay CLI-only?** One answer covers all four.

**Recommendation: make the existing URL origin *smart* — don't add buttons.**
When the URL the user pastes/types is a recognized media link (YouTube first), the
daemon routes to `fabric -y` (transcript ingestion) instead of the generic Jina
scrape; every other URL stays the generic fetch. No new "YouTube" origin button in
the kit or loom. Spotify and arbitrary `scrape_*` stay **CLI-only** (`cosmic-fabric`),
because Spotify needs extra setup and the scrape patterns are niche.

**Why:** the kit's whole value is *few, fast* affordances (select → inference →
review → close). A wall of source-type buttons (URL / YouTube / Spotify / …) is
exactly the "everything is too much" we curated away. One "URL" affordance that
does the right thing per link is a behavior win; separate buttons are visual
clutter that fails the principle. The daemon already owns the fetch, so detection
lives in one place.

**Confirm:** YouTube auto-ingests its transcript via a *smart URL origin* (no new
button) — yes? Or do you want an explicit picked "YouTube" source type?

---

## Decision 2 — Per-model knobs (temperature · topP · penalties · thinking)

REST `ChatOptions` exposes `temperature`, `topP`, `frequencyPenalty`,
`presencePenalty`, `thinking`. Today a **variant** already carries `ctx`,
`thinking`, `temperature`. **Discriminator: variant field (declared once) vs
per-run slider (clutter) vs both.** One answer covers all the knobs.

**Recommendation: variant fields only — no per-run sliders.** Add `topP`,
`frequencyPenalty`, `presencePenalty` to the `Variant` struct and the Models
editor, sitting next to the `ctx`/`thinking`/`temperature` already there. A
variant *is* a named bundle of deployment parameters — `qwen3/deep` already means
"more context, full thinking." To run hotter, you pick (or define) a variant, you
don't fiddle a slider on every run.

**Why:** per-run sliders are the archetypal knob-fiddling that "configure behavior,
not aesthetics" exists to reject — they invite tweaking without changing *what*
you can do, and they'd clutter every kit run. Variants keep the parameter space
*named and reusable*, which is the whole point of the two-level model→variant
design. `seed` (reproducibility) is a `cosmic-fabric run --seed` CLI flag, not a
variant field — it's a one-off, not a deployment property.

**Confirm:** sampling knobs become variant fields in the Models editor (declared
once, like ctx/thinking already are), with **no** per-run override UI — agreed?

---

## Decision 3 — Session management (list · resume · wipe · export)

fabric persists sessions server-side (`sessionName`; `GET /sessions/names`; CLI
`--listsessions` / `--wipesession` / `--printsession`). The session surface (#3)
today seeds a fresh session per launch. **Discriminator: a dedicated session
picker, or CLI-only + "remembers last"?**

**Recommendation: a lightweight session picker *in the session surface* — not a
management console in the loom.** The session surface gets a small session
dropdown: the `GET /sessions/names` list to resume, plus **New** and **Wipe**.
Export is **the per-artifact send-to control we already built** ("Copy
Conversation") — not a separate exporter. The `cosmic-fabric` CLI gets `session`
subcommands (`list`/`resume`/`wipe`) for scripting.

**Why:** sessions are surface #3's reason to exist; managing them belongs *there*,
where they live, keeping the surface lightweight (IM-style, "lighter than Alpaca")
rather than building an Alpaca-grade session manager in the loom. Export already
has a home in the per-artifact controls — adding a second export path would
duplicate it and muddy the "send-to" model.

**Confirm:** session list/resume/wipe lives in the session surface (a session
dropdown), the loom stays out of it, and export = the existing Copy Conversation
control — agreed?

---

## Decision 4 — Key-gated multimodal (STT · TTS · image-gen)

All three are cloud-key-gated and currently unkeyed on this box: STT needs an
OpenAI key, TTS a Gemini key, image-gen either. Vision is the precedent — we
already settled the **capability rule** (auto-pick a vision-capable instantiation).
**Discriminator: how does an *unavailable* capability present — hidden, greyed
with a hint, or offered-then-fails?** This is whether the capability rule
generalizes from vision to the key-gated ones.

**Recommendation: generalize the capability rule, and present unavailable
capabilities *greyed with a "needs <vendor> key" hint* — not hidden, not
silently failing.** When a capable instantiation exists, the rule resolves to it
(as vision does today). When none exists, the surface shows the affordance
**disabled with the hint** (e.g. "Transcribe — needs an OpenAI key · run
`fabric --setup`"). Slot the modalities into the structures we already have:
**STT = an input source**, **TTS = a per-artifact "Speak" destination** (fits the
send-to model directly), **image-gen = a response type**. All via daemon
CLI shell-out (REST has no multimodal).

**Why:** hiding capabilities makes the product feel incomplete and leaves the user
no path to enable them; offer-then-fail is hostile. Greyed-with-hint teaches the
exact next action and reflects a *real capability boundary* — which is behavior,
not aesthetics, so it passes the principle. Reusing input-source / send-to /
response-type means zero new UI primitives.

**Confirm:** unavailable multimodal capabilities show **greyed + "needs <vendor>
key" hint** (vs hidden), and they reuse existing primitives (STT=source,
TTS=Speak send-to destination, image-gen=response type) — agreed?

---

## Decision 5 — The niche tail (extensions · dry-run · output-session · seed)

**Recommendation: CLI-only or already-covered — no surface work.**
- **dry-run** ≈ the existing `assemble` op (render the prompt without running).
  No new feature; maybe alias `cosmic-fabric run --dry-run` → `assemble`.
- **output / output-session** (`-o`, `--output-session`) ≈ shell redirection
  (`cosmic-fabric run … > file`); a `--seed` and `-o` flag on the CLI if ever
  wanted, but no UI.
- **extensions** — fabric's extension system is power-user; a `cosmic-fabric`
  passthrough at most, not a surface.

**Confirm:** agree these stay CLI-level / already-covered, with no panel UI?

---

## Suggested sequencing (if the above lands roughly as recommended)

No-regret first, identity-dependent last:

1. **Decision 2 (sampling → variant fields)** — pure extension of the Models
   editor + `Variant` struct; no new UX surface, no key, low risk. Do first.
2. **Decision 1 (smart URL → YouTube)** — daemon-only detection + one CLI
   shell-out; high value (richest source), invisible until it helps.
3. **Decision 3 (session picker)** — completes surface #3; the session dropdown is
   small and self-contained.
4. **Decision 4 (generalized capability rule + greyed hints)** — the most UI, and
   the modalities are unkeyed today, so it pays off only after a key is added; do
   it once the pattern (greyed-hint) is confirmed, even before keys exist, so the
   affordances are *present and honest*.
5. **Decision 5** — opportunistic, as flags land.

## Feature → surface → decision (cheat-sheet)

| feature | surfaces where it shows | decision |
|---|---|---|
| variables / context / strategy / language / search | all (done) | — shipped `d12cda9` |
| YouTube / rich scrape | loom + kit URL origin | **1** smart URL |
| temperature / topP / penalties | Models editor (variant) | **2** variant field |
| thinking | Models editor (variant, exists) | **2** variant field |
| seed | CLI flag | **2** / **5** |
| sessions list/resume/wipe | session surface | **3** picker |
| session export | per-artifact send-to (exists) | **3** reuse |
| STT / TTS / image-gen | source / send-to / response | **4** capability rule + greyed hint |
| dry-run / output / extensions | CLI | **5** none |

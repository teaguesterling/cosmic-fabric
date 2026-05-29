# Tutorial — your first 30 minutes with cosmic-fabric

A hands-on path: do each task, see the result, learn the idea behind it. By the
end you'll have run patterns from every surface, curated your own set, configured
models, asked about an image, and turned a result into a conversation.

> First time? Install + key-up first: [`getting-started.md`](getting-started.md).
> This tutorial assumes the daemon is up (`cosmic-fabricd` — the launcher/panel
> auto-spawn it) and the Fabric applet is on your panel. It doubles as the
> GUI-validation walkthrough (Phase 2 §A in `fabric-on-the-desktop.html`).

Pieces you'll touch: the **kit** (panel · launcher · quick-action), the **loom**
(the workbench window), and **session** (chat).

---

## 1 · Your first run — summarize from the panel  *(the kit)*

1. Copy a paragraph of text anywhere (Ctrl+C).
2. Click the **Fabric** icon in your panel.
3. Under **Run**, click **Summarize**.

**You'll see:** the result streams into the popup, a notification fires, and the
summary is on your clipboard.

**The idea:** the panel runs the chosen pattern on whatever's on your clipboard,
through the local model — your everyday one-click path.

---

## 2 · Act on a selection, no copy-paste  *(the quick-action)*

1. **Highlight** some text in any app (just select it — don't copy).
2. Press your quick-action shortcut (**Super+Shift+F** if you set the default).
3. Click a verb in the grid (e.g. **Explain**).

**You'll see:** a small window with the result, already copied. **Copy**,
**↪ Chat**, **↺ Another**, or **Close**.

**The idea:** "select → inference → review → close." It reads your *primary
selection* (highlighted text), so there's no copy step. The grid shows only your
**active set** (see §4).

> Prefer typing? **Super** → type a verb name → Enter runs it on the selection
> (the launcher).

---

## 3 · Summarize a web page  *(the loom · URL source)*

1. Open the workbench: panel popup → **Open workspace…** (or run
   `cosmic-fabric-panel window`).
2. In **Run** mode, click the **URL** source button.
3. Paste a link (e.g. a blog post), click **Fetch**.
4. Pick **Summarize** from the pattern dropdown, click **Run**.

**You'll see:** "fetched · N chars markdown" (the page scraped to text), the
**Prompt** card showing the assembled prompt, then the **Response** streaming in.

**The idea:** the daemon fetches the page (keyless) and feeds it as the input —
the source is *polymorphic*. The Prompt card is the prompt-first view: you can see
exactly what goes to the model before (and after) you Run.

---

## 4 · Make it yours — curate patterns  *(the loom · Library)*

fabric ships ~265 patterns; you don't want all of them in your quick surfaces.

1. In the workbench, switch to **Library**.
2. Search (e.g. `extract`), and **★** a couple you like; **★** off ones you don't.
3. Click a pattern's name → a config row appears; leave it on **Default** for now.

**You'll see:** the **★** set is what now shows in the panel popup and the
quick-action grid (reopen them to confirm).

**The idea:** your **active set** drives the fast surfaces. It's stored as
include/exclude globs in `policy.toml` — `scribe-*` is just the default pack; the
app isn't tied to any name.

---

## 5 · Fast vs deep — configure models  *(the loom · Models)*

1. Switch to **Models**. You'll see your model instantiations as cards.
2. Open **qwen3** (Edit): note its **fast** (ctx 2048) and **deep** (ctx 16384)
   variants, and the ★ default.
3. Add a variant or category, or set the **default variant**.
4. Back in **Library**, set a heavy pattern (e.g. *visualize*) to **use** a cloud
   model (e.g. `sonnet`) via its dropdown.

**You'll see:** each model card lists its params + **"used by:"** (the usage
index), so you can reason about your whole setup in one place.

**The idea:** a *model instantiation* = a model + deployment params (ctx, thinking,
temperature), grouped as model → variants. Patterns point at one by name. No
hardcoded model names anywhere — it's all your config.

---

## 6 · Ask about an image  *(the loom · vision)*

1. In **Run** mode, click the **Image** source button. *(Shortcut: copy an image
   first — the workbench opens straight into Image mode with it loaded.)*
2. Paste a path to an image (e.g. `~/Pictures/Screenshots/shot.png`), or click
   **From clipboard** to grab a copied image.
3. In the text box, type your question: *"What's in this screenshot?"*
4. Click **Run**.

**You'll see:** the Prompt card notes it's a vision run; the Response is the
model's description.

**The idea:** an image needs a model that can *see*. The **capability rule**
notices the image and auto-swaps your text default for a vision-capable model
(local `llama3.2-vision` if you have it, else a vision-capable cloud model) — you
don't pick it manually. *(Native image picker is coming; for now it's a path.)*

---

## 7 · Turn a result into a conversation  *(escalate → session)*

1. Run anything that gives you a Response (§3 or §6).
2. Click **↪ Chat** on the Response card (or on a quick-action result).

**You'll see:** a chat window opens with the result carried into the input;
add a follow-up ("now make it shorter") and **Send**. It remembers the thread.

**The idea:** one-offs are for "select → answer → done." When you need to go
deeper, *escalate* into a **session** — a lightweight multi-turn chat (fabric keeps
the history). It's not a heavyweight chat app; for that, hand off to a full agent.

---

## Where to go next

- **Route results** — each artifact (prompt / response / conversation) has a
  send-to control: **Copy**, **Save to file**, more later.
- **Unlock more** — add an **OpenAI** key (speech-to-text, image generation) or a
  **Gemini** key (text-to-speech, image generation) via `fabric --setup`. See the
  capability cheat-sheet in [`getting-started.md`](getting-started.md).
- **The big picture** — [`fabric-on-the-desktop.html`](fabric-on-the-desktop.html)
  (interactive feature map) and [`manual.md`](manual.md) (reference).

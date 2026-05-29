"""cosmic-fabric shared core — policy, fabric REST client, ollama placement.

Used by `cosmic-fabricd`. This is the logic that will be ported to Rust for the
Phase-2 panel/settings; keeping it small and dependency-free (stdlib only) on
purpose. The launcher does NOT import this — it talks to the daemon over a socket.
"""
import json
import os
import re
import socket
import subprocess
import time
import urllib.request

POLICY = os.path.expanduser("~/.config/cosmic-fabric/policy.toml")


# ---------- policy ----------------------------------------------------------
def load_policy():
    pol = {
        "default": {"model": "qwen3:14b-iq4xs", "vendor": "Ollama", "extra": []},
        "patterns": {},
        "output": {"mode": "notify"},
        "ollama": {"bin": "/opt/ollama/bin/ollama", "url": "http://localhost:11434", "warn_below_gpu": 99},
        "surface": {"include": [], "exclude": []},
        "models": {},
    }
    try:
        import tomllib
        with open(POLICY, "rb") as f:
            d = tomllib.load(f)
        pol["default"] = {**pol["default"], **d.get("default", {})}
        pol["patterns"] = d.get("patterns", {})
        pol["output"] = {**pol["output"], **(d.get("output", {}) or {})}
        pol["ollama"] = {**pol["ollama"], **(d.get("ollama", {}) or {})}
        pol["surface"] = {**pol["surface"], **(d.get("surface", {}) or {})}
        pol["models"] = d.get("models", {}) or {}
    except FileNotFoundError:
        pass
    except Exception as e:  # malformed policy must not break the daemon
        print(f"[core] policy load error: {e}")
    return pol


def active_patterns(pol, all_names):
    """The curated working set: patterns matching an `include` glob (or all, if
    `include` is empty) and no `exclude` glob. Mirrors the Rust
    `Policy::active_patterns`. (A custom pack is just an `include` glob in config.)"""
    import fnmatch
    surf = pol.get("surface") or {}
    inc = surf.get("include") or []
    exc = surf.get("exclude") or []
    def any_match(globs, n):
        return any(fnmatch.fnmatch(n, g) for g in globs)
    return [n for n in all_names
            if (not inc or any_match(inc, n)) and not any_match(exc, n)]


def resolve_model(pattern, pol):
    c = {**pol["default"], **pol["patterns"].get(pattern, {})}
    return c.get("model"), c.get("vendor"), c.get("extra", []) or []


_INST_KEYS = ("model", "vendor", "ctx", "thinking", "temperature", "extra", "capabilities")
_VARIANT_PARAMS = ("ctx", "thinking", "temperature", "extra")


def _resolve_use(use, models):
    """`"model"` (→ its default variant) or `"model/variant"` → an effective
    instantiation: base (vendor/model/capabilities) + the variant's params;
    categories = base ∪ variant. A model with no variants resolves to its base."""
    if not use:
        return None
    name, _, variant = use.partition("/")
    m = models.get(name)
    if not m:
        return None
    inst = {k: m[k] for k in ("vendor", "model", "capabilities") if k in m}
    cats = list(m.get("categories", []) or [])
    variants = m.get("variants", {}) or {}
    v = {}
    if variants:
        vname = variant or m.get("default") or next(iter(variants))
        v = variants.get(vname, {}) or {}
    for k in _VARIANT_PARAMS:
        if k in v:
            inst[k] = v[k]
    inst["categories"] = cats + list(v.get("categories", []) or [])
    return inst


def _inst_of(cfg, models):
    """An assignment (default/pattern dict) → an instantiation dict, or None.
    Honors `use` → a model[/variant]; else legacy inline model/vendor."""
    cfg = cfg or {}
    if cfg.get("use"):
        r = _resolve_use(cfg["use"], models)
        if r:
            return r
    if cfg.get("model"):  # legacy inline (transition-read)
        return {k: cfg[k] for k in _INST_KEYS if k in cfg}
    return None


def _first_capable(models, capability):
    """The first model instantiation (by its default variant) whose capabilities
    include `capability` — the capability selection rule. Prefers local (Ollama)
    so vision runs stay on-box when possible."""
    def caps(m):
        return [c.lower() for c in (m.get("capabilities") or [])]
    names = sorted(models, key=lambda n: (models[n].get("vendor", "").lower() != "ollama", n))
    for name in names:
        if capability.lower() in caps(models[name]):
            return _resolve_use(name, models)
    return None


def resolve(pattern, pol, need_capability=None):
    """Resolve a pattern to an effective model instantiation (explicit selection):
    the pattern's own (named `use` or legacy inline) wins, else the default's.
    If `need_capability` is set and the chosen instantiation can't satisfy it
    (e.g. a text-only model for an image run), the **capability rule** swaps in the
    first capable instantiation instead. Returns a normalized dict."""
    models = pol.get("models") or {}
    pat = pol.get("patterns", {}).get(pattern, {}) or {}
    inst = _inst_of(pat, models) or _inst_of(pol.get("default", {}), models) or {}
    if need_capability:
        have = [c.lower() for c in (inst.get("capabilities") or [])]
        if need_capability.lower() not in have:
            capable = _first_capable(models, need_capability)
            if capable:
                inst = capable  # the rule: a vision run must use a vision model
    return {
        "model": inst.get("model"),
        "vendor": inst.get("vendor"),
        "ctx": inst.get("ctx"),
        "thinking": inst.get("thinking"),
        "temperature": inst.get("temperature"),
        "extra": list(inst.get("extra", []) or []),
        "capabilities": list(inst.get("capabilities", []) or []),
        "categories": list(inst.get("categories", []) or []),
    }


def inst_to_options(inst):
    """Instantiation params → ChatRequest options. Typed fields (thinking,
    temperature) plus `extra` passthrough (CLI-style flags)."""
    opt = extra_to_options(inst.get("extra", []) or [])
    th = inst.get("thinking")
    if th is not None:
        if str(th).lower() in ("off", "none", "false", "0"):
            opt["thinking"] = "off"
            opt["suppressThink"] = True   # qwen3 convention: also strip <think>
        else:
            opt["thinking"] = str(th)
    if inst.get("temperature") is not None:
        try:
            opt["temperature"] = float(inst["temperature"])
        except (TypeError, ValueError):
            pass
    return opt


def pick_ctx(input_text, gen_margin=1024, tiers=(2048, 8192, 16384, 32768)):
    """Right-size the context window for an input: smallest tier that fits
    `input_tokens (~len/4) + a generation margin`. Keeps short inputs on a small,
    fully-GPU-resident cache; lets long inputs (scraped pages, files) grow rather
    than truncate. Returned as `modelContextLength` for the Ollama run."""
    need = len(input_text) // 4 + gen_margin
    for t in tiers:
        if need <= t:
            return t
    return tiers[-1]


def extra_to_options(extra):
    """Translate the policy's CLI-style `extra` flags into ChatRequest fields,
    so existing policy.toml keeps working over REST."""
    opt = {}
    for tok in extra:
        if tok.startswith("--thinking="):
            opt["thinking"] = tok.split("=", 1)[1]
        elif tok == "--suppress-think":
            opt["suppressThink"] = True
        elif tok == "--raw":
            opt["raw"] = True
        elif tok.startswith("--temperature="):
            try:
                opt["temperature"] = float(tok.split("=", 1)[1])
            except ValueError:
                pass
    return opt


# ---------- fabric REST client ---------------------------------------------
# Where the daemon runs fabric's REST API. fabric --serve defaults --address to
# ":8080" (= 0.0.0.0, all interfaces) — exposing the API + your keys on the LAN.
# We always pin it to **loopback**, and by default to a **random free port** (the
# daemon is fabric's only client + its spawner, so nothing else needs to discover
# it). The chosen port is persisted so a daemon restart reuses the live fabric
# instead of orphaning it. Override with COSMIC_FABRIC_ADDRESS=127.0.0.1:PORT.


def _free_port():
    """Ask the OS for an unused loopback TCP port."""
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]
    finally:
        s.close()


def _fabric_responds(addr, timeout=1.0):
    try:
        with urllib.request.urlopen(f"http://{addr}/patterns/names", timeout=timeout):
            return True
    except Exception:
        return False


def _port_file():
    base = os.environ.get("XDG_RUNTIME_DIR") or os.path.expanduser("~/.cache/cosmic-fabric")
    try:
        os.makedirs(base, exist_ok=True)
    except OSError:
        pass
    return os.path.join(base, "cosmic-fabric.fabric-addr")


def resolve_fabric_address():
    """The fabric --serve address (always loopback). Precedence:
    1) $COSMIC_FABRIC_ADDRESS (explicit override);
    2) the port from a prior run, if a fabric still answers there (reuse — no
       orphaned instance on restart);
    3) a fresh OS-assigned free port (persisted for next time)."""
    env = os.environ.get("COSMIC_FABRIC_ADDRESS")
    if env:
        return env
    pf = _port_file()
    try:
        prev = open(pf).read().strip()
        if prev and _fabric_responds(prev):
            return prev
    except Exception:
        pass
    addr = f"127.0.0.1:{_free_port()}"
    try:
        open(pf, "w").write(addr)
    except OSError:
        pass
    return addr


class FabricClient:
    def __init__(self, url=None, log=lambda m: None):
        # Default to the resolved (random/persisted) loopback address.
        self.url = (url or f"http://{resolve_fabric_address()}").rstrip("/")
        self.log = log

    def _get(self, path, timeout=5):
        with urllib.request.urlopen(self.url + path, timeout=timeout) as r:
            return json.load(r)

    def alive(self):
        try:
            self._get("/patterns/names", timeout=2)
            return True
        except Exception:
            return False

    def ensure_serve(self, wait=25):
        if self.alive():
            return True
        env = dict(os.environ)
        env["PATH"] = os.path.expanduser("~/.local/bin") + os.pathsep + env.get("PATH", "")
        address = self.url.split("//", 1)[-1]  # "127.0.0.1:PORT" — bind here (loopback)
        self.log(f"fabric --serve not up; starting it on {address}")
        try:
            subprocess.Popen(["fabric", "--serve", "--address", address],
                             env=env, start_new_session=True,
                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        except Exception as e:
            self.log(f"failed to spawn fabric --serve: {e}")
            return False
        for _ in range(wait * 2):
            if self.alive():
                self.log("fabric --serve is up")
                return True
            time.sleep(0.5)
        self.log("fabric --serve did not come up in time")
        return False

    def list_patterns(self):
        try:
            return self._get("/patterns/names")
        except Exception as e:
            self.log(f"list_patterns failed: {e}")
            return []

    def assemble_prompt(self, pattern, user_input, variables=None):
        """Render a pattern's prompt WITHOUT executing it — for handing off to
        an interactive agent (Claude Desktop/Code). Returns the pattern's system
        prompt (with {{vars}} substituted) followed by the input. No model run."""
        d = self._get("/patterns/" + pattern)
        sysp = d.get("Pattern", "") if isinstance(d, dict) else str(d)
        for k, v in (variables or {}).items():
            sysp = sysp.replace("{{" + k + "}}", str(v))
        return (sysp.rstrip() + "\n\n" + (user_input or "")).strip()

    def list_models(self):
        """Available models. /models/names returns {"models":[...]}."""
        try:
            d = self._get("/models/names")
            return d.get("models", d) if isinstance(d, dict) else d
        except Exception as e:
            self.log(f"list_models failed: {e}")
            return []

    def run_image(self, image_path, question, model, vendor, pattern=None, timeout=300):
        """Vision run: attach an image and ask about it. Shells out to the fabric
        CLI (`-a`) because the REST `/chat` API has no attachment field. Returns
        the model's text. Raises on failure."""
        env = dict(os.environ)
        env["PATH"] = os.path.expanduser("~/.local/bin") + os.pathsep + env.get("PATH", "")
        cmd = ["fabric", "-a", image_path, "--model", model, "--vendor", vendor]
        if pattern:
            cmd += ["--pattern", pattern]
        r = subprocess.run(cmd, input=(question or "Describe this image."), text=True,
                           capture_output=True, timeout=timeout, env=env)
        if r.returncode != 0:
            raise RuntimeError((r.stderr or "fabric -a failed").strip()[:400])
        return r.stdout.strip()

    def model_catalog(self):
        """{vendor: [models]} from /models/names — for the per-pattern picker."""
        try:
            d = self._get("/models/names")
            v = d.get("vendors", {}) if isinstance(d, dict) else {}
            return v if isinstance(v, dict) else {}
        except Exception as e:
            self.log(f"model_catalog failed: {e}")
            return {}

    def run(self, pattern, user_input, model, vendor, variables=None, options=None,
            timeout=600, on_chunk=None, model_ctx=None, session=None):
        """POST /chat (SSE) → accumulated text. If `on_chunk` is given, it's
        called with each content fragment as it streams. `model_ctx` sets the
        requested context window (`modelContextLength`) — used to right-size the
        Ollama KV cache for large inputs (web pages, files). `session` sets
        `sessionName` so fabric maintains multi-turn conversation history server-
        side (the Session surface). Raises on error."""
        prompt = {
            "userInput": user_input,
            "patternName": pattern or "",
            "model": model,
            "vendor": vendor,
            "variables": variables or {},
        }
        if session:
            prompt["sessionName"] = session
        body = {
            "prompts": [prompt],
            "model": model,
        }
        if model_ctx:
            body["modelContextLength"] = model_ctx
        body.update(options or {})
        req = urllib.request.Request(self.url + "/chat", data=json.dumps(body).encode(),
                                     headers={"Content-Type": "application/json"})
        out, raw_seen, err = [], 0, None
        with urllib.request.urlopen(req, timeout=timeout) as r:
            for raw in r:  # SSE: "data: {json}\n\n" (lenient: also bare json lines)
                line = raw.decode("utf-8", "replace").strip()
                if not line:
                    continue
                if line.startswith("data:"):
                    line = line[5:].strip()
                if line == "[DONE]":
                    break
                try:
                    ev = json.loads(line)
                except json.JSONDecodeError:
                    if raw_seen < 5:
                        self.log(f"non-json SSE line: {line[:160]!r}")
                    continue
                raw_seen += 1
                if raw_seen <= 3:
                    self.log(f"sse[{raw_seen}] type={ev.get('type')!r} keys={list(ev)} clen={len(ev.get('content','') or '')}")
                if ev.get("type") == "error":
                    err = ev.get("content") or "fabric error"
                if ev.get("type") == "complete":
                    break  # fabric's end-of-stream marker (don't wait for socket close)
                if ev.get("content"):
                    out.append(ev["content"])
                    if on_chunk:
                        on_chunk(ev["content"])
        if err:
            raise RuntimeError(err)
        return "".join(out).strip()


# ---------- web source ingestion (URL → text) -------------------------------
_TAG_RE = re.compile(r"<[^>]+>")
_DROP_RE = re.compile(r"<(script|style|head|noscript)[^>]*>.*?</\1>", re.S | re.I)
_WS_RE = re.compile(r"\n\s*\n\s*\n+")


def _html_to_text(html):
    """Naive, dependency-free HTML → text for the readability fallback."""
    html = _DROP_RE.sub("", html)
    html = re.sub(r"<(br|/p|/div|/h[1-6]|/li)[^>]*>", "\n", html, flags=re.I)
    text = _TAG_RE.sub("", html)
    text = (text.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">")
                .replace("&quot;", '"').replace("&#39;", "'").replace("&nbsp;", " "))
    return _WS_RE.sub("\n\n", text).strip()


def fetch_url(url, mode="scrape", timeout=10):
    """Fetch a web page as text, for use as fabric `input`.

    - scrape (default): Jina AI reader (https://r.jina.ai/<url>) → clean markdown.
      Works keyless on this box; same backend fabric's `--scrape_url` uses.
    - readability: direct fetch + naive tag strip (no-dep fallback, no network
      dependency on Jina).
    Kept tight (10s, no retry): the daemon serves this on the caller's own
    connection thread; the UI re-issues on failure.
    """
    if not url or not url.lower().startswith(("http://", "https://")):
        raise ValueError("url must start with http:// or https://")
    if mode == "readability":
        req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0 cosmic-fabric"})
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return _html_to_text(r.read().decode("utf-8", "replace"))
    req = urllib.request.Request("https://r.jina.ai/" + url,
                                 headers={"User-Agent": "cosmic-fabric"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.read().decode("utf-8", "replace").strip()


# ---------- ollama placement / status --------------------------------------
def _ollama_ps(ollama_url):
    with urllib.request.urlopen(ollama_url.rstrip("/") + "/api/ps", timeout=5) as r:
        return json.load(r).get("models", [])


def gpu_placement(model, ollama_url):
    try:
        for m in _ollama_ps(ollama_url):
            if model in (m.get("model"), m.get("name")):
                s, v = m.get("size", 0), m.get("size_vram", 0)
                return (100.0 * v / s) if s else None
    except Exception:
        pass
    return None


def loaded_models(ollama_url):
    """What's resident now: [{model, gpu_pct, ctx, vram_mib}]."""
    out = []
    try:
        for m in _ollama_ps(ollama_url):
            s, v = m.get("size", 0), m.get("size_vram", 0)
            out.append({
                "model": m.get("name") or m.get("model"),
                "gpu_pct": round(100.0 * v / s, 1) if s else None,
                "ctx": m.get("context_length"),
                "vram_mib": round(v / 1048576),
            })
    except Exception:
        pass
    return out


def gpu_vram():
    """{used, free, total} MiB via nvidia-smi, or None."""
    try:
        r = subprocess.run(["nvidia-smi", "--query-gpu=memory.used,memory.free,memory.total",
                            "--format=csv,noheader,nounits"], capture_output=True, text=True, timeout=5)
        used, free, total = (int(x.strip()) for x in r.stdout.splitlines()[0].split(","))
        return {"used": used, "free": free, "total": total}
    except Exception:
        return None

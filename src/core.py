"""cosmic-fabric shared core — policy, fabric REST client, ollama placement.

Used by `cosmic-fabricd`. This is the logic that will be ported to Rust for the
Phase-2 panel/settings; keeping it small and dependency-free (stdlib only) on
purpose. The launcher does NOT import this — it talks to the daemon over a socket.
"""
import json
import os
import re
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
class FabricClient:
    def __init__(self, url="http://localhost:8080", log=lambda m: None):
        self.url = url.rstrip("/")
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
        self.log("fabric --serve not up; starting it")
        try:
            subprocess.Popen(["fabric", "--serve"], env=env, start_new_session=True,
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
            timeout=600, on_chunk=None, model_ctx=None):
        """POST /chat (SSE) → accumulated text. If `on_chunk` is given, it's
        called with each content fragment as it streams. `model_ctx` sets the
        requested context window (`modelContextLength`) — used to right-size the
        Ollama KV cache for large inputs (web pages, files). Raises on error."""
        body = {
            "prompts": [{
                "userInput": user_input,
                "patternName": pattern,
                "model": model,
                "vendor": vendor,
                "variables": variables or {},
            }],
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

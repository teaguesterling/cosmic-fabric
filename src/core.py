"""cosmic-fabric shared core — policy, fabric REST client, ollama placement.

Used by `cosmic-fabricd`. This is the logic that will be ported to Rust for the
Phase-2 panel/settings; keeping it small and dependency-free (stdlib only) on
purpose. The launcher does NOT import this — it talks to the daemon over a socket.
"""
import http.client
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
        "tools": {},   # decision 5: [tools.<name>] sections (allow_domains, roots, …)
        # woollama inference seam: when enabled, plain (non-tool, non-image) runs
        # route their inference through the woollama router — fabric assembles the
        # prompt, woollama infers. Off by default; fabric stays the fallback.
        "woollama": {"enabled": False, "address": None, "bin": None},
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
        pol["tools"] = d.get("tools", {}) or {}
        pol["woollama"] = {**pol["woollama"], **(d.get("woollama", {}) or {})}
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


_INST_KEYS = ("model", "vendor", "ctx", "thinking", "temperature",
              "top_p", "frequency_penalty", "presence_penalty",
              "extra", "capabilities")
_VARIANT_PARAMS = ("ctx", "thinking", "temperature",
                   "top_p", "frequency_penalty", "presence_penalty",
                   "extra")


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
        "top_p": inst.get("top_p"),
        "frequency_penalty": inst.get("frequency_penalty"),
        "presence_penalty": inst.get("presence_penalty"),
        "extra": list(inst.get("extra", []) or []),
        "capabilities": list(inst.get("capabilities", []) or []),
        "categories": list(inst.get("categories", []) or []),
    }


def inst_to_options(inst):
    """Instantiation params → ChatRequest options. Typed fields (thinking,
    temperature, sampling) plus `extra` passthrough (CLI-style flags). Snake-
    case instantiation keys map to fabric's camelCase ChatOptions JSON tags."""
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
    # Sampling knobs (variant-typed, per decision 2). snake_case → camelCase.
    for src, dst in (("top_p", "topP"),
                     ("frequency_penalty", "frequencyPenalty"),
                     ("presence_penalty", "presencePenalty")):
        v = inst.get(src)
        if v is None:
            continue
        try:
            opt[dst] = float(v)
        except (TypeError, ValueError):
            pass
    return opt


# fabric ChatOptions (camelCase) → OpenAI chat/completions params (snake_case).
# Only the knobs that have a standard OpenAI equivalent carry over; fabric-only
# fields (thinking, suppressThink, raw) are dropped on the woollama path (a
# documented slice-1 gap — they'd need ollama `extra_body` to round-trip).
_OPENAI_OPT_MAP = {
    "temperature": "temperature",
    "topP": "top_p",
    "frequencyPenalty": "frequency_penalty",
    "presencePenalty": "presence_penalty",
}


def to_openai_options(inst):
    """Resolve an instantiation's sampling knobs to OpenAI-compatible params for
    the woollama path. Built on `inst_to_options` so policy semantics stay in one
    place; we just rename the fabric camelCase keys that OpenAI also understands."""
    fab = inst_to_options(inst)
    return {dst: fab[src] for src, dst in _OPENAI_OPT_MAP.items() if src in fab}


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
    so existing policy.toml keeps working over REST. Flag names mirror fabric's
    own CLI (verified from `fabric -h`)."""
    opt = {}
    # (cli flag prefix, ChatOptions key, parser) — ordered for predictable behavior.
    _FLOAT_FLAGS = (
        ("--temperature=",       "temperature",      float),
        ("--topp=",              "topP",             float),
        ("--frequencypenalty=",  "frequencyPenalty", float),
        ("--presencepenalty=",   "presencePenalty",  float),
    )
    for tok in extra:
        if tok.startswith("--thinking="):
            opt["thinking"] = tok.split("=", 1)[1]
        elif tok == "--suppress-think":
            opt["suppressThink"] = True
        elif tok == "--raw":
            opt["raw"] = True
        else:
            for prefix, key, cast in _FLOAT_FLAGS:
                if tok.startswith(prefix):
                    try:
                        opt[key] = cast(tok.split("=", 1)[1])
                    except ValueError:
                        pass
                    break
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
            timeout=600, on_chunk=None, model_ctx=None, session=None,
            context=None, strategy=None, language=None, search=False):
        """POST /chat (SSE) → accumulated text. If `on_chunk` is given, it's
        called with each content fragment as it streams. Beyond model/ctx/session,
        these are all native /chat fields: `variables` ({{vars}} substitution),
        `context` (a named context prepended), `strategy` (a prompt strategy),
        `language` (output language code), `search` (model-side web search).
        Raises on error."""
        prompt = {
            "userInput": user_input,
            "patternName": pattern or "",
            "model": model,
            "vendor": vendor,
            "variables": variables or {},
        }
        if session:
            prompt["sessionName"] = session
        if context:
            prompt["contextName"] = context
        if strategy:
            prompt["strategyName"] = strategy
        body = {
            "prompts": [prompt],
            "model": model,
        }
        if model_ctx:
            body["modelContextLength"] = model_ctx
        if language:
            body["language"] = language
        body.update(options or {})
        if search:
            body["search"] = True  # ChatOptions; after options so it isn't clobbered
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


# ---------- woollama router client (OpenAI-compatible) ----------------------
# The inference seam: when the [woollama] policy block is enabled, plain runs
# route through the woollama router instead of fabric's /chat. fabric still
# assembles the prompt; woollama does the inference (model "ollama/<id>"). We
# *discover* a live woollama (never spawn it — a missing server just falls back
# to fabric), mirroring woollama's own discovery file. See the integration plan.

def _woollama_runtime_dir():
    # Mirror woollama.binding._runtime_dir so we read the same path it writes.
    return os.environ.get("XDG_RUNTIME_DIR") or "/tmp"


def resolve_woollama_address():
    """The live woollama address (loopback `host:port`), or None if no server is
    discoverable. Precedence:
    1) $COSMIC_FABRIC_WOOLLAMA_ADDRESS (explicit override);
    2) the address woollama persists to $XDG_RUNTIME_DIR/woollama.addr.
    Unlike fabric we never spawn woollama; None just means 'fall back to fabric'."""
    env = os.environ.get("COSMIC_FABRIC_WOOLLAMA_ADDRESS")
    if env:
        return env.strip()
    try:
        with open(os.path.join(_woollama_runtime_dir(), "woollama.addr")) as f:
            addr = f.read().strip()
        return addr or None
    except Exception:
        return None


def _tcp_transport(addr):
    """'host:port' → ('tcp', host, port). None for an unparseable address."""
    if not addr:
        return None
    host, _, port = addr.strip().partition(":")
    try:
        return ("tcp", host or "127.0.0.1", int(port))
    except (TypeError, ValueError):
        return None


def resolve_woollama_transport(address=None):
    """How to reach woollama, preferring its owner-only local surface:
    1) an explicit `address` (policy [woollama].address or the env override) → TCP;
    2) the Unix socket $XDG_RUNTIME_DIR/woollama.sock (woollama's default for
       local clients; 0600; existence == discovery) → UDS;
    3) the persisted loopback TCP address (woollama degrades to TCP-only when it
       can't bind the socket).
    Returns ('unix', path) | ('tcp', host, port) | None (no server discoverable)."""
    explicit = address or os.environ.get("COSMIC_FABRIC_WOOLLAMA_ADDRESS")
    if explicit:
        return _tcp_transport(explicit)
    sock = os.path.join(_woollama_runtime_dir(), "woollama.sock")
    if os.path.exists(sock):
        return ("unix", sock)
    return _tcp_transport(resolve_woollama_address())


class _UnixHTTPConnection(http.client.HTTPConnection):
    """http.client speaking to an AF_UNIX stream socket — woollama's owner-only
    local surface. Same request/response machinery as HTTPConnection; only the
    underlying connect() differs."""

    def __init__(self, sock_path, timeout=None):
        super().__init__("localhost", timeout=timeout)
        self._sock_path = sock_path

    def connect(self):
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        if self.timeout is not None:
            s.settimeout(self.timeout)
        s.connect(self._sock_path)
        self.sock = s


def woollama_enabled(pol):
    """True when the [woollama] policy block opts the plain run path into the
    router. Defaults off (load_policy seeds enabled=False)."""
    return bool((pol.get("woollama") or {}).get("enabled"))


class WoollamaClient:
    """Minimal OpenAI-compatible client for the woollama router. stdlib-only, to
    match core.py's dependency-free style. `model` is woollama's namespaced id —
    `"ollama/<id>"` for local Ollama pass-through. Prefers woollama's owner-only
    Unix socket, falling back to the loopback TCP address (see
    `resolve_woollama_transport`). Pass `address` (policy `[woollama].address`) to
    force a specific TCP endpoint, or `transport` to inject one (tests)."""

    def __init__(self, address=None, transport=None, log=lambda m: None):
        self._address = address
        self._transport = transport if transport is not None else resolve_woollama_transport(address)
        self.log = log

    @property
    def url(self):
        """A display string for the resolved endpoint (for logs), or None."""
        t = self._transport
        if not t:
            return None
        return f"unix:{t[1]}" if t[0] == "unix" else f"http://{t[1]}:{t[2]}"

    def _connect(self, timeout):
        """An HTTPConnection for the resolved transport. Raises if no server."""
        t = self._transport
        if not t:
            raise RuntimeError("no woollama server discoverable")
        if t[0] == "unix":
            return _UnixHTTPConnection(t[1], timeout=timeout)
        return http.client.HTTPConnection(t[1], t[2], timeout=timeout)

    def alive(self):
        if not self._transport:
            return False
        try:
            conn = self._connect(2)
            try:
                conn.request("GET", "/v1/models")
                r = conn.getresponse()
                r.read()
                return r.status == 200
            finally:
                conn.close()
        except Exception:
            return False

    def _locate_woollamad(self, bin_hint=None):
        """Find the woollamad binary: explicit `[woollama].bin` → PATH (with
        ~/.local/bin prepended) → ~/.local/bin/woollamad. The Rust port may name
        the binary `woollama-server`, so try both. Returns a path or None."""
        import shutil
        if bin_hint:
            p = os.path.expanduser(bin_hint)
            if os.path.exists(p):
                return p
        search = os.path.expanduser("~/.local/bin") + os.pathsep + os.environ.get("PATH", "")
        for name in ("woollamad", "woollama-server"):
            p = shutil.which(name, path=search)
            if p:
                return p
        fallback = os.path.expanduser("~/.local/bin/woollamad")
        return fallback if os.path.exists(fallback) else None

    def ensure_serve(self, bin=None, wait=25):
        """Own a woollama instance. **Discover-first:** if one is already reachable
        (a running `woollama.service`, or any user-launched woollamad), attach to it
        and return. Otherwise spawn a **keyless** woollamad and wait for its socket.
        Keyless on purpose — an auto-spawned owner instance must not hold a billed
        `ANTHROPIC_API_KEY` (cloud still works when the *discovered* instance has one).
        Returns True if woollama is reachable afterward. Mirrors
        `FabricClient.ensure_serve` but never spawns when one is already up."""
        if self.alive():
            return True
        exe = self._locate_woollamad(bin)
        if not exe:
            self.log("woollama not running and no woollamad binary found "
                     "(set [woollama].bin or install woollamad); leaving inference to fabric")
            return False
        env = dict(os.environ)
        env["PATH"] = os.path.expanduser("~/.local/bin") + os.pathsep + env.get("PATH", "")
        env.pop("ANTHROPIC_API_KEY", None)  # keyless owner instance (budget)
        self.log(f"woollama not up; starting {exe} (keyless)")
        try:
            subprocess.Popen([exe], env=env, start_new_session=True,
                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        except Exception as e:
            self.log(f"failed to spawn woollamad: {e}")
            return False
        for _ in range(wait * 2):
            # the socket/.addr didn't exist at construction; re-resolve as it appears.
            self._transport = resolve_woollama_transport(self._address)
            if self.alive():
                self.log("woollama is up")
                return True
            time.sleep(0.5)
        self.log("woollama did not come up in time")
        return False

    def list_models(self, timeout=5):
        """GET /v1/models → the router's addressable model ids (e.g.
        'ollama/qwen3:14b', 'woollama/<recipe>'). Raises if no server."""
        conn = self._connect(timeout)
        try:
            conn.request("GET", "/v1/models")
            d = json.load(conn.getresponse())
        finally:
            conn.close()
        return [m["id"] for m in d.get("data", []) if m.get("id")]

    def list_patterns(self, timeout=5):
        """GET /w1/patterns → the pattern names woollama exposes (fabric-sourced +
        recipes). woollama's own `/w1` namespace (templating is not OpenAI, so it
        lives off `/v1`). Raises if no server."""
        conn = self._connect(timeout)
        try:
            conn.request("GET", "/w1/patterns")
            d = json.load(conn.getresponse())
        finally:
            conn.close()
        return [p["name"] for p in d.get("data", [])
                if isinstance(p, dict) and p.get("name")]

    def render(self, pattern, input_text, variables=None, timeout=30):
        """POST /w1/patterns/<name>/render → the rendered prompt (system + {{vars}}
        substituted + input), no model run — woollama owns the templating. For the
        `assemble` op (prompt preview / agent hand-off)."""
        from urllib.parse import quote
        body = {"input": input_text or ""}
        if variables:
            body["variables"] = variables
        conn = self._connect(timeout)
        try:
            conn.request("POST", f"/w1/patterns/{quote(pattern, safe='')}/render",
                         body=json.dumps(body).encode(),
                         headers={"Content-Type": "application/json"})
            resp = conn.getresponse()
            raw = resp.read()
            if resp.status >= 400:
                raise RuntimeError(
                    f"woollama /w1 render {resp.status}: {raw[:200].decode('utf-8', 'replace')}")
            d = json.loads(raw)
        finally:
            conn.close()
        return d.get("prompt", "")

    def _run_pattern_request(self, pattern, input_text, variables, model, options, stream, timeout):
        from urllib.parse import quote
        body = {"input": input_text or "", "stream": bool(stream)}
        if variables:
            body["variables"] = variables
        if model:
            body["model"] = model
        if options:
            body["options"] = options
        conn = self._connect(timeout)
        conn.request("POST", f"/w1/patterns/{quote(pattern, safe='')}/run",
                     body=json.dumps(body).encode(),
                     headers={"Content-Type": "application/json"})
        return conn

    def run_pattern(self, pattern, input_text, variables=None, model=None, options=None, timeout=600):
        """POST /w1/patterns/<name>/run → the assistant's text. woollama renders the
        pattern (system + {{vars}}) and infers; `model` overrides the recipe's
        inferencer per call. woollama owns the templating (its /w1 namespace)."""
        conn = self._run_pattern_request(pattern, input_text, variables, model, options, False, timeout)
        try:
            raw = conn.getresponse().read()
            d = json.loads(raw)
        finally:
            conn.close()
        return ((d.get("choices") or [{}])[0].get("message") or {}).get("content", "").strip()

    def run_pattern_stream(self, pattern, input_text, on_chunk=None, variables=None,
                           model=None, options=None, timeout=600):
        """POST /w1/patterns/<name>/run (stream:true) → accumulated text, calling
        `on_chunk` with each OpenAI SSE delta."""
        conn = self._run_pattern_request(pattern, input_text, variables, model, options, True, timeout)
        out = []
        try:
            for raw in conn.getresponse():
                line = raw.decode("utf-8", "replace").strip()
                if not line or not line.startswith("data:"):
                    continue
                line = line[5:].strip()
                if line == "[DONE]":
                    break
                try:
                    ev = json.loads(line)
                except json.JSONDecodeError:
                    continue
                delta = ((ev.get("choices") or [{}])[0].get("delta") or {}).get("content")
                if delta:
                    out.append(delta)
                    if on_chunk:
                        on_chunk(delta)
        finally:
            conn.close()
        return "".join(out).strip()

    def _body(self, model, prompt, options, stream):
        body = {
            "model": model,
            "messages": [{"role": "user", "content": prompt}],
            "stream": stream,
        }
        body.update(options or {})
        return body

    def chat(self, model, prompt, options=None, timeout=600):
        """POST /v1/chat/completions (non-streaming) → the assistant's text.
        Raises if no server is discoverable or the request fails."""
        conn = self._connect(timeout)
        try:
            conn.request("POST", "/v1/chat/completions",
                         body=json.dumps(self._body(model, prompt, options, False)).encode(),
                         headers={"Content-Type": "application/json"})
            d = json.load(conn.getresponse())
        finally:
            conn.close()
        return ((d.get("choices") or [{}])[0].get("message") or {}).get("content", "").strip()

    def chat_stream(self, model, prompt, on_chunk=None, options=None, timeout=600):
        """POST /v1/chat/completions with stream:true → accumulated text, calling
        `on_chunk` with each content delta. Parses OpenAI SSE (`data: {json}` lines,
        terminated by `data: [DONE]`)."""
        conn = self._connect(timeout)
        out = []
        try:
            conn.request("POST", "/v1/chat/completions",
                         body=json.dumps(self._body(model, prompt, options, True)).encode(),
                         headers={"Content-Type": "application/json"})
            for raw in conn.getresponse():  # HTTPResponse iterates by line
                line = raw.decode("utf-8", "replace").strip()
                if not line or not line.startswith("data:"):
                    continue
                line = line[5:].strip()
                if line == "[DONE]":
                    break
                try:
                    ev = json.loads(line)
                except json.JSONDecodeError:
                    continue
                delta = ((ev.get("choices") or [{}])[0].get("delta") or {}).get("content")
                if delta:
                    out.append(delta)
                    if on_chunk:
                        on_chunk(delta)
        finally:
            conn.close()
        return "".join(out).strip()

    def respond(self, model, input_text, conversation_id=None, key=None, timeout=600):
        """POST /v1/responses (stateful) → (assistant_text, conversation_id). For
        the state-owning backends (claude-resume / managed-agents). Non-streaming —
        woollama defers stateful streaming.

        Pass `key` (the caller's own session name) for attach-by-key: woollama owns
        the durable key→conversation map (persisted across woollama restarts), so we
        never hold the conversation id ourselves — the same key continues the same
        conversation. Alternatively pass an explicit `conversation_id` (it takes
        precedence). Neither starts a fresh, un-keyed conversation."""
        body = {"model": model, "input": input_text, "store": True}
        if conversation_id:
            body["conversation"] = conversation_id
        elif key:
            body["key"] = key
        conn = self._connect(timeout)
        try:
            conn.request("POST", "/v1/responses",
                         body=json.dumps(body).encode(),
                         headers={"Content-Type": "application/json"})
            resp = conn.getresponse()
            raw = resp.read()
            # Surface a non-2xx (e.g. 501 "no stateful backend for this model" when
            # no conversation store is wired) as an exception, so callers can fall
            # back rather than silently delivering an empty reply.
            if resp.status >= 400:
                raise RuntimeError(
                    f"woollama /v1/responses {resp.status}: "
                    f"{raw[:300].decode('utf-8', 'replace')}")
            d = json.loads(raw)
        finally:
            conn.close()
        # The conversation handle comes back as {"id": "conv_…"} (or a bare string).
        c = d.get("conversation")
        conv = c.get("id") if isinstance(c, dict) else c
        # Assistant text: prefer the message's `output_text` parts so we don't
        # surface a `reasoning` item; fall back to any text part. (status ==
        # "requires_action" — the managed-agents interactive path — yields no
        # output text here; that path isn't wired yet.)
        text, fallback = "", ""
        for item in (d.get("output") or []):
            for part in (item.get("content") or []):
                t = part.get("text") if isinstance(part, dict) else None
                if not t:
                    continue
                if part.get("type") == "output_text":
                    text = t
                    break
                fallback = fallback or t
            if text:
                break
        return (text or fallback).strip(), conv


def woollama_model(model, vendor):
    """cosmic-fabric `{model, vendor}` → woollama's namespaced model id. Ollama
    is the local pass-through prefix; other vendors map by lowercased name (the
    woollama inferencer registry: anthropic/openai/groq/…)."""
    return f"{(vendor or 'ollama').lower()}/{model}"


def woollama_status(pol):
    """Observability snapshot of the inference seam, for the `status` op / panel:
    whether routing is enabled, whether a server is reachable, the resolved
    endpoint (unix:… or http://…), and which backend a plain eligible run uses
    *right now* — woollama only when both enabled and reachable, else fabric."""
    enabled = woollama_enabled(pol)
    client = WoollamaClient(address=(pol.get("woollama") or {}).get("address"))
    reachable = client.alive()
    return {
        "enabled": enabled,
        "reachable": reachable,
        "endpoint": client.url,
        "active_backend": "woollama" if (enabled and reachable) else "fabric",
    }


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


# ---------- tool calling -----------------------------------------------------
# Local execution path for tool-using runs (decision 5; doc/design/
# tool-calling-plan.md). The daemon owns the loop end-to-end on this path;
# fabric is unadulterated for plain runs. The model speaks tool calls natively
# (Ollama /api/chat tools=[]); we validate, execute via Python callbacks,
# sanitize, and feed back. Three centralized hygiene rules in execute_tool /
# _sanitize_tool_result protect against the failures the live tests surfaced
# (D=exception, E=schema, H=empty result causes hallucination).

from dataclasses import dataclass
from typing import Callable, Optional
import urllib.parse


@dataclass
class ToolSpec:
    """Definition of one tool the daemon knows how to execute.

    `parameters` is a JSON-Schema fragment (the format Ollama + OpenAI use for
    function-call tools). `run` is the Python callback — it receives the parsed
    args dict and the current policy dict; it returns a string (or raises).
    `mode` determines execution: 'daemon' (in-process), 'panel-confirm' (panel
    must approve first), 'mcp' (future)."""
    name: str
    description: str
    parameters: dict
    run: Callable[[dict, dict], str]
    mode: str = "daemon"

    def to_chat_tool(self) -> dict:
        """The JSON shape passed to the model in `tools=[...]`. Ollama + OpenAI
        both consume this format directly."""
        return {
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters,
            },
        }


# ----- built-in tools (Axis 1, layer 1) -------------------------------------

def _http_get_safe(args: dict, policy: dict) -> str:
    """http_get callback. Fetch a URL, return cleaned **markdown** (not raw
    HTML) via the same Jina path `core.fetch_url(mode="scrape")` uses for the
    URL-source origin — way better signal density for the model. The URL's
    host must be on the policy `[tools.http_get] allow_domains` list; an empty
    or missing allow-list means *no domain is allowed* (deny-by-default).

    The URL is percent-encoded path-side before fetching so non-ASCII chars
    (e.g. "Reykjavík") don't blow up urllib's ASCII-only header handling.
    """
    url = args.get("url", "")
    if not isinstance(url, str) or not url.lower().startswith(("http://", "https://")):
        raise ValueError("url must be a string starting with http:// or https://")
    host = (urllib.parse.urlparse(url).hostname or "").lower()
    allow = ((policy.get("tools") or {}).get("http_get") or {}).get("allow_domains") or []
    allow = [h.lower() for h in allow]
    if not allow:
        raise PermissionError("http_get: no allow_domains configured in policy.toml "
                              "([tools.http_get] allow_domains = [...])")
    if host not in allow:
        raise PermissionError(f"http_get: host {host!r} is not in the allow-list {allow}")
    # Percent-encode non-ASCII path/query characters; leave scheme + host intact.
    parts = urllib.parse.urlsplit(url)
    safe_path = urllib.parse.quote(parts.path, safe="/%")
    safe_query = urllib.parse.quote(parts.query, safe="=&%")
    encoded = urllib.parse.urlunsplit(
        (parts.scheme, parts.netloc, safe_path, safe_query, parts.fragment))
    # Delegate to the shared fetch_url which already does Jina (clean markdown).
    return fetch_url(encoded, mode="scrape", timeout=30)


def _read_file_safe(args: dict, policy: dict) -> str:
    """read_file callback. Read a UTF-8 file by *relative* path under one of
    the configured roots (default: the daemon's cwd). Explicit defenses:
    - absolute paths rejected outright;
    - any `..` component rejected outright (no parent-dir traversal);
    - after resolution via realpath, the candidate must lie under a root —
      symlinks pointing outside are caught here, not by string prefix.
    The cwd default is per the design plan (NOT `$HOME`, which would expose
    `~/.ssh`, `~/.config/fabric/.env`, browser cookies, etc.)."""
    path_arg = args.get("path", "")
    if not isinstance(path_arg, str) or not path_arg:
        raise ValueError("path is required and must be a string")
    if os.path.isabs(path_arg):
        raise PermissionError("read_file: absolute paths not allowed; use a relative path")
    parts = path_arg.replace("\\", "/").split("/")
    if any(p == ".." for p in parts):
        raise PermissionError("read_file: parent-directory references (..) not allowed")
    roots_cfg = ((policy.get("tools") or {}).get("read_file") or {}).get("roots") or [os.getcwd()]
    roots = [os.path.realpath(os.path.expanduser(r)) for r in roots_cfg]
    for root in roots:
        candidate = os.path.realpath(os.path.join(root, path_arg))
        # `candidate` must be inside root (allows root itself for edge case)
        if not (candidate == root or candidate.startswith(root.rstrip(os.sep) + os.sep)):
            continue
        if os.path.isfile(candidate):
            with open(candidate, "r", encoding="utf-8", errors="replace") as f:
                return f.read()
    raise FileNotFoundError(f"read_file: {path_arg!r} not found under any configured root "
                            f"({len(roots)} root{'s' if len(roots) != 1 else ''})")


_HTTP_GET_PARAMS = {
    "type": "object",
    "properties": {
        "url": {"type": "string",
                "description": "The URL to fetch (http:// or https://). Host must be on the allow-list."},
    },
    "required": ["url"],
}

_READ_FILE_PARAMS = {
    "type": "object",
    "properties": {
        "path": {"type": "string",
                 "description": "Path relative to a configured root (cwd by default). No '..' or absolute paths."},
    },
    "required": ["path"],
}


def _run_shell_confirmed(args: dict, policy: dict) -> str:
    """run_shell_confirmed callback. ONLY called after panel approval (the
    gating happens upstream in `run_with_tools`'s panel-confirm branch — if it
    reaches this function, the user said Approve). Executes the command via a
    list-form subprocess (no shell metacharacter expansion), captures stdout +
    stderr, returns the combined output. 60s timeout. No environment variable
    inheritance beyond what subprocess.run defaults to."""
    cmd = args.get("command")
    if not isinstance(cmd, list) or not cmd:
        raise ValueError("command must be a non-empty list of strings "
                         "(no shell expansion — pass argv as an array)")
    if any(not isinstance(part, str) for part in cmd):
        raise ValueError("command must be a list of strings")
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=60,
                       stdin=subprocess.DEVNULL)
    out = (r.stdout or "")
    if r.stderr:
        out += ("\n[stderr]\n" + r.stderr)
    return out + f"\n[exit {r.returncode}]"


_RUN_SHELL_PARAMS = {
    "type": "object",
    "properties": {
        "command": {
            "type": "array",
            "items": {"type": "string"},
            "description": ("argv array — e.g. [\"git\",\"log\",\"-n\",\"5\",\"--oneline\"]. "
                            "No shell, so no glob/quote/redirect expansion. The user must "
                            "approve before execution."),
        },
    },
    "required": ["command"],
}


def builtin_tools() -> dict:
    """The code-defined tool registry (Axis 1 layer 1). Returns
    {name: ToolSpec}. The callbacks take (args, policy) so the registry can be
    constructed once and the current policy is supplied at execute time —
    avoids stale-policy bugs across config reloads."""
    return {
        "http_get": ToolSpec(
            name="http_get",
            description=("Fetch the contents of a URL as text. The host must be on the "
                         "allow-list configured in policy.toml. Use for retrieving public "
                         "web pages, REST endpoints, or any HTTP-accessible resource."),
            parameters=_HTTP_GET_PARAMS,
            run=_http_get_safe,
            mode="daemon",
        ),
        "read_file": ToolSpec(
            name="read_file",
            description=("Read a UTF-8 text file by relative path. Jailed to the configured "
                         "roots (cwd by default). Use for inspecting local source files, "
                         "configs, or documents the user has placed in scope."),
            parameters=_READ_FILE_PARAMS,
            run=_read_file_safe,
            mode="daemon",
        ),
        "run_shell_confirmed": ToolSpec(
            name="run_shell_confirmed",
            description=("Run a shell command (no shell expansion; argv array) AFTER the "
                         "user explicitly approves it in the panel. Use sparingly — only "
                         "when the task genuinely needs to execute something locally. "
                         "60s timeout."),
            parameters=_RUN_SHELL_PARAMS,
            run=_run_shell_confirmed,
            mode="panel-confirm",
        ),
    }


# ----- executor + hygiene (Axis 2) ------------------------------------------

TOOL_RESULT_MAX_CHARS = 16_000   # ~4K tokens; a typical Ollama context-window-friendly cap


def _validate_args(parameters: dict, args: dict) -> Optional[str]:
    """Minimal JSON-Schema validation: required keys present, primitive types
    match. Returns None on success, an error message string on failure. Not a
    full validator — covers the cases that matter for tool callbacks (missing
    required field; wrong primitive type) without pulling in a jsonschema dep.
    Letting un-validated extras through is intentional (forward compat)."""
    if not isinstance(args, dict):
        return f"args must be an object, got {type(args).__name__}"
    required = parameters.get("required") or []
    missing = [k for k in required if k not in args]
    if missing:
        return f"missing required args: {missing}"
    props = parameters.get("properties") or {}
    type_check = {
        "string":  lambda v: isinstance(v, str),
        "integer": lambda v: isinstance(v, int) and not isinstance(v, bool),
        "number":  lambda v: isinstance(v, (int, float)) and not isinstance(v, bool),
        "boolean": lambda v: isinstance(v, bool),
        "array":   lambda v: isinstance(v, list),
        "object":  lambda v: isinstance(v, dict),
    }
    for key, spec in props.items():
        if key not in args:
            continue
        wanted = spec.get("type")
        if wanted in type_check and not type_check[wanted](args[key]):
            return f"{key!r} must be {wanted}, got {type(args[key]).__name__}"
    return None


def sanitize_tool_result(name: str, raw) -> str:
    """Daemon-side hygiene before a tool result is fed back to the model.

    The H failure from the live tests (empty result → hallucinated value)
    lives here: empty/None/whitespace-only becomes an explicit sentinel rather
    than confusing the model into 'blank canvas' mode. Large results are
    truncated with an explicit marker the model can recognize."""
    if raw is None:
        return f"TOOL {name} RETURNED NO DATA"
    if isinstance(raw, str):
        if not raw.strip():
            return f"TOOL {name} RETURNED EMPTY STRING"
        s = raw
    else:
        s = str(raw)
    if len(s) > TOOL_RESULT_MAX_CHARS:
        head = s[:TOOL_RESULT_MAX_CHARS]
        return f"{head}\n[truncated: showed {TOOL_RESULT_MAX_CHARS} of {len(s)} chars]"
    return s


def execute_tool(spec: ToolSpec, args: dict, policy: dict) -> str:
    """Validate args, dispatch to the tool's callback, wrap exceptions,
    sanitize the result. Returns a string suitable for the `tool` message
    going back to the model.

    Note: panel-confirm tools must be approved by the panel *before* this is
    called; the gating happens upstream in the daemon loop.
    """
    err = _validate_args(spec.parameters, args)
    if err is not None:
        return f"SCHEMA ERROR for {spec.name}: {err}"
    try:
        raw = spec.run(args, policy)
    except Exception as e:
        return f"ERROR in {spec.name}: {type(e).__name__}: {e}"
    return sanitize_tool_result(spec.name, raw)


# ----- sidecar pattern metadata (Axis 1, the per-pattern declaration) -------

def pattern_meta(name: str, patterns_dir: Optional[str] = None) -> dict:
    """Read a pattern's sidecar `meta.toml` — cosmic-fabric-only metadata that
    fabric itself ignores (the pattern dir keeps its `system.md` unchanged).
    Returns `{}` when absent or unreadable.

    Schema (all optional):
        tool_use = true|false           # opt this pattern into the tool loop
        tools = ["http_get", ...]       # the allow-list of tool names; absent
                                        # means "any registered tool"
        tool_max_turns = 8              # per-pattern cap; falls back to default
    """
    base = patterns_dir or os.path.expanduser("~/.config/fabric/patterns")
    path = os.path.join(base, name, "meta.toml")
    if not os.path.isfile(path):
        return {}
    try:
        import tomllib  # Python 3.11+
        with open(path, "rb") as f:
            return tomllib.load(f) or {}
    except Exception:
        return {}


# ----- the multi-turn tool loop (Axis 3) ------------------------------------

DEFAULT_TOOL_MAX_TURNS = 8


def _filter_tools(registry: dict, allow_list) -> dict:
    """Tool-allow-list from pattern meta: an absent / empty `tools=[]` means
    *any registered tool is allowed*; an explicit list is least-privilege."""
    if not allow_list:
        return dict(registry)
    return {name: spec for name, spec in registry.items() if name in allow_list}


def run_with_tools(pattern, user_input, model, vendor, *,
                   policy, tools=None, on_chunk=None, on_event=None,
                   max_turns=DEFAULT_TOOL_MAX_TURNS, variables=None,
                   ollama_url="http://localhost:11434", confirm=None,
                   patterns_dir=None, log=lambda m: None):
    """Run a pattern through a bounded multi-turn tool loop, **bypassing fabric**.

    Phase 1 is Ollama-only: the model turn POSTs to `<ollama>/api/chat` with
    `tools=[...]`; tool calls are executed locally; results are appended as
    `role=tool` messages; the loop terminates when the model emits zero
    tool calls or `max_turns` is hit.

    Args:
        pattern: pattern name (read from disk for system prompt)
        user_input: user's input text (will be assembled in alongside the
                    pattern's system prompt)
        model: the resolved Ollama model name
        vendor: must be "Ollama" in Phase 1; other vendors raise NotImplementedError
        policy: the loaded policy dict (passed to each tool callback)
        tools: dict {name: ToolSpec} — the *resolved* registry for this run
               (already filtered by the pattern's allow-list)
        on_chunk: callable(text) for content fragments (called once per turn
                  that produces final content)
        on_event: callable(event_dict) for tool events:
                  {type: "tool_call", name, args, id}
                  {type: "tool_result", name, id, summary}
                  {type: "tool_confirm_required", name, args, id, command_preview}
        max_turns: hard cap on iterations (default 8)
        variables: {{var}} substitutions for the assembled prompt
        confirm: callable(name, args, command_preview) -> bool, used for
                 panel-confirm tools. None = auto-deny those (Phase 1 default
                 if no panel registered).

    Returns: the final assistant text accumulated across all turns.
    Raises: NotImplementedError for non-Ollama; urllib errors for network.
    """
    if (vendor or "").lower() != "ollama":
        raise NotImplementedError(
            f"run_with_tools: phase 1 supports Ollama only (got vendor={vendor!r})")
    if tools is None:
        tools = {}

    # Assemble the system prompt locally (same helper FabricClient uses for
    # `assemble_prompt`, lifted here so we don't hit fabric for the tool path).
    base = patterns_dir or os.path.expanduser("~/.config/fabric/patterns")
    sys_path = os.path.join(base, pattern, "system.md")
    try:
        with open(sys_path, "r", encoding="utf-8") as f:
            system_prompt = f.read()
    except FileNotFoundError:
        raise FileNotFoundError(f"pattern {pattern!r} not found at {sys_path}")
    if variables:
        for k, v in variables.items():
            system_prompt = system_prompt.replace("{{" + str(k) + "}}", str(v))

    messages = [
        {"role": "system", "content": system_prompt},
        {"role": "user",   "content": user_input or ""},
    ]
    tool_schemas = [spec.to_chat_tool() for spec in tools.values()]
    final_text_parts: list[str] = []

    chat_url = ollama_url.rstrip("/") + "/api/chat"

    for turn in range(1, max_turns + 1):
        body = {
            "model": model,
            "stream": False,    # phase 1: per-turn complete responses
            "messages": messages,
            "tools": tool_schemas,
            "options": {"temperature": 0},
        }
        log(f"tool-loop turn {turn}: posting {len(messages)} messages, "
            f"{len(tool_schemas)} tools, model={model}")
        req = urllib.request.Request(
            chat_url, method="POST",
            headers={"Content-Type": "application/json"},
            data=json.dumps(body).encode("utf-8"))
        with urllib.request.urlopen(req, timeout=300) as r:
            resp = json.load(r)

        msg = resp.get("message") or {}
        content = (msg.get("content") or "").strip()
        calls = msg.get("tool_calls") or []

        if calls:
            # Echo the assistant turn (with the calls) so the model sees its
            # own decisions when it gets the tool results back.
            messages.append({"role": "assistant", "content": content,
                             "tool_calls": calls})
            for c in calls:
                fn = c.get("function") or {}
                name = fn.get("name") or ""
                args = fn.get("arguments") or {}
                call_id = c.get("id") or f"call_{turn}_{name}"
                spec = tools.get(name)

                if spec is None:
                    result = f"ERROR: tool {name!r} is not available to this pattern"
                else:
                    # Panel-confirm gating happens BEFORE execute_tool.
                    if spec.mode == "panel-confirm":
                        # Build a preview for the panel: for run_shell_confirmed
                        # the preview is the rendered command; for others, fall
                        # back to a JSON arg dump.
                        cmd = args.get("command")
                        if isinstance(cmd, list):
                            preview = " ".join(cmd)
                        else:
                            preview = json.dumps(args)
                        if on_event:
                            on_event({"type": "tool_confirm_required",
                                      "name": name, "args": args, "id": call_id,
                                      "command_preview": preview})
                        # `confirm(call_id, name, args, preview)` — the daemon
                        # looks the call_id up in its pending registry to match
                        # the panel's separate-connection ack.
                        approved = bool(confirm(call_id, name, args, preview)) if confirm else False
                        if not approved:
                            result = f"USER DENIED execution of {name}({preview})"
                        else:
                            if on_event:
                                on_event({"type": "tool_call", "name": name,
                                          "args": args, "id": call_id})
                            result = execute_tool(spec, args, policy)
                    else:
                        if on_event:
                            on_event({"type": "tool_call", "name": name,
                                      "args": args, "id": call_id})
                        result = execute_tool(spec, args, policy)

                if on_event:
                    summary = result if len(result) <= 120 else result[:120] + "…"
                    on_event({"type": "tool_result", "name": name, "id": call_id,
                              "summary": summary})
                messages.append({"role": "tool", "content": result})
            # Loop continues — next turn the model sees the tool results.
            continue

        # No tool calls → final content for this turn.
        if content:
            final_text_parts.append(content)
            if on_chunk:
                on_chunk(content)
        log(f"tool-loop terminated at turn {turn} ({sum(len(p) for p in final_text_parts)} chars)")
        return "\n".join(final_text_parts).strip()

    # Hit the cap. Emit a clear marker, return whatever final text we accumulated.
    cap_msg = f"\n\n[tool-loop hit max_turns={max_turns} cap; stopped]"
    if on_chunk:
        on_chunk(cap_msg)
    final_text_parts.append(cap_msg)
    log(f"tool-loop hit max_turns={max_turns}")
    return "\n".join(final_text_parts).strip()

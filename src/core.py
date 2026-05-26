"""cosmic-fabric shared core — policy, fabric REST client, ollama placement.

Used by `cosmic-fabricd`. This is the logic that will be ported to Rust for the
Phase-2 panel/settings; keeping it small and dependency-free (stdlib only) on
purpose. The launcher does NOT import this — it talks to the daemon over a socket.
"""
import json
import os
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
    }
    try:
        import tomllib
        with open(POLICY, "rb") as f:
            d = tomllib.load(f)
        pol["default"] = {**pol["default"], **d.get("default", {})}
        pol["patterns"] = d.get("patterns", {})
        pol["output"] = {**pol["output"], **(d.get("output", {}) or {})}
        pol["ollama"] = {**pol["ollama"], **(d.get("ollama", {}) or {})}
    except FileNotFoundError:
        pass
    except Exception as e:  # malformed policy must not break the daemon
        print(f"[core] policy load error: {e}")
    return pol


def resolve_model(pattern, pol):
    c = {**pol["default"], **pol["patterns"].get(pattern, {})}
    return c.get("model"), c.get("vendor"), c.get("extra", []) or []


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

    def run(self, pattern, user_input, model, vendor, variables=None, options=None, timeout=600):
        """POST /chat (SSE) → accumulated text. Raises on a fabric error event."""
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
        if err:
            raise RuntimeError(err)
        return "".join(out).strip()


# ---------- ollama placement -----------------------------------------------
def gpu_placement(model, ollama_url):
    try:
        with urllib.request.urlopen(ollama_url.rstrip("/") + "/api/ps", timeout=5) as r:
            data = json.load(r)
        for m in data.get("models", []):
            if model in (m.get("model"), m.get("name")):
                s, v = m.get("size", 0), m.get("size_vram", 0)
                return (100.0 * v / s) if s else None
    except Exception:
        pass
    return None

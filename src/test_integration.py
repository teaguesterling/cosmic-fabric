"""End-to-end integration tests for the cosmic-fabric woollama client driven
against a live daemon over the REAL socket transport + discovery (vs test_core,
which is pure logic with mocked transport).

Two daemons, two classes:
  - `MockWoollama` — the `mock-woollamad` fixture (echo mode); fast, hermetic,
    asserts exact echo semantics. Always runnable.
  - `RealWoollama` — the REAL `woollamad` binary with a native templated recipe
    and (when fabric is installed) a managed `fabric --serve`; asserts real model
    output. Skipped unless a woollamad binary AND a reachable ollama are present.

Run: `python3 -m unittest test_integration`.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.request
from unittest.mock import patch

sys.path.insert(0, os.path.dirname(os.path.realpath(__file__)))
import core  # noqa: E402

HERE = os.path.dirname(os.path.realpath(__file__))
MOCK = os.path.join(HERE, "mock-woollamad")

# --- Real-daemon integration (complement to the mock) -----------------------
# The REAL woollamad binary: prefer an explicit WOOLLAMAD_BIN (point it at a
# build WITH the fabric backend), else whatever is on PATH — which may predate
# fabric, in which case the fabric-gated checks skip individually.
WOOLLAMAD_BIN = os.environ.get("WOOLLAMAD_BIN") or shutil.which("woollamad")
FABRIC_BIN = shutil.which("fabric")
OLLAMA_URL = os.environ.get("WOOLLAMA_TEST_OLLAMA_URL", "http://127.0.0.1:11434")
# A full inferencer id (e.g. "ollama/qwen2.5:1.5b"); auto-picked from ollama if unset.
TEST_MODEL = os.environ.get("WOOLLAMA_TEST_MODEL")
_SMALL_PREFS = ["qwen2.5:1.5b", "qwen2.5:1.5b-cpu", "llama3.2:latest", "gemma:2b"]


def _ollama_tags():
    """Installed ollama model tags, or None if ollama isn't reachable."""
    try:
        with urllib.request.urlopen(OLLAMA_URL + "/api/tags", timeout=3) as r:
            return [m["name"] for m in json.load(r).get("models", [])]
    except Exception:
        return None


class MockWoollama(unittest.TestCase):
    """Spawn mock-woollamad (echo mode) in a scratch runtime dir, point core's
    discovery at it via XDG_RUNTIME_DIR, and talk to it over the real socket."""

    def setUp(self):
        self.rt = tempfile.mkdtemp()
        env = dict(os.environ, XDG_RUNTIME_DIR=self.rt)
        env.pop("COSMIC_FABRIC_WOOLLAMA_ADDRESS", None)
        self.proc = subprocess.Popen(
            [sys.executable, MOCK, "--echo"], env=env,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        sock = os.path.join(self.rt, "woollama.sock")
        ready = False
        for _ in range(50):  # wait until it actually accepts, not just binds
            if os.path.exists(sock) and core.WoollamaClient(transport=("unix", sock)).alive():
                ready = True
                break
            time.sleep(0.1)
        if not ready:
            self.tearDown()
            self.skipTest("mock-woollamad did not come up")
        self._patch = patch.dict(os.environ,
                                 {"XDG_RUNTIME_DIR": self.rt}, clear=False)
        self._patch.start()
        os.environ.pop("COSMIC_FABRIC_WOOLLAMA_ADDRESS", None)

    def tearDown(self):
        try:
            self._patch.stop()
        except (AttributeError, RuntimeError):
            pass
        if getattr(self, "proc", None):
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except Exception:
                self.proc.kill()
        shutil.rmtree(self.rt, ignore_errors=True)

    def test_discovery_prefers_unix_socket(self):
        self.assertEqual(core.resolve_woollama_transport(),
                         ("unix", os.path.join(self.rt, "woollama.sock")))

    def test_status_active_when_enabled(self):
        s = core.woollama_status({"woollama": {"enabled": True}})
        self.assertTrue(s["reachable"])
        self.assertEqual(s["active_backend"], "woollama")
        self.assertTrue(s["endpoint"].startswith("unix:"))

    def test_chat_roundtrip(self):  # echo mode → reply == prompt
        self.assertEqual(core.WoollamaClient().chat("ollama/mock-model", "PING-123"),
                         "PING-123")

    def test_stream_roundtrip(self):
        chunks = []
        out = core.WoollamaClient().chat_stream(
            "ollama/mock-model", "STREAM-XYZ", on_chunk=chunks.append)
        self.assertEqual(out, "STREAM-XYZ")
        self.assertGreaterEqual(len(chunks), 1)

    def test_list_patterns(self):  # GET /w1/patterns
        self.assertIn("echo-pattern", core.WoollamaClient().list_patterns())

    def test_render(self):  # POST /w1/patterns/<name>/render — woollama owns templating
        prompt = core.WoollamaClient().render(
            "echo-pattern", "BODY-1", variables={"tone": "dry"})
        self.assertIn("echo-pattern", prompt)
        self.assertIn("tone=dry", prompt)  # variables reached woollama
        self.assertIn("BODY-1", prompt)

    def test_run_pattern(self):  # POST /w1/patterns/<name>/run
        out = core.WoollamaClient().run_pattern(
            "echo-pattern", "RUN-1", variables={"tone": "dry"}, model="ollama/mock-model")
        self.assertIn("echo-pattern", out)
        self.assertIn("RUN-1", out)

    def test_run_pattern_stream(self):
        chunks = []
        out = core.WoollamaClient().run_pattern_stream(
            "echo-pattern", "STREAMRUN", on_chunk=chunks.append)
        self.assertIn("STREAMRUN", out)
        self.assertGreaterEqual(len(chunks), 1)


@unittest.skipUnless(WOOLLAMAD_BIN, "no woollamad binary (set WOOLLAMAD_BIN or put it on PATH)")
class RealWoollama(unittest.TestCase):
    """The REAL WoollamaClient against the REAL woollamad over the unix socket —
    the complement to MockWoollama. setUpClass spawns woollamad once in an
    isolated $XDG_RUNTIME_DIR with a native templated recipe and (when fabric is
    installed) a managed `fabric --serve`; the tests drive every client method
    with real-output assertions.

    Skipped unless a woollamad binary AND a reachable ollama are present. The
    fabric-gated checks additionally need the daemon to include the fabric
    backend — point WOOLLAMAD_BIN at such a build; older daemons skip them.
    Override the model with WOOLLAMA_TEST_MODEL (a full id, e.g. ollama/qwen3).
    """

    proc = None
    rt = None
    cfg = None
    has_fabric = False
    model = None
    _old_xdg = None
    _log = None

    @classmethod
    def setUpClass(cls):
        tags = _ollama_tags()
        if not tags:
            raise unittest.SkipTest("ollama not reachable at " + OLLAMA_URL)
        if TEST_MODEL:
            cls.model = TEST_MODEL
        else:
            bare = next((m for m in _SMALL_PREFS if m in tags), tags[0])
            cls.model = "ollama/" + bare

        cls.rt = tempfile.mkdtemp(prefix="cf-real-rt-")
        cls.cfg = tempfile.mkdtemp(prefix="cf-real-cfg-")
        os.chmod(cls.rt, 0o700)

        tone = "{" + "{tone}" + "}"  # a templated variable in the native recipe
        with open(os.path.join(cls.cfg, "recipes.toml"), "w") as f:
            f.write(
                "[recipes.greeter]\n"
                'inferencer = "%s"\n' % cls.model
                + "tools = []\n"
                + 'system = "You are a %s assistant. Reply in one short sentence."\n' % tone
            )
        fabric_cfg = ""
        if FABRIC_BIN:
            fabric_cfg = (
                '"fabric": {"managed": true, "command": "%s", "default_model": "%s"}, '
                % (FABRIC_BIN, cls.model)
            )
        with open(os.path.join(cls.cfg, "mcp.json"), "w") as f:
            f.write("{" + fabric_cfg + '"mcpServers": {}}')

        env = dict(os.environ, XDG_RUNTIME_DIR=cls.rt,
                   WOOLLAMA_CONFIG_DIR=cls.cfg, WOOLLAMA_ADDRESS="127.0.0.1:0")
        env.pop("COSMIC_FABRIC_WOOLLAMA_ADDRESS", None)
        cls._log = open(os.path.join(cls.rt, "woollamad.log"), "w")
        cls.proc = subprocess.Popen(
            [WOOLLAMAD_BIN], env=env, stdout=cls._log, stderr=subprocess.STDOUT)

        sock = os.path.join(cls.rt, "woollama.sock")
        ready = False
        for _ in range(200):  # managed fabric spawn + ~268-pattern source is slow
            if os.path.exists(sock):
                try:
                    if core.WoollamaClient(transport=("unix", sock)).alive():
                        ready = True
                        break
                except Exception:
                    pass
            time.sleep(0.25)
        if not ready:
            cls._stop_proc()
            shutil.rmtree(cls.rt, ignore_errors=True)
            shutil.rmtree(cls.cfg, ignore_errors=True)
            raise unittest.SkipTest("real woollamad did not come up")

        cls._old_xdg = os.environ.get("XDG_RUNTIME_DIR")
        os.environ["XDG_RUNTIME_DIR"] = cls.rt
        os.environ.pop("COSMIC_FABRIC_WOOLLAMA_ADDRESS", None)

        # The daemon must expose the /w1 pattern surface — a build predating pattern
        # templating answers 404 here (non-JSON body). Skip the whole class cleanly
        # rather than erroring on every test.
        try:
            pats = core.WoollamaClient().list_patterns()
        except Exception:
            pats = None
        if not pats or "greeter" not in pats:
            cls._teardown_env()
            cls._stop_proc()
            shutil.rmtree(cls.rt, ignore_errors=True)
            shutil.rmtree(cls.cfg, ignore_errors=True)
            raise unittest.SkipTest(
                "daemon lacks the /w1 pattern surface (need a build with pattern templating)")
        cls.has_fabric = "ai" in pats

    @classmethod
    def _stop_proc(cls):
        if cls.proc:
            cls.proc.terminate()  # SIGTERM → woollamad kills its managed fabric child
            try:
                cls.proc.wait(timeout=10)
            except Exception:
                cls.proc.kill()
            cls.proc = None
        if cls._log:
            cls._log.close()
            cls._log = None

    @classmethod
    def _teardown_env(cls):
        if cls._old_xdg is None:
            os.environ.pop("XDG_RUNTIME_DIR", None)
        else:
            os.environ["XDG_RUNTIME_DIR"] = cls._old_xdg

    @classmethod
    def tearDownClass(cls):
        cls._stop_proc()
        cls._teardown_env()
        for d in (cls.rt, cls.cfg):
            if d:
                shutil.rmtree(d, ignore_errors=True)

    def _need_fabric(self):
        if not self.has_fabric:
            self.skipTest("daemon has no fabric backend (older build or fabric not installed)")

    # --- transport / discovery ---------------------------------------------
    def test_discovery_prefers_unix_socket(self):
        self.assertEqual(core.resolve_woollama_transport(),
                         ("unix", os.path.join(self.rt, "woollama.sock")))

    def test_status_reachable(self):
        s = core.woollama_status({"woollama": {"enabled": True}})
        self.assertTrue(s["reachable"])
        self.assertEqual(s["active_backend"], "woollama")
        self.assertTrue(s["endpoint"].startswith("unix:"))

    # --- discovery surfaces -------------------------------------------------
    def test_list_models_nonempty(self):
        self.assertTrue(len(core.WoollamaClient().list_models()) > 0)

    def test_list_patterns_has_native_recipe(self):
        self.assertIn("greeter", core.WoollamaClient().list_patterns())

    def test_list_patterns_has_fabric(self):
        self._need_fabric()
        self.assertIn("ai", core.WoollamaClient().list_patterns())

    # --- render (woollama owns templating) ---------------------------------
    def test_render_native_substitutes_var(self):
        prompt = core.WoollamaClient().render(
            "greeter", "BODY-1", variables={"tone": "dry"})
        self.assertIn("dry", prompt)                    # variable substituted
        self.assertNotIn("{" + "{tone}" + "}", prompt)  # no leftover token
        self.assertIn("BODY-1", prompt)                 # input appended

    def test_render_fabric_pattern(self):  # GET /patterns/{name} → render_system
        self._need_fabric()
        prompt = core.WoollamaClient().render("ai", "BODY-2")
        self.assertIn("BODY-2", prompt)
        self.assertGreater(len(prompt), len("BODY-2") + 20)  # real system text present

    # --- chat (/v1/chat/completions) ---------------------------------------
    def test_chat_nonempty(self):
        out = core.WoollamaClient().chat(self.model, "Reply with one short word.")
        self.assertTrue(isinstance(out, str) and out.strip())

    def test_chat_stream_multichunk(self):
        chunks = []
        out = core.WoollamaClient().chat_stream(
            self.model, "Count from 1 to 20, one number per line.",
            on_chunk=chunks.append)
        self.assertTrue(out.strip())
        self.assertGreater(len(chunks), 1)  # genuinely streamed, not buffered into one

    # --- run_pattern (render + infer) --------------------------------------
    def test_run_pattern_native(self):
        out = core.WoollamaClient().run_pattern(
            "greeter", "Say hello.", variables={"tone": "dry"})
        self.assertTrue(isinstance(out, str) and out.strip())

    def test_run_pattern_fabric(self):
        self._need_fabric()
        out = core.WoollamaClient().run_pattern("ai", "Say hello.", model=self.model)
        self.assertTrue(isinstance(out, str) and out.strip())

    def test_run_pattern_stream_fabric(self):
        self._need_fabric()
        chunks = []
        out = core.WoollamaClient().run_pattern_stream(
            "ai", "Say hello.", on_chunk=chunks.append, model=self.model)
        self.assertTrue(out.strip())
        self.assertGreaterEqual(len(chunks), 1)


if __name__ == "__main__":
    unittest.main()

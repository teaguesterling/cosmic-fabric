"""End-to-end integration tests for the cosmic-fabric woollama client driven
against a live daemon over the REAL socket transport + discovery (vs test_core,
which is pure logic with mocked transport).

Two daemons, two classes:
  - `MockWoollama` — the `mock-woollamad` fixture (echo mode); fast, hermetic,
    asserts exact echo semantics. Always runnable.
  - `RealWoollama` — the REAL `woollamad` binary with a native templated recipe
    and (when fabric is installed) a managed `fabric --serve`; asserts real model
    output. Skipped unless a woollamad binary AND a reachable ollama are present.
  - `DaemonWoollamaReroute` — drives the `cosmic-fabricd` daemon's own ops
    (`run`/`assemble`/`patterns`) against the mock with `FAB` stubbed to raise,
    proving Phase B routes templating through woollama's `/w1` and never touches
    fabric on the woollama path.

Run: `python3 -m unittest test_integration`.
"""
import copy
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.error
import urllib.request
from unittest.mock import patch

sys.path.insert(0, os.path.dirname(os.path.realpath(__file__)))
import core  # noqa: E402

HERE = os.path.dirname(os.path.realpath(__file__))
MOCK = os.path.join(HERE, "mock-woollamad")


def _load_daemon():
    """Import the hyphen-named, extensionless `cosmic-fabricd` script as a module
    (an explicit SourceFileLoader, since importlib can't infer one without a .py).
    Module-level code only defines things (`FAB` stays None until main()), safe."""
    import importlib.machinery
    path = os.path.join(HERE, "cosmic-fabricd")
    loader = importlib.machinery.SourceFileLoader("cosmic_fabricd", path)
    spec = importlib.util.spec_from_loader("cosmic_fabricd", loader)
    mod = importlib.util.module_from_spec(spec)
    loader.exec_module(mod)
    return mod


class _NoFabric:
    """A FAB stand-in that fails loudly if the daemon touches fabric — the proof
    that the woollama `/w1` path is fabric-free."""
    def __getattr__(self, name):
        raise AssertionError(f"fabric must not be touched on the woollama path (.{name})")

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
# The conversation store the isolated test daemon points at (conv-7 store-backed;
# makes ollama models stateful). Defaults to the reference rest-convstore's
# conventional local port; the conversations test skips cleanly if nothing answers.
CONVSTORE_URL = os.environ.get("WOOLLAMA_TEST_CONVSTORE_URL", "http://127.0.0.1:9000")


def _convstore_up():
    """True if SOMETHING answers HTTP at CONVSTORE_URL (any status — even a 404
    proves a listener; only a connection failure means 'down')."""
    try:
        urllib.request.urlopen(CONVSTORE_URL + "/", timeout=2)
        return True
    except urllib.error.HTTPError:
        return True
    except Exception:
        return False


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

    def test_run_pattern_forwards_extra(self):  # advanced fields → /w1/run body
        out = core.WoollamaClient().run_pattern(
            "echo-pattern", "B", extra={"strategy": "cot", "search": True})
        self.assertIn("strategy=cot", out)
        self.assertIn("search=True", out)


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
    state = None
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
        # A conversation store makes ollama models stateful on /v1/responses
        # (conv-7). The store connects per-request, so a dead URL only means the
        # conversations test skips — nothing else is affected.
        convstore_cfg = ('"conversationStore": {"type": "http", "url": "%s"}, '
                         % CONVSTORE_URL)
        with open(os.path.join(cls.cfg, "mcp.json"), "w") as f:
            f.write("{" + fabric_cfg + convstore_cfg + '"mcpServers": {}}')

        # XDG_STATE_HOME must be isolated too: the conversations test creates
        # handles, and the daemon persists its handle table to
        # $XDG_STATE_HOME/woollama/conversations.json — without this it would
        # write into (and pollute) the user's real live-daemon state.
        cls.state = tempfile.mkdtemp(prefix="cf-real-state-")
        env = dict(os.environ, XDG_RUNTIME_DIR=cls.rt,
                   WOOLLAMA_CONFIG_DIR=cls.cfg, WOOLLAMA_ADDRESS="127.0.0.1:0",
                   XDG_STATE_HOME=cls.state)
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
            shutil.rmtree(cls.state, ignore_errors=True)
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
            shutil.rmtree(cls.state, ignore_errors=True)
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
        for d in (cls.rt, cls.cfg, getattr(cls, "state", None)):
            if d:
                shutil.rmtree(d, ignore_errors=True)

    def _need_fabric(self):
        if not self.has_fabric:
            self.skipTest("daemon has no fabric backend (older build or fabric not installed)")

    # --- conversations (conv-2 discovery + conv-7 store-backed sessions) -----
    def test_conversations_recall_transcript_and_delete(self):
        """The full stateful-session journey cosmic-fabric drives: attach-by-key
        create → server-side recall on turn 2 (no client history) → discovery via
        /v1/conversations (key echoed) → transcript via /items → delete. Needs a
        conversation store (ollama models are stateless-only without one)."""
        if not _convstore_up():
            self.skipTest("no conversation store answering at " + CONVSTORE_URL)
        wool = core.WoollamaClient()
        # Unique per run: the store may be a long-lived shared service.
        key = "cosmic-fabric:test-%d-%d" % (os.getpid(), int(time.time()))

        # Turn 1 seeds a fact (attach-by-key creates the conversation)…
        text, conv = wool.respond(
            self.model,
            "Remember this: my favorite color is teal. Reply with one word: noted",
            key=key, timeout=300)
        self.assertTrue(conv, "responses returns the conversation handle")
        self.assertTrue(text, "turn 1 produced a reply")
        # …turn 2 with the SAME KEY recalls it server-side (we sent no history).
        recall, conv2 = wool.respond(
            self.model, "What is my favorite color? Answer with one word.",
            key=key, timeout=300)
        self.assertEqual(conv, conv2, "same key continues the same conversation")
        self.assertIn("teal", recall.lower(), "server-side recall (got: %r)" % recall)

        # Discovery: listed under our key, key echoed on the object.
        convs = [c for c in wool.conversations(key_prefix=key) if c.get("key") == key]
        self.assertEqual([c["id"] for c in convs], [conv], "conversation discoverable by key")

        # Transcript read (the resume-on-open path): all four turns, oldest first.
        msgs = wool.conversation_items(conv)
        self.assertGreaterEqual(len(msgs), 4, "two user + two assistant turns: %r" % msgs)
        self.assertEqual(msgs[0][0], "user")
        self.assertIn("teal", msgs[0][1].lower(), "turn-1 input at the head of the transcript")
        self.assertTrue(any(r == "assistant" for r, _ in msgs), "assistant turns present")

        # Delete ends the ROUTING HANDLE (the store owns/retains its bytes).
        self.assertTrue(wool.delete_conversation(conv))
        self.assertEqual(wool.conversations(key_prefix=key), [], "gone from discovery")
        self.assertFalse(wool.delete_conversation(conv), "second delete: already gone")

    def test_daemon_session_ops_list_transcript_delete(self):
        """The panel-facing daemon ops over the same surface: seed one session
        turn, then sessions_list shows the NAME (prefix stripped, no conv ids at
        the boundary), session_transcript returns the [role, text] history,
        session_delete ends it, and an unknown session reads as an empty
        transcript (never a create)."""
        if not _convstore_up():
            self.skipTest("no conversation store answering at " + CONVSTORE_URL)
        from unittest.mock import patch as _patch
        session = "test-ops-%d-%d" % (os.getpid(), int(time.time()))
        mod = _load_daemon()
        mod.FAB = _NoFabric()
        mod.log = lambda *a, **k: None
        pol = {"woollama": {"enabled": True}}
        with _patch.object(core, "load_policy", lambda: copy.deepcopy(pol)):
            # Seed a turn under the daemon's key namespace (what stream_run does).
            core.WoollamaClient().respond(
                self.model, "Say the word: seeded",
                key=mod.WOOLLAMA_KEY_PREFIX + session, timeout=300)

            got = mod.handle({"op": "sessions_list"})
            names = [s["session"] for s in got.get("sessions", [])]
            self.assertIn(session, names, "seeded session listed by NAME: %r" % names)
            self.assertNotIn("error", got)

            tr = mod.handle({"op": "session_transcript", "session": session})
            self.assertEqual(tr["session"], session)
            self.assertGreaterEqual(len(tr["messages"]), 2, tr)
            self.assertEqual(tr["messages"][0][0], "user")
            self.assertIn("seeded", tr["messages"][0][1])

            # Unknown session: empty transcript, NOT an error, NOT a create.
            tr2 = mod.handle({"op": "session_transcript", "session": "never-existed"})
            self.assertEqual(tr2["messages"], [])
            self.assertNotIn("error", tr2)

            got = mod.handle({"op": "session_delete", "session": session})
            self.assertTrue(got["deleted"])
            got = mod.handle({"op": "sessions_list"})
            self.assertNotIn(session, [s["session"] for s in got.get("sessions", [])])
            # Idempotent: deleting again reports deleted:false, no error.
            got = mod.handle({"op": "session_delete", "session": session})
            self.assertFalse(got["deleted"])
            self.assertNotIn("error", got)

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


class DaemonWoollamaReroute(unittest.TestCase):
    """Phase B: the `cosmic-fabricd` daemon routes pattern templating through
    woollama's `/w1` and never touches fabric on that path. `FAB` is stubbed to
    raise on any access, so a stray fabric call fails the test; the mock runs in
    echo mode so we can assert the pattern + variables + input reached `/w1`."""

    POL = {
        "default": {"model": "echo-model", "vendor": "x", "extra": []},
        "patterns": {}, "output": {"mode": "notify"},
        "ollama": {"bin": "", "url": "http://127.0.0.1:11434", "warn_below_gpu": 99},
        "surface": {"include": [], "exclude": []},
        "models": {}, "tools": {},
        "woollama": {"enabled": True, "address": None, "bin": None},
    }

    def setUp(self):
        self.rt = tempfile.mkdtemp()
        env = dict(os.environ, XDG_RUNTIME_DIR=self.rt)
        env.pop("COSMIC_FABRIC_WOOLLAMA_ADDRESS", None)
        self.proc = subprocess.Popen(
            [sys.executable, MOCK, "--echo"], env=env,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        sock = os.path.join(self.rt, "woollama.sock")
        ready = False
        for _ in range(50):
            if os.path.exists(sock) and core.WoollamaClient(transport=("unix", sock)).alive():
                ready = True
                break
            time.sleep(0.1)
        if not ready:
            self.tearDown()
            self.skipTest("mock-woollamad did not come up")
        self._patch = patch.dict(os.environ, {"XDG_RUNTIME_DIR": self.rt}, clear=False)
        self._patch.start()
        os.environ.pop("COSMIC_FABRIC_WOOLLAMA_ADDRESS", None)
        self._lp = patch.object(core, "load_policy", lambda: copy.deepcopy(self.POL))
        self._lp.start()
        self.mod = _load_daemon()
        self.mod.FAB = _NoFabric()
        self.mod.log = lambda *a, **k: None

    def tearDown(self):
        for attr in ("_lp", "_patch"):
            obj = getattr(self, attr, None)
            if obj is not None:
                try:
                    obj.stop()
                except (AttributeError, RuntimeError):
                    pass
        if getattr(self, "proc", None):
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except Exception:
                self.proc.kill()
        shutil.rmtree(self.rt, ignore_errors=True)

    def test_sync_run_routes_to_w1_not_fabric(self):
        # A plain pattern → /w1/run (woollama renders + infers); FAB stub untouched.
        out = self.mod._woollama_sync(
            copy.deepcopy(self.POL), "echo-pattern", "RUN-X",
            "qwen", "ollama", {}, {"tone": "dry"})
        self.assertIsNotNone(out)           # woollama handled it (None == fabric fallback)
        self.assertIn("echo-pattern", out)  # the pattern name reached /w1/run
        self.assertIn("tone=dry", out)      # variables reached /w1
        self.assertIn("RUN-X", out)         # the input reached /w1

    def test_recipe_run_uses_chat_not_w1(self):
        # vendor==woollama → recipe owns its prompt → /v1 chat (echo == raw input),
        # no fabric pattern stacked on top.
        out = self.mod._woollama_sync(
            copy.deepcopy(self.POL), "ignored-pat", "RAW-Y",
            "cc-assistant", "woollama", {}, None)
        self.assertEqual(out, "RAW-Y")

    def test_assemble_op_renders_via_w1(self):
        r = self.mod.handle({"op": "assemble", "pattern": "echo-pattern",
                             "input": "ASM-X", "variables": {"tone": "dry"}})
        self.assertIn("prompt", r)
        self.assertIn("echo-pattern", r["prompt"])
        self.assertIn("tone=dry", r["prompt"])
        self.assertIn("ASM-X", r["prompt"])

    def test_patterns_op_sources_from_w1(self):
        r = self.mod.handle({"op": "patterns"})
        self.assertIn("echo-pattern", r.get("patterns", []))

    def test_stream_run_routes_to_w1(self):
        chunks, done = [], []

        def send(msg):
            if "chunk" in msg:
                chunks.append(msg["chunk"])
            if msg.get("done"):
                done.append(msg)

        self.mod.stream_run(
            {"op": "run", "stream": True, "pattern": "echo-pattern",
             "input": "STREAM-X", "model": "qwen", "vendor": "x"}, send)
        joined = "".join(chunks)
        self.assertIn("echo-pattern", joined)   # pattern reached /w1/run (stream)
        self.assertIn("STREAM-X", joined)        # input reached /w1
        self.assertTrue(done and done[0].get("backend") == "woollama")

    def test_models_op_sources_from_woollama(self):
        # The model catalog now comes from woollama's /v1/models (unified picker),
        # split into a {vendor: [model]} map — not from fabric. The `woollama/<name>`
        # recipe/pattern namespace is excluded (those aren't inference models).
        r = self.mod.handle({"op": "models"})
        self.assertIn("ollama", r["vendors"])
        self.assertIn("mock-model", r["vendors"]["ollama"])
        self.assertNotIn("woollama", r["vendors"])  # woollama/echo is a recipe, not a model

    def test_advanced_fields_forwarded_to_w1(self):
        # context/strategy/language/search now route THROUGH woollama (forwarded to
        # its fabric backend), not to cosmic-fabric's fabric. Assert they reach /w1.
        extra = self.mod._woollama_extra(
            {"context": "myctx", "strategy": "cot", "language": "fr", "search": True})
        out = self.mod._woollama_sync(
            copy.deepcopy(self.POL), "echo-pattern", "X", "qwen", "ollama", {}, None, extra)
        self.assertIsNotNone(out)
        for token in ("context=myctx", "strategy=cot", "language=fr", "search=True"):
            self.assertIn(token, out)

    def test_sync_run_raises_on_woollama_request_error(self):
        # woollama is up but rejects the run (HTTP 500) → _woollama_sync raises (it
        # does NOT collapse to None), so the run op can surface the REAL error.
        with self.assertRaises(Exception):
            self.mod._woollama_sync(
                copy.deepcopy(self.POL), "err-pattern", "X", "qwen", "ollama", {}, None)

    def test_sync_run_returns_none_when_woollama_disabled(self):
        # Disabled → None (the caller renders 'backend unavailable'), no raise.
        pol = copy.deepcopy(self.POL)
        pol["woollama"]["enabled"] = False
        self.assertIsNone(
            self.mod._woollama_sync(pol, "echo-pattern", "X", "qwen", "ollama", {}, None))

    def test_run_op_surfaces_real_woollama_error(self):
        # A request-level woollama error is surfaced verbatim (not a generic
        # 'unavailable'), and never falls back to fabric (FAB stub would raise).
        r = self.mod.handle({"op": "run", "pattern": "err-pattern",
                             "input": "X", "model": "qwen", "vendor": "ollama"})
        self.assertIn("error", r)
        self.assertNotIn("output", r)
        self.assertIn("500", r["error"])  # the real /w1 error, not "unavailable"

    def test_stream_run_errors_on_woollama_failure_no_fabric(self):
        # woollama errors → the stream reports an error (no fabric fallback). FAB is
        # the raising stub from setUp, so a stray fabric call would fail the test.
        chunks, done, errors = [], [], []

        def send(msg):
            if "chunk" in msg:
                chunks.append(msg["chunk"])
            if msg.get("done"):
                done.append(msg)
            if "error" in msg:
                errors.append(msg["error"])

        self.mod.stream_run(
            {"op": "run", "stream": True, "pattern": "err-pattern",
             "input": "S", "model": "qwen", "vendor": "x"}, send)
        self.assertTrue(errors)     # error surfaced to the client
        self.assertFalse(done)      # no successful completion


if __name__ == "__main__":
    unittest.main()

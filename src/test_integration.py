"""End-to-end integration test: the cosmic-fabric woollama client driven against
the `mock-woollamad` fixture in an isolated $XDG_RUNTIME_DIR. Unlike test_core
(pure logic, mocked transport), this exercises the REAL socket transport +
discovery against a live process. Run: `python3 -m unittest test_integration`.
"""
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

sys.path.insert(0, os.path.dirname(os.path.realpath(__file__)))
import core  # noqa: E402

HERE = os.path.dirname(os.path.realpath(__file__))
MOCK = os.path.join(HERE, "mock-woollamad")


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


if __name__ == "__main__":
    unittest.main()

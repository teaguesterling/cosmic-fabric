"""Unit tests for the cosmic-fabric daemon engine (core.py) — pure logic only
(no network / GPU). Run: `cd src && python3 -m unittest test_core -v`.

Covers the pieces the daemon resolves on every run: model instantiations +
the capability rule, the active-set globs, context sizing, and option mapping.
"""
import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.realpath(__file__)))
import core  # noqa: E402


# A representative two-level model config (model → variants, categories, caps).
MODELS = {
    "qwen3": {
        "vendor": "Ollama", "model": "qwen3:14b-iq4xs",
        "capabilities": ["text"], "categories": ["local"], "default": "fast",
        "variants": {
            "fast": {"ctx": 2048, "thinking": "off", "temperature": 0.2, "categories": ["quick"]},
            "deep": {"ctx": 16384, "categories": ["thorough"]},
        },
    },
    "sonnet": {"vendor": "Anthropic", "model": "claude-sonnet-4-6",
               "capabilities": ["text", "vision"], "categories": ["cloud"]},
    "llama-vision": {"vendor": "Ollama", "model": "llama3.2-vision:latest",
                     "capabilities": ["text", "vision"], "categories": ["local", "vision"]},
}


class PickCtx(unittest.TestCase):
    def test_tiers(self):
        self.assertEqual(core.pick_ctx("x" * 10), 2048)
        self.assertEqual(core.pick_ctx("x" * 2000), 2048)
        self.assertEqual(core.pick_ctx("x" * 8000), 8192)      # ~2000 tok + margin
        self.assertEqual(core.pick_ctx("x" * 40000), 16384)
        self.assertEqual(core.pick_ctx("x" * 2_000_000), 32768)  # clamps to largest


class ActivePatterns(unittest.TestCase):
    ALL = ["scribe-summarize", "scribe-explain", "extract_wisdom", "create_quiz"]

    def test_empty_include_is_all(self):
        pol = {"surface": {"include": [], "exclude": []}}
        self.assertEqual(core.active_patterns(pol, self.ALL), self.ALL)

    def test_include_glob(self):
        pol = {"surface": {"include": ["scribe-*"], "exclude": []}}
        self.assertEqual(core.active_patterns(pol, self.ALL),
                         ["scribe-summarize", "scribe-explain"])

    def test_exclude_wins(self):
        pol = {"surface": {"include": ["*"], "exclude": ["scribe-explain"]}}
        self.assertNotIn("scribe-explain", core.active_patterns(pol, self.ALL))
        self.assertIn("scribe-summarize", core.active_patterns(pol, self.ALL))

    def test_exact_name_include(self):
        pol = {"surface": {"include": ["extract_wisdom"], "exclude": []}}
        self.assertEqual(core.active_patterns(pol, self.ALL), ["extract_wisdom"])


class Resolve(unittest.TestCase):
    def _pol(self, default, patterns=None):
        return {"default": default, "patterns": patterns or {}, "models": MODELS}

    def test_legacy_inline(self):
        r = core.resolve("p", self._pol({"model": "qwen3:14b-iq4xs", "vendor": "Ollama",
                                          "extra": ["--thinking=off"]}))
        self.assertEqual(r["model"], "qwen3:14b-iq4xs")
        self.assertEqual(r["vendor"], "Ollama")
        self.assertEqual(r["extra"], ["--thinking=off"])

    def test_named_default_variant(self):
        r = core.resolve("p", self._pol({"use": "qwen3"}))   # → its default variant "fast"
        self.assertEqual(r["model"], "qwen3:14b-iq4xs")
        self.assertEqual(r["ctx"], 2048)
        self.assertEqual(r["thinking"], "off")
        self.assertEqual(r["categories"], ["local", "quick"])  # model ∪ variant

    def test_named_explicit_variant(self):
        r = core.resolve("p", self._pol({"use": "qwen3/deep"}))
        self.assertEqual(r["ctx"], 16384)
        self.assertIsNone(r["thinking"])                       # deep doesn't set it
        self.assertEqual(r["categories"], ["local", "thorough"])

    def test_no_variant_model_resolves_to_base(self):
        r = core.resolve("p", self._pol({"use": "sonnet"}))
        self.assertEqual(r["model"], "claude-sonnet-4-6")
        self.assertEqual(r["vendor"], "Anthropic")
        self.assertIsNone(r["ctx"])

    def test_pattern_overrides_default(self):
        pol = self._pol({"use": "qwen3/fast"}, {"viz": {"use": "sonnet"}})
        self.assertEqual(core.resolve("viz", pol)["model"], "claude-sonnet-4-6")
        self.assertEqual(core.resolve("other", pol)["model"], "qwen3:14b-iq4xs")

    def test_unknown_use_falls_back(self):
        # a `use` naming a missing model → falls through to inline/default
        r = core.resolve("p", self._pol({"use": "ghost"}))
        self.assertIsNone(r["model"])  # nothing else to fall back to

    def test_sampling_carries_through_variant(self):
        # snake_case sampling fields on a variant flow through resolve → inst.
        models = {
            "spicy": {
                "vendor": "Ollama", "model": "x:1b",
                "default": "hot",
                "variants": {"hot": {"top_p": 0.8, "frequency_penalty": 0.5,
                                     "presence_penalty": 0.3}},
            },
        }
        r = core.resolve("p", {"default": {"use": "spicy"}, "patterns": {},
                               "models": models})
        self.assertAlmostEqual(r["top_p"], 0.8)
        self.assertAlmostEqual(r["frequency_penalty"], 0.5)
        self.assertAlmostEqual(r["presence_penalty"], 0.3)
        # and they emit on the wire:
        o = core.inst_to_options(r)
        self.assertAlmostEqual(o["topP"], 0.8)
        self.assertAlmostEqual(o["frequencyPenalty"], 0.5)
        self.assertAlmostEqual(o["presencePenalty"], 0.3)


class CapabilityRule(unittest.TestCase):
    def _pol(self):
        return {"default": {"use": "qwen3"}, "patterns": {}, "models": MODELS}

    def test_no_need_keeps_default(self):
        self.assertEqual(core.resolve("p", self._pol())["model"], "qwen3:14b-iq4xs")

    def test_vision_swaps_in_capable(self):
        r = core.resolve("p", self._pol(), need_capability="vision")
        self.assertIn("vision", r["capabilities"])

    def test_prefers_local_vision(self):
        # both sonnet (cloud) and llama-vision (local) can see → prefer local
        r = core.resolve("p", self._pol(), need_capability="vision")
        self.assertEqual(r["model"], "llama3.2-vision:latest")
        self.assertEqual(r["vendor"], "Ollama")

    def test_capable_default_not_swapped(self):
        # default already vision-capable → keep it
        pol = {"default": {"use": "sonnet"}, "patterns": {}, "models": MODELS}
        self.assertEqual(core.resolve("p", pol, need_capability="vision")["model"],
                         "claude-sonnet-4-6")

    def test_missing_capability_returns_default(self):
        # nothing satisfies the capability → leave the chosen instantiation as-is
        pol = {"default": {"use": "qwen3"}, "patterns": {},
               "models": {"qwen3": MODELS["qwen3"]}}
        self.assertEqual(core.resolve("p", pol, need_capability="audio")["model"],
                         "qwen3:14b-iq4xs")


class InstToOptions(unittest.TestCase):
    def test_thinking_off_suppresses(self):
        o = core.inst_to_options({"thinking": "off"})
        self.assertEqual(o["thinking"], "off")
        self.assertTrue(o["suppressThink"])

    def test_thinking_on_passthrough(self):
        self.assertEqual(core.inst_to_options({"thinking": "on"})["thinking"], "on")

    def test_temperature(self):
        self.assertAlmostEqual(core.inst_to_options({"temperature": 0.4})["temperature"], 0.4)

    def test_extra_flags(self):
        o = core.inst_to_options({"extra": ["--suppress-think", "--temperature=0.7"]})
        self.assertTrue(o["suppressThink"])
        self.assertAlmostEqual(o["temperature"], 0.7)

    def test_emits_sampling_knobs(self):
        # snake_case inst keys → camelCase ChatOptions keys (decision 2).
        o = core.inst_to_options({"top_p": 0.9, "frequency_penalty": 0.1,
                                  "presence_penalty": 0.2})
        self.assertAlmostEqual(o["topP"], 0.9)
        self.assertAlmostEqual(o["frequencyPenalty"], 0.1)
        self.assertAlmostEqual(o["presencePenalty"], 0.2)

    def test_skips_none_sampling_knobs(self):
        # None means "use the model default"; the keys must not appear at all.
        o = core.inst_to_options({"top_p": None, "frequency_penalty": None,
                                  "presence_penalty": None, "temperature": None})
        for k in ("topP", "frequencyPenalty", "presencePenalty", "temperature"):
            self.assertNotIn(k, o)

    def test_extra_to_options_legacy_sampling_flags(self):
        # fabric CLI flag names (verified from `fabric -h`) pass through.
        opt = core.extra_to_options(["--topp=0.85", "--presencepenalty=0.1",
                                     "--frequencypenalty=0.3"])
        self.assertAlmostEqual(opt["topP"], 0.85)
        self.assertAlmostEqual(opt["presencePenalty"], 0.1)
        self.assertAlmostEqual(opt["frequencyPenalty"], 0.3)


class ToolHygiene(unittest.TestCase):
    """The four hygiene rules from doc/design/tool-calling-plan.md, Axis 2."""

    def test_sanitize_normal_passthrough(self):
        self.assertEqual(core.sanitize_tool_result("x", "hello"), "hello")

    def test_sanitize_empty_string_sentinel(self):
        # The H fix — empty result must NOT be a blank canvas for hallucination
        self.assertEqual(core.sanitize_tool_result("x", ""), "TOOL x RETURNED EMPTY STRING")

    def test_sanitize_whitespace_only_sentinel(self):
        self.assertEqual(core.sanitize_tool_result("x", "  \n\t "),
                         "TOOL x RETURNED EMPTY STRING")

    def test_sanitize_none_sentinel(self):
        self.assertEqual(core.sanitize_tool_result("x", None), "TOOL x RETURNED NO DATA")

    def test_sanitize_truncates_large(self):
        out = core.sanitize_tool_result("x", "a" * (core.TOOL_RESULT_MAX_CHARS + 100))
        self.assertTrue(out.startswith("a"))
        self.assertIn(f"showed {core.TOOL_RESULT_MAX_CHARS}", out)
        self.assertIn(f"of {core.TOOL_RESULT_MAX_CHARS + 100} chars", out)


class ValidateArgs(unittest.TestCase):
    PARAMS = {"type": "object",
              "properties": {"url": {"type": "string"}, "n": {"type": "integer"}},
              "required": ["url"]}

    def test_valid(self):
        self.assertIsNone(core._validate_args(self.PARAMS, {"url": "x", "n": 1}))

    def test_missing_required(self):
        self.assertIn("missing required", core._validate_args(self.PARAMS, {}))

    def test_wrong_type(self):
        err = core._validate_args(self.PARAMS, {"url": 42})
        self.assertIn("must be string", err)

    def test_extras_pass(self):
        # Extras beyond declared properties are intentionally allowed.
        self.assertIsNone(core._validate_args(self.PARAMS, {"url": "x", "extra": True}))

    def test_args_not_dict(self):
        self.assertIn("must be an object", core._validate_args(self.PARAMS, "nope"))


class ExecuteTool(unittest.TestCase):
    def test_clean_call(self):
        spec = core.ToolSpec(name="echo", description="",
                             parameters={"type": "object",
                                         "properties": {"s": {"type": "string"}},
                                         "required": ["s"]},
                             run=lambda args, pol: f"echoed: {args['s']}")
        self.assertEqual(core.execute_tool(spec, {"s": "hi"}, {}), "echoed: hi")

    def test_schema_error_does_not_invoke_callback(self):
        called = []
        spec = core.ToolSpec(name="echo", description="",
                             parameters={"type": "object",
                                         "properties": {"s": {"type": "string"}},
                                         "required": ["s"]},
                             run=lambda args, pol: called.append(1) or "ok")
        out = core.execute_tool(spec, {}, {})
        self.assertIn("SCHEMA ERROR", out)
        self.assertEqual(called, [], "callback ran despite schema error")

    def test_exception_wrapped(self):
        def boom(args, pol):
            raise ConnectionError("auth server unreachable")
        spec = core.ToolSpec(name="lookup", description="",
                             parameters={"type": "object"},
                             run=boom)
        out = core.execute_tool(spec, {}, {})
        self.assertIn("ERROR in lookup: ConnectionError", out)
        self.assertIn("auth server unreachable", out)

    def test_empty_result_hits_hygiene(self):
        # The end-to-end H fix: callback returns "" → executor returns sentinel.
        spec = core.ToolSpec(name="lookup", description="",
                             parameters={"type": "object"},
                             run=lambda args, pol: "")
        self.assertEqual(core.execute_tool(spec, {}, {}),
                         "TOOL lookup RETURNED EMPTY STRING")


class HttpGetAllowList(unittest.TestCase):
    def test_denied_by_default(self):
        # No [tools.http_get] in policy = no allow-list = deny everything.
        with self.assertRaises(PermissionError):
            core._http_get_safe({"url": "https://example.com"}, {})

    def test_disallowed_host(self):
        pol = {"tools": {"http_get": {"allow_domains": ["allowed.example"]}}}
        with self.assertRaises(PermissionError):
            core._http_get_safe({"url": "https://evil.example"}, pol)

    def test_url_scheme_required(self):
        pol = {"tools": {"http_get": {"allow_domains": ["x"]}}}
        with self.assertRaises(ValueError):
            core._http_get_safe({"url": "ftp://x/y"}, pol)


class ReadFileJail(unittest.TestCase):
    def test_rejects_absolute_path(self):
        with self.assertRaises(PermissionError):
            core._read_file_safe({"path": "/etc/passwd"}, {})

    def test_rejects_dotdot(self):
        with self.assertRaises(PermissionError):
            core._read_file_safe({"path": "../something"}, {})

    def test_reads_file_under_root(self):
        import tempfile, os as _os
        with tempfile.TemporaryDirectory() as td:
            p = _os.path.join(td, "hello.txt")
            with open(p, "w") as f:
                f.write("hi from the test")
            pol = {"tools": {"read_file": {"roots": [td]}}}
            self.assertEqual(
                core._read_file_safe({"path": "hello.txt"}, pol),
                "hi from the test")

    def test_missing_file_raises(self):
        import tempfile
        with tempfile.TemporaryDirectory() as td:
            pol = {"tools": {"read_file": {"roots": [td]}}}
            with self.assertRaises(FileNotFoundError):
                core._read_file_safe({"path": "nope.txt"}, pol)


class ToolLoop(unittest.TestCase):
    """The multi-turn loop, mocked Ollama: turn 1 emits a tool_call, turn 2
    produces final text. Verifies termination + event dispatch + accumulation."""

    def _mock_urlopen(self, responses):
        """Return a context-manager-shaped fake urlopen that yields the next
        JSON response from `responses` on each call."""
        import io
        from contextlib import contextmanager
        @contextmanager
        def mock(_req, timeout=None):
            payload = json.dumps(responses.pop(0)).encode()
            yield io.BytesIO(payload)
        return mock

    def _pattern_dir_with_system(self, td, name, system_text):
        import os as _os
        d = _os.path.join(td, name)
        _os.makedirs(d, exist_ok=True)
        with open(_os.path.join(d, "system.md"), "w") as f:
            f.write(system_text)
        return td

    def test_two_turn_loop_terminates(self):
        import tempfile, json as _json
        from unittest.mock import patch
        responses = [
            # Turn 1 — model wants the tool
            {"message": {"content": "",
                          "tool_calls": [{"id": "c1",
                                          "function": {"name": "echo",
                                                       "arguments": {"s": "hello"}}}]}},
            # Turn 2 — final text after seeing the tool result
            {"message": {"content": "The tool said: echoed: hello", "tool_calls": []}},
        ]
        with tempfile.TemporaryDirectory() as td:
            self._pattern_dir_with_system(td, "p", "You are a test pattern.")
            spec = core.ToolSpec(name="echo", description="",
                                 parameters={"type": "object",
                                             "properties": {"s": {"type": "string"}},
                                             "required": ["s"]},
                                 run=lambda args, pol: f"echoed: {args['s']}")
            events = []
            chunks = []
            with patch("urllib.request.urlopen", self._mock_urlopen(responses)):
                out = core.run_with_tools(
                    "p", "say hi via the tool",
                    model="qwen3:14b-iq4xs", vendor="Ollama",
                    policy={}, tools={"echo": spec},
                    on_chunk=chunks.append, on_event=events.append,
                    patterns_dir=td,
                )
        self.assertEqual(out, "The tool said: echoed: hello")
        self.assertEqual(chunks, ["The tool said: echoed: hello"])
        kinds = [e["type"] for e in events]
        self.assertEqual(kinds, ["tool_call", "tool_result"])
        self.assertEqual(events[0]["name"], "echo")
        self.assertEqual(responses, [], "all mocked responses must have been consumed")

    def test_max_turns_cap_engages(self):
        import tempfile, json as _json
        from unittest.mock import patch
        # Model never stops calling the tool — cap should fire.
        call_resp = {"message": {"content": "",
                                 "tool_calls": [{"id": "c", "function": {"name": "echo",
                                                                          "arguments": {"s": "x"}}}]}}
        responses = [dict(call_resp) for _ in range(20)]
        with tempfile.TemporaryDirectory() as td:
            self._pattern_dir_with_system(td, "p", "test")
            spec = core.ToolSpec(name="echo", description="",
                                 parameters={"type": "object",
                                             "properties": {"s": {"type": "string"}},
                                             "required": ["s"]},
                                 run=lambda args, pol: "ok")
            with patch("urllib.request.urlopen", self._mock_urlopen(responses)):
                out = core.run_with_tools(
                    "p", "go", model="qwen3:14b-iq4xs", vendor="Ollama",
                    policy={}, tools={"echo": spec},
                    max_turns=3, patterns_dir=td)
        self.assertIn("hit max_turns=3", out)

    def test_non_ollama_vendor_raises(self):
        with self.assertRaises(NotImplementedError):
            core.run_with_tools("p", "x", model="claude-opus-4-7",
                                vendor="Anthropic", policy={}, tools={})


class PatternMeta(unittest.TestCase):
    def test_missing_meta_returns_empty(self):
        import tempfile, os as _os
        with tempfile.TemporaryDirectory() as td:
            _os.makedirs(_os.path.join(td, "p"))
            self.assertEqual(core.pattern_meta("p", patterns_dir=td), {})

    def test_reads_tool_use(self):
        import tempfile, os as _os
        with tempfile.TemporaryDirectory() as td:
            d = _os.path.join(td, "p")
            _os.makedirs(d)
            with open(_os.path.join(d, "meta.toml"), "w") as f:
                f.write('tool_use = true\ntools = ["http_get"]\n')
            m = core.pattern_meta("p", patterns_dir=td)
            self.assertTrue(m["tool_use"])
            self.assertEqual(m["tools"], ["http_get"])

    def test_broken_meta_returns_empty(self):
        import tempfile, os as _os
        with tempfile.TemporaryDirectory() as td:
            d = _os.path.join(td, "p")
            _os.makedirs(d)
            with open(_os.path.join(d, "meta.toml"), "w") as f:
                f.write('this is not valid TOML = =')
            self.assertEqual(core.pattern_meta("p", patterns_dir=td), {})


class HtmlToText(unittest.TestCase):
    def test_strips_tags_and_scripts(self):
        html = "<html><head><style>x{}</style></head><body><p>Hello</p><script>bad()</script><p>World</p></body></html>"
        t = core._html_to_text(html)
        self.assertIn("Hello", t)
        self.assertIn("World", t)
        self.assertNotIn("bad()", t)
        self.assertNotIn("<p>", t)


class WoollamaSeam(unittest.TestCase):
    """The woollama inference seam (slice 1): model-name mapping, option
    translation, discovery, and the OpenAI request/SSE parsing — all offline."""

    def _fake_conn(self, payload):
        """A fake http.client connection whose getresponse() yields canned bytes
        (a BytesIO subclass works for json.load, line iteration, and .status)."""
        import io
        data = payload if isinstance(payload, bytes) else payload.encode()
        class _Resp(io.BytesIO):
            status = 200
        class _Conn:
            def request(self, *a, **k): pass
            def getresponse(self): return _Resp(data)
            def close(self): pass
        return _Conn()

    def test_model_mapping(self):
        self.assertEqual(core.woollama_model("qwen3:14b-iq4xs", "Ollama"),
                         "ollama/qwen3:14b-iq4xs")
        self.assertEqual(core.woollama_model("claude-sonnet-4-6", "Anthropic"),
                         "anthropic/claude-sonnet-4-6")
        self.assertEqual(core.woollama_model("x:1b", None), "ollama/x:1b")  # default prefix

    def test_to_openai_options_renames(self):
        o = core.to_openai_options({"temperature": 0.4, "top_p": 0.9,
                                    "frequency_penalty": 0.1, "presence_penalty": 0.2})
        self.assertAlmostEqual(o["temperature"], 0.4)
        self.assertAlmostEqual(o["top_p"], 0.9)
        self.assertAlmostEqual(o["frequency_penalty"], 0.1)
        self.assertAlmostEqual(o["presence_penalty"], 0.2)

    def test_to_openai_options_drops_fabric_only(self):
        # fabric-only knobs (thinking/suppressThink/raw) must NOT leak to OpenAI.
        o = core.to_openai_options({"thinking": "off", "extra": ["--raw"]})
        self.assertEqual(o, {})

    def test_woollama_enabled(self):
        self.assertFalse(core.woollama_enabled({}))
        self.assertFalse(core.woollama_enabled({"woollama": {"enabled": False}}))
        self.assertTrue(core.woollama_enabled({"woollama": {"enabled": True}}))

    def test_resolve_address_env_override(self):
        import os as _os
        from unittest.mock import patch
        with patch.dict(_os.environ, {"COSMIC_FABRIC_WOOLLAMA_ADDRESS": "127.0.0.1:9999"}):
            self.assertEqual(core.resolve_woollama_address(), "127.0.0.1:9999")

    def test_resolve_address_from_file(self):
        import os as _os, tempfile
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as td:
            with open(_os.path.join(td, "woollama.addr"), "w") as f:
                f.write("127.0.0.1:48291\n")
            env = {k: v for k, v in _os.environ.items()
                   if k != "COSMIC_FABRIC_WOOLLAMA_ADDRESS"}
            env["XDG_RUNTIME_DIR"] = td
            with patch.dict(_os.environ, env, clear=True):
                self.assertEqual(core.resolve_woollama_address(), "127.0.0.1:48291")

    def test_resolve_address_none_when_absent(self):
        import os as _os, tempfile
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as td:
            env = {k: v for k, v in _os.environ.items()
                   if k != "COSMIC_FABRIC_WOOLLAMA_ADDRESS"}
            env["XDG_RUNTIME_DIR"] = td  # empty dir → no .addr
            with patch.dict(_os.environ, env, clear=True):
                self.assertIsNone(core.resolve_woollama_address())

    def _empty_runtime_env(self, td):
        import os as _os
        env = {k: v for k, v in _os.environ.items()
               if k != "COSMIC_FABRIC_WOOLLAMA_ADDRESS"}
        env["XDG_RUNTIME_DIR"] = td
        return env

    def test_client_no_server_is_not_alive(self):
        import os as _os, tempfile
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as td:  # no .sock / .addr → no server
            with patch.dict(_os.environ, self._empty_runtime_env(td), clear=True):
                c = core.WoollamaClient()
                self.assertIsNone(c.url)
                self.assertFalse(c.alive())
                with self.assertRaises(RuntimeError):
                    c.chat("ollama/x", "hi")

    def test_transport_prefers_unix_socket(self):
        import os as _os, tempfile
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as td:
            sock = _os.path.join(td, "woollama.sock")
            open(sock, "w").close()
            with open(_os.path.join(td, "woollama.addr"), "w") as f:
                f.write("127.0.0.1:5\n")  # both present → socket wins
            with patch.dict(_os.environ, self._empty_runtime_env(td), clear=True):
                self.assertEqual(core.resolve_woollama_transport(), ("unix", sock))

    def test_transport_falls_back_to_tcp(self):
        import os as _os, tempfile
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as td:
            with open(_os.path.join(td, "woollama.addr"), "w") as f:
                f.write("127.0.0.1:43251\n")  # no socket → use the .addr
            with patch.dict(_os.environ, self._empty_runtime_env(td), clear=True):
                self.assertEqual(core.resolve_woollama_transport(),
                                 ("tcp", "127.0.0.1", 43251))

    def test_transport_explicit_address_wins(self):
        import os as _os, tempfile
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as td:
            open(_os.path.join(td, "woollama.sock"), "w").close()  # present but ignored
            with patch.dict(_os.environ, self._empty_runtime_env(td), clear=True):
                self.assertEqual(core.resolve_woollama_transport("127.0.0.1:9001"),
                                 ("tcp", "127.0.0.1", 9001))

    def test_transport_none_when_absent(self):
        import os as _os, tempfile
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as td:
            with patch.dict(_os.environ, self._empty_runtime_env(td), clear=True):
                self.assertIsNone(core.resolve_woollama_transport())

    def test_status_disabled_no_server(self):
        import os as _os, tempfile
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as td:
            with patch.dict(_os.environ, self._empty_runtime_env(td), clear=True):
                self.assertEqual(core.woollama_status({}),
                                 {"enabled": False, "reachable": False,
                                  "endpoint": None, "active_backend": "fabric"})

    def test_status_enabled_but_unreachable_is_fabric(self):
        import os as _os, tempfile
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as td:  # no server discoverable
            with patch.dict(_os.environ, self._empty_runtime_env(td), clear=True):
                s = core.woollama_status({"woollama": {"enabled": True}})
                self.assertTrue(s["enabled"])
                self.assertFalse(s["reachable"])
                self.assertEqual(s["active_backend"], "fabric")  # reachability gates it

    def test_status_enabled_and_reachable_is_woollama(self):
        import os as _os, tempfile
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as td:
            with open(_os.path.join(td, "woollama.addr"), "w") as f:
                f.write("127.0.0.1:8888\n")
            with patch.dict(_os.environ, self._empty_runtime_env(td), clear=True), \
                 patch.object(core.WoollamaClient, "alive", lambda self: True):
                s = core.woollama_status({"woollama": {"enabled": True}})
                self.assertTrue(s["reachable"])
                self.assertEqual(s["endpoint"], "http://127.0.0.1:8888")
                self.assertEqual(s["active_backend"], "woollama")

    def test_chat_parses_openai_response(self):
        body = json.dumps({"choices": [{"message": {"role": "assistant",
                                                    "content": "  hello there  "}}]})
        c = core.WoollamaClient(transport=("tcp", "127.0.0.1", 1))
        c._connect = lambda timeout: self._fake_conn(body)
        self.assertEqual(c.chat("ollama/x", "hi"), "hello there")

    def test_chat_stream_parses_sse(self):
        # OpenAI SSE: data lines (some empty deltas), blank lines, then [DONE].
        sse = (
            'data: {"choices":[{"delta":{"role":"assistant"}}]}\n'
            '\n'
            'data: {"choices":[{"delta":{"content":"Hel"}}]}\n'
            'data: {"choices":[{"delta":{"content":"lo"}}]}\n'
            ': a comment line that is not data\n'
            'data: {"choices":[{"delta":{}}]}\n'
            'data: [DONE]\n'
        )
        c = core.WoollamaClient(transport=("tcp", "127.0.0.1", 1))
        c._connect = lambda timeout: self._fake_conn(sse)
        chunks = []
        out = c.chat_stream("ollama/x", "hi", on_chunk=chunks.append)
        self.assertEqual(out, "Hello")
        self.assertEqual(chunks, ["Hel", "lo"])

    def test_unix_transport_uses_af_unix_connection(self):
        # An ('unix', path) transport produces an AF_UNIX-backed connection.
        c = core.WoollamaClient(transport=("unix", "/run/user/0/woollama.sock"))
        conn = c._connect(2)
        self.assertIsInstance(conn, core._UnixHTTPConnection)
        self.assertEqual(c.url, "unix:/run/user/0/woollama.sock")

    def test_request_body_shape(self):
        c = core.WoollamaClient(transport=("tcp", "x", 1))
        body = c._body("ollama/qwen3", "the prompt", {"temperature": 0.3}, True)
        self.assertEqual(body["model"], "ollama/qwen3")
        self.assertEqual(body["messages"], [{"role": "user", "content": "the prompt"}])
        self.assertTrue(body["stream"])
        self.assertAlmostEqual(body["temperature"], 0.3)


if __name__ == "__main__":
    unittest.main()

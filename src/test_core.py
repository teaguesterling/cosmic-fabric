"""Unit tests for the cosmic-fabric daemon engine (core.py) — pure logic only
(no network / GPU). Run: `cd src && python3 -m unittest test_core -v`.

Covers the pieces the daemon resolves on every run: model instantiations +
the capability rule, the active-set globs, context sizing, and option mapping.
"""
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


class HtmlToText(unittest.TestCase):
    def test_strips_tags_and_scripts(self):
        html = "<html><head><style>x{}</style></head><body><p>Hello</p><script>bad()</script><p>World</p></body></html>"
        t = core._html_to_text(html)
        self.assertIn("Hello", t)
        self.assertIn("World", t)
        self.assertNotIn("bad()", t)
        self.assertNotIn("<p>", t)


if __name__ == "__main__":
    unittest.main()

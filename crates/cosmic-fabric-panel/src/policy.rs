//! Typed read/write of `~/.config/cosmic-fabric/policy.toml` for the settings
//! window. The daemon re-reads the file per run, so edits take effect live.
//! (Note: round-tripping via the `toml` crate drops the example's comments.)

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub fn path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/cosmic-fabric/policy.toml")
}

/// A deployment instantiation of a model: params that override/extend the base.
/// `temperature`/`top_p`/penalties are pure sampling knobs (variant-typed, not
/// per-run sliders — see `doc/design/decision-2-sampling-as-variant-fields.md`).
/// `None` = "use the model default, don't send the field."
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Variant {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctx: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
}

/// A base model (vendor + model + classification), with named deployment
/// variants. `default` names the variant `use = "model"` resolves to.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Model {
    pub vendor: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variants: BTreeMap<String, Variant>,
}

/// What `default` / a pattern uses: a named `use = "model[/variant]"`, or a
/// legacy inline `model`/`vendor` (transition-read, treated as anonymous).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Assignment {
    #[serde(rename = "use", default, skip_serializing_if = "Option::is_none")]
    pub use_: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<String>,
}

impl Assignment {
    /// A short label of what this assignment points at, for display.
    pub fn label(&self) -> String {
        if let Some(u) = &self.use_ {
            u.clone()
        } else if let Some(m) = &self.model {
            format!("{m} (inline)")
        } else {
            "default".into()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputCfg {
    pub mode: String,
}
impl Default for OutputCfg {
    fn default() -> Self {
        Self { mode: "notify".into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaCfg {
    pub bin: String,
    pub url: String,
    pub warn_below_gpu: u32,
}
impl Default for OllamaCfg {
    fn default() -> Self {
        Self {
            bin: "/opt/ollama/bin/ollama".into(),
            url: "http://localhost:11434".into(),
            warn_below_gpu: 99,
        }
    }
}

/// The personalization profile's curated set, as include/exclude globs over
/// pattern names (`*`/`?` wildcards, or exact names). A pattern is active when it
/// matches an `include` glob (or `include` is empty = all) and no `exclude` glob.
/// `exclude` wins. A custom pack is just a glob in the config (e.g.
/// `include = ["mypack-*"]`) — pack names are data, never baked into the code.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Surface {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// Minimal glob match supporting `*` (any run) and `?` (one char); no wildcard
/// means an exact-name match.
pub fn glob_match(pat: &str, text: &str) -> bool {
    fn rec(p: &[u8], t: &[u8]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some(b'*') => rec(&p[1..], t) || (!t.is_empty() && rec(p, &t[1..])),
            Some(b'?') => !t.is_empty() && rec(&p[1..], &t[1..]),
            Some(&c) => !t.is_empty() && t[0] == c && rec(&p[1..], &t[1..]),
        }
    }
    rec(pat.as_bytes(), text.as_bytes())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Policy {
    #[serde(default)]
    pub default: Assignment,
    #[serde(default)]
    pub patterns: BTreeMap<String, Assignment>,
    #[serde(default)]
    pub output: OutputCfg,
    #[serde(default)]
    pub ollama: OllamaCfg,
    #[serde(default)]
    pub surface: Surface,
    #[serde(default)]
    pub models: BTreeMap<String, Model>,
}

impl Policy {
    /// The curated working set: every pattern in `all` that passes the
    /// include/exclude globs.
    pub fn active_patterns(&self, all: &[String]) -> Vec<String> {
        all.iter().filter(|p| self.is_active(p)).cloned().collect()
    }

    pub fn is_active(&self, name: &str) -> bool {
        let included = self.surface.include.is_empty()
            || self.surface.include.iter().any(|g| glob_match(g, name));
        let excluded = self.surface.exclude.iter().any(|g| glob_match(g, name));
        included && !excluded
    }

    /// Force a pattern in/out of the active set with an exact-name entry, so the
    /// edit holds regardless of the surrounding globs (exact `exclude` overrides
    /// an `include` glob; exact `include` overrides nothing it doesn't need to).
    pub fn set_active(&mut self, name: &str, on: bool) {
        if on {
            self.surface.exclude.retain(|g| g != name);
            if !self.is_active(name) {
                self.surface.include.push(name.to_string());
            }
        } else {
            self.surface.include.retain(|g| g != name);
            if self.is_active(name) {
                self.surface.exclude.push(name.to_string());
            }
        }
    }

    pub fn toggle_active(&mut self, name: &str) {
        let on = self.is_active(name);
        self.set_active(name, !on);
    }

    /// All selectable `use` targets: each model's default (`"model"`) plus every
    /// `"model/variant"`. For the per-pattern picker.
    pub fn use_options(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (name, m) in &self.models {
            out.push(name.clone());
            for v in m.variants.keys() {
                out.push(format!("{name}/{v}"));
            }
        }
        out
    }

    /// Reverse index: a `use` target → the assignments that reference it
    /// ("default" or a pattern name). Powers the Models view's usage display.
    pub fn usage(&self) -> BTreeMap<String, Vec<String>> {
        let mut idx: BTreeMap<String, Vec<String>> = BTreeMap::new();
        if let Some(u) = &self.default.use_ {
            idx.entry(u.clone()).or_default().push("default".into());
        }
        for (pat, a) in &self.patterns {
            if let Some(u) = &a.use_ {
                idx.entry(u.clone()).or_default().push(pat.clone());
            }
        }
        idx
    }
}

pub fn load() -> Policy {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(p: &Policy) -> Result<(), String> {
    let s = toml::to_string_pretty(p).map_err(|e| e.to_string())?;
    let path = path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, s).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_policy_loads_and_round_trips() {
        let p = load();
        eprintln!(
            "loaded: default={} patterns={} models={} mode={} ollama={}",
            p.default.label(),
            p.patterns.len(),
            p.models.len(),
            p.output.mode,
            p.ollama.url
        );
        // The installed policy.toml has a per-pattern override; if load()
        // had silently fallen back to Default, patterns would be empty (and a
        // save would clobber the user's config). Guard against that.
        assert!(
            !p.patterns.is_empty(),
            "policy.toml parsed to empty patterns — load() likely fell back to Default"
        );
        // Data survives a save/load round-trip.
        let s = toml::to_string_pretty(&p).unwrap();
        let p2: Policy = toml::from_str(&s).unwrap();
        assert_eq!(p.default.label(), p2.default.label());
        assert_eq!(p.patterns.len(), p2.patterns.len());
        assert_eq!(p.models.len(), p2.models.len());
        assert_eq!(p.output.mode, p2.output.mode);
    }

    #[test]
    fn glob_matches() {
        assert!(glob_match("pack-*", "pack-summarize"));
        assert!(!glob_match("pack-*", "extract_wisdom"));
        assert!(glob_match("*wisdom", "extract_wisdom"));
        assert!(glob_match("extract_*", "extract_wisdom"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "exacto"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("a?c", "abc"));
    }

    #[test]
    fn active_set_globs_and_toggle() {
        let all: Vec<String> = ["pack-x", "pack-y", "extract_wisdom", "summarize"]
            .iter().map(|s| s.to_string()).collect();
        // empty include = all
        let mut p = Policy::default();
        assert_eq!(p.active_patterns(&all).len(), 4);
        // include glob narrows
        p.surface.include = vec!["pack-*".into()];
        assert_eq!(p.active_patterns(&all), vec!["pack-x", "pack-y"]);
        // toggle one extra pattern in (exact include added)
        p.toggle_active("extract_wisdom");
        assert!(p.is_active("extract_wisdom"));
        assert_eq!(p.active_patterns(&all).len(), 3);
        // toggle a glob-matched one out (exact exclude added, overrides include)
        p.toggle_active("pack-x");
        assert!(!p.is_active("pack-x"));
        assert!(p.is_active("pack-y"));
        assert_eq!(p.active_patterns(&all).len(), 2);
    }
}

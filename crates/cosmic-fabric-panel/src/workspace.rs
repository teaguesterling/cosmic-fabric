//! The Fabric **workspace** window (`cosmic-fabric-panel window`) — the
//! prompt-first console: pick a source (clipboard / typed text / file / URL),
//! pick a pattern, see the **assembled prompt** update automatically, **Run** to
//! get a response, and route either with Copy. A real `cosmic::app` window (same
//! mechanism as Settings); all work goes through `cosmic-fabricd`.
//!
//! Scoped for v1: Copy is the only destination (prompt / response / conversation)
//! + Save result to a file. The full customizable destination registry
//! (Claude/Alpaca, etc.) lands with the Settings slice. Audio/Image origins are
//! shown disabled — they arrive with the model-capability work.

use std::time::Duration;

use cosmic::iced::futures::StreamExt;
use cosmic::{
    app,
    iced::{widget::Column, Alignment, Length},
    theme,
    widget::{button, combo_box, container, divider, dropdown, icon, scrollable, text, text_editor, text_input},
    Element,
};

use std::collections::BTreeMap;

use crate::daemon::{self, RunResult, Status};
use crate::policy::{self, Assignment, Policy};

pub const WORKSPACE_APP_ID: &str = "com.github.teaguesterling.CosmicFabric.Workspace";

/// Sentinel in the vendor dropdown meaning "no per-pattern override — use the
/// global default model."
const DEFAULT_VENDOR: &str = "Default";

/// The synthetic first entry in the Run-tab model picker that clears the per-run
/// override (fall back to the pattern's configured model).
const DEFAULT_MODEL_LABEL: &str = "(pattern default)";

/// `thinking` dropdown values: index 0 = inherit/none, 1 = off, 2 = on. Used
/// both on the (collapsed) add-variant row and the per-variant inline editor.
const THINKING_OPTS: [&str; 3] = ["(default)", "off", "on"];

/// Per-variant numeric knobs the user can edit inline (decision 2). `Ctx` is
/// u32; the others are f32. `thinking` is a dropdown, not part of this enum.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VariantField {
    Ctx,
    Temperature,
    TopP,
    FrequencyPenalty,
    PresencePenalty,
}

/// Text the user has typed into a variant's inline editor, kept in
/// `Workspace::variant_edits` so the input shows whatever they're in the middle
/// of typing — even values that don't parse yet (e.g. `"0."` on the way to
/// `"0.5"`). When a field parses, it's also written through to the policy;
/// empty = clear the field (set `Option` to None).
#[derive(Clone, Default, Debug)]
pub struct VariantEdit {
    pub ctx: String,
    pub temperature: String,
    pub top_p: String,
    pub frequency_penalty: String,
    pub presence_penalty: String,
}

impl VariantEdit {
    /// Initial editor text reflecting whatever's currently committed in the
    /// variant. Empty string = the policy holds `None`. Used to seed the map
    /// the first time a row is rendered.
    pub fn from_variant(v: &policy::Variant) -> Self {
        fn f<T: std::fmt::Display>(o: Option<T>) -> String {
            o.map(|x| x.to_string()).unwrap_or_default()
        }
        Self {
            ctx: f(v.ctx),
            temperature: f(v.temperature),
            top_p: f(v.top_p),
            frequency_penalty: f(v.frequency_penalty),
            presence_penalty: f(v.presence_penalty),
        }
    }

    pub fn set(&mut self, field: VariantField, s: String) {
        match field {
            VariantField::Ctx => self.ctx = s,
            VariantField::Temperature => self.temperature = s,
            VariantField::TopP => self.top_p = s,
            VariantField::FrequencyPenalty => self.frequency_penalty = s,
            VariantField::PresencePenalty => self.presence_penalty = s,
        }
    }

    pub fn get(&self, field: VariantField) -> &str {
        match field {
            VariantField::Ctx => &self.ctx,
            VariantField::Temperature => &self.temperature,
            VariantField::TopP => &self.top_p,
            VariantField::FrequencyPenalty => &self.frequency_penalty,
            VariantField::PresencePenalty => &self.presence_penalty,
        }
    }
}

/// Apply user text to a `Variant`. Returns `true` iff the policy was changed
/// (so the caller can decide whether to persist). Empty text means "clear"
/// (set the field to `None`); non-empty parseable text sets the field;
/// non-empty unparseable text is a no-op (the edit state keeps the user's text
/// visible while they fix the typo).
fn apply_variant_field(v: &mut policy::Variant, field: VariantField, s: &str) -> bool {
    let s = s.trim();
    fn pf32(s: &str) -> Option<Option<f32>> {
        if s.is_empty() { return Some(None); }
        s.parse::<f32>().ok().map(Some)
    }
    fn pu32(s: &str) -> Option<Option<u32>> {
        if s.is_empty() { return Some(None); }
        s.parse::<u32>().ok().map(Some)
    }
    match field {
        VariantField::Ctx => match pu32(s) { Some(n) => { v.ctx = n; true } None => false },
        VariantField::Temperature => match pf32(s) { Some(n) => { v.temperature = n; true } None => false },
        VariantField::TopP => match pf32(s) { Some(n) => { v.top_p = n; true } None => false },
        VariantField::FrequencyPenalty => match pf32(s) { Some(n) => { v.frequency_penalty = n; true } None => false },
        VariantField::PresencePenalty => match pf32(s) { Some(n) => { v.presence_penalty = n; true } None => false },
    }
}

pub fn run() -> cosmic::iced::Result {
    // Close to the pre-run content height; `fit_window` refines it as the layout
    // changes (collapse/expand, run, mode/origin switches).
    let settings = cosmic::app::Settings::default().size(cosmic::iced::Size::new(640.0, 620.0));
    cosmic::app::run::<Workspace>(settings, ())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    Clipboard,
    Text,
    File,
    Url,
    Image,
}

/// Which produced artifact a send-to control routes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Artifact {
    Prompt,
    Response,
    Conversation,
}

/// A send-to destination. The registry is fixed for now (Copy / Save built;
/// Claude/Alpaca disabled until goo's route layer lands; Manage → Settings); it
/// will become user-editable in the Settings slice — hence the data model.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dest {
    Copy,
    SaveFile,
    Claude,
    Alpaca,
    Manage,
}

struct DestSpec {
    dest: Dest,
    label: &'static str,
    enabled: bool,
    note: Option<&'static str>,
}

/// Which consolidated popover is open: Copy (pick an artifact) or Send (pick a
/// destination). Replaces the old per-artifact menu key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuKind {
    Copy,
    Send,
}

/// The destination registry shown in every send-to menu.
fn destinations() -> [DestSpec; 5] {
    [
        DestSpec { dest: Dest::Copy, label: "Copy", enabled: true, note: None },
        DestSpec { dest: Dest::SaveFile, label: "Save to file…", enabled: true, note: None },
        DestSpec { dest: Dest::Claude, label: "Claude Desktop", enabled: true, note: Some("via clipboard") },
        DestSpec { dest: Dest::Alpaca, label: "Alpaca", enabled: false, note: Some("needs goo route") },
        DestSpec { dest: Dest::Manage, label: "Manage destinations…", enabled: true, note: None },
    ]
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WorkMode {
    Run,
    Library,
    Models,
}

pub struct Workspace {
    core: cosmic::app::Core,
    status: Option<Status>,
    policy: Policy,
    all_patterns: Vec<String>,   // every pattern fabric has (unfiltered)
    patterns: Vec<String>,       // the active/curated set (run dropdown)
    pattern_labels: Vec<String>, // pretty labels for the active set
    pattern_state: combo_box::State<String>, // searchable picker state (mirrors pattern_labels)
    selected_idx: Option<usize>,
    run_model: Option<String>, // per-run model override (woollama id); None = pattern default
    run_model_state: combo_box::State<String>, // searchable picker over woollama's models
    woollama_models: Vec<String>,
    mode: WorkMode,
    library_query: String,
    catalog: BTreeMap<String, Vec<String>>, // vendor → models, for the picker
    lib_selected: Option<String>,           // pattern being configured in Library
    // --- Models editor drafts ---
    model_selected: Option<String>, // model whose editor is open
    am_name: String,                // new-model name
    am_vendor: Option<usize>,       // new-model vendor (index into catalog keys)
    am_model: Option<String>,       // new-model model name (from the vendor's catalog)
    am_model_state: combo_box::State<String>, // searchable model picker (rebuilt per vendor)
    cat_draft: String,              // selected model's categories (comma-edited)
    av_name: String,                // new-variant name (add row is name-only)
    /// In-progress text for each variant's inline knobs, keyed by (model, vname).
    /// Renders take from here; commits write through to `policy.models.*.variants`.
    variant_edits: BTreeMap<(String, String), VariantEdit>,

    origin: Origin,
    source: text_editor::Content,
    url_input: String,
    file_input: String,
    image_path: String,             // image source: a path (native picker is monitor-time)
    transform_note: Option<String>, // e.g. "fetched · 4,210 chars markdown"

    prompt: Option<String>,
    prompt_collapsed: bool,
    asm_gen: u64,
    edit_gen: u64,

    response: Option<String>,
    response_collapsed: bool,
    ran: bool, // a run has been started → reveal the Response card
    result_meta: Option<String>,
    running: bool,
    pending: Option<(u64, String, String, Option<String>)>, // (run id, pattern, input, model override)
    run_seq: u64,

    error: Option<String>,
    status_msg: Option<String>, // transient (e.g. "saved to …")
    open_menu: Option<MenuKind>, // which consolidated popover is open
}

#[derive(Debug, Clone)]
pub enum Message {
    StatusDone(Result<Status, String>),
    PatternsDone(Result<Vec<String>, String>),
    SetOrigin(Origin),
    SourceAction(text_editor::Action),
    LoadClipboard,
    UrlInput(String),
    FetchUrl,
    FetchDone(Result<(String, usize), String>),
    FileInput(String),
    LoadFile,
    ImagePath(String),
    ImageFromClipboard,
    RunImageDone(Result<RunResult, String>),
    PickPattern(String),
    PickRunModel(String),
    WoollamaModelsDone(Result<Vec<String>, String>),
    AssembleDebounced(u64),
    AssembleDone(u64, Result<String, String>),
    TogglePrompt,
    ToggleResponse,
    Run,
    RunEvent(daemon::RunEvent),
    ToggleMenu(MenuKind),
    CloseMenu,
    Route(Artifact, Dest),
    SetMode(WorkMode),
    LibraryQuery(String),
    ToggleActive(String),
    CatalogDone(Result<BTreeMap<String, Vec<String>>, String>),
    LibSelect(String),
    SetPatternUse(String, String),
    // --- Models editor ---
    AmName(String),
    AmVendor(usize),
    AmModel(String),
    AddModel,
    SelectModel(String),
    DeleteModel(String),
    CatDraft(String),
    CommitCats(String),
    SetModelDefaultVariant(String, String),
    AvName(String),
    AddVariant(String),
    DeleteVariant(String, String),
    /// Edit an inline numeric knob on an existing variant: model, vname, field, new text.
    SetVariantField(String, String, VariantField, String),
    /// Edit the `thinking` dropdown on an existing variant: model, vname, dropdown index.
    SetVariantThinking(String, String, usize),
    SetGlobalUse(String),
    Retry,
    Clear,
    OpenSettings,
    ContinueInChat,
    DismissError,
}

/// One-line, ≤80-char rendering of a tool's args JSON for inline display.
/// `{"url":"https://wikipedia/Tokyo"}` → `url="https://wikipedia/Tokyo"`.
/// (Same shape as the helper in session.rs — kept in both surfaces so each
/// can drift its formatting independently if needed.)
fn tool_args_summary(args: &serde_json::Value) -> String {
    let obj = match args.as_object() {
        Some(o) => o,
        None => return args.to_string(),
    };
    let parts: Vec<String> = obj
        .iter()
        .map(|(k, v)| match v {
            serde_json::Value::String(s) => {
                let s = if s.len() > 60 { format!("{}…", &s[..60]) } else { s.clone() };
                format!("{k}={s:?}")
            }
            _ => format!("{k}={v}"),
        })
        .collect();
    let joined = parts.join(", ");
    if joined.len() > 80 { format!("{}…", &joined[..80]) } else { joined }
}

/// Humanize a pattern name for display (shared with the popup): separators →
/// spaces, first letter upper. Pack-name-agnostic.
pub fn pretty(name: &str) -> String {
    let base = name.replace(['-', '_'], " ");
    let mut c = base.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => base,
    }
}

impl cosmic::Application for Workspace {
    type Message = Message;
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    const APP_ID: &'static str = WORKSPACE_APP_ID;

    fn init(core: app::Core, _flags: Self::Flags) -> (Self, app::Task<Self::Message>) {
        // If an image is on the clipboard, open straight into Image mode with it
        // loaded (ready for a vision Run); else load clipboard text as the source.
        let clip_image = daemon::clipboard_image();
        let mut me = Self {
            core,
            status: None,
            policy: policy::load(),
            all_patterns: Vec::new(),
            patterns: Vec::new(),
            pattern_labels: Vec::new(),
            pattern_state: combo_box::State::new(Vec::new()),
            selected_idx: None,
            run_model: None,
            run_model_state: combo_box::State::new(Vec::new()),
            woollama_models: Vec::new(),
            mode: WorkMode::Run,
            library_query: String::new(),
            catalog: BTreeMap::new(),
            lib_selected: None,
            model_selected: None,
            am_name: String::new(),
            am_vendor: None,
            am_model: None,
            am_model_state: combo_box::State::new(Vec::new()),
            cat_draft: String::new(),
            av_name: String::new(),
            variant_edits: BTreeMap::new(),
            origin: if clip_image.is_some() { Origin::Image } else { Origin::Clipboard },
            source: text_editor::Content::new(),
            url_input: String::new(),
            file_input: String::new(),
            image_path: clip_image.clone().unwrap_or_default(),
            transform_note: clip_image.as_ref().map(|_| "image from the clipboard".into()),
            prompt: None,
            prompt_collapsed: false,
            asm_gen: 0,
            edit_gen: 0,
            response: None,
            response_collapsed: false,
            ran: false,
            result_meta: None,
            running: false,
            pending: None,
            run_seq: 0,
            error: None,
            status_msg: None,
            open_menu: None,
        };
        me.seed_variant_edits();   // first paint shows what's currently in policy
        let mut tasks = vec![status_task(), patterns_task(), catalog_task(), woollama_models_task()];
        if clip_image.is_none() {
            tasks.push(load_clipboard_task()); // text clipboard → source editor
        }
        (me, cosmic::Task::batch(tasks))
    }

    fn core(&self) -> &cosmic::app::Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut cosmic::app::Core {
        &mut self.core
    }

    fn subscription(&self) -> cosmic::iced::Subscription<Message> {
        match &self.pending {
            Some(p) => cosmic::iced::Subscription::run_with(
                p.clone(),
                |(_, pat, input, model): &(u64, String, String, Option<String>)| {
                    daemon::run_stream(pat.clone(), input.clone(), model.clone())
                        .map(Message::RunEvent)
                },
            ),
            None => cosmic::iced::Subscription::none(),
        }
    }

    fn update(&mut self, message: Message) -> app::Task<Message> {
        match message {
            Message::StatusDone(Ok(s)) => {
                // If woollama routing is off, the Run-row model picker is hidden —
                // drop any stale override so it can't silently apply.
                if !s.woollama.enabled {
                    self.run_model = None;
                }
                self.status = Some(s);
            }
            Message::StatusDone(Err(e)) => self.error = Some(e),
            Message::PatternsDone(Ok(p)) => {
                self.all_patterns = p;
                self.recompute_active();
                return self.fit_window(); // first fit once the window exists
            }
            Message::PatternsDone(Err(e)) => self.error = Some(e),

            Message::SetOrigin(o) => {
                self.origin = o;
                self.transform_note = None;
                self.status_msg = None;
                return self.fit_window();
            }
            Message::SourceAction(action) => {
                let is_edit = matches!(action, text_editor::Action::Edit(_));
                self.source.perform(action);
                if is_edit {
                    self.edit_gen += 1;
                    return debounce_assemble(self.edit_gen);
                }
            }
            Message::LoadClipboard => {
                let t = daemon::clipboard();
                self.source = text_editor::Content::with_text(&t);
                self.origin = Origin::Clipboard;
                self.transform_note = None;
                return self.trigger_assemble();
            }
            Message::UrlInput(s) => self.url_input = s,
            Message::FetchUrl => {
                let url = self.url_input.trim().to_string();
                if url.is_empty() {
                    self.error = Some("Enter a URL to fetch.".into());
                } else {
                    self.transform_note = Some("fetching…".into());
                    self.error = None;
                    return cosmic::Task::perform(
                        daemon::fetch_url(url, "scrape".into()),
                        |r| cosmic::Action::App(Message::FetchDone(r)),
                    );
                }
            }
            Message::FetchDone(Ok((text, chars))) => {
                self.source = text_editor::Content::with_text(&text);
                self.transform_note = Some(format!("fetched · {chars} chars markdown → feeds the prompt"));
                return self.trigger_assemble();
            }
            Message::FetchDone(Err(e)) => {
                self.transform_note = None;
                self.error = Some(format!("fetch failed: {e}"));
            }
            Message::FileInput(s) => self.file_input = s,
            Message::ImagePath(s) => self.image_path = s,
            Message::ImageFromClipboard => match daemon::clipboard_image() {
                Some(p) => {
                    self.image_path = p;
                    self.transform_note = Some("image grabbed from the clipboard".into());
                    self.error = None;
                }
                None => self.error = Some("No image on the clipboard — copy one first.".into()),
            },
            Message::LoadFile => {
                let path = expand_tilde(self.file_input.trim());
                match std::fs::read_to_string(&path) {
                    Ok(t) => {
                        let chars = t.chars().count();
                        self.source = text_editor::Content::with_text(&t);
                        self.transform_note = Some(format!("loaded {chars} chars from file"));
                        self.error = None;
                        return self.trigger_assemble();
                    }
                    Err(e) => self.error = Some(format!("read failed: {e}")),
                }
            }
            Message::PickPattern(label) => {
                // The combobox yields the chosen pretty label; map it back to its
                // index in the active set.
                self.selected_idx = self.pattern_labels.iter().position(|l| *l == label);
                return self.trigger_assemble();
            }
            Message::PickRunModel(id) => {
                // The synthetic "(pattern default)" entry clears the override.
                self.run_model = (id != DEFAULT_MODEL_LABEL).then_some(id);
            }
            Message::WoollamaModelsDone(Ok(models)) => {
                self.woollama_models = models;
                let mut opts = Vec::with_capacity(self.woollama_models.len() + 1);
                opts.push(DEFAULT_MODEL_LABEL.to_string());
                opts.extend(self.woollama_models.iter().cloned());
                self.run_model_state = combo_box::State::new(opts);
            }
            Message::WoollamaModelsDone(Err(_)) => self.woollama_models.clear(),
            Message::AssembleDebounced(g) => {
                if g == self.edit_gen {
                    return self.trigger_assemble();
                }
            }
            Message::AssembleDone(seq, res) => {
                if seq == self.asm_gen {
                    match res {
                        Ok(p) => self.prompt = Some(p),
                        Err(e) => self.error = Some(e),
                    }
                }
            }
            Message::TogglePrompt => {
                self.prompt_collapsed = !self.prompt_collapsed;
                return self.fit_window();
            }
            Message::ToggleResponse => {
                self.response_collapsed = !self.response_collapsed;
                return self.fit_window();
            }

            Message::Run => {
                if self.origin == Origin::Image {
                    let path = self.image_path.trim().to_string();
                    if path.is_empty() {
                        self.error = Some("Enter an image path first.".into());
                        return app::Task::none();
                    }
                    let question = self.source.text();
                    let pattern = self.selected_idx.map(|i| self.patterns[i].clone());
                    self.response = Some(String::new());
                    self.on_run_started();
                    self.result_meta = None;
                    self.running = true;
                    self.error = None;
                    self.status_msg = None;
                    // vision run is non-streaming (CLI shell-out) → a one-shot Task
                    let fit = self.fit_window();
                    return cosmic::Task::batch([
                        fit,
                        cosmic::Task::perform(
                            daemon::run_image(path, question, pattern),
                            |r| cosmic::Action::App(Message::RunImageDone(r)),
                        ),
                    ]);
                }
                let Some(idx) = self.selected_idx else {
                    self.error = Some("Pick a pattern first.".into());
                    return app::Task::none();
                };
                let input = self.source.text();
                if input.trim().is_empty() {
                    self.error = Some("Source is empty — load or type something first.".into());
                    return app::Task::none();
                }
                let pattern = self.patterns[idx].clone();
                self.run_seq += 1;
                self.pending = Some((self.run_seq, pattern, input, self.run_model.clone()));
                self.response = Some(String::new());
                self.on_run_started();
                self.result_meta = None;
                self.running = true;
                self.error = None;
                self.status_msg = None;
                return self.fit_window();
            }
            Message::RunEvent(ev) => match ev {
                daemon::RunEvent::Chunk(s) => {
                    if let Some(r) = self.response.as_mut() {
                        r.push_str(&s);
                    }
                }
                daemon::RunEvent::ToolCall { name, args, .. } => {
                    // Inline trace as the model decides. Phase 1: prepended to
                    // the response text; a collapsed trace card is later polish.
                    let r = self.response.get_or_insert_with(String::new);
                    if !r.is_empty() && !r.ends_with('\n') {
                        r.push('\n');
                    }
                    r.push_str(&format!("\u{1F50E} {name}({})\n", tool_args_summary(&args)));
                }
                daemon::RunEvent::ToolResult { name, summary, .. } => {
                    let r = self.response.get_or_insert_with(String::new);
                    r.push_str(&format!("  \u{2713} {name}: {summary}\n"));
                }
                daemon::RunEvent::ToolConfirmRequired { name, command_preview, .. } => {
                    // Phase 1 placeholder — modal handling is Task 23.
                    let r = self.response.get_or_insert_with(String::new);
                    r.push_str(&format!(
                        "\u{26A0} {name} requires confirmation: {command_preview}\n"));
                }
                daemon::RunEvent::Done(rr) => {
                    self.running = false;
                    self.pending = None;
                    if self.response.as_deref().unwrap_or("").is_empty() {
                        self.response = rr.output.clone();
                    }
                    self.result_meta = Some(meta_line(&rr));
                }
                daemon::RunEvent::Error(e) => {
                    self.running = false;
                    self.pending = None;
                    self.error = Some(e);
                }
            },
            Message::RunImageDone(Ok(rr)) => {
                self.running = false;
                self.response = Some(rr.output.clone().unwrap_or_default());
                self.result_meta = Some(format!("{}  \u{00b7} vision", meta_line(&rr)));
            }
            Message::RunImageDone(Err(e)) => {
                self.running = false;
                self.error = Some(e);
            }

            Message::ToggleMenu(a) => {
                self.open_menu = if self.open_menu == Some(a) { None } else { Some(a) };
            }
            Message::CloseMenu => self.open_menu = None,
            Message::Route(a, dest) => {
                self.open_menu = None;
                let (text, name) = self.artifact_text(a);
                match dest {
                    Dest::Copy => {
                        daemon::set_clipboard(&text);
                        self.status_msg = Some(format!("{name} copied."));
                    }
                    Dest::SaveFile => {
                        let pat = self.selected_idx.map(|i| self.patterns[i].as_str());
                        match save_to_file(&text, pat) {
                            Ok(p) => self.status_msg = Some(format!("Saved to {p}")),
                            Err(e) => self.error = Some(format!("save failed: {e}")),
                        }
                    }
                    Dest::Manage => {
                        if let Ok(exe) = std::env::current_exe() {
                            let _ = std::process::Command::new(exe).arg("settings").spawn();
                        }
                    }
                    Dest::Claude => {
                        // Stub handoff until goo's route layer carries the payload:
                        // stage on the clipboard + nudge. (A prompt assembled for an
                        // agent is the usual case; works for any artifact.)
                        daemon::set_clipboard(&text);
                        self.status_msg =
                            Some(format!("{name} copied — paste into Claude Desktop (Ctrl+V)."));
                    }
                    Dest::Alpaca => {} // disabled; never fired
                }
            }
            Message::SetMode(m) => {
                self.mode = m;
                return self.fit_window();
            }
            Message::LibraryQuery(q) => self.library_query = q,
            Message::ToggleActive(name) => {
                self.policy.toggle_active(&name);
                self.persist();
                self.recompute_active();
            }
            Message::CatalogDone(Ok(c)) => self.catalog = c,
            Message::CatalogDone(Err(e)) => self.error = Some(e),
            Message::LibSelect(name) => {
                self.lib_selected = if self.lib_selected.as_deref() == Some(name.as_str()) {
                    None
                } else {
                    Some(name)
                };
            }
            Message::SetPatternUse(name, target) => {
                if target == DEFAULT_VENDOR {
                    self.policy.patterns.remove(&name); // use the global default
                } else {
                    self.policy.patterns.insert(
                        name,
                        Assignment { use_: Some(target), ..Default::default() },
                    );
                }
                self.persist();
            }
            Message::AmName(s) => self.am_name = s,
            Message::AmVendor(i) => {
                self.am_vendor = Some(i);
                self.am_model = None;
                // Rebuild the searchable model picker for the chosen vendor.
                let vendors: Vec<String> = self.catalog.keys().cloned().collect();
                let models = vendors
                    .get(i)
                    .and_then(|v| self.catalog.get(v))
                    .cloned()
                    .unwrap_or_default();
                self.am_model_state = combo_box::State::new(models);
            }
            Message::AmModel(name) => self.am_model = Some(name),
            Message::AddModel => {
                let name = self.am_name.trim().to_string();
                let vendors: Vec<String> = self.catalog.keys().cloned().collect();
                if !name.is_empty() && !self.policy.models.contains_key(&name) {
                    if let Some(vendor) = self.am_vendor.and_then(|i| vendors.get(i)).cloned() {
                        let model = self.am_model.clone().unwrap_or_default();
                        if !model.is_empty() {
                            self.policy.models.insert(
                                name.clone(),
                                policy::Model { vendor, model, ..Default::default() },
                            );
                            self.persist();
                            self.am_name.clear();
                            self.am_vendor = None;
                            self.am_model = None;
                            self.am_model_state = combo_box::State::new(Vec::new());
                            self.cat_draft.clear();
                            self.model_selected = Some(name);
                        }
                    }
                }
            }
            Message::SelectModel(name) => {
                self.cat_draft = self
                    .policy
                    .models
                    .get(&name)
                    .map(|m| m.categories.join(", "))
                    .unwrap_or_default();
                self.model_selected = if self.model_selected.as_deref() == Some(name.as_str()) {
                    None
                } else {
                    Some(name)
                };
            }
            Message::DeleteModel(name) => {
                self.policy.models.remove(&name);
                if self.model_selected.as_deref() == Some(name.as_str()) {
                    self.model_selected = None;
                }
                self.persist();
            }
            Message::CatDraft(s) => self.cat_draft = s,
            Message::CommitCats(name) => {
                if let Some(m) = self.policy.models.get_mut(&name) {
                    m.categories = self
                        .cat_draft
                        .split(',')
                        .map(|x| x.trim().to_string())
                        .filter(|x| !x.is_empty())
                        .collect();
                    self.persist();
                }
            }
            Message::SetModelDefaultVariant(name, v) => {
                if let Some(m) = self.policy.models.get_mut(&name) {
                    m.default = if v.is_empty() { None } else { Some(v) };
                    self.persist();
                }
            }
            Message::AvName(s) => self.av_name = s,
            Message::AddVariant(model) => {
                // Add-row is now name-only — the knobs are edited inline on the
                // new row once it exists. New variants start "empty" (all None);
                // the user fills knobs in via the SetVariantField path.
                let vname = self.av_name.trim().to_string();
                if !vname.is_empty() {
                    if let Some(m) = self.policy.models.get_mut(&model) {
                        m.variants.insert(vname.clone(), policy::Variant::default());
                        if m.default.is_none() {
                            m.default = Some(vname.clone());
                        }
                        // Ensure the new row has an edit entry from the start
                        // (so text_input can borrow its strings on first paint).
                        self.variant_edits
                            .entry((model.clone(), vname))
                            .or_default();
                        self.persist();
                        self.av_name.clear();
                    }
                }
            }
            Message::DeleteVariant(model, vname) => {
                if let Some(m) = self.policy.models.get_mut(&model) {
                    m.variants.remove(&vname);
                    if m.default.as_deref() == Some(vname.as_str()) {
                        m.default = m.variants.keys().next().cloned();
                    }
                    self.persist();
                }
                // and drop any lingering edit state for the removed row
                self.variant_edits.remove(&(model, vname));
            }
            Message::SetVariantField(model, vname, field, s) => {
                // Always update the edit state so the input box echoes what the
                // user typed (even un-parseable values like "0." mid-keystroke).
                self.variant_edits
                    .entry((model.clone(), vname.clone()))
                    .or_default()
                    .set(field, s.clone());
                // Try to commit to the policy: empty → clear; parseable → set;
                // un-parseable → leave the prior policy value (the edit-state
                // text stays visible so the user can fix the typo).
                let mut changed = false;
                if let Some(m) = self.policy.models.get_mut(&model) {
                    if let Some(v) = m.variants.get_mut(&vname) {
                        changed = apply_variant_field(v, field, &s);
                    }
                }
                if changed {
                    self.persist();
                }
            }
            Message::SetVariantThinking(model, vname, idx) => {
                // THINKING_OPTS: 0=(default)/None, 1=off, 2=on.
                let new = match idx {
                    1 => Some("off".to_string()),
                    2 => Some("on".to_string()),
                    _ => None,
                };
                if let Some(m) = self.policy.models.get_mut(&model) {
                    if let Some(v) = m.variants.get_mut(&vname) {
                        v.thinking = new;
                        self.persist();
                    }
                }
            }
            Message::SetGlobalUse(u) => {
                self.policy.default = Assignment { use_: Some(u), ..Default::default() };
                self.persist();
            }
            Message::Retry => return self.update(Message::Run),
            Message::Clear => {
                self.source = text_editor::Content::new();
                self.prompt = None;
                self.response = None;
                self.result_meta = None;
                self.transform_note = None;
                self.error = None;
                self.status_msg = None;
                // back to the pre-run layout: Prompt open, Response hidden
                self.ran = false;
                self.prompt_collapsed = false;
                self.response_collapsed = false;
                self.run_model = None; // drop any per-run model override
                return self.fit_window();
            }
            Message::OpenSettings => {
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(exe).arg("settings").spawn();
                }
            }
            Message::ContinueInChat => {
                // Escalate this result into a chat, seeded with the response.
                if let (Ok(exe), Some(r)) = (std::env::current_exe(), &self.response) {
                    if !r.trim().is_empty() {
                        let _ = std::process::Command::new(exe).arg("session").arg(r).spawn();
                    }
                }
            }
            Message::DismissError => self.error = None,
        }
        app::Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let s = theme::active().cosmic().spacing;
        let mut col = Column::new().spacing(s.space_s).padding(s.space_m);

        // ---- header: title + status pill ----
        let mut header = cosmic::iced::widget::row![text::title3("Fabric")]
            .spacing(s.space_s)
            .align_y(Alignment::Center);
        header = header.push(cosmic::widget::Space::new().width(Length::Fill));
        if let Some(st) = &self.status {
            header = header.push(text::caption(self.status_pill(st)));
        }
        // mode toggle: Run (console) ⇄ Library (curate patterns) ⇄ Models (configs)
        let mode_btn = |label: &'static str, m: WorkMode, cur: WorkMode| {
            if cur == m {
                button::suggested(label)
            } else {
                button::text(label).on_press(Message::SetMode(m))
            }
        };
        header = header.push(
            cosmic::iced::widget::row![
                mode_btn("Run", WorkMode::Run, self.mode),
                mode_btn("Library", WorkMode::Library, self.mode),
                mode_btn("Models", WorkMode::Models, self.mode),
            ]
            .spacing(2),
        );
        // settings gear, top-right (replaces the old footer "Settings…")
        header = header.push(
            button::icon(icon::from_name("emblem-system-symbolic"))
                .on_press(Message::OpenSettings),
        );
        col = col.push(header);
        col = col.push(divider::horizontal::default());

        match self.mode {
            WorkMode::Models => {
                col = col.push(self.models_view(&s));
            }
            WorkMode::Library => {
                col = col.push(self.library_view(&s));
            }
            WorkMode::Run => {
                // ---- source ----
                col = col.push(self.source_section(&s));

                // ---- pattern + run ----
                // The pattern picker drives the whole source→assemble→run flow,
                // so it's a labelled, searchable combobox: type to filter the
                // active set (the placeholder names the step when empty).
                let selected_label = self.selected_idx.and_then(|i| self.pattern_labels.get(i));
                let mut runrow = cosmic::iced::widget::row![
                    text::body("Pattern"),
                    combo_box(
                        &self.pattern_state,
                        "Choose a pattern\u{2026}",
                        selected_label,
                        Message::PickPattern,
                    )
                    .width(Length::Fixed(280.0)),
                ]
                .spacing(s.space_s)
                .align_y(Alignment::Center);
                // Per-run model override — shown only when woollama routing is on
                // (it sources the model list). Default = the pattern's model.
                if self.status.as_ref().map_or(false, |st| st.woollama.enabled) {
                    runrow = runrow.push(text::body("Model"));
                    runrow = runrow.push(
                        combo_box(
                            &self.run_model_state,
                            DEFAULT_MODEL_LABEL,
                            self.run_model.as_ref(),
                            Message::PickRunModel,
                        )
                        .width(Length::Fixed(220.0)),
                    );
                }
                let run_btn = button::suggested(if self.running { "Running…" } else { "Run" });
                runrow = runrow.push(if self.running {
                    run_btn
                } else {
                    run_btn.on_press(Message::Run)
                });
                col = col.push(runrow);

                col = col.push(self.prompt_card(&s));
                // Response card stays hidden until the first Run.
                if self.ran {
                    col = col.push(self.response_card(&s));
                }
                // Copy ▾ (prompt card) + Send ▾ (response card) replace the old
                // footer of per-artifact Copy buttons, Chat, and Refresh.
            }
        }

        if let Some(msg) = &self.status_msg {
            col = col.push(text::caption(msg.clone()));
        }
        if let Some(err) = &self.error {
            col = col.push(
                cosmic::iced::widget::row![
                    text::caption(err.clone()).width(Length::Fill),
                    button::text("Dismiss").on_press(Message::DismissError),
                ]
                .align_y(Alignment::Center),
            );
        }

        container(scrollable(col))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

impl Workspace {
    fn status_pill(&self, st: &Status) -> String {
        let serve = if st.serve { "\u{25cf} up" } else { "\u{25cb} down" };
        let model = st.default_model.clone().unwrap_or_else(|| "—".into());
        let gpu = st
            .loaded
            .first()
            .and_then(|l| l.gpu_pct)
            .map(|p| {
                let warn = if p < 99.0 { " \u{26a0}" } else { "" };
                format!("  \u{00b7}  {p:.0}% GPU{warn}")
            })
            .unwrap_or_default();
        let wool = st
            .woollama_badge()
            .map(|b| format!("  \u{00b7}  {b}"))
            .unwrap_or_default();
        format!("serve {serve}  \u{00b7}  {model}{gpu}{wool}")
    }

    /// On Run: reveal + expand the Response card and collapse the (now
    /// secondary) Prompt — focus shifts from "what am I sending" to "the answer".
    fn on_run_started(&mut self) {
        self.ran = true;
        self.prompt_collapsed = true;
        self.response_collapsed = false;
    }

    /// A size-to-content height for the Run console so the window doesn't leave a
    /// tall empty box below the cards. The view is wrapped in a scrollable, so an
    /// underestimate scrolls gracefully — we lean a touch generous. Library/Models
    /// are long lists, so they keep a tall window.
    fn target_height(&self) -> f32 {
        if self.mode != WorkMode::Run {
            return 780.0;
        }
        let mut h: f32 = 96.0; // window padding + header + divider
        h += 32.0 + 36.0 + 132.0 + 30.0; // source: tabs, loader, editor, char row
        if matches!(self.origin, Origin::Url | Origin::File | Origin::Image) {
            h += 44.0; // an input+button loader row
        }
        if self.origin == Origin::Image {
            h += 28.0; // vision caption
        }
        h += 44.0; // pattern + run row
        h += 44.0 + if self.prompt_collapsed { 0.0 } else { 166.0 }; // prompt card
        if self.ran {
            h += 44.0 + if self.response_collapsed { 0.0 } else { 196.0 }; // response card
        }
        h += 28.0; // status/error breathing room
        h.clamp(360.0, 1000.0)
    }

    /// Resize the window to fit the current content (see `target_height`).
    fn fit_window(&self) -> app::Task<Message> {
        match self.core.main_window_id() {
            Some(id) => {
                cosmic::iced::window::resize(id, cosmic::iced::Size::new(640.0, self.target_height()))
            }
            None => app::Task::none(),
        }
    }

    fn recompute_active(&mut self) {
        self.patterns = self.policy.active_patterns(&self.all_patterns);
        self.pattern_labels = self.patterns.iter().map(|n| pretty(n)).collect();
        // Rebuild the searchable picker's options to match the active set.
        self.pattern_state = combo_box::State::new(self.pattern_labels.clone());
        if let Some(i) = self.selected_idx {
            if i >= self.patterns.len() {
                self.selected_idx = None;
            }
        }
    }

    fn persist(&mut self) {
        if let Err(e) = policy::save(&self.policy) {
            self.error = Some(format!("save failed: {e}"));
        }
    }

    /// Pre-populate `variant_edits` from the loaded policy so the inline editors
    /// show the *current* values on first paint. Without this, an existing
    /// variant with `ctx = 2048` would render an empty input until the user
    /// typed. Called once at init; AddVariant tops up the new row on its own.
    fn seed_variant_edits(&mut self) {
        for (mname, m) in &self.policy.models {
            for (vname, v) in &m.variants {
                let key = (mname.clone(), vname.clone());
                self.variant_edits
                    .entry(key)
                    .or_insert_with(|| VariantEdit::from_variant(v));
            }
        }
    }

    /// The Library: curate which patterns are in your active set (★) and click a
    /// pattern to configure its model/vendor. Search reveals the full set; with no
    /// query it shows your active set.
    fn library_view(&self, s: &cosmic::cosmic_theme::Spacing) -> Element<'_, Message> {
        let total = self.all_patterns.len();
        let active_n = self.patterns.len();
        let q = self.library_query.trim().to_lowercase();

        let rows: Vec<&String> = if q.is_empty() {
            self.patterns.iter().collect()
        } else {
            self.all_patterns
                .iter()
                .filter(|p| p.to_lowercase().contains(&q))
                .take(80)
                .collect()
        };

        let mut list = Column::new().spacing(2);
        for name in rows {
            let active = self.policy.is_active(name);
            let star = button::text(if active { "\u{2605}" } else { "\u{2606}" })
                .on_press(Message::ToggleActive(name.clone()));
            let model_note = self
                .policy
                .patterns
                .get(name)
                .map(|a| a.label())
                .unwrap_or_else(|| "default".into());
            let row = cosmic::iced::widget::row![
                star,
                button::text(pretty(name))
                    .width(Length::Fixed(230.0))
                    .on_press(Message::LibSelect(name.clone())),
                cosmic::widget::Space::new().width(Length::Fill),
                text::caption(model_note),
            ]
            .spacing(s.space_xs)
            .align_y(Alignment::Center);
            list = list.push(row);
        }

        let hint = if q.is_empty() {
            format!("{active_n} active \u{00b7} {total} total \u{00b7} search to add more")
        } else {
            format!("matches for \u{201c}{}\u{201d} (\u{2605} = in your set)", self.library_query)
        };

        let mut col = Column::new()
            .spacing(s.space_xs)
            .push(
                text_input("Search all patterns…", &self.library_query)
                    .on_input(Message::LibraryQuery),
            )
            .push(text::caption(hint))
            .push(scrollable(list).height(Length::Fixed(330.0)));

        if let Some(name) = &self.lib_selected {
            col = col.push(self.pattern_config(s, name));
        }
        col.into()
    }

    /// Per-pattern config: which model instantiation this pattern uses. Options
    /// are "Default" (the global default) + each model[/variant] from the Models
    /// view. Defining the model+params happens once, in Models.
    fn pattern_config(&self, s: &cosmic::cosmic_theme::Spacing, name: &str) -> Element<'_, Message> {
        let mut opts: Vec<String> = vec![DEFAULT_VENDOR.to_string()];
        opts.extend(self.policy.use_options());
        let cur = self.policy.patterns.get(name).and_then(|a| a.use_.clone());
        let idx = match &cur {
            Some(u) => opts.iter().position(|o| o == u),
            None => Some(0), // "Default"
        };
        let nm = name.to_string();
        let opts_cb = opts.clone();
        let use_dd = dropdown(opts, idx, move |i| {
            Message::SetPatternUse(nm.clone(), opts_cb[i].clone())
        });

        let body: Element<_> = if self.policy.models.is_empty() {
            text::caption("No models defined yet — add them in the Models tab.").into()
        } else {
            cosmic::iced::widget::row![text::body("Use").width(Length::Fixed(50.0)), use_dd]
                .spacing(s.space_xs)
                .align_y(Alignment::Center)
                .into()
        };

        container(Column::new().spacing(s.space_xxs).push(text::heading(pretty(name))).push(body))
            .padding(s.space_xs)
            .class(theme::Container::Card)
            .into()
    }

    /// The Models view: define/edit model instantiations + variants, classify
    /// with categories, set the default variant, and see who uses each — the
    /// legible, editable inventory.
    fn models_view(&self, s: &cosmic::cosmic_theme::Spacing) -> Element<'_, Message> {
        let usage = self.policy.usage();
        let chips = |items: &[String]| -> String {
            if items.is_empty() { ": —".into() } else { format!(" [{}]", items.join(", ")) }
        };

        // ---- global default ----
        let duses = self.policy.use_options();
        let dcur = self.policy.default.use_.clone();
        let didx = dcur.as_ref().and_then(|u| duses.iter().position(|o| o == u));
        let duses_cb = duses.clone();
        let default_dd = dropdown(duses, didx, move |i| Message::SetGlobalUse(duses_cb[i].clone()));
        let default_row = cosmic::iced::widget::row![
            text::body("Default model").width(Length::Fixed(110.0)),
            default_dd,
            text::caption(match &dcur {
                Some(_) => String::new(),
                None => format!("currently inline: {}", self.policy.default.label()),
            }),
        ]
        .spacing(s.space_xs)
        .align_y(Alignment::Center);

        // ---- add a model ----
        let vendors: Vec<String> = self.catalog.keys().cloned().collect();
        let vendor_dd = dropdown(vendors, self.am_vendor, Message::AmVendor);
        // Searchable model picker for the chosen vendor's catalog (often long).
        let model_cb = combo_box(
            &self.am_model_state,
            "model\u{2026}",
            self.am_model.as_ref(),
            Message::AmModel,
        )
        .width(Length::Fixed(200.0));
        let add_row = cosmic::iced::widget::row![
            text_input("new model name", &self.am_name).on_input(Message::AmName).width(Length::Fixed(150.0)),
            vendor_dd,
            model_cb,
            button::standard("Add").on_press(Message::AddModel),
        ]
        .spacing(s.space_xs)
        .align_y(Alignment::Center);

        // ---- model cards ----
        let mut list = Column::new().spacing(s.space_s);
        for (name, m) in &self.policy.models {
            let editing = self.model_selected.as_deref() == Some(name.as_str());
            let n1 = name.clone();
            let n2 = name.clone();
            let mut card = Column::new().spacing(s.space_xxs).push(
                cosmic::iced::widget::row![
                    text::heading(name.clone()),
                    cosmic::widget::Space::new().width(Length::Fill),
                    text::caption(format!("{} \u{00b7} {}", m.model, m.vendor)),
                    button::text(if editing { "Done" } else { "Edit" }).on_press(Message::SelectModel(n1)),
                    button::text("\u{2715}").on_press(Message::DeleteModel(n2)),
                ]
                .spacing(s.space_xxs)
                .align_y(Alignment::Center),
            );
            card = card.push(text::caption(format!(
                "capabilities{}   categories{}",
                chips(&m.capabilities),
                chips(&m.categories)
            )));
            if let Some(users) = usage.get(name) {
                card = card.push(text::caption(format!("used by: {}", users.join(", "))));
            }
            for (vname, v) in &m.variants {
                let star = if m.default.as_deref() == Some(vname.as_str()) { "\u{2605} " } else { "  " };
                let usage_key = format!("{name}/{vname}");
                let used = usage.get(&usage_key)
                    .map(|u| format!("  \u{2190} {}", u.join(", ")))
                    .unwrap_or_default();

                if editing {
                    // Row 1 — identity: star · name · thinking dropdown · delete.
                    let thinking_idx = match v.thinking.as_deref() {
                        Some("off") => Some(1usize),
                        Some("on") => Some(2usize),
                        _ => None,
                    };
                    let (mn_th, vn_th) = (name.clone(), vname.clone());
                    let (mn_del, vn_del) = (name.clone(), vname.clone());
                    let row1 = cosmic::iced::widget::row![
                        text::caption(format!("{star}{vname}{used}")),
                        cosmic::widget::Space::new().width(Length::Fill),
                        dropdown(
                            THINKING_OPTS.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                            thinking_idx,
                            move |i| Message::SetVariantThinking(mn_th.clone(), vn_th.clone(), i),
                        ),
                        button::text("\u{2715}").on_press(Message::DeleteVariant(mn_del, vn_del)),
                    ]
                    .spacing(s.space_xxs)
                    .align_y(Alignment::Center);
                    card = card.push(row1);

                    // Row 2 — knobs. text_input values are borrowed from the
                    // edit map (lifetime tied to &self); missing entry → "".
                    let edit_ref = self.variant_edits.get(&(name.clone(), vname.clone()));
                    let ctx_v = edit_ref.map(|e| e.ctx.as_str()).unwrap_or("");
                    let temp_v = edit_ref.map(|e| e.temperature.as_str()).unwrap_or("");
                    let top_v = edit_ref.map(|e| e.top_p.as_str()).unwrap_or("");
                    let freq_v = edit_ref.map(|e| e.frequency_penalty.as_str()).unwrap_or("");
                    let pres_v = edit_ref.map(|e| e.presence_penalty.as_str()).unwrap_or("");
                    let (mn_c, vn_c) = (name.clone(), vname.clone());
                    let (mn_t, vn_t) = (name.clone(), vname.clone());
                    let (mn_p, vn_p) = (name.clone(), vname.clone());
                    let (mn_f, vn_f) = (name.clone(), vname.clone());
                    let (mn_r, vn_r) = (name.clone(), vname.clone());
                    let row2 = cosmic::iced::widget::row![
                        text_input("ctx", ctx_v)
                            .on_input(move |s| Message::SetVariantField(mn_c.clone(), vn_c.clone(), VariantField::Ctx, s))
                            .width(Length::Fixed(60.0)),
                        text_input("temp", temp_v)
                            .on_input(move |s| Message::SetVariantField(mn_t.clone(), vn_t.clone(), VariantField::Temperature, s))
                            .width(Length::Fixed(60.0)),
                        text_input("topP", top_v)
                            .on_input(move |s| Message::SetVariantField(mn_p.clone(), vn_p.clone(), VariantField::TopP, s))
                            .width(Length::Fixed(60.0)),
                        text_input("freqPen", freq_v)
                            .on_input(move |s| Message::SetVariantField(mn_f.clone(), vn_f.clone(), VariantField::FrequencyPenalty, s))
                            .width(Length::Fixed(70.0)),
                        text_input("presPen", pres_v)
                            .on_input(move |s| Message::SetVariantField(mn_r.clone(), vn_r.clone(), VariantField::PresencePenalty, s))
                            .width(Length::Fixed(70.0)),
                    ]
                    .spacing(s.space_xxs)
                    .align_y(Alignment::Center);
                    card = card.push(row2);
                } else {
                    // Read-only compact summary — covers all knobs that are set.
                    let mut parts: Vec<String> = Vec::new();
                    if let Some(c) = v.ctx { parts.push(format!("ctx {c}")); }
                    if let Some(t) = &v.thinking { parts.push(format!("think {t}")); }
                    if let Some(t) = v.temperature { parts.push(format!("temp {t}")); }
                    if let Some(t) = v.top_p { parts.push(format!("topP {t}")); }
                    if let Some(t) = v.frequency_penalty { parts.push(format!("freqPen {t}")); }
                    if let Some(t) = v.presence_penalty { parts.push(format!("presPen {t}")); }
                    let line = text::caption(format!(
                        "{star}{vname}: {}{}",
                        if parts.is_empty() { "base params".into() } else { parts.join(", ") },
                        used,
                    ));
                    card = card.push(line);
                }
            }

            if editing {
                card = card.push(divider::horizontal::default());
                // categories editor
                let nm_c = name.clone();
                card = card.push(
                    cosmic::iced::widget::row![
                        text::body("Categories").width(Length::Fixed(90.0)),
                        text_input("comma, separated", &self.cat_draft)
                            .on_input(Message::CatDraft)
                            .on_submit(move |_| Message::CommitCats(nm_c.clone())),
                    ]
                    .spacing(s.space_xs)
                    .align_y(Alignment::Center),
                );
                // default-variant picker
                if !m.variants.is_empty() {
                    let vnames: Vec<String> = m.variants.keys().cloned().collect();
                    let vidx = m.default.as_ref().and_then(|d| vnames.iter().position(|v| v == d));
                    let nm_d = name.clone();
                    let vnames_cb = vnames.clone();
                    card = card.push(
                        cosmic::iced::widget::row![
                            text::body("Default variant").width(Length::Fixed(90.0)),
                            dropdown(vnames, vidx, move |i| Message::SetModelDefaultVariant(
                                nm_d.clone(),
                                vnames_cb[i].clone()
                            )),
                        ]
                        .spacing(s.space_xs)
                        .align_y(Alignment::Center),
                    );
                }
                // Add-variant row: name only — knobs are edited inline on the
                // resulting row (decision 2's settled UX).
                let nm_v = name.clone();
                card = card.push(
                    cosmic::iced::widget::row![
                        text_input("new variant name", &self.av_name)
                            .on_input(Message::AvName)
                            .on_submit(move |_| Message::AddVariant(nm_v.clone()))
                            .width(Length::Fixed(160.0)),
                        button::standard("+ new variant").on_press(Message::AddVariant(name.clone())),
                    ]
                    .spacing(s.space_xs)
                    .align_y(Alignment::Center),
                );
            }
            list = list.push(container(card).padding(s.space_xs).class(theme::Container::Card));
        }
        if self.policy.models.is_empty() {
            list = list.push(text::caption("No models yet — add one above."));
        }

        Column::new()
            .spacing(s.space_xs)
            .push(default_row)
            .push(add_row)
            .push(text::caption("\u{2605} = default variant.  Edit a model to add variants / categories.  Variant knobs edit inline; blank = use the model default."))
            .push(scrollable(list).height(Length::Fixed(380.0)))
            .into()
    }

    fn source_section(&self, s: &cosmic::cosmic_theme::Spacing) -> Element<'_, Message> {
        let mut col = Column::new().spacing(s.space_xs);

        // origin picker
        let mut origins = cosmic::iced::widget::row![]
            .spacing(s.space_xxs)
            .align_y(Alignment::Center);
        origins = origins.push(text::heading("Source"));
        origins = origins.push(cosmic::widget::Space::new().width(Length::Fill));
        for (o, label) in [
            (Origin::Clipboard, "Clipboard"),
            (Origin::Text, "Text"),
            (Origin::File, "File"),
            (Origin::Url, "URL"),
            (Origin::Image, "Image"),
        ] {
            origins = origins.push(origin_btn(label, self.origin == o, Some(Message::SetOrigin(o))));
        }
        // disabled future origin
        origins = origins.push(origin_btn("Audio", false, None));
        col = col.push(origins);

        // per-origin loader controls
        match self.origin {
            Origin::Clipboard => {
                col = col.push(
                    cosmic::iced::widget::row![
                        button::icon(icon::from_name("edit-paste-symbolic"))
                            .on_press(Message::LoadClipboard),
                        text::caption("Load from clipboard"),
                    ]
                    .spacing(s.space_xxs)
                    .align_y(Alignment::Center),
                );
            }
            Origin::Url => {
                col = col.push(
                    cosmic::iced::widget::row![
                        text_input("https://…", &self.url_input)
                            .on_input(Message::UrlInput)
                            .on_submit(|_| Message::FetchUrl)
                            .width(Length::Fill),
                        button::standard("Fetch").on_press(Message::FetchUrl),
                    ]
                    .spacing(s.space_xs)
                    .align_y(Alignment::Center),
                );
            }
            Origin::File => {
                col = col.push(
                    cosmic::iced::widget::row![
                        text_input("/path/to/file or ~/file", &self.file_input)
                            .on_input(Message::FileInput)
                            .on_submit(|_| Message::LoadFile)
                            .width(Length::Fill),
                        button::standard("Load").on_press(Message::LoadFile),
                    ]
                    .spacing(s.space_xs)
                    .align_y(Alignment::Center),
                );
            }
            Origin::Image => {
                col = col.push(
                    cosmic::iced::widget::row![
                        text_input("/path/to/image.png", &self.image_path)
                            .on_input(Message::ImagePath)
                            .width(Length::Fill),
                        button::standard("From clipboard").on_press(Message::ImageFromClipboard),
                    ]
                    .spacing(s.space_xs)
                    .align_y(Alignment::Center),
                );
                col = col.push(text::caption(
                    "Vision run — the box below is your question; Run auto-picks a vision model.",
                ));
            }
            Origin::Text => {}
        }

        // the editable source itself (for Image: your question about it)
        col = col.push(
            text_editor(&self.source)
                .placeholder("Type or load the source text…")
                .height(Length::Fixed(120.0))
                .padding(s.space_xs)
                .on_action(Message::SourceAction),
        );
        let chars = self.source.text().chars().count();
        let note = match &self.transform_note {
            Some(n) => format!("{chars} chars  \u{00b7}  {n}"),
            None => format!("{chars} chars"),
        };
        col = col.push(
            cosmic::iced::widget::row![
                text::caption(note),
                cosmic::widget::Space::new().width(Length::Fill),
                button::icon(icon::from_name("edit-clear-all-symbolic")).on_press(Message::Clear),
            ]
            .align_y(Alignment::Center),
        );
        col.into()
    }

    fn prompt_card(&self, s: &cosmic::cosmic_theme::Spacing) -> Element<'_, Message> {
        if self.origin == Origin::Image {
            return container(
                Column::new()
                    .spacing(s.space_xxs)
                    .push(text::heading("Prompt"))
                    .push(text::caption(
                        "\u{1f5bc} Vision run — the image + your question go to the model; \
                         no text prompt is assembled.",
                    )),
            )
            .padding(s.space_xs)
            .class(theme::Container::Card)
            .into();
        }
        let title = self
            .selected_idx
            .map(|i| format!("Prompt  \u{00b7}  {}", self.pattern_labels[i]))
            .unwrap_or_else(|| "Prompt".into());
        let chars = self.prompt.as_deref().map(|p| p.chars().count()).unwrap_or(0);
        let arrow = if self.prompt_collapsed { "\u{25b8}" } else { "\u{25be}" };
        let head = cosmic::iced::widget::row![
            button::text(format!("{arrow}  {title}")).on_press(Message::TogglePrompt),
            cosmic::widget::Space::new().width(Length::Fill),
            text::caption(format!("assembled \u{00b7} {chars} chars")),
            self.copy_control(),
        ]
        .spacing(s.space_xs)
        .align_y(Alignment::Center);

        let mut card = Column::new().spacing(s.space_xxs).push(head);
        if !self.prompt_collapsed {
            let body = match &self.prompt {
                Some(p) if !p.is_empty() => scrollable(text::monotext(p.clone()))
                    .height(Length::Fixed(150.0)),
                _ => scrollable(text::body("Pick a pattern to assemble the prompt."))
                    .height(Length::Fixed(40.0)),
            };
            card = card.push(body);
        }
        container(card)
            .padding(s.space_xs)
            .class(theme::Container::Card)
            .into()
    }

    fn response_card(&self, s: &cosmic::cosmic_theme::Spacing) -> Element<'_, Message> {
        let meta = if self.running {
            "running…".to_string()
        } else {
            self.result_meta.clone().unwrap_or_default()
        };
        let arrow = if self.response_collapsed { "\u{25b8}" } else { "\u{25be}" };
        let head = cosmic::iced::widget::row![
            button::text(format!("{arrow}  Response")).on_press(Message::ToggleResponse),
            cosmic::widget::Space::new().width(Length::Fill),
            text::caption(meta),
            self.send_control(),
        ]
        .spacing(s.space_xs)
        .align_y(Alignment::Center);

        let mut card = Column::new().spacing(s.space_xxs).push(head);
        if !self.response_collapsed {
            let body = match &self.response {
                Some(r) if !r.is_empty() => {
                    scrollable(text::body(r.clone())).height(Length::Fixed(180.0))
                }
                _ => {
                    let placeholder = if self.running {
                        "Generating\u{2026}"
                    } else {
                        "Press Run to generate a response."
                    };
                    scrollable(text::body(placeholder)).height(Length::Fixed(40.0))
                }
            };
            card = card.push(body);
        }
        container(card)
            .padding(s.space_xs)
            .class(theme::Container::Card)
            .into()
    }

    fn conversation(&self) -> String {
        let src = self.source.text();
        let prompt = self.prompt.clone().unwrap_or_default();
        let resp = self.response.clone().unwrap_or_default();
        format!(
            "## Source\n\n{src}\n\n## Prompt\n\n{prompt}\n\n## Response\n\n{resp}\n"
        )
    }

    fn artifact_text(&self, a: Artifact) -> (String, &'static str) {
        match a {
            Artifact::Prompt => (self.prompt.clone().unwrap_or_default(), "Prompt"),
            Artifact::Response => (self.response.clone().unwrap_or_default(), "Response"),
            Artifact::Conversation => (self.conversation(), "Conversation"),
        }
    }

    fn has_response(&self) -> bool {
        self.response.as_deref().map(|r| !r.is_empty()).unwrap_or(false)
    }

    /// Consolidated **Copy ▾**: primary copies the most relevant artifact
    /// (Response after a run, else Prompt); the ▾ picks Prompt / Response /
    /// Conversation explicitly. Lives on the Prompt card header.
    fn copy_control(&self) -> Element<'_, Message> {
        let default_art = if self.ran { Artifact::Response } else { Artifact::Prompt };
        let can_default = match default_art {
            Artifact::Prompt => self.prompt.is_some(),
            _ => self.has_response(),
        };
        let primary = {
            let b = button::standard("Copy");
            if can_default { b.on_press(Message::Route(default_art, Dest::Copy)) } else { b }
        };
        let caret = button::text("\u{25be}").on_press(Message::ToggleMenu(MenuKind::Copy));
        let anchor = cosmic::iced::widget::row![primary, caret]
            .spacing(2)
            .align_y(Alignment::Center);
        let mut pop = cosmic::widget::popover(anchor);
        if self.open_menu == Some(MenuKind::Copy) {
            pop = pop
                .popup(self.copy_menu())
                .on_close(Message::CloseMenu)
                .position(cosmic::widget::popover::Position::Bottom);
        }
        pop.into()
    }

    fn copy_menu(&self) -> Element<'_, Message> {
        let sp = theme::active().cosmic().spacing;
        let items = [
            ("Copy Prompt", Artifact::Prompt, self.prompt.is_some()),
            ("Copy Response", Artifact::Response, self.has_response()),
            ("Copy Conversation", Artifact::Conversation, self.ran),
        ];
        let mut menu = Column::new().spacing(2).padding(sp.space_xxs);
        for (label, art, enabled) in items {
            let b = button::text(label).width(Length::Fixed(200.0));
            let b = if enabled { b.on_press(Message::Route(art, Dest::Copy)) } else { b };
            menu = menu.push(b);
        }
        container(menu).class(theme::Container::Card).into()
    }

    /// Consolidated **Send ▾**: the destination registry (minus Copy, which has
    /// its own control) plus "Continue in Chat", routing the Conversation. Lives
    /// on the Response card header.
    fn send_control(&self) -> Element<'_, Message> {
        let anchor = button::standard("Send \u{25be}").on_press(Message::ToggleMenu(MenuKind::Send));
        let mut pop = cosmic::widget::popover(anchor);
        if self.open_menu == Some(MenuKind::Send) {
            pop = pop
                .popup(self.send_menu())
                .on_close(Message::CloseMenu)
                .position(cosmic::widget::popover::Position::Bottom);
        }
        pop.into()
    }

    fn send_menu(&self) -> Element<'_, Message> {
        let sp = theme::active().cosmic().spacing;
        let mut menu = Column::new().spacing(2).padding(sp.space_xxs);
        for d in destinations() {
            if d.dest == Dest::Copy {
                continue; // Copy is its own control now
            }
            let label = match d.note {
                Some(n) => format!("{}   ({n})", d.label),
                None => d.label.to_string(),
            };
            let b = button::text(label).width(Length::Fixed(220.0));
            let b = if d.enabled {
                b.on_press(Message::Route(Artifact::Conversation, d.dest))
            } else {
                b
            };
            menu = menu.push(b);
        }
        menu = menu.push(
            button::text("Continue in Chat")
                .width(Length::Fixed(220.0))
                .on_press(Message::ContinueInChat),
        );
        container(menu).class(theme::Container::Card).into()
    }

    fn trigger_assemble(&mut self) -> app::Task<Message> {
        if self.origin == Origin::Image {
            self.prompt = None; // image runs send the image + question, not an assembled text prompt
            return app::Task::none();
        }
        let Some(idx) = self.selected_idx else {
            self.prompt = None;
            return app::Task::none();
        };
        let pattern = self.patterns[idx].clone();
        let input = self.source.text();
        self.asm_gen += 1;
        let seq = self.asm_gen;
        cosmic::Task::perform(daemon::assemble(pattern, input), move |r| {
            cosmic::Action::App(Message::AssembleDone(seq, r))
        })
    }
}

// cosmic's `button::{text,standard,suggested}` return distinct `Builder` types,
// so these helpers collapse to `Element` (with on_press folded in; omitting it
// disables the button).
fn origin_btn(label: &str, active: bool, msg: Option<Message>) -> Element<'_, Message> {
    if active {
        let b = button::suggested(label);
        match msg {
            Some(m) => b.on_press(m).into(),
            None => b.into(),
        }
    } else {
        let b = button::text(label);
        match msg {
            Some(m) => b.on_press(m).into(),
            None => b.into(),
        }
    }
}

fn meta_line(rr: &RunResult) -> String {
    let model = rr.model.clone().unwrap_or_default();
    let place = rr
        .placement
        .map(|p| format!("  \u{00b7}  {p:.0}% GPU"))
        .unwrap_or_default();
    format!("{model}{place}")
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_string()
}

fn save_to_file(text: &str, pattern: Option<&str>) -> Result<String, String> {
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let pat = pattern.unwrap_or("fabric");
    let path = format!("{home}/{pat}-{ts}.md");
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(path)
}

fn woollama_models_task() -> app::Task<Message> {
    cosmic::Task::perform(daemon::woollama_models(), |r| {
        cosmic::Action::App(Message::WoollamaModelsDone(r))
    })
}

fn status_task() -> app::Task<Message> {
    cosmic::Task::perform(daemon::status(), |r| {
        cosmic::Action::App(Message::StatusDone(r))
    })
}

fn patterns_task() -> app::Task<Message> {
    cosmic::Task::perform(daemon::patterns(), |r| {
        cosmic::Action::App(Message::PatternsDone(r))
    })
}

fn catalog_task() -> app::Task<Message> {
    cosmic::Task::perform(daemon::catalog(), |r| {
        cosmic::Action::App(Message::CatalogDone(r))
    })
}

fn load_clipboard_task() -> app::Task<Message> {
    cosmic::Task::perform(async {}, |_| cosmic::Action::App(Message::LoadClipboard))
}

fn debounce_assemble(gen: u64) -> app::Task<Message> {
    cosmic::Task::perform(
        async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            gen
        },
        |g| cosmic::Action::App(Message::AssembleDebounced(g)),
    )
}

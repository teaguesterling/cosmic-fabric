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
    widget::{button, container, divider, dropdown, scrollable, text, text_editor, text_input},
    Element,
};

use std::collections::BTreeMap;

use crate::daemon::{self, RunResult, Status};
use crate::policy::{self, Assignment, Policy};

pub const WORKSPACE_APP_ID: &str = "com.github.teaguesterling.CosmicFabric.Workspace";

/// Sentinel in the vendor dropdown meaning "no per-pattern override — use the
/// global default model."
const DEFAULT_VENDOR: &str = "Default";

/// New-variant `thinking` dropdown: index 0 = inherit/none, 1 = off, 2 = on.
const THINKING_OPTS: [&str; 3] = ["(default)", "off", "on"];

pub fn run() -> cosmic::iced::Result {
    let settings = cosmic::app::Settings::default().size(cosmic::iced::Size::new(640.0, 780.0));
    cosmic::app::run::<Workspace>(settings, ())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    Clipboard,
    Text,
    File,
    Url,
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
    selected_idx: Option<usize>,
    mode: WorkMode,
    library_query: String,
    catalog: BTreeMap<String, Vec<String>>, // vendor → models, for the picker
    lib_selected: Option<String>,           // pattern being configured in Library
    // --- Models editor drafts ---
    model_selected: Option<String>, // model whose editor is open
    am_name: String,                // new-model name
    am_vendor: Option<usize>,       // new-model vendor (index into catalog keys)
    am_model: Option<usize>,        // new-model model (index into that vendor's list)
    cat_draft: String,              // selected model's categories (comma-edited)
    av_name: String,                // new-variant name
    av_ctx: String,                 // new-variant ctx (numeric text)
    av_thinking: Option<usize>,     // new-variant thinking (index into THINKING_OPTS)

    origin: Origin,
    source: text_editor::Content,
    url_input: String,
    file_input: String,
    transform_note: Option<String>, // e.g. "fetched · 4,210 chars markdown"

    prompt: Option<String>,
    prompt_collapsed: bool,
    asm_gen: u64,
    edit_gen: u64,

    response: Option<String>,
    result_meta: Option<String>,
    running: bool,
    pending: Option<(u64, String, String)>, // (run id, pattern, input) → stream sub
    run_seq: u64,

    error: Option<String>,
    status_msg: Option<String>, // transient (e.g. "saved to …")
    open_menu: Option<Artifact>, // which send-to menu is open
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
    PickPattern(usize),
    AssembleDebounced(u64),
    AssembleDone(u64, Result<String, String>),
    TogglePrompt,
    Run,
    RunEvent(daemon::RunEvent),
    ToggleMenu(Artifact),
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
    AmModel(usize),
    AddModel,
    SelectModel(String),
    DeleteModel(String),
    CatDraft(String),
    CommitCats(String),
    SetModelDefaultVariant(String, String),
    AvName(String),
    AvCtx(String),
    AvThinking(usize),
    AddVariant(String),
    DeleteVariant(String, String),
    SetGlobalUse(String),
    Retry,
    Clear,
    OpenSettings,
    OpenSession,
    Refresh,
    DismissError,
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
        let me = Self {
            core,
            status: None,
            policy: policy::load(),
            all_patterns: Vec::new(),
            patterns: Vec::new(),
            pattern_labels: Vec::new(),
            selected_idx: None,
            mode: WorkMode::Run,
            library_query: String::new(),
            catalog: BTreeMap::new(),
            lib_selected: None,
            model_selected: None,
            am_name: String::new(),
            am_vendor: None,
            am_model: None,
            cat_draft: String::new(),
            av_name: String::new(),
            av_ctx: String::new(),
            av_thinking: None,
            origin: Origin::Clipboard,
            source: text_editor::Content::new(),
            url_input: String::new(),
            file_input: String::new(),
            transform_note: None,
            prompt: None,
            prompt_collapsed: false,
            asm_gen: 0,
            edit_gen: 0,
            response: None,
            result_meta: None,
            running: false,
            pending: None,
            run_seq: 0,
            error: None,
            status_msg: None,
            open_menu: None,
        };
        (
            me,
            cosmic::Task::batch([
                status_task(),
                patterns_task(),
                catalog_task(),
                load_clipboard_task(),
            ]),
        )
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
                |(_, pat, input): &(u64, String, String)| {
                    daemon::run_stream(pat.clone(), input.clone()).map(Message::RunEvent)
                },
            ),
            None => cosmic::iced::Subscription::none(),
        }
    }

    fn update(&mut self, message: Message) -> app::Task<Message> {
        match message {
            Message::StatusDone(Ok(s)) => self.status = Some(s),
            Message::StatusDone(Err(e)) => self.error = Some(e),
            Message::PatternsDone(Ok(p)) => {
                self.all_patterns = p;
                self.recompute_active();
            }
            Message::PatternsDone(Err(e)) => self.error = Some(e),

            Message::SetOrigin(o) => {
                self.origin = o;
                self.transform_note = None;
                self.status_msg = None;
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
            Message::PickPattern(idx) => {
                self.selected_idx = Some(idx);
                return self.trigger_assemble();
            }
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
            Message::TogglePrompt => self.prompt_collapsed = !self.prompt_collapsed,

            Message::Run => {
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
                self.pending = Some((self.run_seq, pattern, input));
                self.response = Some(String::new());
                self.result_meta = None;
                self.running = true;
                self.error = None;
                self.status_msg = None;
            }
            Message::RunEvent(ev) => match ev {
                daemon::RunEvent::Chunk(s) => {
                    if let Some(r) = self.response.as_mut() {
                        r.push_str(&s);
                    }
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
            Message::SetMode(m) => self.mode = m,
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
            }
            Message::AmModel(i) => self.am_model = Some(i),
            Message::AddModel => {
                let name = self.am_name.trim().to_string();
                let vendors: Vec<String> = self.catalog.keys().cloned().collect();
                if !name.is_empty() && !self.policy.models.contains_key(&name) {
                    if let Some(vendor) = self.am_vendor.and_then(|i| vendors.get(i)).cloned() {
                        let model = self
                            .am_model
                            .and_then(|mi| self.catalog.get(&vendor).and_then(|m| m.get(mi)))
                            .cloned()
                            .unwrap_or_default();
                        if !model.is_empty() {
                            self.policy.models.insert(
                                name.clone(),
                                policy::Model { vendor, model, ..Default::default() },
                            );
                            self.persist();
                            self.am_name.clear();
                            self.am_vendor = None;
                            self.am_model = None;
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
            Message::AvCtx(s) => self.av_ctx = s,
            Message::AvThinking(i) => self.av_thinking = Some(i),
            Message::AddVariant(model) => {
                let vname = self.av_name.trim().to_string();
                if !vname.is_empty() {
                    if let Some(m) = self.policy.models.get_mut(&model) {
                        let ctx = self.av_ctx.trim().parse::<u32>().ok();
                        let thinking = match self.av_thinking {
                            Some(1) => Some("off".into()),
                            Some(2) => Some("on".into()),
                            _ => None,
                        };
                        m.variants.insert(
                            vname.clone(),
                            policy::Variant { ctx, thinking, ..Default::default() },
                        );
                        if m.default.is_none() {
                            m.default = Some(vname);
                        }
                        self.persist();
                        self.av_name.clear();
                        self.av_ctx.clear();
                        self.av_thinking = None;
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
            }
            Message::OpenSettings => {
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(exe).arg("settings").spawn();
                }
            }
            Message::OpenSession => {
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(exe).arg("session").spawn();
                }
            }
            Message::Refresh => return cosmic::Task::batch([status_task(), patterns_task()]),
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
                let mut runrow = cosmic::iced::widget::row![dropdown(
                    &self.pattern_labels,
                    self.selected_idx,
                    Message::PickPattern,
                )]
                .spacing(s.space_s)
                .align_y(Alignment::Center);
                let run_btn = button::suggested(if self.running { "Running…" } else { "Run" });
                runrow = runrow.push(if self.running {
                    run_btn
                } else {
                    run_btn.on_press(Message::Run)
                });
                col = col.push(runrow);

                col = col.push(self.prompt_card(&s));
                col = col.push(self.response_card(&s));

                col = col.push(divider::horizontal::default());
                let mut foot = cosmic::iced::widget::row![
                    self.sendto(Artifact::Conversation, "Copy conversation", true),
                    button::text("Clear").on_press(Message::Clear),
                ]
                .spacing(s.space_xs)
                .align_y(Alignment::Center);
                foot = foot.push(cosmic::widget::Space::new().width(Length::Fill));
                foot = foot.push(button::text("Chat…").on_press(Message::OpenSession));
                foot = foot.push(button::text("Refresh").on_press(Message::Refresh));
                foot = foot.push(button::text("Settings…").on_press(Message::OpenSettings));
                col = col.push(foot);
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
        format!("serve {serve}  \u{00b7}  {model}{gpu}")
    }

    fn recompute_active(&mut self) {
        self.patterns = self.policy.active_patterns(&self.all_patterns);
        self.pattern_labels = self.patterns.iter().map(|n| pretty(n)).collect();
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
        let cur_vendor = self.am_vendor.and_then(|i| vendors.get(i)).cloned();
        let vendor_dd = dropdown(vendors, self.am_vendor, Message::AmVendor);
        let models_for_vendor: Vec<String> =
            cur_vendor.as_ref().and_then(|v| self.catalog.get(v)).cloned().unwrap_or_default();
        let model_dd = dropdown(models_for_vendor, self.am_model, Message::AmModel);
        let add_row = cosmic::iced::widget::row![
            text_input("new model name", &self.am_name).on_input(Message::AmName).width(Length::Fixed(150.0)),
            vendor_dd,
            model_dd,
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
                let mut parts: Vec<String> = Vec::new();
                if let Some(c) = v.ctx { parts.push(format!("ctx {c}")); }
                if let Some(t) = &v.thinking { parts.push(format!("think {t}")); }
                if let Some(t) = v.temperature { parts.push(format!("temp {t}")); }
                let key = format!("{name}/{vname}");
                let used = usage.get(&key).map(|u| format!("  \u{2190} {}", u.join(", "))).unwrap_or_default();
                let line = text::caption(format!(
                    "{star}{vname}: {}{}",
                    if parts.is_empty() { "base params".into() } else { parts.join(", ") },
                    used,
                ));
                if editing {
                    let (mn, vn) = (name.clone(), vname.clone());
                    card = card.push(
                        cosmic::iced::widget::row![
                            line,
                            cosmic::widget::Space::new().width(Length::Fill),
                            button::text("\u{2715}").on_press(Message::DeleteVariant(mn, vn)),
                        ]
                        .align_y(Alignment::Center),
                    );
                } else {
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
                // add-variant row
                let nm_v = name.clone();
                card = card.push(
                    cosmic::iced::widget::row![
                        text_input("variant", &self.av_name).on_input(Message::AvName).width(Length::Fixed(90.0)),
                        text_input("ctx", &self.av_ctx).on_input(Message::AvCtx).width(Length::Fixed(70.0)),
                        dropdown(THINKING_OPTS.iter().map(|s| s.to_string()).collect::<Vec<_>>(), self.av_thinking, Message::AvThinking),
                        button::standard("Add variant").on_press(Message::AddVariant(nm_v)),
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
            .push(text::caption("\u{2605} = default variant.  Edit a model to add variants / categories."))
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
        ] {
            origins = origins.push(origin_btn(label, self.origin == o, Some(Message::SetOrigin(o))));
        }
        // disabled future origins
        origins = origins.push(origin_btn("Audio", false, None));
        origins = origins.push(origin_btn("Image", false, None));
        col = col.push(origins);

        // per-origin loader controls
        match self.origin {
            Origin::Clipboard => {
                col = col.push(
                    button::text("Load from clipboard").on_press(Message::LoadClipboard),
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
            Origin::Text => {}
        }

        // the editable source itself
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
        col = col.push(text::caption(note));
        col.into()
    }

    fn prompt_card(&self, s: &cosmic::cosmic_theme::Spacing) -> Element<'_, Message> {
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
        ]
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
            card = card.push(
                cosmic::iced::widget::row![
                    cosmic::widget::Space::new().width(Length::Fill),
                    self.sendto(Artifact::Prompt, "Copy prompt", self.prompt.is_some()),
                ]
                .align_y(Alignment::Center),
            );
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
        let head = cosmic::iced::widget::row![
            text::heading("Response"),
            cosmic::widget::Space::new().width(Length::Fill),
            text::caption(meta),
        ]
        .align_y(Alignment::Center);

        let body = match &self.response {
            Some(r) if !r.is_empty() => {
                scrollable(text::body(r.clone())).height(Length::Fixed(180.0))
            }
            _ => scrollable(text::body("Press Run to generate a response."))
                .height(Length::Fixed(40.0)),
        };
        let has = self.response.as_deref().map(|r| !r.is_empty()).unwrap_or(false);
        let actions = cosmic::iced::widget::row![
            cosmic::widget::Space::new().width(Length::Fill),
            self.sendto(Artifact::Response, "Copy response", has),
        ]
        .spacing(s.space_xs)
        .align_y(Alignment::Center);

        container(Column::new().spacing(s.space_xxs).push(head).push(body).push(actions))
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

    /// A send-to control: a default **Copy** button + a `▾` that opens the
    /// destination registry as a popover menu.
    fn sendto(&self, a: Artifact, default_label: &str, enabled: bool) -> Element<'_, Message> {
        let primary = {
            let b = button::standard(default_label.to_string());
            if enabled {
                b.on_press(Message::Route(a, Dest::Copy))
            } else {
                b
            }
        };
        let caret = {
            let b = button::text("\u{25be}");
            if enabled {
                b.on_press(Message::ToggleMenu(a))
            } else {
                b
            }
        };
        let anchor = cosmic::iced::widget::row![primary, caret]
            .spacing(2)
            .align_y(Alignment::Center);

        let mut pop = cosmic::widget::popover(anchor);
        if self.open_menu == Some(a) {
            pop = pop
                .popup(self.dest_menu(a))
                .on_close(Message::CloseMenu)
                .position(cosmic::widget::popover::Position::Bottom);
        }
        pop.into()
    }

    fn dest_menu(&self, a: Artifact) -> Element<'_, Message> {
        let sp = theme::active().cosmic().spacing;
        let mut menu = Column::new().spacing(2).padding(sp.space_xxs);
        for d in destinations() {
            let label = match d.note {
                Some(n) => format!("{}   ({n})", d.label),
                None => d.label.to_string(),
            };
            let b = button::text(label).width(Length::Fixed(220.0));
            let b = if d.enabled {
                b.on_press(Message::Route(a, d.dest))
            } else {
                b
            };
            menu = menu.push(b);
        }
        container(menu).class(theme::Container::Card).into()
    }

    fn trigger_assemble(&mut self) -> app::Task<Message> {
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

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

use crate::daemon::{self, RunResult, Status};

pub const WORKSPACE_APP_ID: &str = "com.github.teaguesterling.CosmicFabric.Workspace";

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

pub struct Workspace {
    core: cosmic::app::Core,
    status: Option<Status>,
    patterns: Vec<String>,      // raw scribe-* names (daemon)
    pattern_labels: Vec<String>, // pretty labels for the dropdown
    selected_idx: Option<usize>,

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
    Retry,
    Clear,
    OpenSettings,
    Refresh,
    DismissError,
}

/// Pretty verb label for a `scribe-*` pattern name (shared with the popup).
pub fn pretty(name: &str) -> String {
    let base = name.strip_prefix("scribe-").unwrap_or(name).replace(['-', '_'], " ");
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
            patterns: Vec::new(),
            pattern_labels: Vec::new(),
            selected_idx: None,
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
            cosmic::Task::batch([status_task(), patterns_task(), load_clipboard_task()]),
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
                self.patterns = p.into_iter().filter(|n| n.starts_with("scribe-")).collect();
                self.pattern_labels = self.patterns.iter().map(|n| pretty(n)).collect();
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
        col = col.push(header);
        col = col.push(divider::horizontal::default());

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

        // ---- prompt card ----
        col = col.push(self.prompt_card(&s));
        // ---- response card ----
        col = col.push(self.response_card(&s));

        // ---- conversation + footer ----
        col = col.push(divider::horizontal::default());
        let mut foot = cosmic::iced::widget::row![
            self.sendto(Artifact::Conversation, "Copy conversation", true),
            button::text("Clear").on_press(Message::Clear),
        ]
        .spacing(s.space_xs)
        .align_y(Alignment::Center);
        foot = foot.push(cosmic::widget::Space::new().width(Length::Fill));
        foot = foot.push(button::text("Refresh").on_press(Message::Refresh));
        foot = foot.push(button::text("Settings…").on_press(Message::OpenSettings));
        col = col.push(foot);

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

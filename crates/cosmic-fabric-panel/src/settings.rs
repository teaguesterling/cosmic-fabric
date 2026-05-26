//! Standalone settings window (`cosmic-fabric-panel settings`) — edits
//! `~/.config/cosmic-fabric/policy.toml`. Auto-saves on each change; the daemon
//! re-reads the file per run, so edits take effect immediately.
//!
//! Model selection uses coarse tiers (Default / Local / Cloud) rather than the
//! full model list — better UX and matches the deployment policy. For other
//! models, edit policy.toml directly.

use cosmic::{
    app,
    iced::{
        widget::{row, Column},
        Alignment, Length,
    },
    theme,
    widget::{button, container, divider, radio, scrollable, text, text_input},
    Element,
};

use crate::daemon;
use crate::policy::{self, ModelPick, Policy};

pub const SETTINGS_APP_ID: &str = "com.github.teaguesterling.CosmicFabric.Settings";

pub fn run() -> cosmic::iced::Result {
    let settings = cosmic::app::Settings::default().size(cosmic::iced::Size::new(560.0, 720.0));
    cosmic::app::run::<SettingsApp>(settings, ())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    Default,
    Local,
    Haiku,
    Sonnet,
}
impl Tier {
    fn short(self) -> &'static str {
        match self {
            Tier::Default => "Default",
            Tier::Local => "Local",
            Tier::Haiku => "Haiku",
            Tier::Sonnet => "Sonnet",
        }
    }
    fn long(self) -> &'static str {
        match self {
            Tier::Default => "Default",
            Tier::Local => "Local (iq4xs)",
            Tier::Haiku => "Cloud \u{00b7} Haiku",
            Tier::Sonnet => "Cloud \u{00b7} Sonnet",
        }
    }
    fn pick(self) -> Option<ModelPick> {
        match self {
            Tier::Default => None,
            Tier::Local => Some(ModelPick::default()),
            Tier::Haiku => Some(ModelPick {
                model: "claude-haiku-4-5".into(),
                vendor: "Anthropic".into(),
                extra: vec![],
            }),
            Tier::Sonnet => Some(ModelPick {
                model: "claude-sonnet-4-6".into(),
                vendor: "Anthropic".into(),
                extra: vec![],
            }),
        }
    }
    fn of_model(model: &str) -> Tier {
        if model.contains("haiku") {
            Tier::Haiku
        } else if model.contains("sonnet") {
            Tier::Sonnet
        } else {
            Tier::Local
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Notify,
    Dialog,
    Edit,
    Clipboard,
}
impl Mode {
    const ALL: [Mode; 4] = [Mode::Notify, Mode::Dialog, Mode::Edit, Mode::Clipboard];
    fn label(self) -> &'static str {
        match self {
            Mode::Notify => "Notification with View/Edit buttons",
            Mode::Dialog => "Open a dialog window",
            Mode::Edit => "Open in the editor",
            Mode::Clipboard => "Clipboard only",
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Mode::Notify => "notify",
            Mode::Dialog => "dialog",
            Mode::Edit => "edit",
            Mode::Clipboard => "clipboard",
        }
    }
    fn from_str(s: &str) -> Mode {
        match s {
            "dialog" => Mode::Dialog,
            "edit" => Mode::Edit,
            "clipboard" => Mode::Clipboard,
            _ => Mode::Notify,
        }
    }
}

pub struct SettingsApp {
    core: cosmic::app::Core,
    policy: Policy,
    patterns: Vec<String>,
    warn_str: String,
    last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    PatternsDone(Result<Vec<String>, String>),
    SetMode(Mode),
    SetDefaultTier(Tier),
    SetPatternTier(String, Tier),
    SetOllamaUrl(String),
    SetWarn(String),
    DismissError,
}

impl cosmic::Application for SettingsApp {
    type Message = Message;
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    const APP_ID: &'static str = SETTINGS_APP_ID;

    fn init(core: app::Core, _flags: Self::Flags) -> (Self, app::Task<Self::Message>) {
        let policy = policy::load();
        let warn_str = policy.ollama.warn_below_gpu.to_string();
        let me = Self {
            core,
            policy,
            patterns: Vec::new(),
            warn_str,
            last_error: None,
        };
        (me, patterns_task())
    }
    fn core(&self) -> &cosmic::app::Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut cosmic::app::Core {
        &mut self.core
    }

    fn update(&mut self, message: Message) -> app::Task<Message> {
        match message {
            Message::PatternsDone(Ok(p)) => self.patterns = p,
            Message::PatternsDone(Err(e)) => self.last_error = Some(e),
            Message::SetMode(m) => {
                self.policy.output.mode = m.as_str().into();
                self.persist();
            }
            Message::SetDefaultTier(t) => {
                if let Some(p) = t.pick() {
                    self.policy.default = p;
                    self.persist();
                }
            }
            Message::SetPatternTier(name, t) => {
                match t.pick() {
                    Some(p) => {
                        self.policy.patterns.insert(name, p);
                    }
                    None => {
                        self.policy.patterns.remove(&name);
                    }
                }
                self.persist();
            }
            Message::SetOllamaUrl(s) => {
                self.policy.ollama.url = s;
                self.persist();
            }
            Message::SetWarn(s) => {
                self.warn_str = s.clone();
                if let Ok(n) = s.trim().parse::<u32>() {
                    self.policy.ollama.warn_below_gpu = n;
                    self.persist();
                }
            }
            Message::DismissError => self.last_error = None,
        }
        app::Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let spacing = theme::active().cosmic().spacing;
        let mut col = Column::new()
            .spacing(spacing.space_s)
            .padding(spacing.space_m);

        col = col.push(text::title3("Fabric settings"));
        col = col.push(divider::horizontal::default());

        // ---- default model ----
        col = col.push(text::heading("Default model"));
        let dt = Tier::of_model(&self.policy.default.model);
        let mut dr = row![].spacing(spacing.space_s);
        for t in [Tier::Local, Tier::Haiku, Tier::Sonnet] {
            dr = dr.push(radio(text::body(t.long()), t, Some(dt), Message::SetDefaultTier));
        }
        col = col.push(dr);
        col = col.push(divider::horizontal::default());

        // ---- result delivery ----
        col = col.push(text::heading("Result delivery"));
        let cur = Mode::from_str(&self.policy.output.mode);
        for m in Mode::ALL {
            col = col.push(radio(text::body(m.label()), m, Some(cur), Message::SetMode));
        }
        col = col.push(divider::horizontal::default());

        // ---- per-pattern model ----
        col = col.push(text::heading("Per-pattern model"));
        col = col.push(text::caption("Default = use the default model above."));
        for name in self.patterns.iter().filter(|n| n.starts_with("scribe-")) {
            let sel = self
                .policy
                .patterns
                .get(name)
                .map(|p| Tier::of_model(&p.model))
                .unwrap_or(Tier::Default);
            let mut r = row![text::body(name.clone()).width(Length::Fixed(170.0))]
                .spacing(spacing.space_xs)
                .align_y(Alignment::Center);
            for t in [Tier::Default, Tier::Local, Tier::Haiku, Tier::Sonnet] {
                let n = name.clone();
                r = r.push(radio(
                    text::caption(t.short()),
                    t,
                    Some(sel),
                    move |t| Message::SetPatternTier(n.clone(), t),
                ));
            }
            col = col.push(r);
        }
        col = col.push(divider::horizontal::default());

        // ---- ollama ----
        col = col.push(text::heading("Ollama"));
        col = col.push(
            row![
                text::body("Server URL").width(Length::Fixed(140.0)),
                text_input("http://localhost:11434", &self.policy.ollama.url)
                    .on_input(Message::SetOllamaUrl)
                    .width(Length::Fill),
            ]
            .spacing(spacing.space_xs)
            .align_y(Alignment::Center),
        );
        col = col.push(
            row![
                text::body("Warn below GPU%").width(Length::Fixed(140.0)),
                text_input("99", &self.warn_str)
                    .on_input(Message::SetWarn)
                    .width(Length::Fixed(80.0)),
            ]
            .spacing(spacing.space_xs)
            .align_y(Alignment::Center),
        );

        if let Some(err) = &self.last_error {
            col = col.push(divider::horizontal::default());
            col = col.push(
                row![
                    text::body(err.clone()).width(Length::Fill),
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

impl SettingsApp {
    fn persist(&mut self) {
        if let Err(e) = policy::save(&self.policy) {
            self.last_error = Some(format!("save failed: {e}"));
        }
    }
}

fn patterns_task() -> app::Task<Message> {
    cosmic::Task::perform(daemon::patterns(), |r| {
        cosmic::Action::App(Message::PatternsDone(r))
    })
}

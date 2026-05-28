//! Standalone settings window (`cosmic-fabric-panel settings`) — edits the
//! non-model globals of `~/.config/cosmic-fabric/policy.toml` (result delivery,
//! Ollama URL, GPU-warn threshold). Auto-saves; the daemon re-reads per run.
//!
//! Model configuration moved to the Workbench's **Models** view (named model
//! instantiations); this window no longer touches models.

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

use crate::policy::{self, Policy};

pub const SETTINGS_APP_ID: &str = "com.github.teaguesterling.CosmicFabric.Settings";

pub fn run() -> cosmic::iced::Result {
    let settings = cosmic::app::Settings::default().size(cosmic::iced::Size::new(560.0, 520.0));
    cosmic::app::run::<SettingsApp>(settings, ())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Notify,
    Dialog,
    Edit,
    Clipboard,
    Panel,
}
impl Mode {
    const ALL: [Mode; 5] = [Mode::Notify, Mode::Dialog, Mode::Edit, Mode::Clipboard, Mode::Panel];
    fn label(self) -> &'static str {
        match self {
            Mode::Notify => "Notification with View/Edit buttons",
            Mode::Dialog => "Open a dialog window",
            Mode::Edit => "Open in the editor",
            Mode::Clipboard => "Clipboard only",
            Mode::Panel => "Show in the Fabric panel (needs the panel applet)",
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Mode::Notify => "notify",
            Mode::Dialog => "dialog",
            Mode::Edit => "edit",
            Mode::Clipboard => "clipboard",
            Mode::Panel => "panel",
        }
    }
    fn from_str(s: &str) -> Mode {
        match s {
            "dialog" => Mode::Dialog,
            "edit" => Mode::Edit,
            "clipboard" => Mode::Clipboard,
            "panel" => Mode::Panel,
            _ => Mode::Notify,
        }
    }
}

pub struct SettingsApp {
    core: cosmic::app::Core,
    policy: Policy,
    warn_str: String,
    last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    SetMode(Mode),
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
            warn_str,
            last_error: None,
        };
        (me, app::Task::none())
    }
    fn core(&self) -> &cosmic::app::Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut cosmic::app::Core {
        &mut self.core
    }

    fn update(&mut self, message: Message) -> app::Task<Message> {
        match message {
            Message::SetMode(m) => {
                self.policy.output.mode = m.as_str().into();
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
        col = col.push(text::caption("Models are configured in the Workbench → Models."));
        col = col.push(divider::horizontal::default());

        // ---- result delivery ----
        col = col.push(text::heading("Result delivery"));
        let cur = Mode::from_str(&self.policy.output.mode);
        for m in Mode::ALL {
            col = col.push(radio(text::body(m.label()), m, Some(cur), Message::SetMode));
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

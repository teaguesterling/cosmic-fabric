//! The Session surface (`cosmic-fabric-panel session`) — a lightweight IM-style
//! chat dialog for multi-turn / CoT, backed by fabric **sessions** (`sessionName`,
//! history kept server-side). Lighter than the Workbench; depth lives here, while
//! the loom/kit stay single-shot. Turns relay through `raw_query` by default.

use cosmic::iced::futures::StreamExt;
use cosmic::{
    app,
    iced::{widget::Column, Alignment, Length},
    theme,
    widget::{button, container, divider, scrollable, text, text_input},
    Element,
};

use crate::daemon;

pub const SESSION_APP_ID: &str = "com.github.teaguesterling.CosmicFabric.Session";

pub fn run() -> cosmic::iced::Result {
    let settings = cosmic::app::Settings::default().size(cosmic::iced::Size::new(560.0, 720.0));
    cosmic::app::run::<SessionApp>(settings, ())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    User,
    Assistant,
}

pub struct SessionApp {
    core: cosmic::app::Core,
    session: String,
    messages: Vec<(Role, String)>,
    input: String,
    pending: Option<(u64, String, String)>, // (id, session, input) → stream key
    run_seq: u64,
    streaming: bool,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    InputChanged(String),
    Send,
    ChatEvent(daemon::RunEvent),
    NewSession,
}

fn new_session_name() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("chat-{ts}")
}

impl cosmic::Application for SessionApp {
    type Message = Message;
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    const APP_ID: &'static str = SESSION_APP_ID;

    fn init(core: app::Core, _flags: Self::Flags) -> (Self, app::Task<Self::Message>) {
        let me = Self {
            core,
            session: new_session_name(),
            messages: Vec::new(),
            input: String::new(),
            pending: None,
            run_seq: 0,
            streaming: false,
            error: None,
        };
        (me, app::Task::none())
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
                |(_, session, input): &(u64, String, String)| {
                    daemon::chat_stream(session.clone(), input.clone()).map(Message::ChatEvent)
                },
            ),
            None => cosmic::iced::Subscription::none(),
        }
    }

    fn update(&mut self, message: Message) -> app::Task<Message> {
        match message {
            Message::InputChanged(s) => self.input = s,
            Message::Send => {
                let input = self.input.trim().to_string();
                if input.is_empty() || self.streaming {
                    return app::Task::none();
                }
                self.messages.push((Role::User, input.clone()));
                self.messages.push((Role::Assistant, String::new()));
                self.run_seq += 1;
                self.pending = Some((self.run_seq, self.session.clone(), input));
                self.input.clear();
                self.streaming = true;
                self.error = None;
            }
            Message::ChatEvent(ev) => match ev {
                daemon::RunEvent::Chunk(c) => {
                    if let Some(last) = self.messages.last_mut() {
                        last.1.push_str(&c);
                    }
                }
                daemon::RunEvent::Done(_) => {
                    self.streaming = false;
                    self.pending = None;
                }
                daemon::RunEvent::Error(e) => {
                    self.streaming = false;
                    self.pending = None;
                    self.error = Some(e);
                    // drop the empty assistant placeholder on error
                    if matches!(self.messages.last(), Some((Role::Assistant, t)) if t.is_empty()) {
                        self.messages.pop();
                    }
                }
            },
            Message::NewSession => {
                self.session = new_session_name();
                self.messages.clear();
                self.input.clear();
                self.error = None;
                self.streaming = false;
                self.pending = None;
            }
        }
        app::Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let s = theme::active().cosmic().spacing;

        let header = cosmic::iced::widget::row![
            text::title3("Fabric chat"),
            text::caption(self.session.clone()),
            cosmic::widget::Space::new().width(Length::Fill),
            button::text("New chat").on_press(Message::NewSession),
        ]
        .spacing(s.space_s)
        .align_y(Alignment::Center);

        let mut convo = Column::new().spacing(s.space_s);
        for (role, body) in &self.messages {
            let (who, class) = match role {
                Role::User => ("You", theme::Container::Card),
                Role::Assistant => ("Fabric", theme::Container::Primary),
            };
            let shown = if body.is_empty() && self.streaming {
                "…".to_string()
            } else {
                body.clone()
            };
            let block = Column::new()
                .spacing(s.space_xxs)
                .push(text::caption(who))
                .push(text::body(shown));
            convo = convo.push(container(block).padding(s.space_xs).class(class).width(Length::Fill));
        }
        if self.messages.is_empty() {
            convo = convo.push(text::caption(
                "Start a conversation — fabric keeps the history for this session.",
            ));
        }

        let input_row = cosmic::iced::widget::row![
            text_input("Message…", &self.input)
                .on_input(Message::InputChanged)
                .on_submit(|_| Message::Send)
                .width(Length::Fill),
            if self.streaming {
                button::suggested("…")
            } else {
                button::suggested("Send").on_press(Message::Send)
            },
        ]
        .spacing(s.space_xs)
        .align_y(Alignment::Center);

        let mut col = Column::new()
            .spacing(s.space_s)
            .padding(s.space_m)
            .push(header)
            .push(divider::horizontal::default())
            .push(scrollable(convo).height(Length::Fill).width(Length::Fill));
        if let Some(e) = &self.error {
            col = col.push(text::caption(e.clone()));
        }
        col = col.push(input_row);

        container(col).width(Length::Fill).height(Length::Fill).into()
    }
}

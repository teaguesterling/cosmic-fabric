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
    /// A panel-confirm tool is waiting on the user (id, command preview).
    /// While `Some`, the chat surface renders an inline Approve/Deny card
    /// over the input area. The daemon-side run-loop thread is blocked on
    /// the matching tool-confirm event with a 60s timeout.
    pending_confirm: Option<(String, String)>,
}

#[derive(Debug, Clone)]
pub enum Message {
    InputChanged(String),
    Send,
    ChatEvent(daemon::RunEvent),
    NewSession,
    /// User clicked Approve/Deny on a pending confirm card.
    ConfirmTool { id: String, approved: bool },
    /// The tool_confirm op completed (ack from daemon).
    ConfirmAcked(Result<(), String>),
}

/// One-line, ≤80-char representation of a tool's args for inline display.
/// `{"url": "https://wikipedia/Tokyo"}` → `url="https://wikipedia/Tokyo"`.
fn compact_args(args: &serde_json::Value) -> String {
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
        // Escalate-from-a-one-off: trailing args (`… session <text>`) seed the
        // input, so "↪ Chat" on a result drops you into a chat carrying it.
        let seed = std::env::args().skip(2).collect::<Vec<_>>().join(" ");
        let me = Self {
            core,
            session: new_session_name(),
            messages: Vec::new(),
            input: seed,
            pending: None,
            run_seq: 0,
            streaming: false,
            error: None,
            pending_confirm: None,
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
                daemon::RunEvent::ToolCall { name, args, .. } => {
                    // Inline annotation on the current assistant turn, so the
                    // user sees what's happening as the model decides it.
                    // Phase 1 styling: plain caption lines; pill rendering is
                    // a follow-on polish slice.
                    if let Some(last) = self.messages.last_mut() {
                        let args_summary = compact_args(&args);
                        if !last.1.is_empty() && !last.1.ends_with('\n') {
                            last.1.push('\n');
                        }
                        last.1.push_str(&format!("\u{1F50E} {name}({args_summary})\n"));
                    }
                }
                daemon::RunEvent::ToolResult { name, summary, .. } => {
                    if let Some(last) = self.messages.last_mut() {
                        last.1.push_str(&format!("  \u{2713} {name}: {summary}\n"));
                    }
                }
                daemon::RunEvent::ToolConfirmRequired { id, name, command_preview, .. } => {
                    // Surface the request inline AND set pending_confirm so the
                    // view renders an Approve/Deny card. The daemon's run-loop
                    // is blocked waiting for our `tool_confirm` ack.
                    if let Some(last) = self.messages.last_mut() {
                        last.1.push_str(&format!(
                            "\u{26A0} {name} wants to run: {command_preview}\n"));
                    }
                    self.pending_confirm = Some((id, command_preview));
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
                self.pending_confirm = None;
            }
            Message::ConfirmTool { id, approved } => {
                self.pending_confirm = None;
                // Append a brief audit line to the current assistant turn so
                // there's a visible record of what the user decided.
                if let Some(last) = self.messages.last_mut() {
                    let v = if approved { "approved" } else { "denied" };
                    last.1.push_str(&format!("  \u{2192} {v}\n"));
                }
                return cosmic::Task::perform(
                    daemon::send_tool_confirm(id, approved),
                    |r| cosmic::Action::App(Message::ConfirmAcked(r)),
                );
            }
            Message::ConfirmAcked(Ok(())) => {}  // ack — daemon got it, loop continues
            Message::ConfirmAcked(Err(e)) => {
                self.error = Some(format!("confirm failed: {e}"));
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
            let hint = if self.input.trim().is_empty() {
                "Start a conversation — fabric keeps the history for this session."
            } else {
                "Carried over from a result — edit / add your question below, then send."
            };
            convo = convo.push(text::caption(hint));
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
        // Inline confirm card above the input area when a panel-confirm tool
        // is pending. Daemon's run-loop is blocked on the matching event —
        // user must click before the 60s timeout fires.
        if let Some((id, preview)) = &self.pending_confirm {
            let approve_id = id.clone();
            let deny_id = id.clone();
            let preview_txt = preview.clone();
            let card = Column::new()
                .spacing(s.space_xxs)
                .push(text::heading("\u{26A0} Tool wants to run"))
                .push(text::body(preview_txt))
                .push(
                    cosmic::iced::widget::row![
                        button::standard("Deny").on_press(
                            Message::ConfirmTool { id: deny_id, approved: false }),
                        cosmic::widget::Space::new().width(Length::Fixed(8.0)),
                        button::suggested("Approve").on_press(
                            Message::ConfirmTool { id: approve_id, approved: true }),
                    ]
                    .align_y(Alignment::Center),
                );
            col = col.push(container(card).padding(s.space_xs).class(theme::Container::Card));
        }
        col = col.push(input_row);

        container(col).width(Length::Fill).height(Length::Fill).into()
    }
}

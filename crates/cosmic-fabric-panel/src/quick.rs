//! The quick-action (`cosmic-fabric-panel quick`) — bind a global shortcut to it
//! (COSMIC Settings → Keyboard → Custom Shortcuts → Spawn `cosmic-fabric-panel
//! quick`). It grabs the current **selection**, shows a no-typing grid of your
//! active patterns, runs the chosen one, and shows the result inline (and copies
//! it). The kit's fast "select → inference → review → close" loop.

use cosmic::iced::futures::StreamExt;
use cosmic::{
    app,
    iced::{widget::Column, Alignment, Length},
    theme,
    widget::{button, container, divider, scrollable, text},
    Element,
};

use crate::daemon::{self, RunResult};
use crate::workspace::pretty;

pub const QUICK_APP_ID: &str = "com.github.teaguesterling.CosmicFabric.Quick";

pub fn run() -> cosmic::iced::Result {
    let settings = cosmic::app::Settings::default().size(cosmic::iced::Size::new(440.0, 540.0));
    cosmic::app::run::<QuickApp>(settings, ())
}

pub struct QuickApp {
    core: cosmic::app::Core,
    selection: String,
    patterns: Vec<String>, // active set
    result: Option<String>,
    result_meta: Option<String>,
    ran_pattern: Option<String>,
    running: bool,
    pending: Option<(u64, String, String)>,
    run_seq: u64,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    PatternsDone(Result<Vec<String>, String>),
    RunPattern(String),
    RunEvent(daemon::RunEvent),
    Copy,
    Chat,
    Another,
    Close,
}

impl cosmic::Application for QuickApp {
    type Message = Message;
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    const APP_ID: &'static str = QUICK_APP_ID;

    fn init(core: app::Core, _flags: Self::Flags) -> (Self, app::Task<Self::Message>) {
        let me = Self {
            core,
            selection: daemon::selection(),
            patterns: Vec::new(),
            result: None,
            result_meta: None,
            ran_pattern: None,
            running: false,
            pending: None,
            run_seq: 0,
            error: None,
        };
        (me, patterns_task())
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
                    daemon::run_stream(pat.clone(), input.clone(), None).map(Message::RunEvent)
                },
            ),
            None => cosmic::iced::Subscription::none(),
        }
    }

    fn update(&mut self, message: Message) -> app::Task<Message> {
        match message {
            Message::PatternsDone(Ok(p)) => {
                self.patterns = crate::policy::load().active_patterns(&p);
            }
            Message::PatternsDone(Err(e)) => self.error = Some(e),
            Message::RunPattern(pattern) => {
                if self.selection.trim().is_empty() {
                    self.error = Some("Nothing selected — highlight some text first.".into());
                    return app::Task::none();
                }
                self.run_seq += 1;
                self.pending = Some((self.run_seq, pattern.clone(), self.selection.clone()));
                self.ran_pattern = Some(pattern);
                self.result = Some(String::new());
                self.result_meta = None;
                self.running = true;
                self.error = None;
            }
            Message::RunEvent(ev) => match ev {
                daemon::RunEvent::Chunk(c) => {
                    if let Some(r) = self.result.as_mut() {
                        r.push_str(&c);
                    }
                }
                // Tool events: quick-action surface doesn't route to
                // tool-using patterns yet (single-shot is the kit's whole
                // point). Render the tool trace inline as plain text so the
                // user sees what happened even if a pattern they routed here
                // happens to be tool-enabled.
                daemon::RunEvent::ToolCall { name, .. }
                | daemon::RunEvent::ToolResult { name, .. }
                | daemon::RunEvent::ToolConfirmRequired { name, .. } => {
                    let r = self.result.get_or_insert_with(String::new);
                    if !r.is_empty() && !r.ends_with('\n') { r.push('\n'); }
                    r.push_str(&format!("[tool: {name}]\n"));
                }
                daemon::RunEvent::Done(rr) => {
                    self.running = false;
                    self.pending = None;
                    if self.result.as_deref().unwrap_or("").is_empty() {
                        self.result = rr.output.clone();
                    }
                    self.result_meta = Some(meta_line(&rr));
                    if let Some(r) = &self.result {
                        daemon::set_clipboard(r); // deliver: copied regardless of how they close
                    }
                }
                daemon::RunEvent::Error(e) => {
                    self.running = false;
                    self.pending = None;
                    self.error = Some(e);
                }
            },
            Message::Copy => {
                if let Some(r) = &self.result {
                    daemon::set_clipboard(r);
                }
            }
            Message::Chat => {
                // Escalate the result into a chat, then close this quick popup.
                if let (Ok(exe), Some(r)) = (std::env::current_exe(), &self.result) {
                    if !r.trim().is_empty() {
                        let _ = std::process::Command::new(exe).arg("session").arg(r).spawn();
                        std::process::exit(0);
                    }
                }
            }
            Message::Another => {
                self.result = None;
                self.result_meta = None;
                self.ran_pattern = None;
                self.error = None;
            }
            Message::Close => std::process::exit(0),
        }
        app::Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let s = theme::active().cosmic().spacing;
        let mut col = Column::new().spacing(s.space_s).padding(s.space_m);

        col = col.push(text::title3("Run on selection"));
        let sel = self.selection.trim();
        let preview = if sel.is_empty() {
            "— nothing selected —".to_string()
        } else {
            let p: String = sel.chars().take(160).collect();
            format!("{p}{}", if sel.chars().count() > 160 { "…" } else { "" })
        };
        col = col.push(text::caption(preview));
        col = col.push(divider::horizontal::default());

        if self.result.is_some() || self.running {
            // result / review
            let head = self.ran_pattern.clone().map(|p| pretty(&p)).unwrap_or_default();
            let meta = if self.running {
                "running…".to_string()
            } else {
                self.result_meta.clone().unwrap_or_default()
            };
            col = col.push(
                cosmic::iced::widget::row![
                    text::heading(head),
                    cosmic::widget::Space::new().width(Length::Fill),
                    text::caption(meta),
                ]
                .align_y(Alignment::Center),
            );
            let body = self.result.clone().unwrap_or_default();
            col = col.push(scrollable(text::body(body)).height(Length::Fill).width(Length::Fill));
            col = col.push(
                cosmic::iced::widget::row![
                    button::standard("Copy").on_press(Message::Copy),
                    button::text("\u{21aa} Chat").on_press(Message::Chat),
                    button::text("\u{21ba} Another").on_press(Message::Another),
                    cosmic::widget::Space::new().width(Length::Fill),
                    button::suggested("Close").on_press(Message::Close),
                ]
                .spacing(s.space_xs)
                .align_y(Alignment::Center),
            );
        } else {
            // pattern grid
            let mut list = Column::new().spacing(2);
            for name in &self.patterns {
                list = list.push(
                    button::text(pretty(name))
                        .width(Length::Fill)
                        .on_press(Message::RunPattern(name.clone())),
                );
            }
            if self.patterns.is_empty() {
                list = list.push(text::caption("No active patterns — curate them in the Workbench."));
            }
            col = col.push(scrollable(list).height(Length::Fill).width(Length::Fill));
            col = col.push(
                cosmic::iced::widget::row![
                    cosmic::widget::Space::new().width(Length::Fill),
                    button::text("Close").on_press(Message::Close),
                ]
                .align_y(Alignment::Center),
            );
        }

        if let Some(e) = &self.error {
            col = col.push(text::caption(e.clone()));
        }

        container(col).width(Length::Fill).height(Length::Fill).into()
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

fn patterns_task() -> app::Task<Message> {
    cosmic::Task::perform(daemon::patterns(), |r| {
        cosmic::Action::App(Message::PatternsDone(r))
    })
}

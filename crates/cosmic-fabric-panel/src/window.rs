//! The libcosmic panel applet: an icon → popup with deployment status and a
//! quick-run pane (pick a `scribe-*` pattern → run it on the clipboard → see the
//! result). All work goes through `cosmic-fabricd` over the socket.

use cosmic::{
    app,
    applet::padded_control,
    iced::{
        platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup},
        widget::Column,
        window, Length,
    },
    widget::{button, container, divider, scrollable, text},
    Element,
};

use crate::daemon::{self, RunResult, Status};

pub struct Window {
    core: cosmic::app::Core,
    popup: Option<window::Id>,
    status: Option<Status>,
    patterns: Vec<String>,
    result: Option<String>,
    result_meta: Option<String>,
    running: bool,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    CloseRequested(window::Id),
    Refresh,
    StatusDone(Result<Status, String>),
    PatternsDone(Result<Vec<String>, String>),
    RunPattern(String),
    RunDone(Result<RunResult, String>),
    CopyResult,
}

impl cosmic::Application for Window {
    type Message = Message;
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    const APP_ID: &'static str = crate::APP_ID;

    fn init(core: app::Core, _flags: Self::Flags) -> (Self, app::Task<Self::Message>) {
        (
            Self {
                core,
                popup: None,
                status: None,
                patterns: Vec::new(),
                result: None,
                result_meta: None,
                running: false,
                error: None,
            },
            app::Task::none(),
        )
    }

    fn core(&self) -> &cosmic::app::Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut cosmic::app::Core {
        &mut self.core
    }
    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }

    fn update(&mut self, message: Message) -> app::Task<Message> {
        match message {
            Message::TogglePopup => {
                if let Some(p) = self.popup.take() {
                    return destroy_popup(p);
                }
                let new_id = window::Id::unique();
                self.popup = Some(new_id);
                let popup_settings = self.core.applet.get_popup_settings(
                    self.core.main_window_id().unwrap(),
                    new_id,
                    None,
                    None,
                    None,
                );
                cosmic::Task::batch([get_popup(popup_settings), status_task(), patterns_task()])
            }
            Message::CloseRequested(id) => {
                if self.popup == Some(id) {
                    self.popup = None;
                }
                app::Task::none()
            }
            Message::Refresh => cosmic::Task::batch([status_task(), patterns_task()]),
            Message::StatusDone(Ok(s)) => {
                self.status = Some(s);
                app::Task::none()
            }
            Message::StatusDone(Err(e)) => {
                self.error = Some(e);
                app::Task::none()
            }
            Message::PatternsDone(Ok(p)) => {
                self.patterns = p;
                app::Task::none()
            }
            Message::PatternsDone(Err(e)) => {
                self.error = Some(e);
                app::Task::none()
            }
            Message::RunPattern(pattern) => {
                let input = daemon::clipboard();
                if input.trim().is_empty() {
                    self.error = Some("Clipboard is empty — copy some text first.".into());
                    return app::Task::none();
                }
                self.running = true;
                self.result = None;
                self.result_meta = None;
                self.error = None;
                cosmic::Task::perform(daemon::run(pattern, input), |r| {
                    cosmic::Action::App(Message::RunDone(r))
                })
            }
            Message::RunDone(res) => {
                self.running = false;
                match res {
                    Ok(r) if r.error.is_some() => self.error = r.error,
                    Ok(r) => {
                        self.result = r.output;
                        let model = r.model.unwrap_or_default();
                        let place = r
                            .placement
                            .map(|p| format!(" \u{00b7} {p:.0}% GPU"))
                            .unwrap_or_default();
                        self.result_meta = Some(format!("{model}{place}"));
                    }
                    Err(e) => self.error = Some(e),
                }
                app::Task::none()
            }
            Message::CopyResult => {
                if let Some(r) = &self.result {
                    daemon::set_clipboard(r);
                }
                app::Task::none()
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        self.core
            .applet
            .icon_button("system-run-symbolic")
            .on_press(Message::TogglePopup)
            .into()
    }

    fn view_window(&self, _id: window::Id) -> Element<'_, Message> {
        let mut col = Column::new().spacing(6).padding([8, 0]);

        // ---- status ----
        if let Some(s) = &self.status {
            let serve = if s.serve { "\u{25cf} up" } else { "\u{25cb} down" };
            let model = match (&s.default_model, &s.default_vendor) {
                (Some(m), Some(v)) => format!("{m} ({v})"),
                _ => "—".into(),
            };
            let gpu = s
                .loaded
                .first()
                .and_then(|l| l.gpu_pct)
                .map(|p| format!(" \u{00b7} {p:.0}% GPU"))
                .unwrap_or_default();
            col = col.push(padded_control(text::caption(format!(
                "serve {serve}  \u{00b7}  {model}{gpu}"
            ))));
        }

        // ---- quick run ----
        col = col.push(padded_control(text::heading("Run on clipboard")));
        let mut list = Column::new().spacing(2);
        for name in self.patterns.iter().filter(|n| n.starts_with("scribe-")) {
            list = list.push(
                button::text(name.clone())
                    .width(Length::Fill)
                    .on_press(Message::RunPattern(name.clone())),
            );
        }
        col = col.push(padded_control(
            scrollable(list).height(Length::Fixed(160.0)),
        ));

        // ---- result ----
        if self.running {
            col = col.push(padded_control(text::body("running\u{2026}")));
        }
        if let Some(out) = &self.result {
            col = col.push(padded_control(divider::horizontal::default()));
            if let Some(meta) = &self.result_meta {
                col = col.push(padded_control(text::caption(meta.clone())));
            }
            col = col.push(padded_control(
                scrollable(text::body(out.clone())).height(Length::Fixed(180.0)),
            ));
            col = col.push(padded_control(
                button::text("Copy").on_press(Message::CopyResult),
            ));
        }
        if let Some(err) = &self.error {
            col = col.push(padded_control(text::caption(err.clone())));
        }

        col = col.push(padded_control(divider::horizontal::default()));
        col = col.push(padded_control(
            button::text("Refresh").on_press(Message::Refresh),
        ));

        self.core
            .applet
            .popup_container(container(col).width(Length::Fixed(340.0)))
            .into()
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        Some(Message::CloseRequested(id))
    }
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

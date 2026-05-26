//! The libcosmic panel applet: an icon that opens a popup showing the fabric
//! deployment status from `cosmic-fabricd`. (Status-only first slice; quick-run
//! + a result pane come next.)

use cosmic::{
    app,
    applet::padded_control,
    iced::{
        platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup},
        widget::Column,
        window, Length,
    },
    widget::{button, container, divider, text},
    Element,
};

use crate::daemon::{self, Status};

pub struct Window {
    core: cosmic::app::Core,
    popup: Option<window::Id>,
    status: Option<Status>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    CloseRequested(window::Id),
    Refresh,
    StatusDone(Result<Status, String>),
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
                cosmic::Task::batch([get_popup(popup_settings), status_task()])
            }
            Message::CloseRequested(id) => {
                if self.popup == Some(id) {
                    self.popup = None;
                }
                app::Task::none()
            }
            Message::Refresh => status_task(),
            Message::StatusDone(Ok(s)) => {
                self.status = Some(s);
                self.error = None;
                app::Task::none()
            }
            Message::StatusDone(Err(e)) => {
                self.error = Some(e);
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
        col = col.push(padded_control(text::heading("Fabric")));

        match &self.status {
            Some(s) => {
                col = col.push(padded_control(text::body(format!(
                    "serve: {}",
                    if s.serve { "\u{25cf} up" } else { "\u{25cb} down" }
                ))));
                if let (Some(m), Some(v)) = (&s.default_model, &s.default_vendor) {
                    col = col.push(padded_control(text::body(format!("default: {m} ({v})"))));
                }
                if s.loaded.is_empty() {
                    col = col.push(padded_control(text::caption("no model loaded")));
                } else {
                    for l in &s.loaded {
                        let m = l.model.clone().unwrap_or_default();
                        let pct = l
                            .gpu_pct
                            .map(|p| format!("{p:.0}% GPU"))
                            .unwrap_or_default();
                        let ctx = l.ctx.map(|c| format!(", ctx {c}")).unwrap_or_default();
                        col = col.push(padded_control(text::body(format!("{m}: {pct}{ctx}"))));
                    }
                }
                if let Some(vr) = &s.vram {
                    col = col.push(padded_control(text::caption(format!(
                        "VRAM: {} / {} MiB free",
                        vr.free, vr.total
                    ))));
                }
            }
            None => {
                col = col.push(padded_control(text::body(
                    self.error
                        .clone()
                        .unwrap_or_else(|| "loading\u{2026}".into()),
                )));
            }
        }

        col = col.push(padded_control(divider::horizontal::default()));
        col = col.push(padded_control(
            button::text("Refresh").on_press(Message::Refresh),
        ));

        self.core
            .applet
            .popup_container(container(col).width(Length::Fixed(300.0)))
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

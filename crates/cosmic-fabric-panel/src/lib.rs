pub mod daemon;
pub mod policy;
pub mod settings;
pub mod window;
pub mod workspace;

pub const APP_ID: &str = "com.github.teaguesterling.CosmicFabric";

pub fn run_applet() -> cosmic::iced::Result {
    cosmic::applet::run::<window::Window>(())
}

pub fn run_settings() -> cosmic::iced::Result {
    settings::run()
}

pub fn run_workspace() -> cosmic::iced::Result {
    workspace::run()
}

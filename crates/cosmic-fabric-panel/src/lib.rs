pub mod daemon;
pub mod window;

pub const APP_ID: &str = "com.github.teaguesterling.CosmicFabric";

pub fn run_applet() -> cosmic::iced::Result {
    cosmic::applet::run::<window::Window>(())
}

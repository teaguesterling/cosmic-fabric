use tracing_subscriber::EnvFilter;

fn main() -> cosmic::iced::Result {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "cosmic_fabric_panel=info,warn".into());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
    cosmic_fabric_panel::run_applet()
}

use tracing_subscriber::EnvFilter;

fn main() -> cosmic::iced::Result {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "cosmic_fabric_panel=info,warn".into());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    match std::env::args().nth(1).as_deref() {
        Some("settings") => cosmic_fabric_panel::run_settings(),
        Some("window") => cosmic_fabric_panel::run_workspace(),
        Some(other) => {
            eprintln!("unknown subcommand: {other}\nusage: cosmic-fabric-panel [settings|window]");
            std::process::exit(2);
        }
        None => cosmic_fabric_panel::run_applet(),
    }
}

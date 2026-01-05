use anyhow::Result;
use clap::Parser;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::*;

mod app;
mod controller;
mod events;
mod grpc;
mod proto;
mod ui;

use controller::Controller;
use events::spawn_event_reader;

#[derive(Parser, Debug)]
#[command(name = "lazysluice")]
#[command(about = "Terminal UI client for Sluice", long_about = None)]
struct Args {
    /// gRPC endpoint URL, e.g. http://localhost:50051
    #[arg(
        long,
        env = "SLUICE_ENDPOINT",
        default_value = "http://localhost:50051"
    )]
    endpoint: String,

    /// Optional CA certificate PEM path for TLS.
    #[arg(long, env = "SLUICE_TLS_CA")]
    tls_ca: Option<std::path::PathBuf>,

    /// Optional TLS domain name (SNI).
    #[arg(long, env = "SLUICE_TLS_DOMAIN")]
    tls_domain: Option<String>,

    /// Subscription credits window size.
    #[arg(long, env = "SLUICE_CREDITS_WINDOW", default_value_t = 128)]
    credits_window: u32,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .try_init();

    let args = Args::parse();

    run_tui(args).await
}

async fn run_tui(args: Args) -> Result<()> {
    // Install panic hook to restore terminal on panic
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
        original_hook(panic_info);
    }));

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create controller
    let mut ctrl = Controller::new(
        args.endpoint,
        args.tls_ca,
        args.tls_domain,
        args.credits_window,
    );

    // Initial connect
    ctrl.connect().await;

    // Spawn event reader (tick every 50ms for responsive message polling)
    let mut events = spawn_event_reader(std::time::Duration::from_millis(50));

    // Main loop
    let result = loop {
        // Poll subscription for new messages
        ctrl.poll_subscription().await;

        // Draw UI
        terminal.draw(|f| ui::draw(f, &ctrl.state))?;

        // Wait for next event
        if let Some(event) = events.recv().await {
            if !ctrl.handle(event).await {
                break Ok(());
            }
        } else {
            // Channel closed
            break Ok(());
        }
    };

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_defaults_to_localhost() {
        let args = Args::try_parse_from(["lazysluice"]).expect("parse should succeed");
        assert_eq!(args.endpoint, "http://localhost:50051");
        assert_eq!(args.credits_window, 128);
        assert!(args.tls_ca.is_none());
        assert!(args.tls_domain.is_none());
    }

    #[test]
    fn endpoint_flag_overrides_default() {
        let args = Args::try_parse_from([
            "lazysluice",
            "--endpoint",
            "http://127.0.0.1:12345",
            "--credits-window",
            "7",
        ])
        .expect("parse should succeed");
        assert_eq!(args.endpoint, "http://127.0.0.1:12345");
        assert_eq!(args.credits_window, 7);
    }
}

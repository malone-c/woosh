//! Woosh — terminal white noise generator.
//!
//! - `woosh`        — launch TUI (spawns daemon if needed)
//! - `woosh daemon` — start audio daemon in the background
//! - `woosh stop`   — send QUIT to the running daemon
//! - `woosh status` — print daemon state and exit

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::io::{Read as _, Write as _};
use tracing_subscriber::EnvFilter;
use woosh::daemon;

/// A terminal white noise generator.
#[derive(Debug, Parser)]
#[command(name = "woosh", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

/// Available subcommands.
#[derive(Debug, Subcommand)]
enum Commands {
    /// Start the audio daemon.
    Daemon {
        /// Run in the foreground (do not daemonize).
        #[arg(long, hide = true)]
        no_daemonize: bool,
    },
    /// Stop the running daemon.
    Stop,
    /// Print the current daemon state to stdout.
    Status,
}

fn main() -> Result<()> {
    // Initialize logging with RUST_LOG env var (default: info)
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info"))
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Daemon { no_daemonize }) => run_daemon(no_daemonize),
        Some(Commands::Stop) => run_stop(),
        Some(Commands::Status) => run_status(),
        None => run_tui(),
    }
}

/// Start audio daemon (daemonizes unless `--no-daemonize`).
///
/// # Errors
/// Returns an error if the daemon fails to start.
fn run_daemon(no_daemonize: bool) -> Result<()> {
    daemon::start(no_daemonize)
}

/// Stop the running daemon by sending QUIT over the Unix socket.
///
/// # Errors
/// Returns an error if the socket cannot be reached.
fn run_stop() -> Result<()> {
    let socket_path = daemon::lifecycle::socket_path()?;
    let mut stream = std::os::unix::net::UnixStream::connect(&socket_path)
        .map_err(|_| anyhow::anyhow!("daemon is not running (cannot connect to socket)"))?;
    stream.write_all(b"QUIT\n")?;
    Ok(())
}

/// Query daemon status and print to stdout.
///
/// # Errors
/// Returns an error if the socket cannot be reached or no response is received.
fn run_status() -> Result<()> {
    let socket_path = daemon::lifecycle::socket_path()?;
    let mut stream = std::os::unix::net::UnixStream::connect(&socket_path)
        .map_err(|_| anyhow::anyhow!("daemon is not running (cannot connect to socket)"))?;
    stream.write_all(b"STATUS\n")?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf)?;
    print!("{buf}");
    Ok(())
}

/// Launch the TUI, auto-spawning the daemon if needed.
///
/// # Errors
/// Returns an error if the TUI cannot be initialised.
fn run_tui() -> Result<()> {
    if !daemon::lifecycle::daemon_is_alive()? {
        let exe = std::env::current_exe()?;
        std::process::Command::new(exe)
            .arg("daemon")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;

        // Poll for the socket to appear (up to 500 ms).
        let socket_path = daemon::lifecycle::socket_path()?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        loop {
            if socket_path.exists() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                anyhow::bail!("daemon did not start in time");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    let socket_path = daemon::lifecycle::socket_path()?;
    woosh::tui::run(socket_path)
}

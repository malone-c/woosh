//! Woosh — terminal white noise generator.
//!
//! - `woosh`                   — launch TUI (spawns daemon if needed)
//! - `woosh pink|white|brown`  — play noise preset and exit
//! - `woosh place <name>`      — play place sound by name and exit
//! - `woosh daemon`            — start audio daemon in the background
//! - `woosh stop`              — send QUIT to the running daemon
//! - `woosh status`            — print daemon state and exit

use anyhow::{anyhow, bail, Result};
use clap::{Parser, Subcommand};
use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::thread::sleep;
use std::time::{Duration, Instant};
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
    /// Play pink noise and exit.
    Pink,
    /// Play white noise and exit.
    White,
    /// Play brown noise and exit.
    Brown,
    /// Play a place sound by name and exit (e.g. `woosh place tokyo` or `woosh place coffee shop`).
    Place {
        /// Place name — multi-word names work without quotes.
        #[arg(trailing_var_arg = true, required = true)]
        name: Vec<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging with RUST_LOG env var (default: info)
    // Skip for daemon subcommand as it initializes its own file-based logging
    if !matches!(cli.command, Some(Commands::Daemon { .. })) {
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| EnvFilter::new("info"))
            )
            .init();
    }

    match cli.command {
        Some(Commands::Daemon { no_daemonize }) => run_daemon(no_daemonize),
        Some(Commands::Stop) => run_stop(),
        Some(Commands::Status) => run_status(),
        Some(Commands::Pink) => run_play_preset("pink"),
        Some(Commands::White) => run_play_preset("white"),
        Some(Commands::Brown) => run_play_preset("brown"),
        Some(Commands::Place { name }) => run_play_place(&name.join(" ")),
        None => run_tui(),
    }
}

/// Start audio daemon (daemonizes unless `--no-daemonize`).
fn run_daemon(no_daemonize: bool) -> Result<()> {
    daemon::start(no_daemonize)
}

/// Stop the running daemon by sending QUIT over the Unix socket.
fn run_stop() -> Result<()> {
    let socket_path = daemon::lifecycle::socket_path()?;
    let mut stream = UnixStream::connect(&socket_path)
        .map_err(|_| anyhow!("daemon is not running (cannot connect to socket)"))?;
    stream.write_all(b"QUIT\n")?;
    Ok(())
}

/// Query daemon status and print to stdout.
fn run_status() -> Result<()> {
    let socket_path = daemon::lifecycle::socket_path()?;
    let mut stream = UnixStream::connect(&socket_path)
        .map_err(|_| anyhow!("daemon is not running (cannot connect to socket)"))?;
    stream.write_all(b"STATUS\n")?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf)?;
    print!("{buf}");
    Ok(())
}

/// Ensure the daemon is running, spawning it if not.
fn ensure_daemon_running() -> Result<()> {
    if !daemon::lifecycle::daemon_is_alive()? {
        let exe = std::env::current_exe()?;
        std::process::Command::new(exe)
            .arg("daemon")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let socket_path = daemon::lifecycle::socket_path()?;
        let ready_path = daemon::lifecycle::ready_path()?;
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            if socket_path.exists() && ready_path.exists() {
                break;
            }
            if Instant::now() >= deadline {
                bail!("daemon did not start in time");
            }
            sleep(Duration::from_millis(10));
        }
    }
    Ok(())
}

/// Send `PLAY <preset>` to the daemon, spawning it first if needed.
fn run_play_preset(preset: &str) -> Result<()> {
    ensure_daemon_running()?;
    let socket_path = daemon::lifecycle::socket_path()?;
    let mut stream = UnixStream::connect(&socket_path)
        .map_err(|_| anyhow!("daemon is not running"))?;
    stream.write_all(format!("PLAY {preset}\n").as_bytes())?;
    Ok(())
}

/// Send `PLAY_PLACE <location>` to the daemon, spawning it first if needed.
fn run_play_place(location: &str) -> Result<()> {
    ensure_daemon_running()?;
    let socket_path = daemon::lifecycle::socket_path()?;
    let mut stream = UnixStream::connect(&socket_path)
        .map_err(|_| anyhow!("daemon is not running"))?;
    stream.write_all(format!("PLAY_PLACE {location}\n").as_bytes())?;
    Ok(())
}

/// Launch the TUI, auto-spawning the daemon if needed.
fn run_tui() -> Result<()> {
    ensure_daemon_running()?;
    let socket_path = daemon::lifecycle::socket_path()?;
    woosh::tui::run(socket_path)
}

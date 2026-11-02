//! Woosh — terminal white noise generator.
//!
//! - `woosh`                   — launch TUI (spawns daemon if needed)
//! - `woosh pink|white|brown`  — play noise preset and exit
//! - `woosh <place>`           — play a place sound (e.g. `woosh tokyo`) and exit
//! - `woosh place <name>`      — same, for names that match a subcommand
//! - `woosh daemon`            — start audio daemon in the background
//! - `woosh stop`              — send QUIT to the running daemon
//! - `woosh status`            — print daemon state and exit

use anyhow::{anyhow, bail, Result};
use clap::{Parser, Subcommand};
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::thread::sleep;
use std::time::{Duration, Instant};
use tracing_subscriber::EnvFilter;
use woosh::daemon;
use woosh::daemon::state::NoisePreset;

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
    /// Play a place sound by name and exit (e.g. `woosh place coffee shop`).
    Place {
        /// Place name — multi-word names work without quotes.
        #[arg(trailing_var_arg = true, required = true)]
        name: Vec<String>,
    },
    /// Any other words name a place sound to play, e.g. `woosh tokyo`.
    #[command(external_subcommand)]
    PlaceShortcut(Vec<String>),
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // The daemon logs to its own file; everything else logs to stderr (RUST_LOG, default info).
    if !matches!(cli.command, Some(Commands::Daemon { .. })) {
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
            )
            .with_writer(std::io::stderr)
            .init();
    }

    match cli.command {
        Some(Commands::Daemon { no_daemonize }) => daemon::start(no_daemonize),
        Some(Commands::Stop) => run_stop(),
        Some(Commands::Status) => {
            print!("{}", request("STATUS")?);
            Ok(())
        }
        Some(Commands::Pink) => play(&format!("PLAY {}", NoisePreset::Pink)),
        Some(Commands::White) => play(&format!("PLAY {}", NoisePreset::White)),
        Some(Commands::Brown) => play(&format!("PLAY {}", NoisePreset::Brown)),
        Some(Commands::Place { name } | Commands::PlaceShortcut(name)) => {
            play(&format!("PLAY_PLACE {}", name.join(" ")))
        }
        None => run_tui(),
    }
}

/// Connects to the running daemon's socket.
fn connect() -> Result<UnixStream> {
    let socket_path = daemon::lifecycle::socket_path()?;
    UnixStream::connect(&socket_path)
        .map_err(|_| anyhow!("daemon is not running (cannot connect to socket)"))
}

/// Stop the running daemon by sending QUIT, which has no reply.
fn run_stop() -> Result<()> {
    connect()?.write_all(b"QUIT\n")?;
    Ok(())
}

/// Sends one command to the daemon and returns its one-line reply.
fn request(command: &str) -> Result<String> {
    let mut stream = connect()?;
    stream.write_all(format!("{command}\n").as_bytes())?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    Ok(reply)
}

/// Spawns the daemon if needed, sends a play command, and fails if the daemon rejects it.
fn play(command: &str) -> Result<()> {
    ensure_daemon_running()?;
    let reply = request(command)?;
    if let Some(message) = reply.strip_prefix("ERROR ") {
        bail!("{}", message.trim());
    }
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

/// Launch the TUI, auto-spawning the daemon if needed.
fn run_tui() -> Result<()> {
    ensure_daemon_running()?;
    let socket_path = daemon::lifecycle::socket_path()?;
    woosh::tui::run(socket_path)
}

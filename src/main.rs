//! Woosh — terminal white noise generator.
//!
//! - `woosh`        — launch TUI (spawns daemon if needed)
//! - `woosh daemon` — start audio daemon in the foreground
//! - `woosh stop`   — send QUIT to the running daemon
//! - `woosh status` — print daemon state and exit

use anyhow::Result;
use clap::{Parser, Subcommand};

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
    /// Start the audio daemon in the foreground.
    Daemon,
    /// Stop the running daemon.
    Stop,
    /// Print the current daemon state to stdout.
    Status,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Daemon) => run_daemon(),
        Some(Commands::Stop) => run_stop(),
        Some(Commands::Status) => run_status(),
        None => run_tui(),
    }
}

/// Placeholder: start audio daemon.
///
/// # Errors
/// Returns an error if the daemon fails to start.
#[allow(clippy::unnecessary_wraps)]
fn run_daemon() -> Result<()> {
    println!("daemon: not yet implemented");
    Ok(())
}

/// Placeholder: stop running daemon.
///
/// # Errors
/// Returns an error if the stop command cannot be delivered.
#[allow(clippy::unnecessary_wraps)]
fn run_stop() -> Result<()> {
    println!("stop: not yet implemented");
    Ok(())
}

/// Placeholder: print daemon status.
///
/// # Errors
/// Returns an error if the daemon cannot be queried.
#[allow(clippy::unnecessary_wraps)]
fn run_status() -> Result<()> {
    println!("status: not yet implemented");
    Ok(())
}

/// Placeholder: launch TUI.
///
/// # Errors
/// Returns an error if the TUI cannot be initialised.
#[allow(clippy::unnecessary_wraps)]
fn run_tui() -> Result<()> {
    println!("tui: not yet implemented");
    Ok(())
}

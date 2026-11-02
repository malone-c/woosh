use anyhow::{Context, Result};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use tracing_subscriber::EnvFilter;

/// Returns the woosh data directory: `~/Library/Application Support/woosh/` on
/// macOS, `${XDG_DATA_HOME:-~/.local/share}/woosh/` on Linux.
///
/// # Errors
/// Returns an error if the system data directory cannot be determined.
pub fn data_dir() -> Result<PathBuf> {
    let base = dirs::data_dir().context("cannot determine data directory")?;
    Ok(base.join("woosh"))
}

/// Returns the path to the PID file.
///
/// # Errors
/// Returns an error if the data directory cannot be determined.
pub fn pid_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("woosh.pid"))
}

/// Returns the path to the Unix socket.
///
/// # Errors
/// Returns an error if the data directory cannot be determined.
pub fn socket_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("woosh.sock"))
}

/// Returns the path to the readiness file.
///
/// # Errors
/// Returns an error if the data directory cannot be determined.
pub fn ready_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("woosh.ready"))
}

/// Returns the path to the daemon log file.
///
/// # Errors
/// Returns an error if the data directory cannot be determined.
pub fn log_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("woosh.log"))
}

/// Returns `true` if a daemon process recorded in the PID file is alive.
/// If the PID file exists but the process is dead, cleans up stale files.
///
/// # Errors
/// Returns an error if the data directory cannot be determined.
pub fn daemon_is_alive() -> Result<bool> {
    let path = pid_path()?;
    let Ok(contents) = fs::read_to_string(&path) else {
        return Ok(false);
    };
    let Ok(pid) = contents.trim().parse::<i32>() else {
        return Ok(false);
    };
    // SAFETY: kill(pid, 0) is a standard liveness check with no side effects.
    let alive = unsafe { libc::kill(pid, 0) } == 0;

    if !alive {
        let _ = fs::remove_file(&path);
        if let Ok(sock_path) = socket_path() {
            let _ = fs::remove_file(sock_path);
        }
        if let Ok(ready) = ready_path() {
            let _ = fs::remove_file(ready);
        }
    }

    Ok(alive)
}

/// Writes the current process ID to the PID file.
///
/// # Errors
/// Returns an error if the file cannot be created or written.
pub fn write_pid_file(pid_path: &Path) -> Result<()> {
    let mut f = fs::File::create(pid_path)
        .with_context(|| format!("cannot create pid file: {}", pid_path.display()))?;
    writeln!(f, "{}", std::process::id())?;
    Ok(())
}

/// Forks the process into the background using the `daemonize` crate, which
/// writes the child's PID to `pid_path`. The child's stdout and stderr are
/// appended to the log file.
///
/// # Errors
/// Returns an error if daemonization fails.
pub fn daemonize(pid_path: &Path, log_path: &Path) -> Result<()> {
    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("cannot open log file: {}", log_path.display()))?;
    let log_file2 = log_file.try_clone()?;

    daemonize::Daemonize::new()
        .pid_file(pid_path)
        .stdout(log_file)
        .stderr(log_file2)
        .start()
        .context("daemonize failed")?;
    Ok(())
}

/// Initialises tracing to append to the log file (no ANSI colour codes), with
/// verbosity from `RUST_LOG` (default `info`).
///
/// # Errors
/// Returns an error if the log file cannot be opened.
pub fn init_logging(log_path: &Path) -> Result<()> {
    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("cannot open log file: {}", log_path.display()))?;

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::sync::Mutex::new(log_file))
        .with_ansi(false)
        .init();
    Ok(())
}

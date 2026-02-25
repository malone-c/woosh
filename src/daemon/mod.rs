pub mod audio;
pub mod ipc;
pub mod lifecycle;
pub mod state;

use anyhow::Result;
use audio::AudioCommand;
use state::{DaemonState, NoisePreset};
use std::sync::{Arc, Mutex};

/// Start the audio daemon.
///
/// # Errors
/// Returns an error if the daemon cannot be started (e.g. already running,
/// socket bind failure, audio device unavailable).
pub fn start(no_daemonize: bool) -> Result<()> {
    use std::fs;

    let data_dir = lifecycle::data_dir()?;
    fs::create_dir_all(&data_dir)?;

    let pid_path = lifecycle::pid_path()?;
    let socket_path = lifecycle::socket_path()?;
    let log_path = lifecycle::log_path()?;

    if lifecycle::daemon_is_alive()? {
        anyhow::bail!("daemon is already running");
    }

    if no_daemonize {
        lifecycle::write_pid_file(&pid_path)?;
    } else {
        lifecycle::maybe_daemonize(&pid_path, &log_path)?;
    }

    lifecycle::init_logging(&log_path)?;

    // Load config; fall back to defaults on any error.
    let config = crate::config::load().unwrap_or_default();
    let initial_preset = config
        .defaults
        .preset
        .parse::<NoisePreset>()
        .unwrap_or(NoisePreset::White);

    let state: Arc<Mutex<DaemonState>> = Arc::new(Mutex::new(DaemonState {
        preset: initial_preset,
        volume: config.defaults.volume,
        ..DaemonState::default()
    }));

    let sample_buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = std::sync::mpsc::channel::<AudioCommand>();

    audio::spawn_audio_thread(Arc::clone(&state), rx, Arc::clone(&sample_buf));

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(ipc::run_ipc_server(socket_path, state, tx, sample_buf))?;

    Ok(())
}

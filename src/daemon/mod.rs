pub mod audio;
pub mod eq;
pub mod ipc;
pub mod lifecycle;
pub mod mpv;
pub mod state;

use anyhow::Result;
use audio::AudioCommand;
use eq::N_BANDS;
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
    let ready_path = lifecycle::ready_path()?;
    let log_path = lifecycle::log_path()?;

    if lifecycle::daemon_is_alive()? {
        anyhow::bail!("daemon is already running");
    }

    // Clean up stale ready file from previous run
    let _ = fs::remove_file(&ready_path);

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
    let eq_arc: Arc<Mutex<[f32; N_BANDS]>> = Arc::new(Mutex::new([0.0f32; N_BANDS]));
    let place_eq_arc: Arc<Mutex<[f32; N_BANDS]>> = Arc::new(Mutex::new([0.0f32; N_BANDS]));
    let (tx, rx) = std::sync::mpsc::channel::<AudioCommand>();

    audio::spawn_audio_thread(
        Arc::clone(&state),
        rx,
        Arc::clone(&sample_buf),
        Arc::clone(&eq_arc),
        Arc::clone(&place_eq_arc),
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    
    runtime.block_on(ipc::run_ipc_server(socket_path, state, tx, sample_buf))?;

    Ok(())
}

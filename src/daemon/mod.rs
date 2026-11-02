pub mod audio;
pub mod eq;
pub mod ipc;
pub mod lifecycle;
pub mod mpv;
pub mod state;

use anyhow::Result;
use audio::AudioCommand;
use state::DaemonState;
use std::fs;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use tokio::signal::unix::{signal, SignalKind};

/// Start the audio daemon and run it until `QUIT`, `SIGTERM`, or idle timeout.
///
/// On exit, removes the socket, ready and PID files, then fades out both
/// channels and releases the audio device.
///
/// # Errors
/// Returns an error if the daemon cannot be started (e.g. already running,
/// socket bind failure).
pub fn start(no_daemonize: bool) -> Result<()> {
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
        lifecycle::daemonize(&pid_path, &log_path)?;
    }

    lifecycle::init_logging(&log_path)?;

    let config = crate::config::load().unwrap_or_else(|e| {
        tracing::warn!("using default config: {e:#}");
        crate::config::Config::default()
    });

    let mut initial_state = DaemonState::default();
    initial_state.synth.volume = config.defaults.volume.clamp(0.0, 1.0);
    let state = Arc::new(Mutex::new(initial_state));

    let (audio_tx, audio_rx) = mpsc::channel::<AudioCommand>();
    let audio_thread = audio::spawn_audio_thread(Arc::clone(&state), audio_rx);

    let idle_mins = config.daemon.idle_timeout_mins;
    let idle_timeout = (idle_mins > 0).then(|| Duration::from_secs(idle_mins * 60));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let server_result = runtime.block_on(async {
        let mut sigterm = signal(SignalKind::terminate())?;
        tokio::select! {
            result = ipc::run_ipc_server(&socket_path, &ready_path, state, audio_tx.clone(), idle_timeout) => result,
            _ = sigterm.recv() => {
                tracing::info!("received SIGTERM");
                Ok(())
            }
        }
    });

    // Remove the runtime files first so a new daemon can start while this one fades out.
    for path in [&socket_path, &ready_path, &pid_path] {
        let _ = fs::remove_file(path);
    }
    drop(runtime);
    let _ = audio_tx.send(AudioCommand::Shutdown);
    let _ = audio_thread.join();
    tracing::info!("daemon stopped");

    server_result
}

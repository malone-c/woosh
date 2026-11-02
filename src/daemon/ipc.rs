use crate::daemon::audio::AudioCommand;
use crate::daemon::eq::{GAIN_MAX, GAIN_MIN, N_BANDS};
use crate::daemon::state::{self, ChannelId, DaemonState, NoisePreset, PlayState};
use anyhow::Result;
use std::ffi::OsStr;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;

/// Programs `PLAY_PLACE` needs on `PATH`.
const PLACE_TOOLS: [&str; 2] = ["mpv", "yt-dlp"];

/// RAII guard that decrements a client counter when dropped.
struct ClientCountGuard(Arc<AtomicUsize>);

impl Drop for ClientCountGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Runs the Unix-socket IPC server until a client sends `QUIT` or the daemon
/// has been idle (no clients, both channels stopped) for `idle_timeout`.
///
/// Writes `ready_path` once the socket accepts connections. The caller removes
/// the socket and ready files and shuts down the audio thread afterwards.
///
/// # Errors
/// Returns an error if the socket cannot be bound or the ready file cannot be written.
pub async fn run_ipc_server(
    socket_path: &Path,
    ready_path: &Path,
    state: Arc<Mutex<DaemonState>>,
    audio_tx: Sender<AudioCommand>,
    idle_timeout: Option<Duration>,
) -> Result<()> {
    // Remove stale socket from a previous run.
    let _ = std::fs::remove_file(socket_path);
    let listener = UnixListener::bind(socket_path)?;
    tracing::info!("IPC server listening on {}", socket_path.display());

    let client_count = Arc::new(AtomicUsize::new(0));
    let shutdown = Arc::new(Notify::new());
    if let Some(idle_timeout) = idle_timeout {
        tokio::spawn(watch_idle(
            Arc::clone(&state),
            Arc::clone(&client_count),
            Arc::clone(&shutdown),
            idle_timeout,
        ));
    }

    std::fs::write(ready_path, "")?;
    tracing::info!("daemon ready, wrote {}", ready_path.display());

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _addr) = accepted?;
                let connection = handle_connection(
                    stream,
                    Arc::clone(&state),
                    audio_tx.clone(),
                    Arc::clone(&shutdown),
                    Arc::clone(&client_count),
                );
                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        tracing::warn!("IPC connection error: {e}");
                    }
                });
            }
            () = shutdown.notified() => return Ok(()),
        }
    }
}

/// Notifies `shutdown` once no client has been connected and both channels have
/// been stopped for `idle_timeout`.
async fn watch_idle(
    state: Arc<Mutex<DaemonState>>,
    client_count: Arc<AtomicUsize>,
    shutdown: Arc<Notify>,
    idle_timeout: Duration,
) {
    let mut idle_since: Option<Instant> = None;
    loop {
        tokio::time::sleep(idle_timeout.min(Duration::from_secs(60))).await;
        let both_stopped = {
            let s = state::lock(&state);
            s.synth.play_state == PlayState::Stopped && s.place.play_state == PlayState::Stopped
        };
        if client_count.load(Ordering::Relaxed) > 0 || !both_stopped {
            idle_since = None;
            continue;
        }
        if idle_since.get_or_insert_with(Instant::now).elapsed() >= idle_timeout {
            tracing::info!("auto-shutdown: idle for {idle_timeout:?} with no clients");
            shutdown.notify_one();
            return;
        }
    }
}

async fn handle_connection(
    stream: UnixStream,
    state: Arc<Mutex<DaemonState>>,
    audio_tx: Sender<AudioCommand>,
    shutdown: Arc<Notify>,
    client_count: Arc<AtomicUsize>,
) -> Result<()> {
    client_count.fetch_add(1, Ordering::Relaxed);
    let _guard = ClientCountGuard(client_count);

    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(response) = dispatch(line, &state, &audio_tx) else {
            shutdown.notify_one();
            break;
        };
        writer.write_all(response.as_bytes()).await?;
    }
    Ok(())
}

/// Applies one IPC line and returns its response, or `None` for `QUIT`.
fn dispatch(
    line: &str,
    state: &Mutex<DaemonState>,
    audio_tx: &Sender<AudioCommand>,
) -> Option<String> {
    let (verb, arg) = line.split_once(' ').unwrap_or((line, ""));
    let arg = arg.trim();

    let response = match verb.to_uppercase().as_str() {
        "PLAY" => play(arg, state, audio_tx),
        "PLAY_PLACE" => play_place(arg, state, audio_tx),
        "STOP" => stop(ChannelId::Synth, state, audio_tx),
        "STOP_PLACE" => stop(ChannelId::Place, state, audio_tx),
        "SET_VOLUME" => set_volume(ChannelId::Synth, arg, state, audio_tx),
        "SET_PLACE_VOLUME" => set_volume(ChannelId::Place, arg, state, audio_tx),
        "SET_EQ" => set_eq(ChannelId::Synth, arg, state, audio_tx),
        "SET_PLACE_EQ" => set_eq(ChannelId::Place, arg, state, audio_tx),
        "GET_EQ" => format!("EQ {}\n", format_gains(&state::lock(state).synth.eq_gains)),
        "GET_PLACE_EQ" => format!(
            "PLACE_EQ {}\n",
            format_gains(&state::lock(state).place.eq_gains)
        ),
        "STATUS" => status(state),
        "GET_PLACE_STATUS" => {
            let s = state::lock(state);
            let place = s.place_location.as_deref().unwrap_or("none");
            format!(
                "PLACE_STATUS place={place}:{}:{:.2}\n",
                s.place.play_state, s.place.volume
            )
        }
        "QUIT" => return None,
        other => format!("ERROR unknown command: {other}\n"),
    };
    Some(response)
}

/// Sends `command` to the audio thread and returns the IPC response.
fn send(audio_tx: &Sender<AudioCommand>, command: AudioCommand) -> String {
    match audio_tx.send(command) {
        Ok(()) => "OK\n".to_owned(),
        Err(_) => "ERROR audio thread is not running\n".to_owned(),
    }
}

fn play(arg: &str, state: &Mutex<DaemonState>, audio_tx: &Sender<AudioCommand>) -> String {
    let preset = match arg.parse::<NoisePreset>() {
        Ok(preset) => preset,
        Err(e) => return format!("ERROR {e}\n"),
    };
    let mut s = state::lock(state);
    s.synth.play_state = PlayState::Running;
    s.preset = Some(preset);
    send(audio_tx, AudioCommand::Play(preset))
}

fn play_place(
    location: &str,
    state: &Mutex<DaemonState>,
    audio_tx: &Sender<AudioCommand>,
) -> String {
    if location.is_empty() {
        return "ERROR missing place location\n".to_owned();
    }
    if let Some(tool) = missing_place_tool(&std::env::var_os("PATH").unwrap_or_default()) {
        return format!("ERROR {tool} not found in PATH; place sounds need mpv and yt-dlp\n");
    }
    let mut s = state::lock(state);
    s.place.play_state = PlayState::Running;
    s.place_location = Some(location.to_owned());
    send(audio_tx, AudioCommand::PlayPlace(location.to_owned()))
}

/// Returns the first tool in `PLACE_TOOLS` that is not in any directory of `path`.
fn missing_place_tool(path: &OsStr) -> Option<&'static str> {
    PLACE_TOOLS
        .into_iter()
        .find(|tool| !std::env::split_paths(path).any(|dir| dir.join(tool).is_file()))
}

fn stop(channel: ChannelId, state: &Mutex<DaemonState>, audio_tx: &Sender<AudioCommand>) -> String {
    let mut s = state::lock(state);
    s.channel_mut(channel).play_state = PlayState::Stopped;
    if channel == ChannelId::Place {
        s.place_location = None;
    }
    send(audio_tx, AudioCommand::Stop(channel))
}

fn set_volume(
    channel: ChannelId,
    arg: &str,
    state: &Mutex<DaemonState>,
    audio_tx: &Sender<AudioCommand>,
) -> String {
    let volume = match arg.parse::<f32>() {
        Ok(v) if !v.is_nan() => v.clamp(0.0, 1.0),
        _ => return format!("ERROR invalid volume: {arg}\n"),
    };
    let mut s = state::lock(state);
    s.channel_mut(channel).volume = volume;
    send(audio_tx, AudioCommand::SetVolume(channel, volume))
}

fn set_eq(
    channel: ChannelId,
    arg: &str,
    state: &Mutex<DaemonState>,
    audio_tx: &Sender<AudioCommand>,
) -> String {
    let (band, gain) = arg.split_once(' ').unwrap_or((arg, ""));
    let Ok(band) = band.parse::<usize>() else {
        return "ERROR invalid band index\n".to_owned();
    };
    if band >= N_BANDS {
        return format!("ERROR band index out of range (0..{N_BANDS})\n");
    }
    let gain = match gain.trim().parse::<f32>() {
        Ok(g) if !g.is_nan() => g.clamp(GAIN_MIN, GAIN_MAX),
        _ => return "ERROR invalid gain value\n".to_owned(),
    };
    let mut s = state::lock(state);
    let gains = &mut s.channel_mut(channel).eq_gains;
    gains[band] = gain;
    let gains = *gains;
    send(audio_tx, AudioCommand::SetEq(channel, gains))
}

fn status(state: &Mutex<DaemonState>) -> String {
    let s = state::lock(state);
    let preset = s.preset.map_or("none", NoisePreset::id);
    let place = s.place_location.as_deref().unwrap_or("none");
    let device = if s.device_available {
        "ok"
    } else {
        "unavailable"
    };
    format!(
        "STATUS synth={preset}:{}:{:.2} place={place}:{}:{:.2} device={device}\n",
        s.synth.play_state, s.synth.volume, s.place.play_state, s.place.volume
    )
}

fn format_gains(gains: &[f32; N_BANDS]) -> String {
    gains
        .iter()
        .map(|g| format!("{g:.1}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::missing_place_tool;
    use std::ffi::OsString;

    #[test]
    fn missing_place_tool_reports_first_absent_tool() {
        let dir = tempfile::tempdir().unwrap();
        let path = OsString::from(dir.path());
        assert_eq!(missing_place_tool(&path), Some("mpv"));

        std::fs::write(dir.path().join("mpv"), "").unwrap();
        assert_eq!(missing_place_tool(&path), Some("yt-dlp"));

        std::fs::write(dir.path().join("yt-dlp"), "").unwrap();
        assert_eq!(missing_place_tool(&path), None);
    }
}

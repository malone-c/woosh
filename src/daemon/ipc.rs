use crate::daemon::audio::AudioCommand;
use crate::daemon::lifecycle::remove_pid_file;
use crate::daemon::state::{DaemonState, NoisePreset};
use anyhow::Result;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::MissedTickBehavior;

/// Registry of live sample-stream subscriber channels.
type SubscriberRegistry = Arc<Mutex<Vec<UnboundedSender<String>>>>;

/// Runs the Unix-socket IPC server, accepting connections until the daemon exits.
///
/// # Errors
/// Returns an error if the socket cannot be bound.
pub async fn run_ipc_server(
    socket_path: PathBuf,
    state: Arc<Mutex<DaemonState>>,
    audio_tx: std::sync::mpsc::Sender<AudioCommand>,
    sample_buf: Arc<Mutex<Vec<f32>>>,
) -> Result<()> {
    // Remove stale socket from a previous run.
    let _ = std::fs::remove_file(&socket_path);

    let listener = UnixListener::bind(&socket_path)?;
    tracing::info!("IPC server listening on {}", socket_path.display());

    let subscribers: SubscriberRegistry = Arc::new(Mutex::new(Vec::new()));
    tokio::spawn(run_broadcast_task(
        Arc::clone(&sample_buf),
        Arc::clone(&subscribers),
    ));

    loop {
        let (stream, _addr) = listener.accept().await?;
        let state = Arc::clone(&state);
        let tx = audio_tx.clone();
        let socket_path = socket_path.clone();
        let subs = Arc::clone(&subscribers);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, state, tx, socket_path, subs).await {
                tracing::warn!("IPC connection error: {e}");
            }
        });
    }
}

/// Broadcast task: drain `sample_buf` every 33 ms and push to all subscribers.
async fn run_broadcast_task(sample_buf: Arc<Mutex<Vec<f32>>>, subscribers: SubscriberRegistry) {
    let mut interval = tokio::time::interval(Duration::from_millis(33));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        interval.tick().await;

        let samples: Vec<f32> = {
            let Ok(mut guard) = sample_buf.lock() else {
                continue;
            };
            guard.drain(..).collect()
        };

        if samples.is_empty() {
            continue;
        }

        let msg = format!("SAMPLES {}\n", encode_samples(&samples));

        let Ok(mut subs) = subscribers.lock() else {
            continue;
        };
        subs.retain(|tx| tx.send(msg.clone()).is_ok());
    }
}

/// Encode raw f32 samples as lowercase hex (8 chars/sample, little-endian).
fn encode_samples(samples: &[f32]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(samples.len() * 8);
    for &s in samples {
        for b in s.to_le_bytes() {
            let _ = write!(out, "{b:02x}");
        }
    }
    out
}

async fn handle_connection(
    stream: UnixStream,
    state: Arc<Mutex<DaemonState>>,
    audio_tx: std::sync::mpsc::Sender<AudioCommand>,
    socket_path: PathBuf,
    subscribers: SubscriberRegistry,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_owned();
        if line.is_empty() {
            continue;
        }

        // SUBSCRIBE_SAMPLES switches the connection to push-only mode.
        if line == "SUBSCRIBE_SAMPLES" {
            writer.write_all(b"OK\n").await?;
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            if let Ok(mut subs) = subscribers.lock() {
                subs.push(tx);
            }
            while let Some(msg) = rx.recv().await {
                if writer.write_all(msg.as_bytes()).await.is_err() {
                    break;
                }
            }
            return Ok(());
        }

        match dispatch(&line, &state, &audio_tx, &socket_path) {
            Some(response) => {
                writer.write_all(response.as_bytes()).await?;
            }
            None => {
                // QUIT path — daemon will exit; connection closes.
                break;
            }
        }
    }
    Ok(())
}

/// Parses one IPC line and returns the response string, or `None` for QUIT.
///
/// QUIT calls `process::exit(0)` which never returns; `None` is the logical
/// sentinel but is never actually observed by the caller.
#[allow(clippy::unnecessary_wraps)]
fn dispatch(
    line: &str,
    state: &Arc<Mutex<DaemonState>>,
    audio_tx: &std::sync::mpsc::Sender<AudioCommand>,
    socket_path: &PathBuf,
) -> Option<String> {
    let mut parts = line.splitn(2, ' ');
    let verb = parts.next().unwrap_or("").to_uppercase();
    let arg = parts.next().unwrap_or("").trim();

    match verb.as_str() {
        "PLAY" => match NoisePreset::from_str(arg) {
            Ok(preset) => {
                let _ = audio_tx.send(AudioCommand::Play(preset));
                Some("OK\n".to_owned())
            }
            Err(e) => Some(format!("ERROR {e}\n")),
        },
        "STOP" => {
            let _ = audio_tx.send(AudioCommand::Stop);
            Some("OK\n".to_owned())
        }
        "SET_VOLUME" => {
            match arg.parse::<f32>() {
                Ok(v) => {
                    let clamped = v.clamp(0.0, 1.0);
                    let _ = audio_tx.send(AudioCommand::SetVolume(clamped));
                    // Also update shared state immediately so STATUS reflects it.
                    if let Ok(mut s) = state.lock() {
                        s.volume = clamped;
                    }
                    Some("OK\n".to_owned())
                }
                Err(_) => Some("ERROR invalid volume value\n".to_owned()),
            }
        }
        "STATUS" => {
            let response = if let Ok(s) = state.lock() {
                format!(
                    "STATUS {} preset={} volume={:.2}\n",
                    s.play_state, s.preset, s.volume
                )
            } else {
                "ERROR state lock poisoned\n".to_owned()
            };
            Some(response)
        }
        "QUIT" => {
            let _ = audio_tx.send(AudioCommand::Shutdown);
            remove_pid_file();
            let _ = std::fs::remove_file(socket_path);
            std::process::exit(0);
        }
        _ => Some(format!("ERROR unknown command: {verb}\n")),
    }
}

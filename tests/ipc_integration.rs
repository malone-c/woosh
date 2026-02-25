use std::sync::{Arc, Mutex};
use tempfile::NamedTempFile;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use woosh::daemon::audio::AudioCommand;
use woosh::daemon::ipc::run_ipc_server;
use woosh::daemon::state::DaemonState;

/// Spawn a test IPC server on a unique socket path.
/// Returns `(socket_path, state, audio_rx, sample_buf)`.
async fn spawn_test_server() -> (
    std::path::PathBuf,
    Arc<Mutex<DaemonState>>,
    std::sync::mpsc::Receiver<AudioCommand>,
    Arc<Mutex<Vec<f32>>>,
) {
    // Create a temp file then remove it — we just want a unique path.
    let tmp = NamedTempFile::new().unwrap();
    let socket_path = tmp.path().with_extension("sock");
    drop(tmp);

    let state: Arc<Mutex<DaemonState>> = Arc::new(Mutex::new(DaemonState::default()));
    let (tx, rx) = std::sync::mpsc::channel::<AudioCommand>();
    let sample_buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));

    let sp = socket_path.clone();
    let st = Arc::clone(&state);
    let sb = Arc::clone(&sample_buf);
    tokio::spawn(async move {
        run_ipc_server(sp, st, tx, sb).await.ok();
    });

    // Give the server time to bind.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    (socket_path, state, rx, sample_buf)
}

#[tokio::test]
async fn status_returns_current_state() {
    let (socket_path, _state, _rx, _buf) = spawn_test_server().await;

    let stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    writer.write_all(b"STATUS\n").await.unwrap();

    let mut lines = BufReader::new(reader).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    assert!(line.starts_with("STATUS running"), "got: {line}");
    assert!(line.contains("preset=white"), "got: {line}");
    assert!(line.contains("volume=0.80"), "got: {line}");
}

#[tokio::test]
async fn set_volume_updates_state() {
    let (socket_path, state, _rx, _buf) = spawn_test_server().await;

    let stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    writer.write_all(b"SET_VOLUME 0.5\n").await.unwrap();

    let mut lines = BufReader::new(reader).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    assert_eq!(line, "OK");

    let volume = state.lock().unwrap().volume;
    assert!((volume - 0.5).abs() < 1e-6, "volume={volume}");
}

#[tokio::test]
async fn set_volume_clamps() {
    let (socket_path, state, _rx, _buf) = spawn_test_server().await;

    let stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    writer.write_all(b"SET_VOLUME 1.5\n").await.unwrap();

    let mut lines = BufReader::new(reader).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    assert_eq!(line, "OK");

    let volume = state.lock().unwrap().volume;
    assert!((volume - 1.0).abs() < 1e-6, "volume={volume}");
}

#[tokio::test]
async fn play_white_sends_command() {
    let (socket_path, _state, rx, _buf) = spawn_test_server().await;

    let stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    writer.write_all(b"PLAY white\n").await.unwrap();

    let mut lines = BufReader::new(reader).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    assert_eq!(line, "OK");

    let cmd = rx
        .recv_timeout(std::time::Duration::from_millis(200))
        .unwrap();
    assert!(
        matches!(
            cmd,
            AudioCommand::Play(woosh::daemon::state::NoisePreset::White)
        ),
        "unexpected command: {cmd:?}"
    );
}

#[tokio::test]
async fn play_pink_sends_command() {
    let (socket_path, _state, rx, _buf) = spawn_test_server().await;

    let stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    writer.write_all(b"PLAY pink\n").await.unwrap();

    let mut lines = BufReader::new(reader).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    assert_eq!(line, "OK");

    let cmd = rx
        .recv_timeout(std::time::Duration::from_millis(200))
        .unwrap();
    assert!(
        matches!(
            cmd,
            AudioCommand::Play(woosh::daemon::state::NoisePreset::Pink)
        ),
        "unexpected command: {cmd:?}"
    );
}

#[tokio::test]
async fn play_brown_sends_command() {
    let (socket_path, _state, rx, _buf) = spawn_test_server().await;

    let stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    writer.write_all(b"PLAY brown\n").await.unwrap();

    let mut lines = BufReader::new(reader).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    assert_eq!(line, "OK");

    let cmd = rx
        .recv_timeout(std::time::Duration::from_millis(200))
        .unwrap();
    assert!(
        matches!(
            cmd,
            AudioCommand::Play(woosh::daemon::state::NoisePreset::Brown)
        ),
        "unexpected command: {cmd:?}"
    );
}

#[tokio::test]
async fn unknown_command_returns_error() {
    let (socket_path, _state, _rx, _buf) = spawn_test_server().await;

    let stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    writer.write_all(b"BOGUS\n").await.unwrap();

    let mut lines = BufReader::new(reader).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    assert!(line.starts_with("ERROR"), "got: {line}");
}

#[tokio::test]
async fn stop_command_ok() {
    let (socket_path, _state, _rx, _buf) = spawn_test_server().await;

    let stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    writer.write_all(b"STOP\n").await.unwrap();

    let mut lines = BufReader::new(reader).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    assert_eq!(line, "OK");
}

#[tokio::test]
async fn subscribe_samples_receives_push() {
    let (socket_path, _state, _rx, sample_buf) = spawn_test_server().await;

    // Connect the subscriber.
    let sub_stream = UnixStream::connect(&socket_path).await.unwrap();
    let (sub_reader, mut sub_writer) = sub_stream.into_split();
    sub_writer.write_all(b"SUBSCRIBE_SAMPLES\n").await.unwrap();

    let mut sub_lines = BufReader::new(sub_reader).lines();
    let ok_line = sub_lines.next_line().await.unwrap().unwrap();
    assert_eq!(ok_line, "OK");

    // Push some samples into the buffer so the broadcast task picks them up.
    {
        let mut buf = sample_buf.lock().unwrap();
        buf.extend([0.1_f32, 0.2, 0.3, 0.4]);
    }

    // Wait up to 150 ms for a SAMPLES push message.
    let push_line =
        tokio::time::timeout(std::time::Duration::from_millis(150), sub_lines.next_line())
            .await
            .expect("timed out waiting for SAMPLES push")
            .unwrap()
            .unwrap();

    assert!(push_line.starts_with("SAMPLES "), "got: {push_line}");
}

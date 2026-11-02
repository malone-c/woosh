use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::task::JoinHandle;
use woosh::daemon::audio::AudioCommand;
use woosh::daemon::ipc::run_ipc_server;
use woosh::daemon::state::{ChannelId, DaemonState, NoisePreset, PlayState};

/// An IPC server on a private socket, plus the receiving end of its audio channel.
struct TestServer {
    socket_path: PathBuf,
    state: Arc<Mutex<DaemonState>>,
    audio_rx: Receiver<AudioCommand>,
    server: JoinHandle<anyhow::Result<()>>,
    _dir: TempDir,
}

impl TestServer {
    fn next_command(&self) -> AudioCommand {
        self.audio_rx
            .recv_timeout(Duration::from_millis(200))
            .expect("no audio command sent")
    }

    /// Sends each command on one connection and returns the reply lines.
    async fn send(&self, commands: &[&str]) -> Vec<String> {
        let stream = UnixStream::connect(&self.socket_path).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let mut replies = Vec::new();
        for command in commands {
            writer
                .write_all(format!("{command}\n").as_bytes())
                .await
                .unwrap();
            replies.push(lines.next_line().await.unwrap().unwrap());
        }
        replies
    }

    /// Waits for the server task to return, failing the test after `limit`.
    async fn wait_for_exit(self, limit: Duration) {
        let result = tokio::time::timeout(limit, self.server)
            .await
            .expect("server did not stop");
        result.unwrap().unwrap();
    }
}

/// Puts empty `mpv` and `yt-dlp` files first on `PATH` so `PLAY_PLACE` passes its tool check.
fn install_fake_place_tools() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fake-place-tools");
        std::fs::create_dir_all(&dir).unwrap();
        for tool in ["mpv", "yt-dlp"] {
            std::fs::write(dir.join(tool), "").unwrap();
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
        let dirs = std::iter::once(dir).chain(std::env::split_paths(&path));
        std::env::set_var("PATH", std::env::join_paths(dirs).unwrap());
    });
}

async fn spawn_server(idle_timeout: Option<Duration>) -> TestServer {
    install_fake_place_tools();
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("woosh.sock");
    let ready_path = dir.path().join("woosh.ready");
    let state = Arc::new(Mutex::new(DaemonState::default()));
    let (audio_tx, audio_rx) = std::sync::mpsc::channel();

    let server = tokio::spawn({
        let socket_path = socket_path.clone();
        let ready_path = ready_path.clone();
        let state = Arc::clone(&state);
        async move { run_ipc_server(&socket_path, &ready_path, state, audio_tx, idle_timeout).await }
    });

    tokio::time::timeout(Duration::from_secs(1), async {
        while !ready_path.exists() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("server did not write its ready file");

    TestServer {
        socket_path,
        state,
        audio_rx,
        server,
        _dir: dir,
    }
}

#[tokio::test]
async fn status_reports_both_channels_and_device() {
    let server = spawn_server(None).await;
    assert_eq!(
        server.send(&["STATUS"]).await,
        ["STATUS synth=none:stopped:0.80 place=none:stopped:0.40 device=ok"]
    );
}

#[tokio::test]
async fn play_sends_command_and_marks_synth_running() {
    let server = spawn_server(None).await;
    for preset in NoisePreset::ALL {
        assert_eq!(server.send(&[&format!("PLAY {preset}")]).await, ["OK"]);
        let command = server.next_command();
        assert!(
            matches!(command, AudioCommand::Play(p) if p == preset),
            "unexpected command: {command:?}"
        );
        let state = server.state.lock().unwrap();
        assert_eq!(state.synth.play_state, PlayState::Running);
        assert_eq!(state.preset, Some(preset));
    }
}

#[tokio::test]
async fn play_unknown_preset_returns_error() {
    let server = spawn_server(None).await;
    let reply = server.send(&["PLAY violet"]).await;
    assert_eq!(reply, ["ERROR unknown preset: violet"]);
}

#[tokio::test]
async fn stop_sends_command_and_keeps_preset() {
    let server = spawn_server(None).await;
    assert_eq!(server.send(&["PLAY pink", "STOP"]).await, ["OK", "OK"]);
    server.next_command();
    assert!(matches!(
        server.next_command(),
        AudioCommand::Stop(ChannelId::Synth)
    ));
    let state = server.state.lock().unwrap();
    assert_eq!(state.synth.play_state, PlayState::Stopped);
    assert_eq!(state.preset, Some(NoisePreset::Pink));
}

#[tokio::test]
async fn set_volume_updates_state_and_clamps() {
    let server = spawn_server(None).await;
    assert_eq!(server.send(&["SET_VOLUME 0.5"]).await, ["OK"]);
    assert!(matches!(
        server.next_command(),
        AudioCommand::SetVolume(ChannelId::Synth, v) if (v - 0.5).abs() < 1e-6
    ));

    assert_eq!(
        server
            .send(&["SET_VOLUME 1.5", "SET_PLACE_VOLUME 1.5"])
            .await,
        ["OK", "OK"]
    );
    let state = server.state.lock().unwrap();
    assert!((state.synth.volume - 1.0).abs() < 1e-6, "{state:?}");
    assert!((state.place.volume - 1.0).abs() < 1e-6, "{state:?}");
}

#[tokio::test]
async fn set_volume_rejects_nan_and_garbage() {
    let server = spawn_server(None).await;
    for command in ["SET_VOLUME nan", "SET_PLACE_VOLUME NaN", "SET_VOLUME loud"] {
        let reply = server.send(&[command]).await;
        assert!(
            reply[0].starts_with("ERROR invalid volume"),
            "{command}: {reply:?}"
        );
    }
    let state = server.state.lock().unwrap();
    assert!((state.synth.volume - 0.8).abs() < 1e-6, "{state:?}");
    assert!((state.place.volume - 0.4).abs() < 1e-6, "{state:?}");
}

#[tokio::test]
async fn set_eq_and_get_eq() {
    let server = spawn_server(None).await;
    let replies = server.send(&["SET_EQ 3 4.2", "GET_EQ"]).await;
    assert_eq!(
        replies,
        ["OK", "EQ 0.0 0.0 0.0 4.2 0.0 0.0 0.0 0.0 0.0 0.0"]
    );
    assert!(matches!(
        server.next_command(),
        AudioCommand::SetEq(ChannelId::Synth, gains) if (gains[3] - 4.2).abs() < 1e-6
    ));
}

#[tokio::test]
async fn set_eq_clamps_gain_and_rejects_bad_input() {
    let server = spawn_server(None).await;
    let replies = server
        .send(&[
            "SET_EQ 0 99",
            "SET_EQ 10 1",
            "SET_EQ 1 nan",
            "SET_EQ x 1",
            "GET_EQ",
        ])
        .await;
    assert_eq!(replies[0], "OK");
    assert!(
        replies[1].starts_with("ERROR band index out of range"),
        "{replies:?}"
    );
    assert_eq!(replies[2], "ERROR invalid gain value");
    assert_eq!(replies[3], "ERROR invalid band index");
    assert_eq!(replies[4], "EQ 12.0 0.0 0.0 0.0 0.0 0.0 0.0 0.0 0.0 0.0");
}

#[tokio::test]
async fn set_place_eq_leaves_synth_eq_alone() {
    let server = spawn_server(None).await;
    let replies = server
        .send(&["SET_PLACE_EQ 3 -4.0", "GET_PLACE_EQ", "GET_EQ"])
        .await;
    assert_eq!(
        replies,
        [
            "OK",
            "PLACE_EQ 0.0 0.0 0.0 -4.0 0.0 0.0 0.0 0.0 0.0 0.0",
            "EQ 0.0 0.0 0.0 0.0 0.0 0.0 0.0 0.0 0.0 0.0",
        ]
    );
    assert!(matches!(
        server.next_command(),
        AudioCommand::SetEq(ChannelId::Place, _)
    ));
}

#[tokio::test]
async fn play_place_sends_command_and_marks_place_running() {
    let server = spawn_server(None).await;
    assert_eq!(server.send(&["PLAY_PLACE coffee shop"]).await, ["OK"]);
    let command = server.next_command();
    assert!(
        matches!(command, AudioCommand::PlayPlace(ref location) if location == "coffee shop"),
        "unexpected command: {command:?}"
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.place_location.as_deref(), Some("coffee shop"));
    assert_eq!(state.place.play_state, PlayState::Running);
}

#[tokio::test]
async fn play_place_replaces_previous_location() {
    let server = spawn_server(None).await;
    let replies = server.send(&["PLAY_PLACE paris", "PLAY_PLACE tokyo"]).await;
    assert_eq!(replies, ["OK", "OK"]);
    assert_eq!(
        server.state.lock().unwrap().place_location.as_deref(),
        Some("tokyo")
    );
}

#[tokio::test]
async fn play_place_without_location_returns_error() {
    let server = spawn_server(None).await;
    assert_eq!(
        server.send(&["PLAY_PLACE"]).await,
        ["ERROR missing place location"]
    );
}

#[tokio::test]
async fn stop_place_sends_command_and_clears_location() {
    let server = spawn_server(None).await;
    assert_eq!(
        server.send(&["PLAY_PLACE paris", "STOP_PLACE"]).await,
        ["OK", "OK"]
    );
    server.next_command();
    assert!(matches!(
        server.next_command(),
        AudioCommand::Stop(ChannelId::Place)
    ));
    let state = server.state.lock().unwrap();
    assert_eq!(state.place.play_state, PlayState::Stopped);
    assert_eq!(state.place_location, None);
}

#[tokio::test]
async fn get_place_status_reports_channel_state() {
    let server = spawn_server(None).await;
    let replies = server
        .send(&["PLAY_PLACE berlin", "GET_PLACE_STATUS"])
        .await;
    assert_eq!(replies, ["OK", "PLACE_STATUS place=berlin:running:0.40"]);
}

#[tokio::test]
async fn unknown_command_returns_error() {
    let server = spawn_server(None).await;
    assert_eq!(
        server.send(&["BOGUS"]).await,
        ["ERROR unknown command: BOGUS"]
    );
}

#[tokio::test]
async fn quit_stops_server() {
    let server = spawn_server(None).await;
    let mut stream = UnixStream::connect(&server.socket_path).await.unwrap();
    stream.write_all(b"QUIT\n").await.unwrap();
    server.wait_for_exit(Duration::from_secs(1)).await;
}

#[tokio::test]
async fn idle_timeout_stops_server() {
    let server = spawn_server(Some(Duration::from_millis(100))).await;
    server.wait_for_exit(Duration::from_secs(2)).await;
}

#[tokio::test]
async fn connected_client_prevents_idle_shutdown() {
    let server = spawn_server(Some(Duration::from_millis(100))).await;
    let client = UnixStream::connect(&server.socket_path).await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !server.server.is_finished(),
        "daemon shut down with a client connected"
    );

    drop(client);
    server.wait_for_exit(Duration::from_secs(2)).await;
}

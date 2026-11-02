//! Daemon readiness: clients wait for the ready file, which the IPC server
//! writes only once its socket accepts connections.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::UnixStream;
use woosh::daemon::ipc::run_ipc_server;
use woosh::daemon::state::DaemonState;

/// Starts an IPC server in the background and returns as soon as it is spawned.
fn spawn_server(socket_path: &Path, ready_path: &Path) {
    let state = Arc::new(Mutex::new(DaemonState::default()));
    let (audio_tx, _audio_rx) = std::sync::mpsc::channel();
    let socket_path = socket_path.to_path_buf();
    let ready_path = ready_path.to_path_buf();
    tokio::spawn(async move {
        let _ = run_ipc_server(&socket_path, &ready_path, state, audio_tx, None).await;
    });
}

async fn wait_for(path: &Path) {
    tokio::time::timeout(Duration::from_millis(500), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{} did not appear within 500 ms", path.display()));
}

#[tokio::test]
async fn ready_file_means_socket_accepts_connections() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("woosh.sock");
    let ready_path = dir.path().join("woosh.ready");

    spawn_server(&socket_path, &ready_path);
    wait_for(&ready_path).await;

    UnixStream::connect(&socket_path)
        .await
        .expect("connection failed after the ready file appeared");
}

#[tokio::test]
async fn stale_socket_file_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("woosh.sock");
    let ready_path = dir.path().join("woosh.ready");
    std::fs::write(&socket_path, "left over from a crashed daemon").unwrap();

    spawn_server(&socket_path, &ready_path);
    wait_for(&ready_path).await;

    UnixStream::connect(&socket_path)
        .await
        .expect("server did not replace the stale socket file");
}

/// Integration test to reproduce the race condition where a client attempts
/// to connect to the daemon socket before the daemon has fully initialized
/// and started listening.
///
/// This test should fail intermittently (or consistently depending on timing)
/// until the readiness file mechanism is implemented.
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::UnixStream;
use woosh::daemon::audio::AudioCommand;
use woosh::daemon::ipc::run_ipc_server;
use woosh::daemon::state::DaemonState;

/// Helper to spawn the IPC server in a background task and return immediately.
/// This simulates the daemon startup without the delay that would make the test pass.
fn spawn_daemon_without_waiting(
    socket_path: &Path,
) -> (
    tokio::task::JoinHandle<()>,
    Arc<Mutex<DaemonState>>,
) {
    let state: Arc<Mutex<DaemonState>> = Arc::new(Mutex::new(DaemonState::default()));
    let (tx, _rx) = std::sync::mpsc::channel::<AudioCommand>();
    let sample_buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));

    let st = Arc::clone(&state);
    let sp = socket_path.to_path_buf();
    let sb = sample_buf;
    
    let handle = tokio::spawn(async move {
        let _ = run_ipc_server(sp, st, tx, sb, 0).await;
    });

    (handle, state)
}

#[tokio::test]
async fn test_daemon_startup_race_condition() {
    // Create a temporary socket path
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("test.sock");

    // Spawn the daemon server (this happens in background)
    let (_handle, _state) = spawn_daemon_without_waiting(&socket_path); // PathBuf derefs to &Path

    // DON'T wait for socket file - try to connect immediately
    // This aggressively tests the race condition
    let mut connection_succeeded_before_file = false;
    
    for attempt in 0..100 {
        if socket_path.exists() {
            // Socket file now exists - try to connect immediately
            match UnixStream::connect(&socket_path).await {
                Ok(_) => {
                    println!("✓ Connection succeeded on attempt {}", attempt + 1);
                    break;
                }
                Err(e) if attempt < 99 => {
                    // This demonstrates the race: file exists but not ready
                    println!(
                        "✗ Attempt {}: Socket file exists but connection failed: {e}",
                        attempt + 1
                    );
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    continue;
                }
                Err(e) => {
                    panic!(
                        "✗ RACE CONDITION: Socket file exists but connection failed after \
                        100 attempts: {e}\nThis demonstrates the bug - socket file appeared \
                        before bind completed."
                    );
                }
            }
        } else {
            // Socket file doesn't exist yet - try to connect anyway
            if UnixStream::connect(&socket_path).await.is_ok() {
                connection_succeeded_before_file = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_micros(100)).await;
    }

    assert!(!connection_succeeded_before_file, "Connection should not succeed before socket file exists");
}

#[tokio::test]
async fn test_daemon_connects_after_proper_wait() {
    // This test demonstrates the proper fix: wait for daemon to be truly ready
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("test2.sock");

    // Spawn the daemon server
    let (_handle, _state) = spawn_daemon_without_waiting(&socket_path); // PathBuf derefs to &Path

    // Wait for socket file to exist (current behavior)
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    loop {
        if socket_path.exists() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "daemon socket did not appear within 500ms"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    // Give the daemon a bit more time to complete the bind operation
    // This is what the readiness file will signal properly
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Now connection should succeed
    let result = UnixStream::connect(&socket_path).await;
    assert!(result.is_ok(), "Connection should succeed after proper wait");
    
    println!("✓ Connection succeeded with proper wait time");
}

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::task::JoinHandle;

/// Lightweight IPC client for sending commands to the daemon.
pub struct DaemonClient {
    socket_path: PathBuf,
}

impl DaemonClient {
    /// Connect to the daemon socket (verifies reachability).
    ///
    /// # Errors
    /// Returns an error if the socket cannot be reached.
    pub async fn connect(socket_path: PathBuf) -> Result<Self> {
        UnixStream::connect(&socket_path)
            .await
            .context("cannot connect to daemon")?;
        Ok(Self { socket_path })
    }

    /// Send a command and await the first response line.
    ///
    /// # Errors
    /// Returns an error if the socket cannot be reached or no response is received.
    pub async fn send_command(&self, cmd: &str) -> Result<String> {
        let stream = UnixStream::connect(&self.socket_path).await?;
        let (reader, mut writer) = stream.into_split();
        writer
            .write_all(format!("{cmd}\n").as_bytes())
            .await
            .context("write command")?;
        let mut lines = BufReader::new(reader).lines();
        Ok(lines.next_line().await?.unwrap_or_default())
    }

    /// Send a command without waiting for a response (e.g., `SET_VOLUME`).
    ///
    /// # Errors
    /// Returns an error if the socket cannot be reached.
    pub async fn send_fire_and_forget(&self, cmd: &str) -> Result<()> {
        let mut stream = UnixStream::connect(&self.socket_path).await?;
        stream
            .write_all(format!("{cmd}\n").as_bytes())
            .await
            .context("write fire-and-forget command")?;
        Ok(())
    }
}

/// Open a dedicated subscription connection and forward decoded sample batches
/// to `tx` until the receiver is dropped or the daemon disconnects.
///
/// Reconnects with exponential backoff (100 ms → 5 s max) on failure.
///
/// # Errors
/// Returns an error only if the task cannot be spawned.
pub async fn subscribe_samples(
    socket_path: PathBuf,
    tx: tokio::sync::mpsc::Sender<Vec<f32>>,
) -> Result<JoinHandle<()>> {
    let handle = tokio::spawn(async move {
        let mut backoff = Duration::from_millis(100);
        loop {
            match try_subscribe(&socket_path, &tx).await {
                Ok(()) => {
                    // Receiver was dropped — no point reconnecting.
                    return;
                }
                Err(e) => {
                    tracing::debug!("sample subscription lost ({e}); retrying in {backoff:?}");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(5));
                }
            }
        }
    });
    Ok(handle)
}

/// Attempt a single subscription session. Returns `Ok(())` when the receiver
/// is dropped (clean shutdown) or an error on connection / parse failures.
async fn try_subscribe(
    socket_path: &PathBuf,
    tx: &tokio::sync::mpsc::Sender<Vec<f32>>,
) -> Result<()> {
    let stream = UnixStream::connect(socket_path)
        .await
        .context("connect for SUBSCRIBE_SAMPLES")?;
    let (reader, mut writer) = stream.into_split();
    writer
        .write_all(b"SUBSCRIBE_SAMPLES\n")
        .await
        .context("send SUBSCRIBE_SAMPLES")?;

    let mut lines = BufReader::new(reader).lines();
    let first = lines
        .next_line()
        .await?
        .context("no response to SUBSCRIBE_SAMPLES")?;
    anyhow::ensure!(
        first.trim() == "OK",
        "unexpected subscribe response: {first}"
    );

    while let Some(line) = lines.next_line().await? {
        if let Some(hex) = line.trim().strip_prefix("SAMPLES ") {
            let samples = decode_samples(hex);
            if tx.send(samples).await.is_err() {
                // Receiver dropped — clean shutdown.
                return Ok(());
            }
        }
    }
    anyhow::bail!("subscription stream closed by daemon");
}

/// Decode a hex-encoded sample payload (8 hex chars per f32, little-endian).
fn decode_samples(hex: &str) -> Vec<f32> {
    hex.as_bytes()
        .chunks(8)
        .filter_map(|chunk| {
            if chunk.len() != 8 {
                return None;
            }
            let mut bytes = [0u8; 4];
            for (i, pair) in chunk.chunks(2).enumerate() {
                let s = std::str::from_utf8(pair).ok()?;
                let n = u8::from_str_radix(s, 16).ok()?;
                bytes[i] = n;
            }
            Some(f32::from_le_bytes(bytes))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::decode_samples;

    fn encode_samples(samples: &[f32]) -> String {
        let mut out = String::with_capacity(samples.len() * 8);
        for &s in samples {
            for b in s.to_le_bytes() {
                out.push_str(&format!("{b:02x}"));
            }
        }
        out
    }

    #[test]
    fn test_decode_known_value_one() {
        let hex = "0000803f";
        let samples = decode_samples(hex);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0], 1.0);
    }

    #[test]
    fn test_decode_known_value_half() {
        let hex = "0000003f";
        let samples = decode_samples(hex);
        assert_eq!(samples.len(), 1);
        assert!((samples[0] - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_decode_known_value_negative() {
        let hex = "000080bf";
        let samples = decode_samples(hex);
        assert_eq!(samples.len(), 1);
        assert!((samples[0] - (-1.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn test_decode_known_value_zero() {
        let hex = "00000000";
        let samples = decode_samples(hex);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0], 0.0);
    }

    #[test]
    fn test_roundtrip_various_values() {
        let samples = vec![0.0, 0.25, 0.5, 0.75, 1.0, -0.5, -1.0, 0.1234567];
        let encoded = encode_samples(&samples);
        let decoded = decode_samples(&encoded);
        assert_eq!(decoded.len(), samples.len());
        for (original, decoded_val) in samples.iter().zip(decoded.iter()) {
            assert!(
                (original - decoded_val).abs() < 1e-6,
                "expected {}, got {}",
                original,
                decoded_val
            );
        }
    }

    #[test]
    fn test_roundtrip_small_values() {
        let samples = vec![1e-10, -1e-10, 1e-5, -1e-5];
        let encoded = encode_samples(&samples);
        let decoded = decode_samples(&encoded);
        assert_eq!(decoded.len(), samples.len());
        for (original, decoded_val) in samples.iter().zip(decoded.iter()) {
            assert!(
                (original - decoded_val).abs() < 1e-10,
                "expected {}, got {}",
                original,
                decoded_val
            );
        }
    }

    #[test]
    fn test_decode_multiple_samples() {
        let hex = "0000803f0000003f000080bf";
        let samples = decode_samples(hex);
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[0], 1.0);
        assert!((samples[1] - 0.5).abs() < f32::EPSILON);
        assert!((samples[2] - (-1.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn test_decode_invalid_length_ignored() {
        let hex = "0000803f00";
        let samples = decode_samples(hex);
        assert_eq!(samples.len(), 1);
    }
}

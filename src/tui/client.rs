use crate::daemon::eq::N_BANDS;
use crate::daemon::state::{NoisePreset, PlayState};
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;

/// Longest wait for a reply before the connection is treated as lost.
const REPLY_TIMEOUT: Duration = Duration::from_millis(500);
const MIN_BACKOFF: Duration = Duration::from_millis(100);
const MAX_BACKOFF: Duration = Duration::from_secs(5);

/// The parts of a `STATUS` reply the TUI shows.
#[derive(Debug, PartialEq)]
pub struct Status {
    pub preset: Option<NoisePreset>,
    pub play_state: PlayState,
    pub volume: f32,
    pub device_available: bool,
}

struct Connection {
    lines: Lines<BufReader<OwnedReadHalf>>,
    writer: OwnedWriteHalf,
}

/// IPC client holding one connection to the daemon.
///
/// The open connection makes the daemon count the TUI as a client, which
/// keeps its idle auto-shutdown from firing. When the connection drops, the
/// client reconnects with exponential backoff (100 ms doubling to 5 s); between
/// attempts, requests fail immediately so the UI never waits on a dead socket.
pub struct DaemonClient {
    socket_path: PathBuf,
    connection: Option<Connection>,
    backoff: Duration,
    next_attempt: Instant,
}

impl DaemonClient {
    #[must_use]
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            connection: None,
            backoff: MIN_BACKOFF,
            next_attempt: Instant::now(),
        }
    }

    /// # Errors
    /// Returns an error if the daemon is unreachable or replies with something other than `OK`.
    pub async fn play(&mut self, preset: NoisePreset) -> Result<()> {
        self.expect_ok(&format!("PLAY {preset}")).await
    }

    /// # Errors
    /// Returns an error if the daemon is unreachable or replies with something other than `OK`.
    pub async fn stop(&mut self) -> Result<()> {
        self.expect_ok("STOP").await
    }

    /// # Errors
    /// Returns an error if the daemon is unreachable or replies with something other than `OK`.
    pub async fn set_volume(&mut self, volume: f32) -> Result<()> {
        self.expect_ok(&format!("SET_VOLUME {volume:.2}")).await
    }

    /// Sets one EQ band on both channels, which share one EQ.
    ///
    /// # Errors
    /// Returns an error if the daemon is unreachable or replies with something other than `OK`.
    pub async fn set_eq_band(&mut self, band: usize, gain_db: f32) -> Result<()> {
        self.expect_ok(&format!("SET_EQ {band} {gain_db:.1}"))
            .await?;
        self.expect_ok(&format!("SET_PLACE_EQ {band} {gain_db:.1}"))
            .await
    }

    /// Queries the synth channel's 10-band EQ gains.
    ///
    /// # Errors
    /// Returns an error if the daemon is unreachable or the reply is malformed.
    pub async fn get_eq(&mut self) -> Result<[f32; N_BANDS]> {
        let reply = self.request("GET_EQ").await?;
        let values: Vec<f32> = reply
            .strip_prefix("EQ ")
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();
        values
            .try_into()
            .map_err(|_| anyhow::anyhow!("unexpected GET_EQ reply: {reply}"))
    }

    /// # Errors
    /// Returns an error if the daemon is unreachable or the reply is malformed.
    pub async fn status(&mut self) -> Result<Status> {
        let reply = self.request("STATUS").await?;
        parse_status(&reply).with_context(|| format!("unexpected STATUS reply: {reply}"))
    }

    /// Asks the daemon to fade out and exit. `QUIT` has no reply.
    ///
    /// # Errors
    /// Returns an error if the daemon is unreachable.
    pub async fn quit_daemon(&mut self) -> Result<()> {
        let mut connection = match self.connection.take() {
            Some(connection) => connection,
            None => self.connect().await?,
        };
        connection.writer.write_all(b"QUIT\n").await?;
        Ok(())
    }

    async fn expect_ok(&mut self, command: &str) -> Result<()> {
        let reply = self.request(command).await?;
        if reply != "OK" {
            bail!("daemon rejected {command:?}: {reply}");
        }
        Ok(())
    }

    /// Sends one command and returns the daemon's reply line, reconnecting first if needed.
    async fn request(&mut self, command: &str) -> Result<String> {
        let mut connection = match self.connection.take() {
            Some(connection) => connection,
            None => self.connect().await?,
        };
        let exchange = async {
            connection
                .writer
                .write_all(format!("{command}\n").as_bytes())
                .await?;
            connection
                .lines
                .next_line()
                .await?
                .context("daemon closed the connection")
        };
        let reply = tokio::time::timeout(REPLY_TIMEOUT, exchange)
            .await
            .context("daemon did not reply")??;
        self.connection = Some(connection);
        Ok(reply)
    }

    /// Opens a new connection, at most once per backoff period.
    async fn connect(&mut self) -> Result<Connection> {
        if Instant::now() < self.next_attempt {
            bail!("daemon unreachable; waiting to reconnect");
        }
        match UnixStream::connect(&self.socket_path).await {
            Ok(stream) => {
                self.backoff = MIN_BACKOFF;
                let (reader, writer) = stream.into_split();
                Ok(Connection {
                    lines: BufReader::new(reader).lines(),
                    writer,
                })
            }
            Err(e) => {
                self.next_attempt = Instant::now() + self.backoff;
                self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
                Err(e).context("cannot connect to daemon")
            }
        }
    }
}

/// Parses `STATUS synth=<preset>:<state>:<volume> place=<…> device=<ok|unavailable>`.
fn parse_status(reply: &str) -> Option<Status> {
    let synth = reply.strip_prefix("STATUS synth=")?.split(' ').next()?;
    let mut fields = synth.split(':');
    let preset = fields.next()?.parse().ok();
    let play_state = match fields.next()? {
        "running" => PlayState::Running,
        "stopped" => PlayState::Stopped,
        _ => return None,
    };
    let volume = fields.next()?.parse().ok()?;
    Some(Status {
        preset,
        play_state,
        volume,
        device_available: !reply.ends_with(" device=unavailable"),
    })
}

#[cfg(test)]
mod tests {
    use super::{parse_status, Status};
    use crate::daemon::state::{NoisePreset, PlayState};

    #[test]
    fn parses_running_status() {
        let status =
            parse_status("STATUS synth=pink:running:0.55 place=coffee shop:running:0.40 device=ok");
        assert_eq!(
            status,
            Some(Status {
                preset: Some(NoisePreset::Pink),
                play_state: PlayState::Running,
                volume: 0.55,
                device_available: true,
            })
        );
    }

    #[test]
    fn parses_status_before_first_play() {
        let status = parse_status(
            "STATUS synth=none:stopped:0.80 place=none:stopped:0.40 device=unavailable",
        )
        .unwrap();
        assert_eq!(status.preset, None);
        assert_eq!(status.play_state, PlayState::Stopped);
        assert!(!status.device_available);
    }

    #[test]
    fn rejects_other_replies() {
        assert_eq!(parse_status("OK"), None);
        assert_eq!(parse_status("STATUS synth=pink:paused:0.5"), None);
    }
}

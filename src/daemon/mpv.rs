use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use rodio::Source;

use crate::daemon::audio::{Fade, SAMPLE_RATE};

/// Samples per chunk the reader thread hands to the source (≈ 46 ms).
const CHUNK_SAMPLES: usize = 2_048;
/// Chunks buffered ahead of playback (≈ 5 s) before the reader thread blocks.
const MAX_BUFFERED_CHUNKS: usize = 108;

/// Wraps an `mpv` subprocess and exposes its PCM output as a `rodio::Source`.
///
/// `mpv` decodes the first `YouTube` result for "walking through {location}"
/// that is at least 10 minutes long, skipping its first five minutes, and writes signed 16-bit mono PCM at
/// `SAMPLE_RATE` to stdout. A reader thread converts it to `f32` chunks and
/// sends them over a bounded channel. `--ao=pcm` decodes faster than real time,
/// so the channel fills up; the reader then blocks, mpv blocks on the full
/// pipe, and playback speed paces the whole pipeline.
///
/// `Iterator::next` never blocks or takes a lock: it returns silence while mpv
/// is buffering and `None` once mpv's output has been played to the end.
pub struct MpvSource {
    child: Child,
    chunks: Receiver<Vec<f32>>,
    current: std::vec::IntoIter<f32>,
    fade: Fade,
}

impl MpvSource {
    /// Spawn `mpv` and begin streaming PCM for the first long `YouTube` result for "walking through {location}".
    ///
    /// # Errors
    /// Returns an error if `mpv` cannot be spawned (binary not found, etc.).
    pub fn spawn(location: &str, fade_out: Arc<AtomicBool>) -> anyhow::Result<Self> {
        // mpv only passes recognised URLs to yt-dlp; the ytdl:// prefix makes it treat the search as one.
        let query = format!("ytdl://ytsearch10:walking through {location}");
        let sample_rate = format!("--audio-samplerate={SAMPLE_RATE}");

        let mut child = Command::new("mpv")
            .args([
                &query,
                // Only videos of 10+ minutes, so skipping the first five leaves something to play.
                "--ytdl-raw-options=match-filter=duration>=600",
                "--start=300",
                "--no-video",
                "--no-terminal",
                "--audio-display=no",
                "--ytdl-format=bestaudio/best",
                "--audio-format=s16",
                &sample_rate,
                "--audio-channels=mono",
                "--ao=pcm",
                "--ao-pcm-waveheader=no",
                "--ao-pcm-file=/dev/stdout",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()?;

        let Some(mut stdout) = child.stdout.take() else {
            anyhow::bail!("mpv stdout was not captured");
        };
        let (chunk_tx, chunks) = mpsc::sync_channel(MAX_BUFFERED_CHUNKS);

        // The JoinHandle is dropped so the reader runs detached. It ends when mpv
        // exits (killed in Drop), or when sending fails because the source was dropped.
        std::thread::spawn(move || {
            let mut bytes = [0u8; CHUNK_SAMPLES * 2];
            // read_exact keeps whole 2-byte samples; a partial chunk at end of stream is dropped.
            while stdout.read_exact(&mut bytes).is_ok() {
                let chunk = bytes
                    .chunks_exact(2)
                    .map(|pair| f32::from(i16::from_le_bytes([pair[0], pair[1]])) / 32_768.0)
                    .collect();
                if chunk_tx.send(chunk).is_err() {
                    return;
                }
            }
        });

        Ok(Self {
            child,
            chunks,
            current: Vec::new().into_iter(),
            fade: Fade::new(fade_out),
        })
    }
}

impl Iterator for MpvSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        loop {
            if let Some(sample) = self.current.next() {
                return Some(sample * self.fade.next_gain()?);
            }
            match self.chunks.try_recv() {
                Ok(chunk) => self.current = chunk.into_iter(),
                // The reader has finished and every chunk has been played.
                Err(TryRecvError::Disconnected) => return None,
                Err(TryRecvError::Empty) if self.fade.is_fading_out() => return None,
                // Silence while mpv is buffering; the fade-in only advances on real audio.
                Err(TryRecvError::Empty) => return Some(0.0),
            }
        }
    }
}

impl Source for MpvSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        1
    }
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

impl Drop for MpvSource {
    fn drop(&mut self) {
        // Kill the subprocess and reap it so we don't leave a zombie.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

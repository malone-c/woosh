use std::collections::VecDeque;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::Source;

/// Maximum buffered samples (≈ 5 s at 44 100 Hz mono).
const MAX_BUFFER_SAMPLES: usize = 220_500;

/// Wraps an `mpv` subprocess and exposes its PCM output as a `rodio::Source`.
///
/// `mpv` is instructed to output raw signed-16-bit little-endian mono PCM at
/// 44 100 Hz to stdout.  A dedicated reader thread converts each `i16` frame
/// to an `f32` in `[-1.0, 1.0]` and pushes it into a shared `VecDeque`.
///
/// `Iterator::next` is **non-blocking** (called from rodio's audio thread):
/// it returns `Some(0.0)` (silence) when the buffer is empty but the reader
/// thread is still alive, and `None` only after the reader thread has exited.
#[allow(clippy::module_name_repetitions)]
pub struct MpvSource {
    child: Child,
    buffer: Arc<Mutex<VecDeque<f32>>>,
    reader_done: Arc<AtomicBool>,
    /// Counter for 1.5 s fade-in (0.0 → `66_150.0` samples at `44_100` Hz).
    /// Stored as `f32` (not `u32`) so arithmetic stays in one type and avoids
    /// `cast_precision_loss` lints — the counter never exceeds 66_150 so there
    /// is no precision difference in practice.
    fade_samples: f32,
    /// Shared flag; set to `true` by the audio thread to trigger fade-out.
    fade_out: Arc<AtomicBool>,
    /// Counter for 1.5 s fade-out (0.0 → `66_150.0` samples at `44_100` Hz).
    /// Same rationale as `fade_samples`.
    fade_out_samples: f32,
}

impl MpvSource {
    /// Spawn `mpv` and begin streaming PCM for `ytsearch1:walking through {location}`.
    ///
    /// # Errors
    /// Returns an error if `mpv` cannot be spawned (binary not found, etc.).
    ///
    /// # Panics
    /// Panics if the child process stdout handle cannot be taken (should never happen
    /// because stdout is set to `Stdio::piped()`).
    pub fn spawn(location: &str, fade_out: Arc<AtomicBool>) -> anyhow::Result<Self> {
        let query = format!("ytsearch1:walking through {location}");

        let mut child = Command::new("mpv")
            .args([
                &query,
                "--no-video",
                "--no-terminal",
                "--audio-display=no",
                "--ytdl-format=bestaudio/best",
                "--af=aformat=format=s16le:rate=44100:channels=1",
                "--ao=pcm",
                "--ao-pcm-waveheader=no",
                "--ao-pcm-file=/dev/stdout",
                "--msg-level=all=no",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()?;

        let buffer: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
        let reader_done: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

        let mut stdout = child.stdout.take().expect("stdout was piped");
        let buffer_clone = Arc::clone(&buffer);
        let done_clone = Arc::clone(&reader_done);

        // Reader thread: convert raw s16le PCM → f32 and push into shared buffer.
        // The JoinHandle is intentionally dropped so this thread runs detached.
        std::thread::spawn(move || {
            let mut raw = [0u8; 1024];
            loop {
                match stdout.read(&mut raw) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let samples: Vec<f32> = raw[..n]
                            .chunks_exact(2)
                            .map(|b| {
                                let s = i16::from_le_bytes([b[0], b[1]]);
                                f32::from(s) / 32_768.0_f32
                            })
                            .collect();

                        if let Ok(mut guard) = buffer_clone.lock() {
                            guard.extend(samples);
                            // Cap at ~5 seconds to bound memory.
                            while guard.len() > MAX_BUFFER_SAMPLES {
                                guard.pop_front();
                            }
                        }
                    }
                }
            }
            done_clone.store(true, Ordering::Release);
        });

        Ok(Self {
            child,
            buffer,
            reader_done,
            fade_samples: 0.0,
            fade_out,
            fade_out_samples: 0.0,
        })
    }
}

impl Iterator for MpvSource {
    type Item = f32;

    /// Non-blocking: returns silence while mpv is still buffering, `None` only after exit.
    fn next(&mut self) -> Option<f32> {
        let raw = if let Ok(mut guard) = self.buffer.try_lock() {
            if let Some(sample) = guard.pop_front() {
                sample
            } else {
                if self.reader_done.load(Ordering::Acquire) {
                    return None;
                }
                // Silence while mpv hasn't produced data yet.
                0.0
            }
        } else {
            if self.reader_done.load(Ordering::Acquire) {
                return None;
            }
            0.0
        };
        let fade_in = (self.fade_samples / 66_150.0).min(1.0);
        self.fade_samples = (self.fade_samples + 1.0).min(66_150.0);

        let fade_out = if self.fade_out.load(Ordering::Relaxed) {
            let t = self.fade_out_samples / 66_150.0;
            let fo = 1.0 - t;
            if fo <= 0.0 {
                return None;
            }
            self.fade_out_samples += 1.0;
            fo
        } else {
            1.0
        };

        Some(raw * fade_in * fade_out)
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
        44_100
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

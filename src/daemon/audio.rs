use crate::daemon::state::{DaemonState, NoisePreset, PlayState};
use rand::distributions::{Distribution, Uniform};
use rand::rngs::SmallRng;
use rand::SeedableRng;
use rodio::Source;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Commands sent from the IPC server to the audio thread.
#[derive(Debug)]
pub enum AudioCommand {
    Play(NoisePreset),
    Stop,
    SetVolume(f32),
    Shutdown,
}

/// Per-preset state for the IIR noise generators.
enum NoiseAlgorithm {
    White,
    /// Paul Kellet refined 7-state-variable IIR pink noise filter.
    Pink {
        b: [f32; 7],
    },
    /// Brown (red) noise via first-order integration.
    Brown {
        last: f32,
    },
}

/// A noise source implementing `rodio::Source` that supports white, pink, and brown noise.
///
/// Samples are accumulated locally and flushed in batches of 512 into `sample_buf`
/// (when provided) so the visualizer TUI can read them.
pub struct NoiseSource {
    rng: SmallRng,
    volume: f32,
    dist: Uniform<f32>,
    algorithm: NoiseAlgorithm,
    /// Accumulator before flushing to `sample_buf`.
    local_batch: Vec<f32>,
    /// Shared buffer read by the IPC broadcast task.
    sample_buf: Option<Arc<Mutex<Vec<f32>>>>,
}

impl NoiseSource {
    /// Creates a new `NoiseSource`.
    ///
    /// # Panics
    /// Panics if the system RNG cannot be seeded.
    #[must_use]
    pub fn new(preset: NoisePreset, volume: f32, sample_buf: Option<Arc<Mutex<Vec<f32>>>>) -> Self {
        let algorithm = match preset {
            NoisePreset::White => NoiseAlgorithm::White,
            NoisePreset::Pink => NoiseAlgorithm::Pink { b: [0.0; 7] },
            NoisePreset::Brown => NoiseAlgorithm::Brown { last: 0.0 },
        };
        Self {
            rng: SmallRng::from_rng(rand::thread_rng()).expect("rng init"),
            volume,
            dist: Uniform::new_inclusive(-1.0_f32, 1.0_f32),
            algorithm,
            local_batch: Vec::with_capacity(512),
            sample_buf,
        }
    }

    /// Sets the volume of this source.
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
    }

    /// Flush `local_batch` into `sample_buf` using a non-blocking `try_lock`.
    fn try_flush(&mut self) {
        if let Some(buf) = &self.sample_buf {
            if let Ok(mut guard) = buf.try_lock() {
                guard.extend_from_slice(&self.local_batch);
                self.local_batch.clear();
            } else if self.local_batch.len() > 4_096 {
                // Lock persistently contended; discard to avoid unbounded growth.
                self.local_batch.clear();
            }
        }
    }
}

impl Iterator for NoiseSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let white = self.dist.sample(&mut self.rng);

        let raw = match &mut self.algorithm {
            NoiseAlgorithm::White => white,
            NoiseAlgorithm::Pink { b } => {
                b[0] = 0.99886 * b[0] + white * 0.055_517_9;
                b[1] = 0.99332 * b[1] + white * 0.075_075_9;
                b[2] = 0.969_00 * b[2] + white * 0.153_852;
                b[3] = 0.866_50 * b[3] + white * 0.310_485_6;
                b[4] = 0.550_00 * b[4] + white * 0.532_952_2;
                b[5] = -0.761_6 * b[5] - white * 0.016_898_0;
                let pink = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + white * 0.536_2;
                b[6] = white * 0.115_926;
                pink * 0.11
            }
            NoiseAlgorithm::Brown { last } => {
                *last = (*last + 0.02 * white) / 1.02;
                (*last * 3.5).clamp(-1.0, 1.0)
            }
        };

        let sample = raw * self.volume;

        if self.sample_buf.is_some() {
            self.local_batch.push(sample);
            if self.local_batch.len() >= 512 {
                self.try_flush();
            }
        }

        Some(sample)
    }
}

impl Source for NoiseSource {
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

/// Spawns the dedicated audio thread.
///
/// `rodio::OutputStream` is `!Send`, so this must run on an OS thread,
/// not a Tokio task. The thread owns the sink and processes `AudioCommand`s
/// until `Shutdown` or the channel is closed.
///
/// # Panics
/// Panics if a `rodio::Sink` cannot be created (audio device unavailable).
pub fn spawn_audio_thread(
    state: Arc<Mutex<DaemonState>>,
    rx: std::sync::mpsc::Receiver<AudioCommand>,
    sample_buf: Arc<Mutex<Vec<f32>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let (_stream, handle) = match rodio::OutputStream::try_default() {
            Ok(pair) => pair,
            Err(e) => {
                tracing::error!("audio: failed to open output stream: {e}");
                return;
            }
        };

        let (initial_preset, initial_volume) = state
            .lock()
            .map(|s| (s.preset, s.volume))
            .unwrap_or((NoisePreset::White, 0.8));
        let mut sink = rodio::Sink::try_new(&handle).expect("sink");
        sink.append(NoiseSource::new(
            initial_preset,
            1.0,
            Some(Arc::clone(&sample_buf)),
        ));
        sink.set_volume(initial_volume);

        loop {
            match rx.recv() {
                Ok(AudioCommand::Play(preset)) => {
                    drop(sink);
                    sink = rodio::Sink::try_new(&handle).expect("sink");
                    sink.append(NoiseSource::new(preset, 1.0, Some(Arc::clone(&sample_buf))));
                    let volume = state.lock().map(|s| s.volume).unwrap_or(0.8);
                    sink.set_volume(volume);
                    if let Ok(mut s) = state.lock() {
                        s.play_state = PlayState::Running;
                        s.preset = preset;
                    }
                }
                Ok(AudioCommand::Stop) => {
                    sink.pause();
                    if let Ok(mut s) = state.lock() {
                        s.play_state = PlayState::Stopped;
                    }
                }
                Ok(AudioCommand::SetVolume(v)) => {
                    let clamped = v.clamp(0.0, 1.0);
                    sink.set_volume(clamped);
                    if let Ok(mut s) = state.lock() {
                        s.volume = clamped;
                    }
                }
                Ok(AudioCommand::Shutdown) | Err(_) => break,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_noise_statistics() {
        let mut src = NoiseSource::new(NoisePreset::White, 1.0, None);
        let samples: Vec<f32> = (0..10_000).map(|_| src.next().unwrap()).collect();
        let mean: f32 = samples.iter().sum::<f32>() / 10_000.0;
        let variance: f32 = samples.iter().map(|&x| (x - mean).powi(2)).sum::<f32>() / 10_000.0;
        let std_dev = variance.sqrt();
        // Uniform [-1,1]: mean=0, std_dev=1/√3≈0.577
        assert!(mean.abs() < 0.05, "mean={mean}");
        assert!((std_dev - 0.577).abs() < 0.05, "std_dev={std_dev}");
    }

    #[test]
    fn pink_noise_in_range() {
        let mut src = NoiseSource::new(NoisePreset::Pink, 1.0, None);
        let samples: Vec<f32> = (0..44_100).map(|_| src.next().unwrap()).collect();
        #[allow(clippy::cast_precision_loss)]
        let mean: f32 = samples.iter().sum::<f32>() / samples.len() as f32;
        // Pink noise should have zero mean and bounded output
        assert!(mean.abs() < 0.1, "pink mean={mean}");
        assert!(
            samples.iter().all(|&s| (-1.1..=1.1).contains(&s)),
            "pink sample out of expected range"
        );
    }

    #[test]
    fn brown_noise_clamped() {
        let mut src = NoiseSource::new(NoisePreset::Brown, 1.0, None);
        let samples: Vec<f32> = (0..10_000).map(|_| src.next().unwrap()).collect();
        assert!(
            samples.iter().all(|&s| (-1.0..=1.0).contains(&s)),
            "brown noise not clamped to [-1, 1]"
        );
    }
}

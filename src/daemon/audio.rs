use crate::daemon::eq::{EqProcessor, N_BANDS};
use crate::daemon::mpv::MpvSource;
use crate::daemon::state::{DaemonState, NoisePreset, PlayState};
use rand::distributions::{Distribution, Uniform};
use rand::rngs::SmallRng;
use rand::SeedableRng;
use rodio::Source;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Commands sent from the IPC server to the audio thread.
#[derive(Debug)]
pub enum AudioCommand {
    Play(NoisePreset),
    Stop,
    SetVolume(f32),
    SetEq([f32; N_BANDS]),
    PlayPlace(String),
    StopPlace,
    SetPlaceVolume(f32),
    SetPlaceEq([f32; N_BANDS]),
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
    /// Counter for 1.5 s fade-in (0.0 → `66_150.0` samples at `44_100` Hz).
    /// Stored as `f32` (not `u32`) so arithmetic stays in one type and avoids
    /// `cast_precision_loss` lints — the counter never exceeds `66_150` so there
    /// is no precision difference in practice.
    fade_samples: f32,
    /// Shared flag; set to `true` by the audio thread to trigger fade-out.
    fade_out: Arc<AtomicBool>,
    /// Counter for 1.5 s fade-out (0.0 → `66_150.0` samples at `44_100` Hz).
    /// Same rationale as `fade_samples`.
    fade_out_samples: f32,
}

impl NoiseSource {
    /// Creates a new `NoiseSource`.
    ///
    /// # Panics
    /// Panics if the system RNG cannot be seeded.
    #[must_use]
    pub fn new(
        preset: NoisePreset,
        volume: f32,
        sample_buf: Option<Arc<Mutex<Vec<f32>>>>,
        fade_out: Arc<AtomicBool>,
    ) -> Self {
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
            fade_samples: 0.0,
            fade_out,
            fade_out_samples: 0.0,
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
            NoiseAlgorithm::White => white * 0.1725,
            NoiseAlgorithm::Pink { b } => {
                b[0] = 0.99886 * b[0] + white * 0.055_517_9;
                b[1] = 0.99332 * b[1] + white * 0.075_075_9;
                b[2] = 0.969_00 * b[2] + white * 0.153_852;
                b[3] = 0.866_50 * b[3] + white * 0.310_485_6;
                b[4] = 0.550_00 * b[4] + white * 0.532_952_2;
                b[5] = -0.761_6 * b[5] - white * 0.016_898_0;
                let pink = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + white * 0.536_2;
                b[6] = white * 0.115_926;
                pink * 0.057
            }
            NoiseAlgorithm::Brown { last } => {
                *last = (*last + 0.02 * white) / 1.02;
                (*last * 1.76).clamp(-1.0, 1.0)
            }
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

        let sample = raw * self.volume * fade_in * fade_out;

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
/// not a Tokio task. The thread owns the sinks and processes `AudioCommand`s
/// until `Shutdown` or the channel is closed.
///
/// The `OutputStream` is opened lazily on the first `Play`/`PlayPlace` command
/// and released (dropping the device claim) once both channels are stopped and
/// their fade-outs have drained. This allows Bluetooth headphones to hand off
/// to other audio sources while woosh is idle.
#[allow(clippy::too_many_lines)]
pub fn spawn_audio_thread(
    state: Arc<Mutex<DaemonState>>,
    rx: std::sync::mpsc::Receiver<AudioCommand>,
    sample_buf: Arc<Mutex<Vec<f32>>>,
    eq_arc: Arc<Mutex<[f32; N_BANDS]>>,
    place_eq_arc: Arc<Mutex<[f32; N_BANDS]>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        // Used to distinguish a recv timeout (poll tick) from a channel close.
        // Declared first so it precedes all `let` statements (items_after_statements lint).
        enum Recv {
            Got(AudioCommand),
            Timeout,
            Gone,
        }

        // Stream is opened lazily and dropped when both channels go idle.
        // IMPORTANT: always drop sinks before dropping the stream — the stream
        // owns the mixer thread that the sinks write into.
        let mut opt_stream: Option<(rodio::OutputStream, rodio::OutputStreamHandle)> = None;
        let mut fade_out_flag = Arc::new(AtomicBool::new(false));
        let mut place_fade_out_flag: Option<Arc<AtomicBool>> = None;
        let mut synth_sink: Option<rodio::Sink> = None;
        let mut place_sink: Option<rodio::Sink> = None;

        loop {
            // While either channel is fading, use recv_timeout so we can poll
            // for fade completion every 50 ms and release the device once done.
            let synth_fading = fade_out_flag.load(Ordering::Relaxed)
                && synth_sink.as_ref().is_some_and(|s| !s.empty());
            let place_fading = place_fade_out_flag
                .as_ref()
                .is_some_and(|f| f.load(Ordering::Relaxed))
                && place_sink.as_ref().is_some_and(|s| !s.empty());

            let recv = if synth_fading || place_fading {
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(cmd) => Recv::Got(cmd),
                    Err(RecvTimeoutError::Timeout) => Recv::Timeout,
                    Err(RecvTimeoutError::Disconnected) => Recv::Gone,
                }
            } else {
                match rx.recv() {
                    Ok(cmd) => Recv::Got(cmd),
                    Err(_) => Recv::Gone,
                }
            };

            let cmd = match recv {
                // Channel closed: local variables drop in reverse declaration
                // order, so synth_sink and place_sink drop before opt_stream.
                Recv::Gone => break,
                Recv::Timeout => {
                    // Check whether fading sinks have finished and clean up.
                    if synth_sink.as_ref().is_some_and(rodio::Sink::empty) {
                        synth_sink = None;
                    }
                    if place_sink.as_ref().is_some_and(rodio::Sink::empty) {
                        place_sink = None;
                        place_fade_out_flag = None;
                    }
                    // Release the audio device when both channels are idle.
                    if synth_sink.is_none() && place_sink.is_none() {
                        let both_stopped = state.lock().map_or(true, |s| {
                            s.play_state == PlayState::Stopped
                                && s.place_state == PlayState::Stopped
                        });
                        if both_stopped {
                            opt_stream = None; // releases CoreAudio / ALSA claim
                        }
                    }
                    continue;
                }
                Recv::Got(cmd) => cmd,
            };

            match cmd {
                AudioCommand::Play(preset) => {
                    // Open the audio device lazily on first play.
                    if opt_stream.is_none() {
                        match rodio::OutputStream::try_default() {
                            Ok(pair) => {
                                opt_stream = Some(pair);
                            }
                            Err(e) => {
                                tracing::error!("audio: failed to open output stream: {e}");
                                if let Ok(mut s) = state.lock() {
                                    s.play_state = PlayState::Stopped;
                                }
                                continue;
                            }
                        }
                    }
                    let Some((_, handle)) = &opt_stream else {
                        continue; // unreachable; satisfies the borrow checker
                    };
                    synth_sink = None; // drop old sink, abandoning any active fade
                    fade_out_flag = Arc::new(AtomicBool::new(false));
                    match rodio::Sink::try_new(handle) {
                        Ok(new_sink) => {
                            new_sink.append(EqProcessor::new(
                                NoiseSource::new(
                                    preset,
                                    1.0,
                                    Some(Arc::clone(&sample_buf)),
                                    Arc::clone(&fade_out_flag),
                                ),
                                Arc::clone(&eq_arc),
                            ));
                            let volume = state.lock().map(|s| s.volume).unwrap_or(0.8);
                            new_sink.set_volume(volume);
                            synth_sink = Some(new_sink);
                        }
                        Err(e) => {
                            tracing::error!("audio: failed to create synth sink: {e}");
                        }
                    }
                    if let Ok(mut s) = state.lock() {
                        s.play_state = PlayState::Running;
                        s.preset = Some(preset);
                    }
                }
                AudioCommand::Stop => {
                    fade_out_flag.store(true, Ordering::Release);
                    if let Ok(mut s) = state.lock() {
                        s.play_state = PlayState::Stopped;
                    }
                }
                AudioCommand::SetVolume(v) => {
                    let clamped = v.clamp(0.0, 1.0);
                    if let Some(s) = &synth_sink {
                        s.set_volume(clamped);
                    }
                    if let Ok(mut s) = state.lock() {
                        s.volume = clamped;
                    }
                }
                AudioCommand::SetEq(gains) => {
                    if let Ok(mut guard) = eq_arc.lock() {
                        *guard = gains;
                    }
                }
                AudioCommand::PlayPlace(location) => {
                    // Open the audio device lazily on first play.
                    if opt_stream.is_none() {
                        match rodio::OutputStream::try_default() {
                            Ok(pair) => {
                                opt_stream = Some(pair);
                            }
                            Err(e) => {
                                tracing::error!("audio: failed to open output stream: {e}");
                                if let Ok(mut s) = state.lock() {
                                    s.place_state = PlayState::Stopped;
                                    s.place_location = None;
                                }
                                continue;
                            }
                        }
                    }
                    let Some((_, handle)) = &opt_stream else {
                        continue;
                    };
                    place_sink = None; // kills old mpv via MpvSource::drop
                    let flag = Arc::new(AtomicBool::new(false));
                    match MpvSource::spawn(&location, Arc::clone(&flag)) {
                        Err(e) => {
                            tracing::error!("audio: mpv spawn failed for {location:?}: {e}");
                            if let Ok(mut s) = state.lock() {
                                s.place_state = PlayState::Stopped;
                                s.place_location = None;
                            }
                        }
                        Ok(source) => match rodio::Sink::try_new(handle) {
                            Err(e) => {
                                tracing::error!("audio: place sink failed: {e}");
                                if let Ok(mut s) = state.lock() {
                                    s.place_state = PlayState::Stopped;
                                }
                            }
                            Ok(new_sink) => {
                                new_sink.append(EqProcessor::new(
                                    source,
                                    Arc::clone(&place_eq_arc),
                                ));
                                let vol =
                                    state.lock().map(|s| s.place_volume).unwrap_or(0.4);
                                new_sink.set_volume(vol);
                                if let Ok(mut s) = state.lock() {
                                    s.place_state = PlayState::Running;
                                    s.place_location = Some(location);
                                }
                                place_fade_out_flag = Some(flag);
                                place_sink = Some(new_sink);
                            }
                        },
                    }
                }
                AudioCommand::StopPlace => {
                    if let Some(ref flag) = place_fade_out_flag {
                        flag.store(true, Ordering::Release);
                    }
                    if let Ok(mut s) = state.lock() {
                        s.place_state = PlayState::Stopped;
                        s.place_location = None;
                    }
                }
                AudioCommand::SetPlaceVolume(v) => {
                    let clamped = v.clamp(0.0, 1.0);
                    if let Ok(mut s) = state.lock() {
                        s.place_volume = clamped;
                    }
                    if let Some(ref sink) = place_sink {
                        sink.set_volume(clamped);
                    }
                }
                AudioCommand::SetPlaceEq(gains) => {
                    if let Ok(mut guard) = place_eq_arc.lock() {
                        *guard = gains;
                    }
                    if let Ok(mut s) = state.lock() {
                        s.place_eq_gains = gains;
                    }
                }
                AudioCommand::Shutdown => {
                    // Locals are declared opt_stream → synth_sink → place_sink,
                    // so Rust drops them in reverse: place_sink → synth_sink →
                    // opt_stream. Sinks are torn down before the stream's mixer
                    // thread — the required ordering.
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn white_noise_statistics() {
        let mut src = NoiseSource::new(NoisePreset::White, 1.0, None, Arc::new(AtomicBool::new(false)));
        // Skip past the 1.5 s fade ramp so statistics reflect full-amplitude noise.
        for _ in 0..66_150 {
            let _ = src.next();
        }
        let samples: Vec<f32> = (0..10_000).map(|_| src.next().unwrap()).collect();
        let mean: f32 = samples.iter().sum::<f32>() / 10_000.0;
        let variance: f32 = samples.iter().map(|&x| (x - mean).powi(2)).sum::<f32>() / 10_000.0;
        let std_dev = variance.sqrt();
        // Uniform [-1,1] scaled by 0.1725: mean=0, std_dev≈0.1725/√3≈0.0996
        assert!(mean.abs() < 0.01, "mean={mean}");
        assert!((std_dev - 0.0996).abs() < 0.01, "std_dev={std_dev}");
    }

    #[test]
    fn pink_noise_in_range() {
        let mut src = NoiseSource::new(NoisePreset::Pink, 1.0, None, Arc::new(AtomicBool::new(false)));
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
        let mut src = NoiseSource::new(NoisePreset::Brown, 1.0, None, Arc::new(AtomicBool::new(false)));
        let samples: Vec<f32> = (0..10_000).map(|_| src.next().unwrap()).collect();
        assert!(
            samples.iter().all(|&s| (-1.0..=1.0).contains(&s)),
            "brown noise not clamped to [-1, 1]"
        );
    }

    #[test]
    fn noise_source_fade_out() {
        let fade_out = Arc::new(AtomicBool::new(false));
        let mut src =
            NoiseSource::new(NoisePreset::White, 1.0, None, Arc::clone(&fade_out));

        // Skip past the 1.5 s fade-in ramp so the source is at full amplitude.
        for _ in 0..66_150 {
            src.next().unwrap();
        }

        // Trigger fade-out.
        fade_out.store(true, Ordering::Release);

        // Collect every sample until the source terminates.
        let samples: Vec<f32> = std::iter::from_fn(|| src.next()).collect();

        // The ramp runs for exactly 66_150 samples before returning None.
        assert_eq!(
            samples.len(),
            66_150,
            "expected 66_150 fade-out samples, got {}",
            samples.len()
        );

        // Envelope should be decreasing: RMS near the start > RMS near the midpoint.
        let start_rms =
            (samples[..1_000].iter().map(|s| s * s).sum::<f32>() / 1_000.0).sqrt();
        let mid_rms =
            (samples[32_575..33_575].iter().map(|s| s * s).sum::<f32>() / 1_000.0).sqrt();
        assert!(
            mid_rms < start_rms,
            "fade-out envelope not decreasing: start_rms={start_rms}, mid_rms={mid_rms}"
        );
    }

    #[test]
    #[ignore]
    fn measure_noise_levels() {
        const FADE_IN_SAMPLES: usize = 66_150;
        const MEASURE_SAMPLES: usize = 44_100;

        struct Metrics {
            rms: f32,
            peak: f32,
            crest: f32,
        }

        fn measure(preset: NoisePreset) -> Metrics {
            let mut src = NoiseSource::new(
                preset,
                1.0,
                None,
                Arc::new(AtomicBool::new(false)),
            );
            for _ in 0..FADE_IN_SAMPLES {
                let _ = src.next();
            }
            let samples: Vec<f32> = (0..MEASURE_SAMPLES)
                .map(|_| src.next().unwrap())
                .collect();
            #[allow(clippy::cast_precision_loss)]
            let mean_sq: f32 =
                samples.iter().map(|&s| s * s).sum::<f32>() / samples.len() as f32;
            let rms = mean_sq.sqrt();
            let peak = samples.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
            let crest = if rms > 0.0 { peak / rms } else { 0.0 };
            Metrics { rms, peak, crest }
        }

        let presets = [
            ("White", NoisePreset::White),
            ("Pink", NoisePreset::Pink),
            ("Brown", NoisePreset::Brown),
        ];

        println!();
        println!(
            "{:<8}  {:>10}  {:>10}  {:>12}",
            "Preset", "RMS", "Peak", "Crest Factor"
        );
        println!("{}", "-".repeat(46));
        for (name, preset) in presets {
            let m = measure(preset);
            println!(
                "{:<8}  {:>10.6}  {:>10.6}  {:>12.6}",
                name, m.rms, m.peak, m.crest
            );
        }
        println!();
    }
}

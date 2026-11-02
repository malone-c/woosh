use crate::daemon::eq::{EqProcessor, SharedGains, N_BANDS};
use crate::daemon::mpv::MpvSource;
use crate::daemon::state::{self, ChannelId, DaemonState, NoisePreset, PlayState};
use rand::distributions::{Distribution, Uniform};
use rand::rngs::SmallRng;
use rand::SeedableRng;
use rodio::{OutputStream, OutputStreamHandle, Sink, Source};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Sample rate of every source; all sources are mono.
pub const SAMPLE_RATE: u32 = 44_100;
/// Fade-in and fade-out length in samples: 1.5 s at `SAMPLE_RATE`.
const FADE_SAMPLES: f32 = 66_150.0;
/// How often the audio thread checks for finished sinks while the device is open.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// Wait between attempts to open an unavailable audio device.
const DEVICE_RETRY_INTERVAL: Duration = Duration::from_secs(5);
/// Longest time shutdown waits for fade-outs to finish.
const SHUTDOWN_FADE_LIMIT: Duration = Duration::from_secs(2);

/// Commands sent from the IPC server to the audio thread.
#[derive(Debug)]
pub enum AudioCommand {
    Play(NoisePreset),
    PlayPlace(String),
    /// Fade the channel out.
    Stop(ChannelId),
    SetVolume(ChannelId, f32),
    SetEq(ChannelId, [f32; N_BANDS]),
    /// Fade out both channels, then end the audio thread.
    Shutdown,
}

/// Gain envelope: fades in from silence, and fades out once the shared flag is set.
///
/// Counters are `f32` so the gain arithmetic needs no casts.
pub struct Fade {
    in_samples: f32,
    out_samples: f32,
    out_requested: Arc<AtomicBool>,
}

impl Fade {
    #[must_use]
    pub fn new(fade_out: Arc<AtomicBool>) -> Self {
        Self {
            in_samples: 0.0,
            out_samples: 0.0,
            out_requested: fade_out,
        }
    }

    /// Returns the gain for the next sample, or `None` once the fade-out has finished.
    pub fn next_gain(&mut self) -> Option<f32> {
        let fade_in = self.in_samples / FADE_SAMPLES;
        self.in_samples = (self.in_samples + 1.0).min(FADE_SAMPLES);
        if !self.is_fading_out() {
            return Some(fade_in);
        }
        let fade_out = 1.0 - self.out_samples / FADE_SAMPLES;
        if fade_out <= 0.0 {
            return None;
        }
        self.out_samples += 1.0;
        Some(fade_in * fade_out)
    }

    #[must_use]
    pub fn is_fading_out(&self) -> bool {
        self.out_requested.load(Ordering::Acquire)
    }
}

/// Per-preset state for the noise generators.
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

/// Endless mono noise for one preset, scaled so every preset is about 0.1 RMS.
pub struct NoiseSource {
    rng: SmallRng,
    dist: Uniform<f32>,
    algorithm: NoiseAlgorithm,
    fade: Fade,
}

impl NoiseSource {
    #[must_use]
    pub fn new(preset: NoisePreset, fade_out: Arc<AtomicBool>) -> Self {
        let algorithm = match preset {
            NoisePreset::White => NoiseAlgorithm::White,
            NoisePreset::Pink => NoiseAlgorithm::Pink { b: [0.0; 7] },
            NoisePreset::Brown => NoiseAlgorithm::Brown { last: 0.0 },
        };
        Self {
            rng: SmallRng::from_entropy(),
            dist: Uniform::new_inclusive(-1.0_f32, 1.0_f32),
            algorithm,
            fade: Fade::new(fade_out),
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

        Some(raw * self.fade.next_gain()?)
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
        SAMPLE_RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// A sink playing one source, and the flag that starts that source's fade-out.
struct Voice {
    sink: Sink,
    fade_out: Arc<AtomicBool>,
}

impl Voice {
    fn fade_out(&self) {
        self.fade_out.store(true, Ordering::Release);
    }

    fn is_finished(&self) -> bool {
        self.sink.empty()
    }
}

/// Builds a source and plays it through `eq_gains` on a new sink.
fn start_voice<S>(
    device: &OutputStreamHandle,
    make_source: impl FnOnce(Arc<AtomicBool>) -> anyhow::Result<S>,
    eq_gains: &SharedGains,
    volume: f32,
) -> anyhow::Result<Voice>
where
    S: Source<Item = f32> + Send + 'static,
{
    let fade_out = Arc::new(AtomicBool::new(false));
    let source = make_source(Arc::clone(&fade_out))?;
    let sink = Sink::try_new(device)?;
    sink.set_volume(volume);
    sink.append(EqProcessor::new(source, Arc::clone(eq_gains)));
    Ok(Voice { sink, fade_out })
}

/// Spawns the audio thread, which owns the output device and both channels' sinks.
///
/// `rodio::OutputStream` is `!Send`, so this must be an OS thread, not a Tokio
/// task. The device is opened when a channel starts and released once both
/// channels have stopped and faded out, so Bluetooth headphones can hand off to
/// other apps while woosh is idle. While the device cannot be opened, the thread
/// retries every 5 s and sets `device_available` to false.
///
/// The thread records the outcome of each command in `state` (channel started,
/// failed, stopped, or place stream ended) and runs until `Shutdown` or until
/// every sender is dropped.
pub fn spawn_audio_thread(
    state: Arc<Mutex<DaemonState>>,
    rx: Receiver<AudioCommand>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || run_audio_thread(&state, &rx))
}

#[allow(clippy::too_many_lines)] // one loop owning the !Send device; splitting it means threading six locals through helpers
fn run_audio_thread(state: &Mutex<DaemonState>, rx: &Receiver<AudioCommand>) {
    let synth_eq: SharedGains = Arc::new(Mutex::new([0.0; N_BANDS]));
    let place_eq: SharedGains = Arc::new(Mutex::new([0.0; N_BANDS]));
    // Declared before the voices so it is dropped after them: sinks must be
    // dropped before the stream that runs their mixer.
    let mut device: Option<(OutputStream, OutputStreamHandle)> = None;
    let mut synth: Option<Voice> = None;
    let mut place: Option<Voice> = None;
    // Requests waiting for the device to open.
    let mut pending_synth: Option<NoisePreset> = None;
    let mut pending_place: Option<String> = None;
    let mut next_device_attempt = Instant::now();

    loop {
        let idle = device.is_none() && pending_synth.is_none() && pending_place.is_none();
        let command = if idle {
            match rx.recv() {
                Ok(command) => Some(command),
                Err(_) => return,
            }
        } else {
            match rx.recv_timeout(POLL_INTERVAL) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        };

        match command {
            Some(AudioCommand::Play(preset)) => {
                synth = None;
                pending_synth = Some(preset);
            }
            Some(AudioCommand::PlayPlace(location)) => {
                place = None; // kills the old mpv via MpvSource::drop
                pending_place = Some(location);
            }
            Some(AudioCommand::Stop(ChannelId::Synth)) => {
                pending_synth = None;
                if let Some(voice) = &synth {
                    voice.fade_out();
                }
                state::lock(state).synth.play_state = PlayState::Stopped;
            }
            Some(AudioCommand::Stop(ChannelId::Place)) => {
                pending_place = None;
                if let Some(voice) = &place {
                    voice.fade_out();
                }
                let mut s = state::lock(state);
                s.place.play_state = PlayState::Stopped;
                s.place_location = None;
            }
            Some(AudioCommand::SetVolume(channel, volume)) => {
                let voice = match channel {
                    ChannelId::Synth => &synth,
                    ChannelId::Place => &place,
                };
                if let Some(voice) = voice {
                    voice.sink.set_volume(volume);
                }
            }
            Some(AudioCommand::SetEq(channel, gains)) => {
                let eq_gains = match channel {
                    ChannelId::Synth => &synth_eq,
                    ChannelId::Place => &place_eq,
                };
                *eq_gains.lock().unwrap_or_else(PoisonError::into_inner) = gains;
            }
            Some(AudioCommand::Shutdown) => {
                let voices: Vec<&Voice> = synth.iter().chain(place.iter()).collect();
                for voice in &voices {
                    voice.fade_out();
                }
                let deadline = Instant::now() + SHUTDOWN_FADE_LIMIT;
                while Instant::now() < deadline && !voices.iter().all(|voice| voice.is_finished()) {
                    std::thread::sleep(POLL_INTERVAL);
                }
                return;
            }
            None => {}
        }

        // Open the device for waiting requests, at most once per retry interval.
        let waiting = pending_synth.is_some() || pending_place.is_some();
        if waiting && device.is_none() && Instant::now() >= next_device_attempt {
            device = OutputStream::try_default()
                .map_err(|e| tracing::error!("audio: cannot open output device: {e}"))
                .ok();
            if device.is_none() {
                next_device_attempt = Instant::now() + DEVICE_RETRY_INTERVAL;
            }
            state::lock(state).device_available = device.is_some();
        }

        if let Some((_, handle)) = &device {
            if let Some(preset) = pending_synth.take() {
                let volume = state::lock(state).synth.volume;
                let make_noise = |fade_out| Ok(NoiseSource::new(preset, fade_out));
                synth = start_voice(handle, make_noise, &synth_eq, volume)
                    .map_err(|e| tracing::error!("audio: cannot start {preset} noise: {e:#}"))
                    .ok();
                let mut s = state::lock(state);
                if synth.is_some() {
                    s.synth.play_state = PlayState::Running;
                    s.preset = Some(preset);
                } else {
                    s.synth.play_state = PlayState::Stopped;
                }
            }
            if let Some(location) = pending_place.take() {
                let volume = state::lock(state).place.volume;
                let spawn_mpv = |fade_out| MpvSource::spawn(&location, fade_out);
                place = start_voice(handle, spawn_mpv, &place_eq, volume)
                    .map_err(|e| tracing::error!("audio: cannot play place {location:?}: {e:#}"))
                    .ok();
                let mut s = state::lock(state);
                if place.is_some() {
                    s.place.play_state = PlayState::Running;
                    s.place_location = Some(location);
                } else {
                    s.place.play_state = PlayState::Stopped;
                    s.place_location = None;
                }
            }
        }

        // Drop voices whose source has ended: faded out, or the place stream finished.
        if synth.as_ref().is_some_and(Voice::is_finished) {
            synth = None;
        }
        if place.as_ref().is_some_and(Voice::is_finished) {
            place = None;
            let mut s = state::lock(state);
            if s.place.play_state == PlayState::Running {
                tracing::info!("audio: place stream ended");
            }
            s.place.play_state = PlayState::Stopped;
            s.place_location = None;
        }

        // Release the device once nothing plays or waits for it.
        let nothing_wanted = synth.is_none()
            && place.is_none()
            && pending_synth.is_none()
            && pending_place.is_none();
        if nothing_wanted {
            device = None;
            next_device_attempt = Instant::now();
            state::lock(state).device_available = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(preset: NoisePreset) -> NoiseSource {
        NoiseSource::new(preset, Arc::new(AtomicBool::new(false)))
    }

    /// Advances `src` past its 1.5 s fade-in so it plays at full amplitude.
    fn skip_fade_in(src: &mut NoiseSource) {
        for _ in 0..66_150 {
            src.next().unwrap();
        }
    }

    #[test]
    fn white_noise_statistics() {
        let mut src = source(NoisePreset::White);
        skip_fade_in(&mut src);
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
        let mut src = source(NoisePreset::Pink);
        let samples: Vec<f32> = (0..44_100).map(|_| src.next().unwrap()).collect();
        let mean: f32 = samples.iter().sum::<f32>() / 44_100.0;
        assert!(mean.abs() < 0.1, "pink mean={mean}");
        assert!(
            samples.iter().all(|&s| (-1.1..=1.1).contains(&s)),
            "pink sample out of expected range"
        );
    }

    #[test]
    fn brown_noise_clamped() {
        let mut src = source(NoisePreset::Brown);
        let samples: Vec<f32> = (0..10_000).map(|_| src.next().unwrap()).collect();
        assert!(
            samples.iter().all(|&s| (-1.0..=1.0).contains(&s)),
            "brown noise not clamped to [-1, 1]"
        );
    }

    #[test]
    fn noise_source_fade_out() {
        let fade_out = Arc::new(AtomicBool::new(false));
        let mut src = NoiseSource::new(NoisePreset::White, Arc::clone(&fade_out));
        skip_fade_in(&mut src);

        fade_out.store(true, Ordering::Release);
        let samples: Vec<f32> = std::iter::from_fn(|| src.next()).collect();

        assert_eq!(samples.len(), 66_150, "fade-out should last exactly 1.5 s");

        let start_rms = (samples[..1_000].iter().map(|s| s * s).sum::<f32>() / 1_000.0).sqrt();
        let mid_rms = (samples[32_575..33_575].iter().map(|s| s * s).sum::<f32>() / 1_000.0).sqrt();
        assert!(
            mid_rms < start_rms,
            "fade-out envelope not decreasing: start_rms={start_rms}, mid_rms={mid_rms}"
        );
    }

    #[test]
    fn fade_in_ramps_from_silence() {
        let mut fade = Fade::new(Arc::new(AtomicBool::new(false)));
        let gains: Vec<f32> = (0..66_151).map(|_| fade.next_gain().unwrap()).collect();
        assert!(gains[0].abs() < f32::EPSILON);
        assert!(gains[33_075] > 0.49 && gains[33_075] < 0.51);
        assert!((gains[66_150] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    #[ignore = "prints noise levels; run with `cargo test measure_noise_levels -- --ignored --nocapture`"]
    fn measure_noise_levels() {
        println!();
        println!(
            "{:<8}  {:>10}  {:>10}  {:>12}",
            "Preset", "RMS", "Peak", "Crest Factor"
        );
        println!("{}", "-".repeat(46));
        for preset in NoisePreset::ALL {
            let mut src = source(preset);
            skip_fade_in(&mut src);
            let samples: Vec<f32> = (0..44_100).map(|_| src.next().unwrap()).collect();
            let rms = (samples.iter().map(|&s| s * s).sum::<f32>() / 44_100.0).sqrt();
            let peak = samples.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
            let crest = if rms > 0.0 { peak / rms } else { 0.0 };
            println!(
                "{:<8}  {rms:>10.6}  {peak:>10.6}  {crest:>12.6}",
                preset.label()
            );
        }
        println!();
    }
}

use crate::daemon::eq::N_BANDS;
use crate::daemon::state::{NoisePreset, PlayState};

/// Which screen the TUI is currently showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Preset selection list.
    Presets,
    /// 10-band graphic equalizer.
    Equalizer,
}

/// All mutable state owned by the TUI event loop.
///
/// Playback fields mirror the daemon and are refreshed from `STATUS` and
/// `GET_EQ` every tick; key handlers also update them immediately so repeated
/// key presses within one tick build on each other.
pub struct App {
    pub screen: Screen,
    /// Index into `NoisePreset::ALL` currently highlighted.
    pub selected_preset: usize,
    /// Last preset the daemon played; `s` on the EQ screen resumes it.
    pub active_preset: Option<NoisePreset>,
    pub play_state: PlayState,
    /// Synth volume (0.0–1.0).
    pub volume: f32,
    /// Index of the currently selected EQ band (0..N_BANDS-1).
    pub selected_eq_band: usize,
    /// Per-band EQ gains in dB.
    pub eq_gains: [f32; N_BANDS],
    /// Whether the last sync reached the daemon.
    pub connected: bool,
    /// Whether the daemon has the audio device open (or has not needed it).
    pub device_available: bool,
    pub should_quit: bool,
}

impl App {
    /// Create an `App` with `initial_preset` highlighted on the Presets screen.
    #[must_use]
    pub fn new(initial_preset: NoisePreset) -> Self {
        Self {
            screen: Screen::Presets,
            selected_preset: NoisePreset::ALL
                .iter()
                .position(|&preset| preset == initial_preset)
                .unwrap_or(0),
            active_preset: None,
            play_state: PlayState::Stopped,
            volume: 0.0,
            selected_eq_band: 0,
            eq_gains: [0.0; N_BANDS],
            connected: false,
            device_available: true,
            should_quit: false,
        }
    }
}

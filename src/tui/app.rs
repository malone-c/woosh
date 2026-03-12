use crate::daemon::eq::N_BANDS;
use crate::daemon::state::{NoisePreset, PlayState};

/// Which screen the TUI is currently showing.
pub enum Screen {
    /// Preset selection list.
    Presets,
    /// 10-band graphic equalizer.
    Equalizer,
}

/// All mutable state owned by the TUI event loop.
pub struct App {
    pub screen: Screen,
    pub preset_list: [NoisePreset; 3],
    /// Index into `preset_list` currently highlighted.
    pub selected_preset: usize,
    /// The preset that is actively playing (set after a successful PLAY command).
    pub active_preset: Option<NoisePreset>,
    pub play_state: PlayState,
    /// Volume (0.0–1.0); updated optimistically on ← / →.
    pub volume: f32,
    /// Index of the currently selected EQ band (0..N_BANDS-1).
    pub selected_eq_band: usize,
    /// Per-band EQ gains in dB, mirrors daemon state.
    pub eq_gains: [f32; N_BANDS],
    pub should_quit: bool,
}

impl App {
    /// Create a new `App` with default volume.
    #[must_use]
    pub fn new(volume: f32) -> Self {
        Self {
            screen: Screen::Presets,
            preset_list: [NoisePreset::White, NoisePreset::Pink, NoisePreset::Brown],
            selected_preset: 0,
            active_preset: None,
            play_state: PlayState::Stopped,
            volume,
            selected_eq_band: 0,
            eq_gains: [0.0f32; N_BANDS],
            should_quit: false,
        }
    }
}

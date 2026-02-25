use crate::daemon::state::{NoisePreset, PlayState};

/// Which screen the TUI is currently showing.
pub enum Screen {
    /// Preset selection list.
    Presets,
    /// Live spectrum visualizer.
    Visualizer,
}

/// Number of spectrum bars.
pub const NUM_BARS: usize = 24;

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
    /// Heights of the 24 spectrum bars (0–100).
    pub bar_heights: [u64; NUM_BARS],
    /// Sliding window of recent samples, capped at 4096.
    pub sample_window: Vec<f32>,
    /// Sample rate from config (used for FFT bin mapping).
    pub sample_rate: u32,
    pub should_quit: bool,
}

impl App {
    /// Create a new `App` with the given sample rate.
    #[must_use]
    pub fn new(sample_rate: u32, volume: f32) -> Self {
        Self {
            screen: Screen::Presets,
            preset_list: [NoisePreset::White, NoisePreset::Pink, NoisePreset::Brown],
            selected_preset: 0,
            active_preset: None,
            play_state: PlayState::Stopped,
            volume,
            bar_heights: [0; NUM_BARS],
            sample_window: Vec::with_capacity(4_096),
            sample_rate,
            should_quit: false,
        }
    }
}

use crate::daemon::eq::N_BANDS;
use std::fmt;
use std::str::FromStr;

/// The noise preset (sound profile) the daemon is playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoisePreset {
    White,
    Pink,
    Brown,
}

impl fmt::Display for NoisePreset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::White => write!(f, "white"),
            Self::Pink => write!(f, "pink"),
            Self::Brown => write!(f, "brown"),
        }
    }
}

impl FromStr for NoisePreset {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "white" => Ok(Self::White),
            "pink" => Ok(Self::Pink),
            "brown" => Ok(Self::Brown),
            other => Err(anyhow::anyhow!("unknown preset: {other}")),
        }
    }
}

/// Whether the daemon is currently playing audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    Running,
    Stopped,
}

impl fmt::Display for PlayState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::Stopped => write!(f, "stopped"),
        }
    }
}

/// Shared daemon state, protected by a Mutex.
#[derive(Debug)]
pub struct DaemonState {
    pub play_state: PlayState,
    pub preset: NoisePreset,
    pub volume: f32,
    /// Per-band EQ gains in dB (−12..+12). Index 0 = 31 Hz … 9 = 16 kHz.
    pub eq_gains: [f32; N_BANDS],
    /// Place channel playback state.
    pub place_state: PlayState,
    /// Currently selected place location (if any).
    pub place_location: Option<String>,
    /// Place channel volume (0.0–1.0).
    pub place_volume: f32,
    /// Per-band place EQ gains in dB (−12..+12).
    pub place_eq_gains: [f32; N_BANDS],
}

impl Default for DaemonState {
    fn default() -> Self {
        Self {
            play_state: PlayState::Running,
            preset: NoisePreset::White,
            volume: 0.8,
            eq_gains: [0.0f32; N_BANDS],
            place_state: PlayState::Stopped,
            place_location: None,
            place_volume: 0.4,
            place_eq_gains: [0.0f32; N_BANDS],
        }
    }
}

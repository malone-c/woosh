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
}

impl Default for DaemonState {
    fn default() -> Self {
        Self {
            play_state: PlayState::Running,
            preset: NoisePreset::White,
            volume: 0.8,
        }
    }
}

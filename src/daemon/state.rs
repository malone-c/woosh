use crate::daemon::eq::N_BANDS;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// The noise preset (sound profile) the synth channel plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoisePreset {
    White,
    Pink,
    Brown,
}

impl NoisePreset {
    /// Every preset, in the order the TUI lists them.
    pub const ALL: [Self; 3] = [Self::White, Self::Pink, Self::Brown];

    /// Name used by the IPC protocol, the CLI and the config file.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::White => "white",
            Self::Pink => "pink",
            Self::Brown => "brown",
        }
    }

    /// Capitalised name for display.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::White => "White",
            Self::Pink => "Pink",
            Self::Brown => "Brown",
        }
    }
}

impl fmt::Display for NoisePreset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for NoisePreset {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.id() == s)
            .ok_or_else(|| anyhow::anyhow!("unknown preset: {s}"))
    }
}

/// Whether a channel is playing.
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

/// One of the two independent audio channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelId {
    /// Synthetic noise.
    Synth,
    /// `YouTube` place sound streamed through mpv.
    Place,
}

/// Playback settings that both channels have.
#[derive(Debug, Clone, Copy)]
pub struct Channel {
    pub play_state: PlayState,
    /// Volume (0.0–1.0).
    pub volume: f32,
    /// Per-band EQ gains in dB (−12..+12). Index 0 = 31 Hz … 9 = 16 kHz.
    pub eq_gains: [f32; N_BANDS],
}

/// Shared daemon state, protected by a Mutex.
///
/// The IPC handler records each request here as it accepts it; the audio
/// thread then records the outcome (started, failed, or stream ended).
#[derive(Debug)]
pub struct DaemonState {
    pub synth: Channel,
    /// Last preset played on the synth channel; kept after `STOP` so it can resume.
    pub preset: Option<NoisePreset>,
    pub place: Channel,
    /// Location the place channel is playing.
    pub place_location: Option<String>,
    /// `false` while the audio device cannot be opened and the daemon is retrying.
    pub device_available: bool,
}

impl DaemonState {
    pub fn channel_mut(&mut self, id: ChannelId) -> &mut Channel {
        match id {
            ChannelId::Synth => &mut self.synth,
            ChannelId::Place => &mut self.place,
        }
    }
}

impl Default for DaemonState {
    fn default() -> Self {
        Self {
            synth: Channel {
                play_state: PlayState::Stopped,
                volume: 0.8,
                eq_gains: [0.0; N_BANDS],
            },
            preset: None,
            place: Channel {
                play_state: PlayState::Stopped,
                volume: 0.4,
                eq_gains: [0.0; N_BANDS],
            },
            place_location: None,
            device_available: true,
        }
    }
}

/// Locks the shared state, recovering it if a thread panicked while holding the lock.
pub fn lock(state: &Mutex<DaemonState>) -> MutexGuard<'_, DaemonState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// User-configurable defaults applied when the daemon first starts.
#[derive(Debug, Serialize, Deserialize)]
pub struct DefaultsConfig {
    /// Which noise preset to play on startup (`"white"`, `"pink"`, `"brown"`).
    pub preset: String,
    /// Initial volume (0.0–1.0).
    pub volume: f32,
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            preset: "white".to_owned(),
            volume: 0.8,
        }
    }
}

/// Audio-engine settings.
#[derive(Debug, Serialize, Deserialize)]
pub struct AudioConfig {
    /// Sample rate passed to the noise source (Hz).
    pub sample_rate: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: 44_100,
        }
    }
}

/// Daemon lifecycle settings.
#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// Minutes of inactivity (no clients connected and both channels stopped)
    /// before the daemon automatically shuts down. Set to `0` to disable.
    pub idle_timeout_mins: u64,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            idle_timeout_mins: 15,
        }
    }
}

/// Top-level configuration struct.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    /// Default playback settings.
    pub defaults: DefaultsConfig,
    /// Audio engine settings.
    pub audio: AudioConfig,
    /// Daemon lifecycle settings.
    pub daemon: DaemonConfig,
}

/// Returns the path to the config file (`~/.config/woosh/config.toml`).
///
/// # Errors
/// Returns an error if the XDG config directory cannot be determined.
pub fn config_path() -> Result<PathBuf> {
    let base = dirs::config_dir().context("cannot determine config directory")?;
    Ok(base.join("woosh").join("config.toml"))
}

/// Load the configuration from disk, writing defaults if the file is absent.
///
/// # Errors
/// Returns an error if the file cannot be created, read, or parsed.
pub fn load() -> Result<Config> {
    let path = config_path()?;
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config)?;
        std::fs::write(&path, toml_str)?;
        return Ok(config);
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read config: {}", path.display()))?;
    let config: Config = toml::from_str(&content)
        .with_context(|| format!("cannot parse config: {}", path.display()))?;
    Ok(config)
}

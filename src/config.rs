use crate::daemon::state::NoisePreset;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Playback defaults.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DefaultsConfig {
    /// Preset highlighted when the TUI opens.
    pub preset: NoisePreset,
    /// Synth volume (0.0–1.0) the daemon starts with.
    pub volume: f32,
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            preset: NoisePreset::Pink,
            volume: 0.8,
        }
    }
}

/// Daemon lifecycle settings.
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
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

/// Top-level configuration. Missing sections and keys take their defaults.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub defaults: DefaultsConfig,
    pub daemon: DaemonConfig,
}

/// Returns the path to the config file: `~/Library/Application Support/woosh/config.toml`
/// on macOS, `${XDG_CONFIG_HOME:-~/.config}/woosh/config.toml` on Linux.
///
/// # Errors
/// Returns an error if the config directory cannot be determined.
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

#[cfg(test)]
mod tests {
    use super::Config;
    use crate::daemon::state::NoisePreset;

    #[test]
    fn missing_sections_and_keys_use_defaults() {
        let config: Config = toml::from_str("[defaults]\nvolume = 0.3\n").unwrap();
        assert!((config.defaults.volume - 0.3).abs() < f32::EPSILON);
        assert_eq!(config.defaults.preset, NoisePreset::Pink);
        assert_eq!(config.daemon.idle_timeout_mins, 15);
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let config: Config = toml::from_str("[audio]\nsample_rate = 44100\n").unwrap();
        assert_eq!(config.daemon.idle_timeout_mins, 15);
    }
}

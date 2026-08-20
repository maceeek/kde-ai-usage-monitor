//! Persisted settings, shared by the CLI and the Plasma applet.
//!
//! The applet writes this file so that running the binary by hand shows the
//! same providers the widget is watching. Everything here has a default, so a
//! missing or malformed file is never fatal.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::diagnose;
use crate::providers::{ProviderId, ProviderSet};

pub const DEFAULT_INTERVAL_SECS: u64 = 300;
pub const MIN_INTERVAL_SECS: u64 = 30;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Provider keys to poll.
    pub providers: Vec<String>,
    /// Seconds between polls in `--watch` mode.
    pub interval_secs: u64,
    /// Allow the poller to shell out to the provider's CLI to refresh an
    /// expired token. Off by default: it spawns a process the user did not ask
    /// for, which is a surprise coming from a panel widget.
    pub refresh_tokens: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            providers: ProviderSet::default()
                .iter()
                .map(|provider| provider.key().to_string())
                .collect(),
            interval_secs: DEFAULT_INTERVAL_SECS,
            refresh_tokens: false,
        }
    }
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        Some(
            dirs::config_dir()?
                .join("kde-ai-usage-monitor")
                .join("config.json"),
        )
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        let Ok(content) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str(&content) {
            Ok(config) => config,
            Err(error) => {
                diagnose::log_error(
                    &format!("ignoring unreadable config at {}", path.display()),
                    error,
                );
                Self::default()
            }
        }
    }

    pub fn save(&self) -> std::io::Result<PathBuf> {
        let path = Self::path().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no XDG config directory available",
            )
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, body)?;
        Ok(path)
    }

    /// Enabled providers, falling back to the default set when the file lists
    /// nothing this build understands.
    pub fn provider_set(&self) -> ProviderSet {
        let providers = ProviderSet::from_enabled(
            self.providers
                .iter()
                .filter_map(|key| ProviderId::from_key(key)),
        );
        if providers.is_empty() {
            ProviderSet::default()
        } else {
            providers
        }
    }

    pub fn interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.interval_secs.max(MIN_INTERVAL_SECS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_to_the_default_provider_set() {
        let config = Config::default();
        assert_eq!(config.provider_set(), ProviderSet::default());
        assert_eq!(config.interval(), std::time::Duration::from_secs(300));
    }

    #[test]
    fn unknown_provider_keys_fall_back_to_the_default_set() {
        let config = Config {
            providers: vec!["typewriter".into()],
            ..Default::default()
        };
        assert_eq!(config.provider_set(), ProviderSet::default());
    }

    #[test]
    fn partial_config_files_keep_the_remaining_defaults() {
        let config: Config = serde_json::from_str(r#"{"providers":["codex","cursor"]}"#).unwrap();
        assert_eq!(
            config.provider_set(),
            ProviderSet::from_enabled([ProviderId::Codex, ProviderId::Cursor])
        );
        assert_eq!(config.interval_secs, DEFAULT_INTERVAL_SECS);
    }

    #[test]
    fn polling_intervals_never_drop_below_the_floor() {
        let config = Config {
            interval_secs: 1,
            ..Default::default()
        };
        assert_eq!(
            config.interval(),
            std::time::Duration::from_secs(MIN_INTERVAL_SECS)
        );
    }
}

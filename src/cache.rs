//! Last-known usage, so a failed poll shows stale numbers instead of a blank
//! widget.
//!
//! Upstream keeps this in its settings file next to the window geometry. A CLI
//! that the applet re-runs every few minutes has no long-lived process to hold
//! it in, so it lands in the XDG cache directory.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::diagnose;
use crate::models::UsageData;
use crate::providers::ProviderId;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CachedUsage {
    pub usage: UsageData,
    pub polled_at: i64,
}

/// Keyed by provider. Back-offs ride along under a key no provider can have,
/// so caches written by 0.1 still load.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cache {
    #[serde(flatten)]
    entries: BTreeMap<String, CachedUsage>,
    /// Unix second until which a provider asked not to be polled.
    #[serde(
        default,
        rename = "_retry_at",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    retry_at: BTreeMap<String, i64>,
}

impl Cache {
    pub fn path() -> Option<PathBuf> {
        Some(
            dirs::cache_dir()?
                .join("kde-ai-usage-monitor")
                .join("usage.json"),
        )
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        std::fs::read_to_string(path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_default()
    }

    pub fn get(&self, provider: ProviderId) -> Option<&CachedUsage> {
        self.entries.get(provider.key())
    }

    pub fn insert(&mut self, provider: ProviderId, usage: UsageData, polled_at: i64) {
        self.entries
            .insert(provider.key().to_string(), CachedUsage { usage, polled_at });
        self.retry_at.remove(provider.key());
    }

    /// Seconds a provider still has to wait after asking us to back off.
    pub fn cooldown(&self, provider: ProviderId, now: i64) -> Option<i64> {
        self.retry_at
            .get(provider.key())
            .map(|retry_at| retry_at - now)
            .filter(|remaining| *remaining > 0)
    }

    pub fn retry_at(&self, provider: ProviderId) -> Option<i64> {
        self.retry_at.get(provider.key()).copied()
    }

    pub fn back_off(&mut self, provider: ProviderId, retry_at: i64) {
        self.retry_at.insert(provider.key().to_string(), retry_at);
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        let result = path
            .parent()
            .map(std::fs::create_dir_all)
            .unwrap_or(Ok(()))
            .and_then(|()| serde_json::to_string(self).map_err(std::io::Error::from))
            .and_then(|body| std::fs::write(&path, body));
        if let Err(error) = result {
            // A read-only cache directory is not worth failing a poll over.
            diagnose::log_error("unable to write the usage cache", error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::UsageSection;

    #[test]
    fn cached_entries_round_trip_by_provider_key() {
        let mut cache = Cache::default();
        cache.insert(
            ProviderId::Codex,
            UsageData {
                session: UsageSection::new(33.0, Some(1_000)),
                ..Default::default()
            },
            900,
        );

        let json = serde_json::to_string(&cache).unwrap();
        assert!(json.contains("\"codex\""));

        let decoded: Cache = serde_json::from_str(&json).unwrap();
        let entry = decoded.get(ProviderId::Codex).unwrap();
        assert_eq!(entry.polled_at, 900);
        assert_eq!(entry.usage.session.percentage, 33.0);
        assert!(decoded.get(ProviderId::Claude).is_none());
    }

    #[test]
    fn back_offs_survive_a_round_trip_and_clear_on_success() {
        let mut cache = Cache::default();
        cache.back_off(ProviderId::Claude, 1_000);
        let mut decoded: Cache =
            serde_json::from_str(&serde_json::to_string(&cache).unwrap()).unwrap();
        assert_eq!(decoded.cooldown(ProviderId::Claude, 900), Some(100));
        assert_eq!(decoded.cooldown(ProviderId::Claude, 1_000), None);
        decoded.insert(ProviderId::Claude, UsageData::default(), 950);
        assert_eq!(decoded.cooldown(ProviderId::Claude, 900), None);
    }

    #[test]
    fn caches_from_older_versions_still_load() {
        let decoded: Cache = serde_json::from_str(
            r#"{"claude":{"usage":{"session":{"percentage":12.0,"resets_at":5},"weekly":{"percentage":1.0}},"polled_at":3}}"#,
        )
        .unwrap();
        let entry = decoded.get(ProviderId::Claude).unwrap();
        assert!(entry.usage.session.available);
        assert_eq!(entry.usage.session.percentage, 12.0);
    }
}

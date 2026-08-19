//! Stable provider identities shared by the poller, the CLI output, and the
//! Plasma applet.
//!
//! Ported from the upstream Windows monitor
//! (CodeZeno/Claude-Code-Usage-Monitor, `src/providers.rs`). The Win32 menu
//! command ids are gone; the applet addresses providers by their string key
//! instead.

use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum ProviderId {
    #[default]
    Claude = 0,
    Codex = 1,
    Antigravity = 2,
    OpenCode = 3,
    Cursor = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub id: ProviderId,
    /// Stable key used by the CLI, the cache, and the applet configuration.
    pub key: &'static str,
    pub display_name: &'static str,
    /// Short description shown in the applet's provider list.
    pub description: &'static str,
    /// Label for the short (rolling) usage window.
    pub session_label: &'static str,
    /// Label for the long window when the provider does not report one.
    pub weekly_label: &'static str,
    pub default_enabled: bool,
}

pub const PROVIDER_DESCRIPTORS: [ProviderDescriptor; 5] = [
    ProviderDescriptor {
        id: ProviderId::Claude,
        key: "claude",
        display_name: "Claude Code",
        description: "Collect usage from Anthropic",
        session_label: "5h",
        weekly_label: "7d",
        default_enabled: true,
    },
    ProviderDescriptor {
        id: ProviderId::Codex,
        key: "codex",
        display_name: "Codex",
        description: "Collect usage from OpenAI",
        session_label: "5h",
        weekly_label: "7d",
        default_enabled: false,
    },
    ProviderDescriptor {
        id: ProviderId::Antigravity,
        key: "antigravity",
        display_name: "Antigravity",
        description: "Collect usage from Google",
        session_label: "5h",
        weekly_label: "7d",
        default_enabled: false,
    },
    ProviderDescriptor {
        id: ProviderId::OpenCode,
        key: "opencode",
        display_name: "OpenCode",
        description: "Collect usage from OpenCode Go",
        session_label: "rolling",
        weekly_label: "7d",
        default_enabled: false,
    },
    ProviderDescriptor {
        id: ProviderId::Cursor,
        key: "cursor",
        display_name: "Cursor",
        description: "Collect usage from Cursor",
        session_label: "Auto",
        weekly_label: "API",
        default_enabled: false,
    },
];

impl ProviderId {
    pub const ALL: [Self; 5] = [
        Self::Claude,
        Self::Codex,
        Self::Antigravity,
        Self::OpenCode,
        Self::Cursor,
    ];

    pub const fn descriptor(self) -> &'static ProviderDescriptor {
        &PROVIDER_DESCRIPTORS[self as usize]
    }

    pub fn key(self) -> &'static str {
        self.descriptor().key
    }

    pub fn from_key(key: &str) -> Option<Self> {
        PROVIDER_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.key.eq_ignore_ascii_case(key))
            .map(|descriptor| descriptor.id)
    }
}

/// Compact, copyable selection passed between the CLI, the config file, and
/// the poll loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderSet(u64);

impl ProviderSet {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub fn from_enabled(enabled: impl IntoIterator<Item = ProviderId>) -> Self {
        let mut providers = Self::empty();
        for provider in enabled {
            providers.set(provider, true);
        }
        providers
    }

    pub const fn contains(self, provider: ProviderId) -> bool {
        self.0 & provider.bit() != 0
    }

    pub fn set(&mut self, provider: ProviderId, enabled: bool) {
        if enabled {
            self.0 |= provider.bit();
        } else {
            self.0 &= !provider.bit();
        }
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = ProviderId> {
        ProviderId::ALL
            .into_iter()
            .filter(move |provider| self.contains(*provider))
    }
}

impl Default for ProviderSet {
    fn default() -> Self {
        Self::from_enabled(
            PROVIDER_DESCRIPTORS
                .iter()
                .filter(|descriptor| descriptor.default_enabled)
                .map(|descriptor| descriptor.id),
        )
    }
}

impl ProviderId {
    const fn bit(self) -> u64 {
        1 << self as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_provider_set_comes_from_descriptors() {
        assert_eq!(
            ProviderSet::default(),
            ProviderSet::from_enabled([ProviderId::Claude])
        );
    }

    #[test]
    fn provider_keys_round_trip_through_the_registry() {
        for descriptor in PROVIDER_DESCRIPTORS {
            assert_eq!(ProviderId::from_key(descriptor.key), Some(descriptor.id));
            assert_eq!(descriptor.id.descriptor().key, descriptor.key);
        }
        assert_eq!(ProviderId::from_key("CLAUDE"), Some(ProviderId::Claude));
        assert_eq!(ProviderId::from_key("nope"), None);
    }

    #[test]
    fn provider_sets_track_membership() {
        let mut providers = ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Cursor]);
        assert_eq!(providers.iter().count(), 2);
        assert!(providers.contains(ProviderId::Cursor));
        providers.set(ProviderId::Cursor, false);
        assert_eq!(
            providers.iter().collect::<Vec<_>>(),
            vec![ProviderId::Claude]
        );
    }
}

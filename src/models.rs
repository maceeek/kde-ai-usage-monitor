//! Usage values shared by every provider.
//!
//! Ported from the upstream Windows monitor (`src/models.rs`). Reset instants
//! are kept as Unix seconds rather than `SystemTime`: everything downstream of
//! this crate is JSON, and the applet wants a plain number.

use serde::{Deserialize, Serialize};

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageSection {
    /// The provider reported this window, even if it is idle and carries no
    /// reset instant. A window the provider never mentioned stays `false` and
    /// is left off the snapshot rather than drawn as a misleading 0%.
    ///
    /// Caches written before this field existed only ever held windows that
    /// were shown, hence the default.
    #[serde(default = "default_available")]
    pub available: bool,
    /// Percentage of the window consumed, 0..=100.
    pub percentage: f64,
    /// When the window rolls over, as Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
}

impl UsageSection {
    pub fn new(percentage: f64, resets_at: Option<i64>) -> Self {
        Self {
            available: true,
            percentage: if percentage.is_finite() {
                percentage.clamp(0.0, 100.0)
            } else {
                0.0
            },
            resets_at,
        }
    }

    /// Seconds until the window resets, or `None` once it has passed.
    pub fn resets_in(&self, now: i64) -> Option<i64> {
        self.resets_at
            .map(|resets_at| resets_at - now)
            .filter(|remaining| *remaining > 0)
    }
}

fn default_available() -> bool {
    true
}

/// A quota beyond the two headline windows — Claude's per-model weekly caps,
/// Copilot's metered chat allowance, and whatever else a provider reports.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageLimit {
    /// Stable identifier, derived from the provider's own naming.
    pub key: String,
    /// What the applet prints next to the bar, e.g. `Opus` or `Chat`.
    pub label: String,
    /// The provider says this quota is the one currently constraining usage.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub active: bool,
    pub usage: UsageSection,
}

/// Paid usage beyond the plan's allowance: Claude's extra usage, Codex
/// credits. Amounts are in the account's currency (US dollars in practice).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Credits {
    /// Spent so far in the current billing period, when the provider says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    /// The spending cap, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    /// What is left to spend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<f64>,
}

impl Credits {
    /// Share of the cap already spent, when both figures are known.
    pub fn percentage(&self) -> Option<f64> {
        let limit = self
            .limit
            .filter(|limit| limit.is_finite() && *limit > 0.0)?;
        let used = self
            .used
            .or_else(|| self.remaining.map(|remaining| limit - remaining))?;
        used.is_finite()
            .then(|| (used / limit * 100.0).clamp(0.0, 100.0))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageData {
    /// Short rolling window (five hours for most providers).
    pub session: UsageSection,
    /// Long window (a week for most providers).
    pub weekly: UsageSection,
    /// Provider-supplied override for the short window's label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_label: Option<String>,
    /// Provider-supplied override for the long window's label — OpenCode
    /// reports either `7d` or `30d` depending on which window binds first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekly_label: Option<String>,
    /// A monthly allowance reported alongside the two windows — Codex's
    /// spend-control limit, OpenCode's 30-day window, Copilot's premium pool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly: Option<UsageSection>,
    /// Further quotas, shown in the popup and the tooltip but never in the
    /// panel.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limits: Vec<UsageLimit>,
    /// Paid usage past the allowance, when the account has it switched on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<Credits>,
    /// Subscription name as the provider reports it, e.g. `max` or `plus`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

impl UsageData {
    /// True once either window's reset instant has passed, which means the
    /// numbers on screen are describing a window that no longer exists.
    pub fn is_past_reset(&self, now: i64) -> bool {
        let past = |section: &UsageSection| {
            section.available && section.resets_at.is_some_and(|reset| reset <= now)
        };
        past(&self.session) || past(&self.weekly)
    }

    /// Every window the provider reported, headline ones first.
    pub fn sections(&self) -> impl Iterator<Item = &UsageSection> {
        [&self.session, &self.weekly]
            .into_iter()
            .chain(self.monthly.iter())
            .chain(self.limits.iter().map(|limit| &limit.usage))
            .filter(|section| section.available)
    }
}

/// Lower-case, underscore-separated identifier for a provider-supplied name.
pub fn slug(value: &str) -> String {
    let mut result = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            result.push(character.to_ascii_lowercase());
        } else if !result.is_empty() && !result.ends_with('_') {
            result.push('_');
        }
    }
    let result = result.trim_end_matches('_');
    if result.is_empty() {
        "unnamed".to_string()
    } else {
        result.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_report_time_left_until_reset() {
        let section = UsageSection::new(10.0, Some(1_000));
        assert_eq!(section.resets_in(900), Some(100));
        assert_eq!(section.resets_in(1_000), None);

        let data = UsageData {
            session: section,
            ..Default::default()
        };
        assert!(data.is_past_reset(1_001));
        assert!(!data.is_past_reset(999));
    }

    #[test]
    fn section_percentages_are_clamped() {
        assert_eq!(UsageSection::new(140.0, None).percentage, 100.0);
        assert_eq!(UsageSection::new(-3.0, None).percentage, 0.0);
        assert_eq!(UsageSection::new(f64::NAN, None).percentage, 0.0);
    }

    #[test]
    fn unreported_windows_neither_count_nor_expire() {
        let data = UsageData {
            weekly: UsageSection::new(10.0, Some(1_000)),
            session: UsageSection {
                resets_at: Some(1),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(data.sections().count(), 1);
        assert!(!data.is_past_reset(500));
    }

    #[test]
    fn sections_from_older_caches_count_as_reported() {
        let section: UsageSection = serde_json::from_str(r#"{"percentage":4.0}"#).unwrap();
        assert!(section.available);
    }

    #[test]
    fn credit_percentages_come_from_whichever_figures_exist() {
        let spent = Credits {
            used: Some(5.0),
            limit: Some(20.0),
            remaining: None,
        };
        assert_eq!(spent.percentage(), Some(25.0));
        let balance = Credits {
            used: None,
            limit: Some(20.0),
            remaining: Some(5.0),
        };
        assert_eq!(balance.percentage(), Some(75.0));
        assert_eq!(Credits::default().percentage(), None);
    }

    #[test]
    fn slugs_are_stable_identifiers() {
        assert_eq!(slug("Seven Day / Opus"), "seven_day_opus");
        assert_eq!(slug("??"), "unnamed");
    }
}

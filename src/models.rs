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
    /// Percentage of the window consumed, 0..=100.
    pub percentage: f64,
    /// When the window rolls over, as Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
}

impl UsageSection {
    pub fn new(percentage: f64, resets_at: Option<i64>) -> Self {
        Self {
            percentage: percentage.clamp(0.0, 100.0),
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
}

impl UsageData {
    /// True once either window's reset instant has passed, which means the
    /// numbers on screen are describing a window that no longer exists.
    pub fn is_past_reset(&self, now: i64) -> bool {
        let past = |section: &UsageSection| section.resets_at.is_some_and(|reset| reset <= now);
        past(&self.session) || past(&self.weekly)
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
    }
}

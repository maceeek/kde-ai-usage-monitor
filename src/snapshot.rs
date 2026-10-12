//! The JSON document the Plasma applet reads.
//!
//! This is the whole contract between the Rust side and the QML side, so it is
//! versioned: `schema` only ever goes up, and fields are added rather than
//! repurposed.

use serde::{Deserialize, Serialize};

use crate::cache::Cache;
use crate::format::{format_percentage, humanize_duration};
use crate::models::{now_unix, Credits, UsageData, UsageSection};
use crate::poller::{PollError, ProviderPoll};
use crate::providers::ProviderId;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema: u32,
    pub version: String,
    pub generated_at: i64,
    /// Highest usage across every provider that answered — what the compact
    /// panel representation shows when it has room for exactly one number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
    pub providers: Vec<ProviderSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub provider: String,
    pub name: String,
    pub label: String,
    pub percentage: f64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderSnapshot {
    pub id: String,
    pub name: String,
    /// `ok`, or the `PollError` variant that ended the poll.
    pub state: String,
    pub ok: bool,
    /// True when the numbers come from the cache because this poll failed.
    pub stale: bool,
    /// True when a reset instant has already passed, so the window on screen no
    /// longer exists and the next poll will show different numbers.
    pub past_reset: bool,
    pub polled_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// When a rate-limited provider will next be asked, as Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<i64>,
    /// Subscription name as the provider reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// The short window, absent when the provider has none (Copilot) or did
    /// not report it this time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<SectionSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weekly: Option<SectionSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly: Option<SectionSnapshot>,
    /// Per-model and other secondary quotas, for the popup and tooltip.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limits: Vec<LimitSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<CreditsSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LimitSnapshot {
    pub key: String,
    /// The provider says this is the quota currently holding usage back.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub active: bool,
    #[serde(flatten)]
    pub section: SectionSnapshot,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreditsSnapshot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<f64>,
    /// Share of the cap spent, when there is a cap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percentage: Option<f64>,
    /// `$12.50 of $50.00` or `$10.00 left`.
    pub text: String,
}

impl CreditsSnapshot {
    fn new(credits: &Credits) -> Self {
        let money = |amount: f64| format!("${amount:.2}");
        let text = match (credits.used, credits.limit, credits.remaining) {
            (Some(used), Some(limit), _) => format!("{} of {}", money(used), money(limit)),
            (_, _, Some(remaining)) => format!("{} left", money(remaining)),
            (Some(used), None, None) => format!("{} spent", money(used)),
            _ => String::new(),
        };
        Self {
            used: credits.used,
            limit: credits.limit,
            remaining: credits.remaining,
            percentage: credits.percentage(),
            text,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SectionSnapshot {
    pub label: String,
    pub percentage: f64,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_in: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_in_text: Option<String>,
}

impl SectionSnapshot {
    fn new(section: &UsageSection, label: &str, now: i64) -> Self {
        let resets_in = section.resets_in(now);
        Self {
            label: label.to_string(),
            percentage: section.percentage,
            text: format_percentage(section.percentage),
            resets_at: section.resets_at,
            resets_in,
            resets_in_text: resets_in.map(humanize_duration),
        }
    }
}

impl Snapshot {
    /// Fold this run's poll results together with the cache, so a provider that
    /// failed still shows its last known numbers, marked stale.
    pub fn build(polls: &[ProviderPoll], cache: &mut Cache) -> Self {
        let now = now_unix();
        let providers: Vec<ProviderSnapshot> = polls
            .iter()
            .map(|poll| match &poll.result {
                Ok(usage) => {
                    cache.insert(poll.provider, usage.clone(), now);
                    ProviderSnapshot::ok(poll.provider, usage, now)
                }
                Err(error) => {
                    if let Some(retry_after) = error.retry_after() {
                        cache.back_off(poll.provider, now + retry_after);
                    }
                    ProviderSnapshot::failed(poll.provider, *error, cache, now)
                }
            })
            .collect();

        Self {
            schema: SCHEMA_VERSION,
            version: env!("CARGO_PKG_VERSION").to_string(),
            generated_at: now,
            summary: summarize(&providers),
            providers,
        }
    }
}

impl ProviderSnapshot {
    fn ok(provider: ProviderId, usage: &UsageData, now: i64) -> Self {
        Self {
            id: provider.key().to_string(),
            name: provider.descriptor().display_name.to_string(),
            state: "ok".to_string(),
            ok: true,
            stale: false,
            past_reset: usage.is_past_reset(now),
            polled_at: now,
            message: None,
            retry_at: None,
            ..Self::empty(provider)
        }
        .with_usage(provider, usage, now)
    }

    fn failed(provider: ProviderId, error: PollError, cache: &Cache, now: i64) -> Self {
        let cached = cache.get(provider);
        let snapshot = Self {
            id: provider.key().to_string(),
            name: provider.descriptor().display_name.to_string(),
            state: error.key().to_string(),
            ok: false,
            stale: cached.is_some(),
            past_reset: cached.is_some_and(|cached| cached.usage.is_past_reset(now)),
            polled_at: cached.map(|cached| cached.polled_at).unwrap_or(now),
            message: Some(error.hint(provider)),
            retry_at: cache.retry_at(provider).filter(|retry_at| *retry_at > now),
            ..Self::empty(provider)
        };
        match cached {
            Some(cached) => snapshot.with_usage(provider, &cached.usage, now),
            None => snapshot,
        }
    }

    fn empty(provider: ProviderId) -> Self {
        Self {
            id: provider.key().to_string(),
            name: provider.descriptor().display_name.to_string(),
            state: String::new(),
            ok: false,
            stale: false,
            past_reset: false,
            polled_at: 0,
            message: None,
            retry_at: None,
            plan: None,
            session: None,
            weekly: None,
            monthly: None,
            limits: Vec::new(),
            credits: None,
        }
    }

    /// Fill in every window the provider reported, letting it override the
    /// labels its descriptor supplies (OpenCode reports 7d or 30d depending on
    /// the account).
    fn with_usage(mut self, provider: ProviderId, usage: &UsageData, now: i64) -> Self {
        let descriptor = provider.descriptor();
        let section = |section: &UsageSection, label: &str| {
            section
                .available
                .then(|| SectionSnapshot::new(section, label, now))
        };
        self.session = section(
            &usage.session,
            usage
                .session_label
                .as_deref()
                .unwrap_or(descriptor.session_label),
        );
        self.weekly = section(
            &usage.weekly,
            usage
                .weekly_label
                .as_deref()
                .unwrap_or(descriptor.weekly_label),
        );
        self.monthly = usage
            .monthly
            .as_ref()
            .and_then(|monthly| section(monthly, "30d"));
        self.limits = usage
            .limits
            .iter()
            .filter(|limit| limit.usage.available)
            .map(|limit| LimitSnapshot {
                key: limit.key.clone(),
                active: limit.active,
                section: SectionSnapshot::new(&limit.usage, &limit.label, now),
            })
            .collect();
        self.credits = usage.credits.as_ref().map(CreditsSnapshot::new);
        self.plan = usage.plan.clone();
        self
    }
}

/// The number the panel shows: whichever window, across every provider that
/// answered, is closest to running out.
fn summarize(providers: &[ProviderSnapshot]) -> Option<Summary> {
    providers
        .iter()
        .filter(|provider| !provider.stale)
        .flat_map(|provider| {
            [&provider.session, &provider.weekly, &provider.monthly]
                .into_iter()
                .flatten()
                .map(move |section| (provider, section))
        })
        .max_by(|(_, left), (_, right)| {
            left.percentage
                .partial_cmp(&right.percentage)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(provider, section)| Summary {
            provider: provider.id.clone(),
            name: provider.name.clone(),
            label: section.label.clone(),
            percentage: section.percentage,
            text: section.text.clone(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::UsageSection;

    fn usage(session: f64, weekly: f64) -> UsageData {
        UsageData {
            session: UsageSection::new(session, Some(now_unix() + 3_600)),
            weekly: UsageSection::new(weekly, Some(now_unix() + 86_400)),
            ..Default::default()
        }
    }

    #[test]
    fn successful_polls_land_in_the_cache_and_the_snapshot() {
        let mut cache = Cache::default();
        let polls = vec![ProviderPoll {
            provider: ProviderId::Claude,
            result: Ok(usage(20.0, 61.0)),
        }];

        let snapshot = Snapshot::build(&polls, &mut cache);
        let provider = &snapshot.providers[0];
        assert_eq!(snapshot.schema, SCHEMA_VERSION);
        assert!(provider.ok && !provider.stale);
        assert_eq!(provider.session.as_ref().unwrap().label, "5h");
        assert_eq!(provider.weekly.as_ref().unwrap().text, "61%");
        assert!(provider
            .session
            .as_ref()
            .unwrap()
            .resets_in_text
            .as_deref()
            .is_some());
        assert!(cache.get(ProviderId::Claude).is_some());
    }

    #[test]
    fn a_failed_poll_falls_back_to_cached_numbers_marked_stale() {
        let mut cache = Cache::default();
        cache.insert(ProviderId::Codex, usage(30.0, 40.0), now_unix() - 60);

        let polls = vec![ProviderPoll {
            provider: ProviderId::Codex,
            result: Err(PollError::RequestFailed),
        }];
        let snapshot = Snapshot::build(&polls, &mut cache);
        let provider = &snapshot.providers[0];

        assert!(!provider.ok && provider.stale);
        assert_eq!(provider.state, "request_failed");
        assert_eq!(provider.session.as_ref().unwrap().percentage, 30.0);
        assert!(provider.message.is_some());
        // Stale providers never speak for the panel.
        assert!(snapshot.summary.is_none());
    }

    #[test]
    fn a_first_run_failure_reports_no_numbers_at_all() {
        let mut cache = Cache::default();
        let polls = vec![ProviderPoll {
            provider: ProviderId::Cursor,
            result: Err(PollError::NoCredentials),
        }];

        let snapshot = Snapshot::build(&polls, &mut cache);
        let provider = &snapshot.providers[0];
        assert_eq!(provider.state, "no_credentials");
        assert!(!provider.stale);
        assert!(provider.session.is_none());
    }

    #[test]
    fn the_summary_is_the_window_closest_to_running_out() {
        let mut cache = Cache::default();
        let polls = vec![
            ProviderPoll {
                provider: ProviderId::Claude,
                result: Ok(usage(20.0, 61.0)),
            },
            ProviderPoll {
                provider: ProviderId::Codex,
                result: Ok(usage(88.0, 10.0)),
            },
        ];

        let summary = Snapshot::build(&polls, &mut cache).summary.unwrap();
        assert_eq!(summary.provider, "codex");
        assert_eq!(summary.percentage, 88.0);
        assert_eq!(summary.label, "5h");
        assert_eq!(summary.text, "88%");
    }

    #[test]
    fn unreported_windows_are_left_out_and_extras_are_passed_through() {
        let mut cache = Cache::default();
        let polls = vec![ProviderPoll {
            provider: ProviderId::Copilot,
            result: Ok(UsageData {
                weekly: UsageSection::new(25.0, Some(now_unix() + 86_400)),
                weekly_label: Some("30d".into()),
                monthly: Some(UsageSection::new(40.0, None)),
                limits: vec![crate::models::UsageLimit {
                    key: "chat".into(),
                    label: "Chat".into(),
                    active: true,
                    usage: UsageSection::new(80.0, None),
                }],
                credits: Some(Credits {
                    used: Some(12.5),
                    limit: Some(50.0),
                    remaining: Some(37.5),
                }),
                plan: Some("pro".into()),
                ..Default::default()
            }),
        }];

        let snapshot = Snapshot::build(&polls, &mut cache);
        let provider = &snapshot.providers[0];
        assert!(provider.session.is_none());
        assert_eq!(provider.weekly.as_ref().unwrap().label, "30d");
        assert_eq!(provider.monthly.as_ref().unwrap().text, "40%");
        assert_eq!(provider.limits[0].section.label, "Chat");
        assert!(provider.limits[0].active);
        let credits = provider.credits.as_ref().unwrap();
        assert_eq!(credits.text, "$12.50 of $50.00");
        assert_eq!(credits.percentage, Some(25.0));
        assert_eq!(provider.plan.as_deref(), Some("pro"));
        // The panel number can come from a provider with no session, and a
        // binding monthly cap counts; per-model caps never speak for it.
        assert_eq!(snapshot.summary.as_ref().unwrap().percentage, 40.0);

        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["providers"][0]["limits"][0]["label"], "Chat");
        assert_eq!(json["providers"][0]["limits"][0]["percentage"], 80.0);
    }

    #[test]
    fn a_rate_limited_provider_backs_off_and_says_until_when() {
        let mut cache = Cache::default();
        cache.insert(ProviderId::Claude, usage(30.0, 40.0), now_unix() - 60);
        let polls = vec![ProviderPoll {
            provider: ProviderId::Claude,
            result: Err(PollError::RateLimited {
                retry_after: Some(120),
            }),
        }];

        let snapshot = Snapshot::build(&polls, &mut cache);
        let provider = &snapshot.providers[0];
        assert_eq!(provider.state, "rate_limited");
        assert!(provider.stale);
        assert!(provider.retry_at.is_some());
        assert!(cache.cooldown(ProviderId::Claude, now_unix()).is_some());
    }

    #[test]
    fn provider_supplied_labels_win_over_the_descriptor() {
        let mut cache = Cache::default();
        let polls = vec![ProviderPoll {
            provider: ProviderId::OpenCode,
            result: Ok(UsageData {
                weekly_label: Some("30d".into()),
                ..usage(5.0, 7.0)
            }),
        }];

        let snapshot = Snapshot::build(&polls, &mut cache);
        assert_eq!(snapshot.providers[0].weekly.as_ref().unwrap().label, "30d");
        assert_eq!(
            snapshot.providers[0].session.as_ref().unwrap().label,
            "rolling"
        );
    }
}

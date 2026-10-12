//! GitHub Copilot usage.
//!
//! Ported from the upstream Windows monitor (`src/poller/copilot.rs`, new in
//! v2.16). The endpoint and the quota mapping are upstream's. Credential
//! discovery is rewritten for Linux: upstream reads the Copilot CLI's and the
//! GitHub CLI's Windows Credential Manager entries; here the same logins are
//! found in the token variables, the Copilot editor plugins' `apps.json`, the
//! GitHub CLI's `hosts.yml`, and — when the GitHub CLI keeps its token in the
//! keyring — `gh auth token`.

use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;

use super::{
    build_agent, non_empty_environment, parse_iso8601, request_error, run_with_timeout, PollError,
    PollOptions,
};
use crate::diagnose;
use crate::models::{slug, UsageData, UsageLimit, UsageSection};

/// The quota endpoint the Copilot editors and CLI read for their own usage
/// panels. A GitHub OAuth token from any Copilot-enabled login can read it.
const COPILOT_USER_URL: &str = "https://api.github.com/copilot_internal/user";
/// The precedence the Copilot CLI itself gives its token variables.
const TOKEN_ENVIRONMENT: [&str; 3] = ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"];
const PREMIUM_QUOTA: &str = "premium_interactions";
const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
const GH_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Deserialize)]
struct CopilotUserResponse {
    #[serde(default)]
    copilot_plan: Option<String>,
    #[serde(default)]
    quota_reset_date_utc: Option<String>,
    /// Date-only fallback, e.g. `2026-11-01`.
    #[serde(default)]
    quota_reset_date: Option<String>,
    /// Kept as raw values so one unfamiliar snapshot cannot fail the rest.
    #[serde(default)]
    quota_snapshots: BTreeMap<String, serde_json::Value>,
    /// Copilot Free accounts without snapshots report what is left of each
    /// monthly allowance here, against the totals in `monthly_quotas`.
    #[serde(default)]
    limited_user_quotas: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    monthly_quotas: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    limited_user_reset_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QuotaSnapshot {
    #[serde(default)]
    entitlement: Option<f64>,
    #[serde(default)]
    remaining: Option<f64>,
    #[serde(default)]
    percent_remaining: Option<f64>,
    #[serde(default)]
    unlimited: bool,
}

impl QuotaSnapshot {
    fn used_percentage(&self) -> Option<f64> {
        if self.unlimited {
            return Some(0.0);
        }
        if let Some(remaining) = self.percent_remaining.filter(|value| value.is_finite()) {
            return Some((100.0 - remaining).clamp(0.0, 100.0));
        }
        used_from_remaining(self.entitlement?, self.remaining?)
    }
}

struct TokenCandidate {
    source: String,
    token: String,
}

pub fn poll(_options: PollOptions) -> Result<UsageData, PollError> {
    let candidates = token_candidates();
    if candidates.is_empty() {
        diagnose::log(
            "Copilot poll failed: no GitHub login found (sign in to Copilot in your editor, run 'gh auth login', or set GH_TOKEN)",
        );
        return Err(PollError::NoCredentials);
    }

    let mut last_error = PollError::AuthRequired;
    for candidate in &candidates {
        match fetch_usage(&candidate.token) {
            Ok(usage) => return Ok(usage),
            // Another login may still hold a Copilot seat. A 404 means this
            // token is valid but has no Copilot access.
            Err(error @ (PollError::AuthRequired | PollError::RequestFailed)) => {
                diagnose::log(format!(
                    "Copilot did not accept the login from {}: {error:?}",
                    candidate.source
                ));
                last_error = error;
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error)
}

/// Every login worth trying, most explicit first, without duplicates.
fn token_candidates() -> Vec<TokenCandidate> {
    let mut candidates: Vec<TokenCandidate> = TOKEN_ENVIRONMENT
        .iter()
        .filter_map(|name| {
            Some(TokenCandidate {
                source: format!("${name}"),
                token: non_empty_environment(name)?,
            })
        })
        .collect();

    for path in copilot_editor_files() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            for token in editor_tokens(&content) {
                candidates.push(TokenCandidate {
                    source: path.display().to_string(),
                    token,
                });
            }
        }
    }

    if let Some(path) = gh_hosts_file() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            for token in gh_hosts_tokens(&content) {
                candidates.push(TokenCandidate {
                    source: path.display().to_string(),
                    token,
                });
            }
        }
    }

    // The GitHub CLI keeps its token in the keyring by default, out of reach
    // of a file read; asking it is the one portable way in.
    if candidates.is_empty() {
        if let Some(token) = gh_auth_token() {
            candidates.push(TokenCandidate {
                source: "gh auth token".to_string(),
                token,
            });
        }
    }

    let mut seen = Vec::new();
    candidates.retain(|candidate| {
        if !is_header_safe_token(&candidate.token) || seen.contains(&candidate.token) {
            return false;
        }
        seen.push(candidate.token.clone());
        true
    });
    candidates
}

/// Where the Copilot plugins for VS Code, JetBrains, Neovim and friends keep
/// their GitHub login.
fn copilot_editor_files() -> Vec<PathBuf> {
    let Some(config) = dirs::config_dir() else {
        return Vec::new();
    };
    ["apps.json", "hosts.json"]
        .iter()
        .map(|name| config.join("github-copilot").join(name))
        .collect()
}

/// `{"github.com:Iv1.abc": {"user": "…", "oauth_token": "gho_…"}}`, or the
/// older `{"github.com": {…}}`. Only github.com logins can read the endpoint.
fn editor_tokens(content: &str) -> Vec<String> {
    let Ok(entries) = serde_json::from_str::<BTreeMap<String, serde_json::Value>>(content) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter(|(host, _)| *host == "github.com" || host.starts_with("github.com:"))
        .filter_map(|(_, entry)| entry.get("oauth_token")?.as_str())
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
        .collect()
}

fn gh_hosts_file() -> Option<PathBuf> {
    let directory = non_empty_environment("GH_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| Some(dirs::config_dir()?.join("gh")))?;
    Some(directory.join("hosts.yml"))
}

/// Every `oauth_token` in the `github.com` block of the GitHub CLI's
/// `hosts.yml`. The file is a two-level YAML map the CLI writes itself, so
/// reading it line by line is enough and keeps a YAML parser out of the build.
fn gh_hosts_tokens(content: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut in_github = false;
    for line in content.lines() {
        if !line.starts_with([' ', '\t']) && !line.trim().is_empty() {
            in_github = line
                .trim_end()
                .trim_end_matches(':')
                .trim_matches(['"', '\''])
                == "github.com";
            continue;
        }
        if !in_github {
            continue;
        }
        if let Some(value) = line.trim().strip_prefix("oauth_token:") {
            let value = value.trim().trim_matches(['"', '\'']);
            if !value.is_empty() {
                tokens.push(value.to_string());
            }
        }
    }
    tokens
}

fn gh_auth_token() -> Option<String> {
    let output = run_with_timeout(
        Command::new("gh")
            .args(["auth", "token", "--hostname", "github.com"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()),
        GH_TIMEOUT,
    )?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!token.is_empty()).then_some(token)
}

fn fetch_usage(token: &str) -> Result<UsageData, PollError> {
    let response = match build_agent()?
        .get(COPILOT_USER_URL)
        .set("Authorization", &format!("token {token}"))
        .set("Accept", "application/json")
        .set("User-Agent", USER_AGENT)
        .call()
    {
        Ok(response) => response,
        Err(error) => return Err(request_error(error, "Copilot usage endpoint")),
    };

    let response: CopilotUserResponse = response.into_json().map_err(|error| {
        diagnose::log_error("unable to parse Copilot usage response", error);
        PollError::RequestFailed
    })?;
    usage_from_user(response).ok_or_else(|| {
        diagnose::log("Copilot usage response carried no readable quota");
        PollError::RequestFailed
    })
}

fn usage_from_user(response: CopilotUserResponse) -> Option<UsageData> {
    let snapshots: BTreeMap<String, QuotaSnapshot> = response
        .quota_snapshots
        .into_iter()
        .filter_map(|(key, value)| Some((key, serde_json::from_value(value).ok()?)))
        .collect();
    let reset = parse_iso8601(response.quota_reset_date_utc.as_deref())
        .or_else(|| parse_reset_date(response.quota_reset_date.as_deref()));

    let premium = snapshots
        .get(PREMIUM_QUOTA)
        .and_then(QuotaSnapshot::used_percentage);
    let (pool, limits, resets_at) = match premium {
        Some(premium) => {
            let metered = snapshots
                .iter()
                .filter(|(_, snapshot)| !snapshot.unlimited)
                .filter_map(|(key, snapshot)| Some((key.clone(), snapshot.used_percentage()?)))
                .collect();
            (premium, quota_limits(metered, reset), reset)
        }
        None => {
            // Older Copilot Free responses carry no snapshots; chat is the
            // allowance those accounts run out of.
            let reset =
                reset.or_else(|| parse_reset_date(response.limited_user_reset_date.as_deref()));
            let free = free_quota_usage(&response.limited_user_quotas, &response.monthly_quotas);
            let pool = *free.get("chat")?;
            (pool, quota_limits(free, reset), reset)
        }
    };

    // One pool per calendar month and no short window: the pool is the long
    // window, and the session window stays unreported.
    Some(UsageData {
        weekly: UsageSection::new(pool, resets_at),
        weekly_label: Some("30d".into()),
        limits,
        plan: response.copilot_plan.filter(|plan| !plan.trim().is_empty()),
        ..Default::default()
    })
}

/// Chat and completions are unlimited on paid plans; only metered quotas
/// besides the premium pool are worth listing.
fn quota_limits(quotas: BTreeMap<String, f64>, resets_at: Option<i64>) -> Vec<UsageLimit> {
    quotas
        .into_iter()
        .filter(|(key, _)| key != PREMIUM_QUOTA)
        .map(|(key, percentage)| UsageLimit {
            key: slug(&key),
            label: quota_label(&key),
            active: percentage > 0.0,
            usage: UsageSection::new(percentage, resets_at),
        })
        .collect()
}

fn free_quota_usage(
    remaining: &BTreeMap<String, serde_json::Value>,
    totals: &BTreeMap<String, serde_json::Value>,
) -> BTreeMap<String, f64> {
    totals
        .iter()
        .filter_map(|(key, total)| {
            let used = used_from_remaining(total.as_f64()?, remaining.get(key)?.as_f64()?)?;
            Some((key.clone(), used))
        })
        .collect()
}

fn used_from_remaining(entitlement: f64, remaining: f64) -> Option<f64> {
    if !entitlement.is_finite() || entitlement <= 0.0 || !remaining.is_finite() {
        return None;
    }
    Some(((entitlement - remaining) / entitlement * 100.0).clamp(0.0, 100.0))
}

fn quota_label(key: &str) -> String {
    match key {
        "chat" => "Chat".into(),
        "completions" => "Completions".into(),
        other => other.replace('_', " "),
    }
}

/// Copilot quotas reset at midnight UTC on the given day.
fn parse_reset_date(date: Option<&str>) -> Option<i64> {
    let date = date?.trim();
    parse_iso8601(Some(&format!("{date}T00:00:00Z")))
}

/// Tokens go into a header, so anything that could split it is unusable.
fn is_header_safe_token(token: &str) -> bool {
    !token.is_empty() && token.bytes().all(|byte| byte.is_ascii_graphic())
}

/// Fingerprints only: variables are hashed, files reduced to size and mtime.
/// `gh auth token` is deliberately not run here — this is called every few
/// seconds in `--watch` mode.
pub(super) fn credential_watch_snapshot() -> Vec<String> {
    TOKEN_ENVIRONMENT
        .iter()
        .map(|name| match non_empty_environment(name) {
            Some(value) => secret_signature(name, &value),
            None => format!("{name}|missing"),
        })
        .chain(
            copilot_editor_files()
                .into_iter()
                .chain(gh_hosts_file())
                .map(|path| super::file_watch_signature(&path)),
        )
        .collect()
}

fn secret_signature(source: &str, value: &str) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{source}|present|{}|{:x}", value.len(), hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(json: serde_json::Value) -> Option<UsageData> {
        usage_from_user(serde_json::from_value(json).unwrap())
    }

    #[test]
    fn an_enterprise_response_maps_premium_requests_to_the_monthly_pool() {
        // Shape captured upstream from an Enterprise seat, identifiers dropped.
        let data = usage(serde_json::json!({
            "access_type_sku": "copilot_enterprise_seat_quota",
            "copilot_plan": "enterprise",
            "quota_reset_date": "2026-11-01",
            "quota_reset_date_utc": "2026-11-01T00:00:00.000Z",
            "quota_snapshots": {
                "chat": {"entitlement": 0, "remaining": 0, "percent_remaining": 100.0, "unlimited": true},
                "completions": {"entitlement": 0, "remaining": 0, "percent_remaining": 100.0, "unlimited": true},
                "premium_interactions": {
                    "entitlement": 30000, "remaining": 22500, "percent_remaining": 75.0,
                    "unlimited": false, "overage_permitted": true
                }
            }
        }))
        .unwrap();

        assert!(data.weekly.available);
        assert_eq!(data.weekly.percentage, 25.0);
        assert_eq!(data.weekly_label.as_deref(), Some("30d"));
        assert_eq!(data.weekly.resets_at, Some(1_793_491_200));
        assert!(!data.session.available);
        assert_eq!(data.plan.as_deref(), Some("enterprise"));
        assert!(data.limits.is_empty());
    }

    #[test]
    fn metered_chat_and_completions_become_limits() {
        let data = usage(serde_json::json!({
            "copilot_plan": "free",
            "quota_reset_date_utc": "2026-11-01T00:00:00.000Z",
            "quota_snapshots": {
                "chat": {"entitlement": 50, "remaining": 10, "percent_remaining": 20.0},
                "completions": {"entitlement": 2000, "remaining": 2000, "percent_remaining": 100.0},
                "premium_interactions": {"entitlement": 50, "remaining": 45, "percent_remaining": 90.0}
            }
        }))
        .unwrap();
        assert_eq!(data.weekly.percentage, 10.0);
        let limits: Vec<_> = data
            .limits
            .iter()
            .map(|limit| (limit.label.as_str(), limit.usage.percentage, limit.active))
            .collect();
        assert_eq!(limits, [("Chat", 80.0, true), ("Completions", 0.0, false)]);
    }

    #[test]
    fn overage_and_missing_percentages_fall_back_to_counts() {
        let data = usage(serde_json::json!({
            "quota_reset_date": "2026-11-01",
            "quota_snapshots": {"premium_interactions": {"entitlement": 300, "remaining": -12}}
        }))
        .unwrap();
        assert_eq!(data.weekly.percentage, 100.0);
        assert_eq!(data.weekly.resets_at, Some(1_793_491_200));
    }

    #[test]
    fn a_legacy_free_response_reads_the_chat_allowance() {
        let data = usage(serde_json::json!({
            "limited_user_reset_date": "2026-10-15",
            "limited_user_quotas": {"chat": 40, "completions": 1500},
            "monthly_quotas": {"chat": 50, "completions": 2000}
        }))
        .unwrap();
        assert_eq!(data.weekly.percentage, 20.0);
        assert!(data.weekly.resets_at.is_some());
        assert_eq!(data.limits[1].label, "Completions");
        assert_eq!(data.limits[1].usage.percentage, 25.0);
    }

    #[test]
    fn responses_without_a_readable_pool_are_rejected() {
        assert!(usage(serde_json::json!({})).is_none());
        assert!(usage(serde_json::json!({
            "quota_snapshots": {"premium_interactions": "unexpected", "chat": {"unlimited": true}}
        }))
        .is_none());
        assert!(usage(serde_json::json!({
            "quota_snapshots": {"premium_interactions": {"entitlement": 0, "remaining": 0}}
        }))
        .is_none());
    }

    #[test]
    fn editor_logins_are_read_for_github_com_only() {
        let tokens = editor_tokens(
            r#"{
                "github.com:Iv1.b507a08c87ecfe98": {"user": "me", "oauth_token": "gho_one"},
                "github.example.com:Iv1.x": {"user": "me", "oauth_token": "gho_enterprise"},
                "github.com": {"oauth_token": "gho_two"}
            }"#,
        );
        assert_eq!(tokens, ["gho_two", "gho_one"]);
        assert!(editor_tokens("not json").is_empty());
    }

    #[test]
    fn gh_hosts_tokens_come_from_the_github_com_block() {
        let tokens = gh_hosts_tokens(
            "github.com:\n    users:\n        me:\n            oauth_token: gho_user\n    git_protocol: https\n    oauth_token: gho_top\n    user: me\nghe.example.com:\n    oauth_token: gho_other\n",
        );
        assert_eq!(tokens, ["gho_user", "gho_top"]);
        // A keyring-backed login has no token in the file at all.
        assert!(gh_hosts_tokens("github.com:\n    user: me\n").is_empty());
    }

    #[test]
    fn tokens_that_could_split_a_header_are_refused() {
        assert!(is_header_safe_token("gho_abc123"));
        assert!(!is_header_safe_token(""));
        assert!(!is_header_safe_token("gho_abc\r\nInjected: yes"));
        assert!(!is_header_safe_token("two words"));
    }
}

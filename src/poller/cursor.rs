//! Cursor usage.
//!
//! Ported from the upstream Windows monitor (`src/poller/cursor.rs`). The
//! dashboard endpoint, the session-cookie construction, and the JWT subject
//! extraction are upstream's; the state database lives under the Linux config
//! directory and is read through the system SQLite rather than Windows'.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::{build_agent, non_empty_environment, parse_iso8601, PollError, PollOptions};
use crate::diagnose;
use crate::models::{now_unix, UsageData, UsageSection};

const USAGE_SUMMARY_URL: &str = "https://cursor.com/api/usage-summary";
const SESSION_TOKEN_ENV: &str = "CURSOR_SESSION_TOKEN";
const ACCESS_TOKEN_KEY: &str = "cursorAuth/accessToken";
const ITEM_TABLE_QUERY: &str = "SELECT value FROM ItemTable WHERE key = ?1";

#[derive(Deserialize)]
struct UsageSummaryResponse {
    #[serde(rename = "billingCycleEnd")]
    billing_cycle_end: Option<String>,
    #[serde(rename = "individualUsage")]
    individual_usage: Option<IndividualUsage>,
}

#[derive(Deserialize)]
struct IndividualUsage {
    plan: Option<PlanUsage>,
}

#[derive(Deserialize)]
struct PlanUsage {
    #[serde(rename = "autoPercentUsed")]
    auto_percent_used: Option<f64>,
    #[serde(rename = "apiPercentUsed")]
    api_percent_used: Option<f64>,
    #[serde(rename = "totalPercentUsed")]
    total_percent_used: Option<f64>,
}

pub fn poll(_options: PollOptions) -> Result<UsageData, PollError> {
    let cookie = read_session_cookie().ok_or_else(|| {
        diagnose::log(
            "Cursor poll failed: no session found (sign in to Cursor or set CURSOR_SESSION_TOKEN)",
        );
        PollError::NoCredentials
    })?;
    fetch_usage(&cookie)
}

/// An explicit environment value takes priority over the access token Cursor
/// persists in its own state database.
fn read_session_cookie() -> Option<String> {
    if let Some(token) = non_empty_environment(SESSION_TOKEN_ENV) {
        return normalize_session_cookie(&token);
    }
    cookie_from_access_token(&read_access_token_from_state_db()?)
}

fn normalize_session_cookie(token: &str) -> Option<String> {
    if token.bytes().any(|byte| matches!(byte, b'\r' | b'\n')) {
        return None;
    }
    let token = token
        .trim()
        .strip_prefix("WorkosCursorSessionToken=")
        .unwrap_or(token.trim())
        .trim();
    if token.is_empty() {
        None
    } else if token.contains("%3A%3A") {
        Some(token.to_string())
    } else if token.contains("::") {
        Some(token.replace("::", "%3A%3A"))
    } else {
        cookie_from_access_token(token).or_else(|| Some(token.to_string()))
    }
}

/// The dashboard cookie is `<user id>::<access token>`, URL-encoded.
fn cookie_from_access_token(access_token: &str) -> Option<String> {
    Some(format!(
        "{}%3A%3A{access_token}",
        extract_user_id(access_token)?
    ))
}

fn extract_user_id(jwt: &str) -> Option<String> {
    let payload = jwt.split('.').nth(1)?;
    let decoded = base64_url_decode(payload)?;
    let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    let subject = json.get("sub")?.as_str()?;
    Some(
        subject
            .rsplit_once('|')
            .map(|(_, id)| id.to_string())
            .unwrap_or_else(|| subject.to_string()),
    )
}

fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    if input.len() % 4 == 1 {
        return None;
    }
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        } as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    // Whatever is left over must be padding zeroes, not dropped data.
    let padding_mask = (1u32 << bits).saturating_sub(1);
    (buffer & padding_mask == 0).then_some(output)
}

fn state_db_paths() -> Vec<PathBuf> {
    let Some(config) = dirs::config_dir() else {
        return Vec::new();
    };
    // Cursor's own build and the community Flatpak disagree on capitalisation.
    ["Cursor", "cursor"]
        .iter()
        .map(|directory| {
            config
                .join(directory)
                .join("User")
                .join("globalStorage")
                .join("state.vscdb")
        })
        .collect()
}

fn state_db_path() -> Option<PathBuf> {
    state_db_paths().into_iter().find(|path| path.is_file())
}

fn read_access_token_from_state_db() -> Option<String> {
    let path = state_db_path()?;
    match query_access_token(&path) {
        Ok(token) => token,
        Err(error) => {
            // A running Cursor holds a write lock often enough that a copy is
            // the normal path rather than the exception.
            diagnose::log(format!(
                "Cursor state DB direct read failed ({error}); retrying via temp copy"
            ));
            query_access_token_from_copy(&path)
        }
    }
}

fn query_access_token_from_copy(path: &Path) -> Option<String> {
    let temporary = std::env::temp_dir().join(format!(
        "kde-ai-usage-monitor-cursor-{}-{}.vscdb",
        std::process::id(),
        now_unix()
    ));
    if let Err(error) = std::fs::copy(path, &temporary) {
        diagnose::log(format!("Cursor state DB temp copy failed: {error}"));
        return None;
    }
    let result = query_access_token(&temporary);
    let _ = std::fs::remove_file(&temporary);
    match result {
        Ok(token) => token,
        Err(error) => {
            diagnose::log(format!("Cursor state DB temp-copy read failed: {error}"));
            None
        }
    }
}

fn query_access_token(path: &Path) -> Result<Option<String>, crate::linux_sqlite::Error> {
    crate::linux_sqlite::query_optional_text(path, ITEM_TABLE_QUERY, ACCESS_TOKEN_KEY)
        .map(|token| token.filter(|token| !token.is_empty()))
}

fn fetch_usage(cookie: &str) -> Result<UsageData, PollError> {
    let response = match build_agent()?
        .get(USAGE_SUMMARY_URL)
        .set("Cookie", &format!("WorkosCursorSessionToken={cookie}"))
        .set("User-Agent", "Mozilla/5.0")
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(401 | 403, _)) => return Err(PollError::AuthRequired),
        Err(error) => {
            diagnose::log_error("Cursor usage-summary request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let response: UsageSummaryResponse = response.into_json().map_err(|error| {
        diagnose::log_error("unable to parse Cursor usage-summary response", error);
        PollError::RequestFailed
    })?;

    usage_from_summary(response).ok_or_else(|| {
        diagnose::log("Cursor usage-summary response missing plan usage");
        PollError::RequestFailed
    })
}

/// Cursor bills two pools against one billing cycle, so both windows share a
/// reset instant: `session` carries Auto usage, `weekly` carries API usage.
fn usage_from_summary(response: UsageSummaryResponse) -> Option<UsageData> {
    let plan = response.individual_usage?.plan?;
    let reset = parse_iso8601(response.billing_cycle_end.as_deref());
    Some(UsageData {
        session: UsageSection::new(
            plan.auto_percent_used
                .or(plan.total_percent_used)
                .unwrap_or(0.0),
            reset,
        ),
        weekly: UsageSection::new(plan.api_percent_used.unwrap_or(0.0), reset),
        session_label: Some("Auto".into()),
        weekly_label: Some("API".into()),
    })
}

pub(super) fn credential_watch_snapshot() -> Vec<String> {
    let environment = non_empty_environment(SESSION_TOKEN_ENV)
        .map(|value| secret_signature("environment", &value))
        .unwrap_or_else(|| "environment|missing".into());
    let mut snapshot = vec![environment];
    snapshot.extend(
        state_db_paths()
            .iter()
            .map(|path| super::file_watch_signature(path)),
    );
    snapshot
}

fn secret_signature(source: &str, value: &str) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{source}|present|{}|{:x}", value.len(), hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_user_id_from_a_jwt() {
        let jwt = "header.eyJzdWIiOiJhdXRoMHx1c2VyXzEyMyJ9.signature";
        assert_eq!(extract_user_id(jwt).as_deref(), Some("user_123"));
        assert_eq!(
            cookie_from_access_token(jwt).as_deref(),
            Some("user_123%3A%3Aheader.eyJzdWIiOiJhdXRoMHx1c2VyXzEyMyJ9.signature")
        );
    }

    #[test]
    fn rejects_malformed_base64_and_cookie_header_injection() {
        assert!(base64_url_decode("a").is_none());
        assert!(normalize_session_cookie("value\r\nInjected: yes").is_none());
    }

    #[test]
    fn already_encoded_session_cookies_are_passed_through() {
        assert_eq!(
            normalize_session_cookie("WorkosCursorSessionToken=user_1%3A%3Atoken").as_deref(),
            Some("user_1%3A%3Atoken")
        );
        assert_eq!(
            normalize_session_cookie("user_1::token").as_deref(),
            Some("user_1%3A%3Atoken")
        );
    }

    #[test]
    fn usage_maps_auto_and_api_percentages() {
        let response: UsageSummaryResponse = serde_json::from_str(
            r#"{
                "billingCycleEnd": "2026-08-25T19:27:24.000Z",
                "individualUsage": {
                    "plan": {
                        "autoPercentUsed": 12.5,
                        "apiPercentUsed": 3.0,
                        "totalPercentUsed": 10.0
                    }
                }
            }"#,
        )
        .unwrap();

        let data = usage_from_summary(response).unwrap();
        assert_eq!(data.session.percentage, 12.5);
        assert_eq!(data.weekly.percentage, 3.0);
        assert_eq!(data.session_label.as_deref(), Some("Auto"));
        assert_eq!(data.weekly_label.as_deref(), Some("API"));
        assert_eq!(data.session.resets_at, data.weekly.resets_at);
        assert!(data.session.resets_at.is_some());
    }

    #[test]
    fn plans_without_usage_are_not_a_summary() {
        let response: UsageSummaryResponse =
            serde_json::from_str(r#"{"individualUsage":{}}"#).unwrap();
        assert!(usage_from_summary(response).is_none());
    }
}

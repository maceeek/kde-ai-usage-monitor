//! Codex (OpenAI) usage.
//!
//! Ported from the upstream Windows monitor (`src/poller/codex.rs`). The
//! endpoint and the rate-limit window shapes are upstream's; the Windows shim
//! resolution (`codex.cmd`, `codex.ps1`, `where.exe`) is replaced by a plain
//! `codex` on `PATH`.
//!
//! Brought up to upstream v2.18: windows are placed by their length rather
//! than their slot, accounts on spend-control limits report a monthly window,
//! and the credit balance and plan are passed through.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;

use super::{
    build_agent, non_empty_environment, request_error, run_with_timeout, PollError, PollOptions,
};
use crate::diagnose;
use crate::models::{Credits, UsageData, UsageSection};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_HOME_ENV: &str = "CODEX_HOME";
const REFRESH_TIMEOUT: Duration = Duration::from_secs(30);
/// A window at least this long is the weekly allowance. Codex sends 18000 for
/// the five-hour window and 604800 for the weekly one.
const WEEKLY_WINDOW_THRESHOLD_SECONDS: i64 = 86_400;
/// Codex bills credits at 25 to the dollar.
const CREDITS_PER_DOLLAR: f64 = 25.0;

#[derive(Deserialize)]
struct AuthFile {
    tokens: Option<TokenData>,
}

#[derive(Clone, Deserialize)]
struct TokenData {
    access_token: String,
    account_id: Option<String>,
}

// The API nests `null` inside `Some` for windows it knows about but has no data
// for, so the double `Option` is deliberate.
#[derive(Deserialize)]
pub(super) struct UsageResponse {
    plan_type: Option<String>,
    rate_limit: Option<Option<Box<RateLimitDetails>>>,
    spend_control: Option<SpendControl>,
    credits: Option<Option<Box<CreditBalance>>>,
}

#[derive(Deserialize)]
struct SpendControl {
    individual_limit: Option<IndividualLimit>,
}

/// A monthly spending cap set by a workspace admin, reported instead of (or
/// alongside) the usual windows.
#[derive(Deserialize)]
struct IndividualLimit {
    used_percent: Option<f64>,
    reset_at: Option<i64>,
}

#[derive(Deserialize)]
struct CreditBalance {
    #[serde(default)]
    has_credits: bool,
    #[serde(default)]
    unlimited: bool,
    /// A decimal string, in credits rather than currency.
    balance: Option<String>,
}

#[derive(Deserialize)]
struct RateLimitDetails {
    primary_window: Option<Option<Box<RateLimitWindow>>>,
    secondary_window: Option<Option<Box<RateLimitWindow>>>,
}

#[derive(Deserialize)]
struct RateLimitWindow {
    used_percent: f64,
    reset_at: i64,
    limit_window_seconds: Option<i64>,
}

pub fn poll(options: PollOptions) -> Result<UsageData, PollError> {
    let credentials = read_credentials().ok_or_else(|| {
        diagnose::log("Codex poll failed: no credentials found");
        PollError::NoCredentials
    })?;

    match fetch_usage(&credentials.access_token, credentials.account_id.as_deref()) {
        Err(PollError::AuthRequired) if options.refresh_tokens => {
            cli_refresh_token();
            let refreshed = read_credentials().ok_or(PollError::TokenExpired)?;
            fetch_usage(&refreshed.access_token, refreshed.account_id.as_deref())
        }
        result => result,
    }
}

fn fetch_usage(token: &str, account_id: Option<&str>) -> Result<UsageData, PollError> {
    let mut request = build_agent()?
        .get(USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("User-Agent", "codex-cli");

    if let Some(account_id) = account_id.filter(|value| !value.is_empty()) {
        request = request.set("ChatGPT-Account-Id", account_id);
    }

    let response = match request.call() {
        Ok(response) => response,
        Err(error) => return Err(request_error(error, "Codex usage endpoint")),
    };

    let response: UsageResponse = response.into_json().map_err(|error| {
        diagnose::log_error("unable to parse Codex usage response", error);
        PollError::RequestFailed
    })?;

    usage_from_response(response).ok_or(PollError::RequestFailed)
}

fn usage_from_response(response: UsageResponse) -> Option<UsageData> {
    let details = response.rate_limit.flatten();
    let monthly = response
        .spend_control
        .and_then(|control| control.individual_limit)
        .and_then(|limit| section_from_individual_limit(&limit));
    if details.is_none() && monthly.is_none() {
        return None;
    }

    let mut data = UsageData::default();
    // Place windows by length, not by slot: Codex has sent the weekly window in
    // `primary_window` with `secondary_window` empty while the five-hour
    // window was switched off, which put a weekly figure in the session bar.
    if let Some(details) = details {
        for (window, default_is_weekly) in [
            (details.primary_window.flatten(), false),
            (details.secondary_window.flatten(), true),
        ] {
            let Some(window) = window else { continue };
            let section = section_from_window(&window);
            let is_weekly = window
                .limit_window_seconds
                .map(|seconds| seconds >= WEEKLY_WINDOW_THRESHOLD_SECONDS)
                .unwrap_or(default_is_weekly);
            if is_weekly {
                data.weekly = section;
            } else {
                data.session = section;
            }
        }
    }

    if let Some(monthly) = monthly {
        // With no weekly window the monthly cap is the long window that binds.
        if !data.weekly.available {
            data.weekly = monthly;
            data.weekly_label = Some("30d".to_string());
        } else {
            data.monthly = Some(monthly);
        }
    }

    data.credits = response
        .credits
        .flatten()
        .and_then(|credits| credits_from_balance(&credits));
    data.plan = response.plan_type.filter(|plan| !plan.trim().is_empty());
    Some(data)
}

fn section_from_window(window: &RateLimitWindow) -> UsageSection {
    UsageSection::new(window.used_percent, Some(window.reset_at))
}

fn section_from_individual_limit(limit: &IndividualLimit) -> Option<UsageSection> {
    let percentage = limit.used_percent.filter(|value| value.is_finite())?;
    Some(UsageSection::new(percentage, limit.reset_at))
}

/// The balance in dollars. Codex states no ceiling, so there is nothing to
/// draw a bar against — just what is left.
fn credits_from_balance(credits: &CreditBalance) -> Option<Credits> {
    if !credits.has_credits || credits.unlimited {
        return None;
    }
    let balance = credits.balance.as_deref()?.trim().parse::<f64>().ok()?;
    balance.is_finite().then(|| Credits {
        remaining: Some(balance.max(0.0) / CREDITS_PER_DOLLAR),
        ..Default::default()
    })
}

fn auth_path() -> Option<PathBuf> {
    auth_path_from(
        non_empty_environment(CODEX_HOME_ENV).map(PathBuf::from),
        dirs::home_dir(),
    )
}

fn auth_path_from(codex_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    match codex_home {
        Some(codex_home) => Some(codex_home.join("auth.json")),
        None => Some(home?.join(".codex").join("auth.json")),
    }
}

fn read_credentials() -> Option<TokenData> {
    let path = auth_path()?;
    let content = std::fs::read_to_string(&path)
        .inspect_err(|error| {
            if diagnose::is_enabled() {
                diagnose::log_error(
                    &format!("unable to read Codex credentials at {}", path.display()),
                    error,
                );
            }
        })
        .ok()?;
    let auth: AuthFile = serde_json::from_str(&content).ok()?;
    auth.tokens.filter(|tokens| !tokens.access_token.is_empty())
}

/// `codex exec .` is the cheapest command that makes the CLI refresh its token.
fn cli_refresh_token() {
    diagnose::log("attempting Codex token refresh via the codex CLI");
    let refreshed = run_with_timeout(
        Command::new("codex")
            .args(["exec", "."])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
        REFRESH_TIMEOUT,
    );
    if refreshed.is_none() {
        diagnose::log("Codex token refresh did not complete");
    }
}

pub(super) fn credential_watch_snapshot() -> Vec<String> {
    match auth_path() {
        Some(path) => vec![super::file_watch_signature(&path)],
        None => vec!["codex:auth-path-missing".into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_maps_both_rate_limit_windows() {
        let response: UsageResponse = serde_json::from_str(
            r#"{"rate_limit":{
                "primary_window":{"used_percent":12.5,"reset_at":1755000000},
                "secondary_window":{"used_percent":48.0,"reset_at":1755600000}
            }}"#,
        )
        .unwrap();

        let data = usage_from_response(response).unwrap();
        assert_eq!(data.session.percentage, 12.5);
        assert_eq!(data.session.resets_at, Some(1_755_000_000));
        assert_eq!(data.weekly.percentage, 48.0);
    }

    #[test]
    fn null_windows_degrade_to_empty_sections() {
        let response: UsageResponse = serde_json::from_str(
            r#"{"rate_limit":{"primary_window":null,"secondary_window":null}}"#,
        )
        .unwrap();
        let data = usage_from_response(response).unwrap();
        assert_eq!(data.session, UsageSection::default());
        assert_eq!(data.weekly, UsageSection::default());
    }

    #[test]
    fn windows_are_placed_by_length_not_by_slot() {
        let response: UsageResponse = serde_json::from_str(
            r#"{"plan_type":"plus","rate_limit":{
                "primary_window":{"used_percent":20,"limit_window_seconds":604800,"reset_at":1787198224},
                "secondary_window":null
            }}"#,
        )
        .unwrap();
        let data = usage_from_response(response).unwrap();
        assert_eq!(data.weekly.percentage, 20.0);
        assert!(!data.session.available);
        assert_eq!(data.plan.as_deref(), Some("plus"));
    }

    #[test]
    fn an_individual_spend_limit_is_the_monthly_window() {
        let alone: UsageResponse = serde_json::from_str(
            r#"{"rate_limit":null,
                "spend_control":{"individual_limit":{"used_percent":24,"reset_at":1793491201}}}"#,
        )
        .unwrap();
        let data = usage_from_response(alone).unwrap();
        assert_eq!(data.weekly.percentage, 24.0);
        assert_eq!(data.weekly_label.as_deref(), Some("30d"));
        assert!(data.monthly.is_none());

        let alongside: UsageResponse = serde_json::from_str(
            r#"{"rate_limit":{"primary_window":{"used_percent":42,"limit_window_seconds":604800,"reset_at":1}},
                "spend_control":{"individual_limit":{"used_percent":24,"reset_at":1793491201}}}"#,
        )
        .unwrap();
        let data = usage_from_response(alongside).unwrap();
        assert_eq!(data.weekly.percentage, 42.0);
        assert_eq!(data.weekly_label, None);
        assert_eq!(data.monthly.unwrap().percentage, 24.0);

        let incomplete: UsageResponse =
            serde_json::from_str(r#"{"rate_limit":null,"spend_control":{"individual_limit":{}}}"#)
                .unwrap();
        assert!(usage_from_response(incomplete).is_none());
    }

    #[test]
    fn a_credit_balance_is_reported_in_dollars() {
        let response: UsageResponse = serde_json::from_str(
            r#"{"rate_limit":{"primary_window":null},
                "credits":{"has_credits":true,"unlimited":false,"balance":"250.0"}}"#,
        )
        .unwrap();
        let credits = usage_from_response(response).unwrap().credits.unwrap();
        assert_eq!(credits.remaining, Some(10.0));
        assert_eq!(credits.percentage(), None);

        let none: UsageResponse = serde_json::from_str(
            r#"{"rate_limit":{"primary_window":null},"credits":{"has_credits":false,"balance":"0"}}"#,
        )
        .unwrap();
        assert!(usage_from_response(none).unwrap().credits.is_none());
    }

    #[test]
    fn a_null_rate_limit_block_is_not_usage() {
        let response: UsageResponse = serde_json::from_str(r#"{"rate_limit":null}"#).unwrap();
        assert!(usage_from_response(response).is_none());
    }

    #[test]
    fn codex_home_overrides_the_default_auth_path() {
        assert_eq!(
            auth_path_from(Some("/opt/codex".into()), Some("/home/user".into())),
            Some(PathBuf::from("/opt/codex/auth.json"))
        );
        assert_eq!(
            auth_path_from(None, Some("/home/user".into())),
            Some(PathBuf::from("/home/user/.codex/auth.json"))
        );
    }
}

//! Codex (OpenAI) usage.
//!
//! Ported from the upstream Windows monitor (`src/poller/codex.rs`). The
//! endpoint and the rate-limit window shapes are upstream's; the Windows shim
//! resolution (`codex.cmd`, `codex.ps1`, `where.exe`) is replaced by a plain
//! `codex` on `PATH`.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;

use super::{build_agent, non_empty_environment, run_with_timeout, PollError, PollOptions};
use crate::diagnose;
use crate::models::{UsageData, UsageSection};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_HOME_ENV: &str = "CODEX_HOME";
const REFRESH_TIMEOUT: Duration = Duration::from_secs(30);

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
    rate_limit: Option<Option<Box<RateLimitDetails>>>,
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
        Err(ureq::Error::Status(code @ (401 | 403), _)) => {
            diagnose::log(format!(
                "Codex usage endpoint returned auth error status {code}; refresh required"
            ));
            return Err(PollError::AuthRequired);
        }
        Err(error) => {
            diagnose::log_error("Codex usage endpoint request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let response: UsageResponse = response.into_json().map_err(|error| {
        diagnose::log_error("unable to parse Codex usage response", error);
        PollError::RequestFailed
    })?;

    usage_from_response(response).ok_or(PollError::RequestFailed)
}

fn usage_from_response(response: UsageResponse) -> Option<UsageData> {
    let details = *response.rate_limit.flatten()?;
    Some(UsageData {
        session: details
            .primary_window
            .flatten()
            .map(|window| section_from_window(&window))
            .unwrap_or_default(),
        weekly: details
            .secondary_window
            .flatten()
            .map(|window| section_from_window(&window))
            .unwrap_or_default(),
        ..Default::default()
    })
}

fn section_from_window(window: &RateLimitWindow) -> UsageSection {
    UsageSection::new(window.used_percent, Some(window.reset_at))
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

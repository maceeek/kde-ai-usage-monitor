//! Claude Code usage.
//!
//! Ported from the upstream Windows monitor (`src/poller/claude.rs`). The
//! endpoints, the OAuth beta header, and the rate-limit header names are
//! upstream's; credential discovery is rewritten for Linux — no WSL probing, no
//! DPAPI-encrypted desktop token cache, and the CLI is a plain `claude` on
//! `PATH` rather than a `.cmd` shim.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;

use super::secret_store;
use super::{
    build_agent, get_header_f64, get_header_i64, non_empty_environment, parse_iso8601,
    run_with_timeout, PollError, PollOptions,
};
use crate::diagnose;
use crate::models::{now_unix, UsageData, UsageSection};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const MODEL_FALLBACK_CHAIN: &[&str] = &["claude-3-haiku-20240307", "claude-haiku-4-5-20251001"];
const CREDENTIALS_FILE_ENV: &str = "CLAUDE_CREDENTIALS_FILE";
const SECRET_TARGETS: &[&str] = &["Claude Code", "Claude Code-credentials", "claude.ai"];
const REFRESH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct UsageResponse {
    five_hour: Option<UsageBucket>,
    seven_day: Option<UsageBucket>,
}

#[derive(Deserialize)]
struct UsageBucket {
    utilization: f64,
    resets_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CredentialSource {
    /// `~/.claude/.credentials.json`, written by the Claude Code CLI.
    File(PathBuf),
    /// The desktop secret store, used when the CLI was built against a keyring.
    Secret(&'static str),
}

struct Credentials {
    access_token: String,
    /// Expiry in Unix milliseconds, as the credentials file stores it.
    expires_at: Option<i64>,
    source: CredentialSource,
}

pub fn poll(options: PollOptions) -> Result<UsageData, PollError> {
    let credentials = read_first_credentials().ok_or_else(|| {
        diagnose::log("Claude poll failed: no credentials found");
        PollError::NoCredentials
    })?;
    let credentials = refresh_or_fallback(credentials, options)?;
    fetch_usage_with_fallback(&credentials.access_token)
}

/// Ask the usage endpoint first, then fall back to the rate-limit headers on a
/// throwaway Messages request. Upstream's two-step, kept intact: the dedicated
/// endpoint sometimes omits reset instants that the headers do carry.
fn fetch_usage_with_fallback(token: &str) -> Result<UsageData, PollError> {
    if let Some(data) = try_usage_endpoint(token)? {
        if data.session.resets_at.is_none() || data.weekly.resets_at.is_none() {
            if let Ok(fallback) = fetch_usage_via_messages(token) {
                let mut merged = data;
                merged.session.resets_at = merged.session.resets_at.or(fallback.session.resets_at);
                merged.weekly.resets_at = merged.weekly.resets_at.or(fallback.weekly.resets_at);
                return Ok(merged);
            }
        }
        return Ok(data);
    }

    fetch_usage_via_messages(token).inspect_err(|_| {
        diagnose::log("usage endpoint and Messages API fallback both failed");
    })
}

fn try_usage_endpoint(token: &str) -> Result<Option<UsageData>, PollError> {
    let response = match build_agent()?
        .get(USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", OAUTH_BETA)
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(code @ (401 | 403), _)) => {
            diagnose::log(format!(
                "usage endpoint returned auth error status {code}; re-login required"
            ));
            return Err(PollError::AuthRequired);
        }
        Err(_) => return Ok(None),
    };

    let Ok(response) = response.into_json::<UsageResponse>() else {
        return Ok(None);
    };

    let mut data = UsageData::default();
    if let Some(bucket) = &response.five_hour {
        data.session = UsageSection::new(
            bucket.utilization,
            parse_iso8601(bucket.resets_at.as_deref()),
        );
    }
    if let Some(bucket) = &response.seven_day {
        data.weekly = UsageSection::new(
            bucket.utilization,
            parse_iso8601(bucket.resets_at.as_deref()),
        );
    }
    Ok(Some(data))
}

fn fetch_usage_via_messages(token: &str) -> Result<UsageData, PollError> {
    let agent = build_agent()?;

    for model in MODEL_FALLBACK_CHAIN {
        let body = serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "."}]
        });

        let response = match agent
            .post(MESSAGES_URL)
            .set("Authorization", &format!("Bearer {token}"))
            .set("anthropic-version", "2023-06-01")
            .set("anthropic-beta", OAUTH_BETA)
            .send_json(&body)
        {
            Ok(response) => response,
            Err(ureq::Error::Status(code @ (401 | 403), _)) => {
                diagnose::log(format!(
                    "messages endpoint returned auth error status {code}; re-login required"
                ));
                return Err(PollError::AuthRequired);
            }
            // A rejected request still carries the rate-limit headers, which is
            // the whole point of the call.
            Err(ureq::Error::Status(_, response)) => response,
            Err(_) => continue,
        };

        let has_headers = ["5h-utilization", "7d-utilization", "status"]
            .iter()
            .any(|suffix| {
                response
                    .header(&format!("anthropic-ratelimit-unified-{suffix}"))
                    .is_some()
            });
        if has_headers {
            return Ok(parse_rate_limit_headers(&response));
        }
    }

    Err(PollError::RequestFailed)
}

fn parse_rate_limit_headers(response: &ureq::Response) -> UsageData {
    let mut data = UsageData {
        session: UsageSection::new(
            get_header_f64(response, "anthropic-ratelimit-unified-5h-utilization") * 100.0,
            get_header_i64(response, "anthropic-ratelimit-unified-5h-reset"),
        ),
        weekly: UsageSection::new(
            get_header_f64(response, "anthropic-ratelimit-unified-7d-utilization") * 100.0,
            get_header_i64(response, "anthropic-ratelimit-unified-7d-reset"),
        ),
        ..Default::default()
    };

    if data.session.percentage > 0.0 || data.weekly.percentage > 0.0 {
        return data;
    }

    // A rejected request reports no utilisation at all, so the representative
    // claim is the only signal for which window is exhausted.
    if response.header("anthropic-ratelimit-unified-status") == Some("rejected") {
        match response.header("anthropic-ratelimit-unified-representative-claim") {
            Some("five_hour") => data.session.percentage = 100.0,
            Some("seven_day") => data.weekly.percentage = 100.0,
            _ => {}
        }
    }

    if data.session.resets_at.is_none() {
        data.session.resets_at = get_header_i64(response, "anthropic-ratelimit-unified-reset");
    }

    data
}

fn read_first_credentials() -> Option<Credentials> {
    credential_sources_in_order().find_map(|source| read_credentials_from_source(&source))
}

/// Credential sources, cheapest first: a file read beats spawning a keyring
/// helper, so a normal CLI login never pays for the secret-store probe.
pub(super) fn credential_sources_in_order() -> impl Iterator<Item = CredentialSource> {
    credentials_paths()
        .into_iter()
        .map(CredentialSource::File)
        .chain(SECRET_TARGETS.iter().copied().map(CredentialSource::Secret))
}

fn credentials_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = non_empty_environment(CREDENTIALS_FILE_ENV).map(PathBuf::from) {
        paths.push(path);
    }
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".claude").join(".credentials.json"));
        paths.push(
            home.join(".config")
                .join("claude")
                .join(".credentials.json"),
        );
    }
    paths
}

fn read_credentials_from_source(source: &CredentialSource) -> Option<Credentials> {
    match source {
        CredentialSource::File(path) => {
            let content = std::fs::read_to_string(path)
                .inspect_err(|error| {
                    if diagnose::is_enabled() {
                        diagnose::log_error(
                            &format!("unable to read credentials at {}", path.display()),
                            error,
                        );
                    }
                })
                .ok()?;
            parse_credentials(&content, source.clone())
        }
        CredentialSource::Secret(target) => {
            let secret = secret_store::read_secret(target)?;
            parse_credentials(&secret, source.clone())
        }
    }
}

fn parse_credentials(content: &str, source: CredentialSource) -> Option<Credentials> {
    let json: serde_json::Value = serde_json::from_str(content).ok()?;
    let oauth = json.get("claudeAiOauth")?;
    Some(Credentials {
        access_token: oauth.get("accessToken")?.as_str()?.to_string(),
        expires_at: oauth.get("expiresAt").and_then(serde_json::Value::as_i64),
        source,
    })
}

/// Walk the credential sources until one yields a live token, optionally
/// nudging the Claude CLI to refresh an expired one along the way.
fn refresh_or_fallback(
    mut credentials: Credentials,
    options: PollOptions,
) -> Result<Credentials, PollError> {
    loop {
        if !is_token_expired(credentials.expires_at) {
            return Ok(credentials);
        }

        let source = credentials.source.clone();
        if options.refresh_tokens {
            cli_refresh_token();
            match read_credentials_from_source(&source) {
                Some(refreshed) if !is_token_expired(refreshed.expires_at) => return Ok(refreshed),
                _ => diagnose::log(format!("credentials from {source:?} still expired")),
            }
        }

        match read_next_credentials_after(&source) {
            Some(next) => credentials = next,
            None => return Err(PollError::TokenExpired),
        }
    }
}

fn read_next_credentials_after(source: &CredentialSource) -> Option<Credentials> {
    credential_sources_in_order()
        .skip_while(|candidate| candidate != source)
        .skip(1)
        .find_map(|candidate| read_credentials_from_source(&candidate))
}

/// `claude -p .` is the cheapest command that makes the CLI refresh its own
/// OAuth token. The environment variables are cleared so the CLI does not think
/// it is being run from inside a Claude Code session.
fn cli_refresh_token() {
    diagnose::log("attempting Claude token refresh via the claude CLI");
    let refreshed = run_with_timeout(
        Command::new(resolve_claude_path())
            .args(["-p", "."])
            .env_remove("CLAUDECODE")
            .env_remove("CLAUDE_CODE_ENTRYPOINT")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
        REFRESH_TIMEOUT,
    );
    if refreshed.is_none() {
        diagnose::log("Claude token refresh did not complete");
    }
}

fn resolve_claude_path() -> PathBuf {
    let local = dirs::home_dir().map(|home| home.join(".local").join("bin").join("claude"));
    match local {
        Some(path) if path.is_file() => path,
        // Anything else is left to PATH resolution.
        _ => PathBuf::from("claude"),
    }
}

fn is_token_expired(expires_at: Option<i64>) -> bool {
    // The credentials file stores milliseconds.
    expires_at.is_some_and(|expires_at| now_unix() * 1_000 >= expires_at)
}

/// Fingerprint of every credential source, used to notice a re-login between
/// polls without keeping the token itself around.
pub(super) fn credential_watch_snapshot() -> Vec<String> {
    let mut snapshot: Vec<String> = credential_sources_in_order()
        .map(|source| match source {
            CredentialSource::File(path) => super::file_watch_signature(&path),
            CredentialSource::Secret(target) => secret_store::watch_signature(target),
        })
        .collect();
    snapshot.sort();
    snapshot.dedup();
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_parse_out_of_the_cli_file_format() {
        let credentials = parse_credentials(
            r#"{"claudeAiOauth":{"accessToken":"tok","expiresAt":1755000000000}}"#,
            CredentialSource::File(PathBuf::from("/tmp/x")),
        )
        .unwrap();
        assert_eq!(credentials.access_token, "tok");
        assert_eq!(credentials.expires_at, Some(1_755_000_000_000));
    }

    #[test]
    fn credentials_without_an_oauth_block_are_rejected() {
        assert!(parse_credentials("{}", CredentialSource::Secret("x")).is_none());
        assert!(parse_credentials("not json", CredentialSource::Secret("x")).is_none());
    }

    #[test]
    fn tokens_without_an_expiry_are_treated_as_live() {
        assert!(!is_token_expired(None));
        assert!(is_token_expired(Some(0)));
    }

    #[test]
    fn file_sources_are_probed_before_the_keyring() {
        let sources: Vec<_> = credential_sources_in_order().collect();
        let first_secret = sources
            .iter()
            .position(|source| matches!(source, CredentialSource::Secret(_)));
        let last_file = sources
            .iter()
            .rposition(|source| matches!(source, CredentialSource::File(_)));
        assert!(last_file < first_secret);
    }

    #[test]
    fn watch_snapshots_cover_every_source_once() {
        let snapshot = credential_watch_snapshot();
        assert!(!snapshot.is_empty());
        let mut sorted = snapshot.clone();
        sorted.dedup();
        assert_eq!(sorted.len(), snapshot.len());
    }
}

//! Claude Code usage.
//!
//! Ported from the upstream Windows monitor (`src/poller/claude.rs`). The
//! endpoints, the OAuth beta header, and the rate-limit header names are
//! upstream's; credential discovery is rewritten for Linux — no WSL probing, no
//! DPAPI-encrypted desktop token cache, and the CLI is a plain `claude` on
//! `PATH` rather than a `.cmd` shim.
//!
//! Brought up to upstream v2.18: the retired Claude 3 Haiku probe is gone, a
//! rate-limited usage endpoint is a back-off rather than a reason to spend a
//! Messages request, and the per-model weekly caps and extra-usage spend the
//! endpoint reports are passed through.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;

use super::secret_store;
use super::{
    build_agent, get_header_f64, get_header_i64, non_empty_environment, parse_iso8601,
    request_error, run_with_timeout, PollError, PollOptions,
};
use crate::diagnose;
use crate::models::{now_unix, slug, Credits, UsageData, UsageLimit, UsageSection};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const OAUTH_BETA: &str = "oauth-2025-04-20";
// The header probe stays on the cheapest tier. Claude 3 Haiku is retired; the
// alias follows Haiku 4.5 snapshots until that generation retires too.
const MODEL_FALLBACK_CHAIN: &[&str] = &["claude-haiku-4-5"];
const CREDENTIALS_FILE_ENV: &str = "CLAUDE_CREDENTIALS_FILE";
const SECRET_TARGETS: &[&str] = &["Claude Code", "Claude Code-credentials", "claude.ai"];
const REFRESH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct UsageResponse {
    five_hour: Option<UsageBucket>,
    seven_day: Option<UsageBucket>,
    /// Paid usage past the plan, in minor currency units.
    spend: Option<SpendResponse>,
    /// The newer list form of every quota on the account.
    limits: Option<Vec<serde_json::Value>>,
    /// Everything else: `seven_day_opus`, `seven_day_sonnet`, `extra_usage`,
    /// and whatever bucket Anthropic adds next.
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct UsageBucket {
    utilization: f64,
    resets_at: Option<String>,
}

#[derive(Deserialize)]
struct SpendResponse {
    #[serde(default)]
    enabled: bool,
    used: Option<SpendAmount>,
    limit: Option<SpendAmount>,
}

/// An amount in minor units with its own exponent, so the scale is
/// self-describing.
#[derive(Deserialize)]
struct SpendAmount {
    amount_minor: f64,
    #[serde(default)]
    exponent: u32,
}

impl SpendAmount {
    fn major(&self) -> f64 {
        self.amount_minor / 10f64.powi(self.exponent.min(18) as i32)
    }
}

/// One entry of the `limits` array.
#[derive(Deserialize)]
struct LimitEntry {
    kind: String,
    #[serde(alias = "utilization")]
    percent: f64,
    resets_at: Option<String>,
    scope: Option<serde_json::Value>,
    #[serde(default)]
    is_active: bool,
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
    /// `pro`, `max`, `team`… as the CLI recorded it at sign-in.
    subscription: Option<String>,
    source: CredentialSource,
}

pub fn poll(options: PollOptions) -> Result<UsageData, PollError> {
    let credentials = read_first_credentials().ok_or_else(|| {
        diagnose::log("Claude poll failed: no credentials found");
        PollError::NoCredentials
    })?;
    let credentials = refresh_or_fallback(credentials, options)?;
    let mut data = fetch_usage_with_fallback(&credentials.access_token)?;
    data.plan = credentials.subscription;
    Ok(data)
}

/// Ask the usage endpoint first, then fall back to the rate-limit headers on a
/// throwaway Messages request. Upstream's two-step, kept intact: the dedicated
/// endpoint sometimes omits reset instants that the headers do carry.
fn fetch_usage_with_fallback(token: &str) -> Result<UsageData, PollError> {
    if let Some(data) = try_usage_endpoint(token)? {
        let missing_reset =
            |section: &UsageSection| section.available && section.resets_at.is_none();
        if missing_reset(&data.session) || missing_reset(&data.weekly) {
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

/// `Ok(None)` means the endpoint is not usable on this account and the
/// Messages probe should answer instead. A rate limit, a server fault, or a
/// network failure is an error: the probe would spend real quota to read
/// headers, and during a rate limit it adds to the load that caused it.
fn try_usage_endpoint(token: &str) -> Result<Option<UsageData>, PollError> {
    let response = match build_agent()?
        .get(USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", OAUTH_BETA)
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(code, _))
            if !is_transient_status(code) && code != 401 && code != 403 =>
        {
            diagnose::log(format!(
                "usage endpoint unavailable for this account (status {code}); trying the Messages API"
            ));
            return Ok(None);
        }
        Err(error) => return Err(request_error(error, "Claude usage endpoint")),
    };

    let response = match response.into_json::<UsageResponse>() {
        Ok(response) => response,
        Err(error) => {
            diagnose::log_error("unexpected Claude usage response", error);
            return Ok(None);
        }
    };

    let data = usage_from_response(response);
    if data.sections().next().is_none() {
        diagnose::log("Claude usage response carried no usable limits");
        return Ok(None);
    }
    Ok(Some(data))
}

fn is_transient_status(code: u16) -> bool {
    code == 429 || code >= 500
}

fn usage_from_response(response: UsageResponse) -> UsageData {
    let section = |bucket: &UsageBucket| {
        UsageSection::new(
            bucket.utilization,
            parse_iso8601(bucket.resets_at.as_deref()),
        )
    };
    let mut data = UsageData {
        session: response.five_hour.as_ref().map(section).unwrap_or_default(),
        weekly: response.seven_day.as_ref().map(section).unwrap_or_default(),
        ..Default::default()
    };

    // Newer responses can carry the headline windows only in the array; they
    // fill in, but never replace, the legacy fields.
    let mut limits = parse_limits(response.limits.as_deref().unwrap_or_default());
    limits.retain(|(limit, scoped)| {
        let headline = match limit.key.as_str() {
            "session" if !scoped => &mut data.session,
            "weekly_all" if !scoped => &mut data.weekly,
            _ => return true,
        };
        if !headline.available {
            *headline = limit.usage;
        }
        false
    });
    let mut limits: Vec<UsageLimit> = limits.into_iter().map(|(limit, _)| limit).collect();
    for limit in parse_legacy_buckets(&response.extra) {
        // The array is authoritative when both forms name the same cap.
        if !limits.iter().any(|existing| existing.label == limit.label) {
            limits.push(limit);
        }
    }
    limits.sort_by(|left, right| left.key.cmp(&right.key));
    limits.dedup_by(|left, right| left.key == right.key);
    data.limits = limits;

    data.credits = response.spend.as_ref().and_then(credits_from_spend);
    data
}

/// The `limits` array. Each entry comes back with whether it is scoped (to a
/// model, a team…), since only unscoped `session` and `weekly_all` entries can
/// stand in for the headline windows.
fn parse_limits(entries: &[serde_json::Value]) -> Vec<(UsageLimit, bool)> {
    entries
        .iter()
        .filter_map(|entry| {
            let Ok(limit) = serde_json::from_value::<LimitEntry>(entry.clone()) else {
                diagnose::log("ignored a malformed entry in the Claude limits array");
                return None;
            };
            if limit.kind.trim().is_empty() || !limit.percent.is_finite() || limit.percent < 0.0 {
                return None;
            }
            let model = limit
                .scope
                .as_ref()
                .and_then(|scope| scope.get("model"))
                .and_then(|model| {
                    ["display_name", "id"].iter().find_map(|field| {
                        model
                            .get(field)
                            .and_then(serde_json::Value::as_str)
                            .filter(|value| !value.trim().is_empty())
                    })
                })
                .map(str::to_string);
            let mut key = slug(&limit.kind);
            if let Some(model) = &model {
                key = format!("{key}_{}", slug(model));
            }
            let label = limit_label(&limit.kind, model.as_deref());
            Some((
                UsageLimit {
                    key,
                    label,
                    active: limit.is_active,
                    usage: UsageSection::new(
                        limit.percent,
                        parse_iso8601(limit.resets_at.as_deref()),
                    ),
                },
                limit.scope.is_some(),
            ))
        })
        .collect()
}

/// Older top-level buckets such as `seven_day_opus`. Only objects with a
/// numeric utilisation are quotas; `null` means the account has no such cap.
fn parse_legacy_buckets(extra: &BTreeMap<String, serde_json::Value>) -> Vec<UsageLimit> {
    extra
        .iter()
        .filter_map(|(key, value)| {
            let percentage = value
                .get("utilization")
                .and_then(serde_json::Value::as_f64)
                .filter(|value| value.is_finite() && *value >= 0.0)?;
            let label = match key.as_str() {
                "extra_usage" => "Extra usage".to_string(),
                _ => {
                    let (kind, rest) = ["seven_day_", "five_hour_"]
                        .iter()
                        .find_map(|prefix| Some((*prefix, key.strip_prefix(prefix)?)))
                        .unwrap_or(("", key.as_str()));
                    limit_label(kind, Some(&capitalize(&rest.replace('_', " "))))
                }
            };
            Some(UsageLimit {
                key: slug(key),
                label,
                active: value
                    .get("is_active")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                usage: UsageSection::new(
                    percentage,
                    parse_iso8601(value.get("resets_at").and_then(serde_json::Value::as_str)),
                ),
            })
        })
        .collect()
}

/// `Opus 7d`, `Sonnet 7d`, `daily team`: the model (or what the quota covers)
/// first, then the window when the kind names one.
fn limit_label(kind: &str, subject: Option<&str>) -> String {
    let kind = kind.to_ascii_lowercase();
    let window = if kind.starts_with("weekly") || kind.starts_with("seven_day") {
        Some("7d")
    } else if kind.starts_with("session") || kind.starts_with("five_hour") {
        Some("5h")
    } else if kind.starts_with("monthly") {
        Some("30d")
    } else if kind.starts_with("daily") {
        Some("1d")
    } else {
        None
    };
    match (subject, window) {
        (Some(subject), Some(window)) => format!("{subject} {window}"),
        (Some(subject), None) => subject.to_string(),
        (None, _) => capitalize(&kind.replace('_', " ")),
    }
}

fn capitalize(value: &str) -> String {
    let mut characters = value.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// Extra usage the account has switched on. Shown whenever it is enabled, not
/// only once a window runs out: the popup is where people check how much
/// headroom they have left.
fn credits_from_spend(spend: &SpendResponse) -> Option<Credits> {
    let limit = spend.limit.as_ref()?.major();
    if !spend.enabled || !limit.is_finite() || limit <= 0.0 {
        return None;
    }
    let used = spend
        .used
        .as_ref()
        .map(SpendAmount::major)
        .filter(|used| used.is_finite())
        .unwrap_or(0.0);
    Some(Credits {
        used: Some(used),
        limit: Some(limit),
        remaining: Some((limit - used).max(0.0)),
    })
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
            Err(error @ ureq::Error::Status(401 | 403, _)) => {
                return Err(request_error(error, "Claude Messages endpoint"));
            }
            // A rejected request — a 429 included — still carries the
            // rate-limit headers, which is the whole point of the call.
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
    let has = |name: &str| response.header(name).is_some();
    let claim = response.header("anthropic-ratelimit-unified-representative-claim");
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
    // A window is only there if the headers mention it; a reset shared by the
    // whole account does not prove a five-hour window exists.
    data.session.available = has("anthropic-ratelimit-unified-5h-utilization")
        || has("anthropic-ratelimit-unified-5h-reset")
        || claim == Some("five_hour");
    data.weekly.available = has("anthropic-ratelimit-unified-7d-utilization")
        || has("anthropic-ratelimit-unified-7d-reset")
        || claim == Some("seven_day");

    if data.session.percentage > 0.0 || data.weekly.percentage > 0.0 {
        return data;
    }

    // A rejected request reports no utilisation at all, so the representative
    // claim is the only signal for which window is exhausted.
    if response.header("anthropic-ratelimit-unified-status") == Some("rejected") {
        match claim {
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
        subscription: oauth
            .get("subscriptionType")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string),
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
        assert_eq!(credentials.subscription, None);

        let credentials = parse_credentials(
            r#"{"claudeAiOauth":{"accessToken":"tok","subscriptionType":"max"}}"#,
            CredentialSource::Secret("x"),
        )
        .unwrap();
        assert_eq!(credentials.subscription.as_deref(), Some("max"));
    }

    fn usage(json: &str) -> UsageData {
        usage_from_response(serde_json::from_str(json).unwrap())
    }

    #[test]
    fn the_legacy_response_fills_both_windows_and_the_model_caps() {
        let data = usage(
            r#"{
                "five_hour": {"utilization": 6.0, "resets_at": "2026-10-12T18:00:00+00:00"},
                "seven_day": {"utilization": 35.0, "resets_at": "2026-10-15T08:00:00+00:00"},
                "seven_day_oauth_apps": null,
                "seven_day_opus": {"utilization": 12.0, "resets_at": null},
                "seven_day_sonnet": {"utilization": 41.5, "resets_at": "2026-10-15T08:00:00+00:00"},
                "extra_usage": {"is_enabled": false, "utilization": null},
                "iguana_necktie": null
            }"#,
        );
        assert!(data.session.available && data.weekly.available);
        assert_eq!(data.session.percentage, 6.0);
        let labels: Vec<_> = data
            .limits
            .iter()
            .map(|limit| limit.label.as_str())
            .collect();
        assert_eq!(labels, ["Opus 7d", "Sonnet 7d"]);
        assert_eq!(data.limits[1].usage.percentage, 41.5);
        assert!(data.limits[1].usage.resets_at.is_some());
        assert!(data.credits.is_none());
    }

    #[test]
    fn the_limits_array_stands_in_for_missing_headline_windows() {
        let data = usage(
            r#"{
                "limits": [
                    {"kind": "session", "percent": 14, "resets_at": "2026-10-12T18:00:00Z"},
                    {"kind": "weekly_all", "percent": 33},
                    {"kind": "weekly_scoped", "percent": 50, "is_active": true,
                     "scope": {"model": {"id": "claude-opus", "display_name": "Opus"}}},
                    {"kind": "weekly_scoped"},
                    null
                ],
                "seven_day_opus": {"utilization": 10}
            }"#,
        );
        assert_eq!(data.session.percentage, 14.0);
        assert_eq!(data.weekly.percentage, 33.0);
        // The array's Opus cap wins over the legacy bucket of the same name.
        assert_eq!(data.limits.len(), 1);
        assert_eq!(data.limits[0].label, "Opus 7d");
        assert_eq!(data.limits[0].usage.percentage, 50.0);
        assert!(data.limits[0].active);
    }

    #[test]
    fn enabled_extra_usage_becomes_credits() {
        let data = usage(
            r#"{
                "five_hour": {"utilization": 100.0},
                "spend": {"enabled": true,
                          "used": {"amount_minor": 1250, "exponent": 2},
                          "limit": {"amount_minor": 5000, "exponent": 2}}
            }"#,
        );
        let credits = data.credits.unwrap();
        assert_eq!(credits.used, Some(12.5));
        assert_eq!(credits.limit, Some(50.0));
        assert_eq!(credits.remaining, Some(37.5));
        assert_eq!(credits.percentage(), Some(25.0));

        let disabled = usage(
            r#"{"spend": {"enabled": false, "limit": {"amount_minor": 5000, "exponent": 2}}}"#,
        );
        assert!(disabled.credits.is_none());
    }

    #[test]
    fn rate_limits_and_outages_are_not_a_reason_to_probe_messages() {
        assert!(is_transient_status(429));
        assert!(is_transient_status(503));
        assert!(!is_transient_status(404));
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

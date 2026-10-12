//! Provider polling.
//!
//! Ported from the upstream Windows monitor (`src/poller.rs`). The shape of the
//! result changed: upstream collapsed a whole poll into one error when nothing
//! answered, which suits a single-widget tray icon. The applet draws one row per
//! provider, so every provider carries its own outcome here.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::diagnose;
use crate::models::UsageData;
use crate::providers::{ProviderId, ProviderSet};

pub mod antigravity;
pub mod claude;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod opencode;
mod secret_store;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Ceiling on a stored back-off. A bad `Retry-After` header must not be able to
/// leave the panel on stale numbers for a day.
pub const MAX_RETRY_AFTER_SECS: i64 = 60 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PollError {
    /// No credentials on disk — the provider's CLI has never signed in here.
    NoCredentials,
    /// Credentials exist but the service rejected them.
    AuthRequired,
    /// The stored token is past its expiry and could not be refreshed.
    TokenExpired,
    /// Network failure, unexpected payload, or an unhandled status code.
    RequestFailed,
    /// The service asked us to back off (HTTP 429, or a 5xx carrying
    /// `Retry-After`). Polls skip the provider until `retry_after` seconds
    /// have passed, as upstream does since v2.15.
    RateLimited { retry_after: Option<i64> },
}

impl PollError {
    pub fn key(self) -> &'static str {
        match self {
            Self::NoCredentials => "no_credentials",
            Self::AuthRequired => "auth_required",
            Self::TokenExpired => "token_expired",
            Self::RequestFailed => "request_failed",
            Self::RateLimited { .. } => "rate_limited",
        }
    }

    /// Human-readable hint pointing at the fix, shown in the applet tooltip.
    pub fn hint(self, provider: ProviderId) -> String {
        let name = provider.descriptor().display_name;
        match self {
            Self::NoCredentials => format!("No {name} credentials found — sign in to {name} first"),
            Self::AuthRequired => format!("{name} rejected the stored token — sign in again"),
            Self::TokenExpired => format!("The {name} token expired — sign in again"),
            Self::RequestFailed => format!("Could not reach the {name} usage endpoint"),
            Self::RateLimited { .. } => {
                format!("{name} is rate limiting usage requests — showing the last known numbers")
            }
        }
    }

    /// Seconds the provider asked us to wait, when it said.
    pub fn retry_after(self) -> Option<i64> {
        match self {
            Self::RateLimited { retry_after } => retry_after,
            _ => None,
        }
    }
}

/// Map a failed request onto the outcome the applet reports, logging it.
///
/// One place for every provider, so a 429 is a back-off everywhere rather than
/// a generic failure that the next poll walks straight back into.
pub(crate) fn request_error(error: ureq::Error, context: &str) -> PollError {
    match error {
        ureq::Error::Status(code @ (401 | 403), _) => {
            diagnose::log(format!("{context} returned auth error status {code}"));
            PollError::AuthRequired
        }
        ureq::Error::Status(code, response) => {
            let retry_after = retry_after_secs(&response, crate::models::now_unix());
            diagnose::log(format!(
                "{context} returned status {code} (retry-after: {retry_after:?})"
            ));
            if code == 429 || (code >= 500 && retry_after.is_some()) {
                PollError::RateLimited { retry_after }
            } else {
                PollError::RequestFailed
            }
        }
        error => {
            diagnose::log_error(&format!("{context} request failed"), error);
            PollError::RequestFailed
        }
    }
}

/// `Retry-After` as seconds from `now`, capped at [`MAX_RETRY_AFTER_SECS`].
pub(crate) fn retry_after_secs(response: &ureq::Response, now: i64) -> Option<i64> {
    parse_retry_after(response.header("retry-after")?, now)
}

fn parse_retry_after(value: &str, now: i64) -> Option<i64> {
    let value = value.trim();
    let seconds = if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        value.parse::<i64>().unwrap_or(MAX_RETRY_AFTER_SECS)
    } else {
        parse_http_date(value)? - now
    };
    (seconds > 0).then(|| seconds.min(MAX_RETRY_AFTER_SECS))
}

/// The IMF-fixdate form of an HTTP date, `Sun, 06 Nov 1994 08:49:37 GMT` — the
/// only one servers are allowed to send.
fn parse_http_date(value: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut parts = value.split_once(", ")?.1.split(' ');
    let day: i64 = parts.next()?.parse().ok()?;
    let month = parts.next()?;
    let month = MONTHS.iter().position(|name| *name == month)? as i64 + 1;
    let year: i64 = parts.next()?.parse().ok()?;
    let time = parts.next()?;
    if parts.next() != Some("GMT") || parts.next().is_some() {
        return None;
    }
    parse_iso8601(Some(&format!("{year:04}-{month:02}-{day:02}T{time}Z")))
}

/// Knobs the CLI and the applet can turn per run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PollOptions {
    /// Spawn the provider's CLI to refresh an expired token.
    pub refresh_tokens: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderPoll {
    pub provider: ProviderId,
    pub result: Result<UsageData, PollError>,
}

/// Poll every enabled provider, keeping each outcome separate.
///
/// `cooldown` reports how long a provider still has to wait after asking us to
/// back off; those are not contacted at all this round.
pub fn poll(
    enabled_providers: ProviderSet,
    options: PollOptions,
    cooldown: impl Fn(ProviderId) -> Option<i64>,
) -> Vec<ProviderPoll> {
    poll_with(enabled_providers, |provider| match cooldown(provider) {
        Some(remaining) => {
            diagnose::log(format!(
                "{} is backing off for another {remaining}s",
                provider.descriptor().display_name
            ));
            Err(PollError::RateLimited {
                retry_after: Some(remaining),
            })
        }
        None => poll_provider(provider, options),
    })
}

fn poll_with(
    enabled_providers: ProviderSet,
    mut poll_provider: impl FnMut(ProviderId) -> Result<UsageData, PollError>,
) -> Vec<ProviderPoll> {
    enabled_providers
        .iter()
        .map(|provider| {
            let result = poll_provider(provider);
            if let Err(error) = &result {
                diagnose::log(format!(
                    "{} usage poll failed: {error:?}",
                    provider.descriptor().display_name
                ));
            }
            ProviderPoll { provider, result }
        })
        .collect()
}

fn poll_provider(provider: ProviderId, options: PollOptions) -> Result<UsageData, PollError> {
    match provider {
        ProviderId::Claude => claude::poll(options),
        ProviderId::Codex => codex::poll(options),
        ProviderId::Antigravity => antigravity::poll(options),
        ProviderId::OpenCode => opencode::poll(options),
        ProviderId::Cursor => cursor::poll(options),
        ProviderId::Copilot => copilot::poll(options),
    }
}

/// Fingerprint every enabled provider's credential sources.
///
/// Upstream watches these to re-poll the moment a user finishes signing in
/// rather than making them wait out the poll interval; `--watch` does the same.
pub fn credential_watch_snapshot(enabled_providers: ProviderSet) -> Vec<String> {
    enabled_providers
        .iter()
        .flat_map(|provider| match provider {
            ProviderId::Claude => claude::credential_watch_snapshot(),
            ProviderId::Codex => codex::credential_watch_snapshot(),
            ProviderId::Antigravity => antigravity::credential_watch_snapshot(),
            ProviderId::OpenCode => opencode::credential_watch_snapshot(),
            ProviderId::Cursor => cursor::credential_watch_snapshot(),
            ProviderId::Copilot => copilot::credential_watch_snapshot(),
        })
        .collect()
}

pub(crate) fn build_agent() -> Result<ureq::Agent, PollError> {
    Ok(ureq::AgentBuilder::new().timeout(REQUEST_TIMEOUT).build())
}

pub(crate) fn get_header_f64(response: &ureq::Response, name: &str) -> f64 {
    response
        .header(name)
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0)
}

pub(crate) fn get_header_i64(response: &ureq::Response, name: &str) -> Option<i64> {
    response
        .header(name)
        .and_then(|value| value.parse::<i64>().ok())
}

/// Parse an ISO 8601 timestamp into Unix seconds.
///
/// Ported verbatim in spirit from upstream: a hand-rolled parser keeps `chrono`
/// and `time` out of the dependency tree for the handful of fields that need it.
/// Offsets are honoured; the APIs involved all emit UTC or an explicit offset.
pub(crate) fn parse_iso8601(value: Option<&str>) -> Option<i64> {
    let value = value?.trim();
    let (datetime, offset_secs) = split_offset(value)?;
    let (date, time) = datetime
        .split_once('T')
        .or_else(|| datetime.split_once(' '))?;

    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    // Upstream v2.4 started rejecting impossible dates instead of rolling them
    // into the next month; the year cap also keeps the day count loop short.
    if date_parts.next().is_some()
        || !(1..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
    {
        return None;
    }

    let mut time_parts = time.split('.').next().unwrap_or(time).split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next().unwrap_or("0").parse().ok()?;
    if time_parts.next().is_some()
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
        || !(0..61).contains(&second)
    {
        return None;
    }

    Some(
        days_from_epoch(year, month, day)? * 86_400 + hour * 3_600 + minute * 60 + second
            - offset_secs,
    )
}

/// Split a timestamp into its naive part and its UTC offset in seconds.
fn split_offset(value: &str) -> Option<(&str, i64)> {
    if let Some(stripped) = value.strip_suffix('Z').or_else(|| value.strip_suffix('z')) {
        return Some((stripped, 0));
    }

    // The offset sign is the last '+' or '-' that follows the time part, so the
    // date's own separators cannot be mistaken for one.
    let time_start = value.find('T').or_else(|| value.find(' '))?;
    let sign_index = value[time_start..]
        .rfind(['+', '-'])
        .map(|index| index + time_start);
    let Some(sign_index) = sign_index else {
        return Some((value, 0));
    };

    let (datetime, offset) = value.split_at(sign_index);
    let negative = offset.starts_with('-');
    let offset = &offset[1..];
    let (hours, minutes) = match offset.split_once(':') {
        Some((hours, minutes)) => (hours, minutes),
        None if offset.len() == 4 => offset.split_at(2),
        None => (offset, "0"),
    };
    let seconds = hours.parse::<i64>().ok()? * 3_600 + minutes.parse::<i64>().ok()? * 60;
    Some((datetime, if negative { -seconds } else { seconds }))
}

fn days_from_epoch(year: i64, month: i64, day: i64) -> Option<i64> {
    if year < 1970 {
        return None;
    }
    let mut days = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    const MONTH_DAYS: [i64; 13] = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 1..month {
        days += MONTH_DAYS[m as usize];
        if m == 2 && is_leap(year) {
            days += 1;
        }
    }
    Some(days + day - 1)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Cheap fingerprint of a credentials file: presence, size, and mtime. A
/// changed fingerprint means the user signed in or out since the last poll.
pub(crate) fn file_watch_signature(path: &Path) -> String {
    let key = format!("file:{}", path.display());
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|value| value.as_secs())
                .unwrap_or(0);
            format!("{key}|present|{}|{modified}", metadata.len())
        }
        Err(_) => format!("{key}|missing"),
    }
}

pub(crate) fn non_empty_environment(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Run a child process, killing it if it outstays `timeout`.
///
/// Upstream needed this to keep `wsl.exe` probes from hanging the UI thread;
/// here it guards the provider CLIs invoked for token refresh.
pub(crate) fn run_with_timeout(
    command: &mut std::process::Command,
    timeout: Duration,
) -> Option<std::process::Output> {
    let mut child = command.spawn().ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests;

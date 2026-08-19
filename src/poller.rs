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
pub mod cursor;
pub mod opencode;
mod secret_store;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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
}

impl PollError {
    pub fn key(self) -> &'static str {
        match self {
            Self::NoCredentials => "no_credentials",
            Self::AuthRequired => "auth_required",
            Self::TokenExpired => "token_expired",
            Self::RequestFailed => "request_failed",
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
        }
    }
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
pub fn poll(enabled_providers: ProviderSet, options: PollOptions) -> Vec<ProviderPoll> {
    poll_with(enabled_providers, |provider| {
        poll_provider(provider, options)
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
    if date_parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let mut time_parts = time.split('.').next().unwrap_or(time).split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next().unwrap_or("0").parse().ok()?;
    if time_parts.next().is_some() {
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

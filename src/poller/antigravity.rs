//! Antigravity (Google) usage.
//!
//! Ported from the upstream Windows monitor (`src/poller/antigravity.rs`). The
//! Code Assist endpoints, the quota-summary selection, and the
//! remaining-fraction maths are upstream's. Credential discovery is rewritten:
//! upstream reads the `gemini:antigravity` entry out of Windows Credential
//! Manager, so here the same target name is looked up in the desktop secret
//! store, with the on-disk auth files the Linux build writes tried first.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

use super::secret_store;
use super::{build_agent, non_empty_environment, parse_iso8601, PollError, PollOptions};
use crate::diagnose;
use crate::models::{UsageData, UsageSection};

const CREDENTIAL_TARGET: &str = "gemini:antigravity";
const AUTH_FILE_ENV: &str = "ANTIGRAVITY_AUTH_FILE";
const ENDPOINTS: &[&str] = &[
    "https://daily-cloudcode-pa.googleapis.com",
    "https://daily-cloudcode-pa.sandbox.googleapis.com",
    "https://cloudcode-pa.googleapis.com",
];

#[derive(Deserialize)]
struct AuthFile {
    token: TokenData,
}

#[derive(Deserialize)]
struct TokenData {
    access_token: String,
}

#[derive(Deserialize)]
struct LoadResponse {
    #[serde(rename = "cloudaicompanionProject")]
    project: Option<String>,
}

#[derive(Deserialize)]
struct ModelsResponse {
    models: HashMap<String, ModelInfo>,
}

#[derive(Deserialize)]
struct ModelInfo {
    #[serde(rename = "quotaInfo")]
    quota_info: Option<QuotaInfo>,
}

#[derive(Deserialize)]
struct QuotaInfo {
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
}

#[derive(Deserialize)]
struct QuotaSummaryResponse {
    groups: Option<Vec<QuotaSummaryGroup>>,
}

#[derive(Deserialize)]
struct QuotaSummaryGroup {
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    description: Option<String>,
    buckets: Option<Vec<QuotaSummaryBucket>>,
}

#[derive(Clone, Deserialize)]
struct QuotaSummaryBucket {
    #[serde(rename = "bucketId")]
    bucket_id: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    window: Option<String>,
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
}

pub fn poll(_options: PollOptions) -> Result<UsageData, PollError> {
    let token = read_credentials().ok_or_else(|| {
        diagnose::log("Antigravity poll failed: no credentials found");
        PollError::NoCredentials
    })?;
    fetch_usage(&token.access_token)
}

/// The three hosts serve the same API to different cohorts, so each is tried
/// before giving up. An auth failure anywhere outranks a transport failure —
/// the token is the thing the user has to fix.
fn fetch_usage(token: &str) -> Result<UsageData, PollError> {
    let mut auth_error = false;
    let mut last_error = PollError::RequestFailed;

    for base_url in ENDPOINTS {
        match fetch_usage_from_endpoint(base_url, token) {
            Ok(data) => return Ok(data),
            Err(PollError::AuthRequired) => auth_error = true,
            Err(error) => last_error = error,
        }
    }

    Err(if auth_error {
        PollError::AuthRequired
    } else {
        last_error
    })
}

fn fetch_usage_from_endpoint(base_url: &str, token: &str) -> Result<UsageData, PollError> {
    let project = fetch_project(base_url, token)?;

    if let Some(project) = project.as_deref() {
        match fetch_quota_summary(base_url, token, project) {
            Ok(data) => return Ok(data),
            Err(PollError::AuthRequired) => return Err(PollError::AuthRequired),
            Err(error) => diagnose::log(format!(
                "Antigravity retrieveUserQuotaSummary failed, falling back to model quota: {error:?}"
            )),
        }
    }

    Ok(UsageData {
        session: fetch_model_quota(base_url, token, project.as_deref())?,
        ..Default::default()
    })
}

fn post_json<T: serde::de::DeserializeOwned>(
    url: &str,
    token: &str,
    body: serde_json::Value,
    context: &str,
) -> Result<T, PollError> {
    let response = match build_agent()?
        .post(url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .set("User-Agent", "antigravity")
        .send_json(&body)
    {
        Ok(response) => response,
        Err(ureq::Error::Status(code @ (401 | 403), _)) => {
            diagnose::log(format!("Antigravity {context} returned auth error {code}"));
            return Err(PollError::AuthRequired);
        }
        Err(error) => {
            diagnose::log_error(&format!("Antigravity {context} request failed"), error);
            return Err(PollError::RequestFailed);
        }
    };

    response.into_json().map_err(|error| {
        diagnose::log_error(
            &format!("unable to parse Antigravity {context} response"),
            error,
        );
        PollError::RequestFailed
    })
}

fn fetch_project(base_url: &str, token: &str) -> Result<Option<String>, PollError> {
    let response: LoadResponse = post_json(
        &format!("{base_url}/v1internal:loadCodeAssist"),
        token,
        serde_json::json!({"metadata": {"ideType": "ANTIGRAVITY"}}),
        "loadCodeAssist",
    )?;
    Ok(response.project.filter(|project| !project.is_empty()))
}

fn fetch_model_quota(
    base_url: &str,
    token: &str,
    project: Option<&str>,
) -> Result<UsageSection, PollError> {
    let response: ModelsResponse = post_json(
        &format!("{base_url}/v1internal:fetchAvailableModels"),
        token,
        match project {
            Some(project) => serde_json::json!({"project": project}),
            None => serde_json::json!({}),
        },
        "fetchAvailableModels",
    )?;

    best_section(response.models.into_iter().filter_map(|(model, info)| {
        is_display_model(&model)
            .then_some(info.quota_info?)
            .and_then(section_from_quota)
    }))
    .ok_or(PollError::RequestFailed)
}

fn fetch_quota_summary(base_url: &str, token: &str, project: &str) -> Result<UsageData, PollError> {
    let response: QuotaSummaryResponse = post_json(
        &format!("{base_url}/v1internal:retrieveUserQuotaSummary"),
        token,
        serde_json::json!({"project": project}),
        "retrieveUserQuotaSummary",
    )?;
    usage_from_summary(response).ok_or(PollError::RequestFailed)
}

fn section_from_quota(quota: QuotaInfo) -> Option<UsageSection> {
    let remaining = quota.remaining_fraction?.clamp(0.0, 1.0);
    Some(UsageSection::new(
        (1.0 - remaining) * 100.0,
        parse_iso8601(quota.reset_time.as_deref()),
    ))
}

fn section_from_summary_bucket(bucket: &QuotaSummaryBucket) -> Option<UsageSection> {
    let remaining = bucket.remaining_fraction?.clamp(0.0, 1.0);
    Some(UsageSection::new(
        (1.0 - remaining) * 100.0,
        parse_iso8601(bucket.reset_time.as_deref()),
    ))
}

/// Antigravity groups quota by model family; the Gemini group is the one the
/// IDE actually spends, so it wins when present.
fn usage_from_summary(response: QuotaSummaryResponse) -> Option<UsageData> {
    let mut fallback = None;

    for group in response.groups.unwrap_or_default() {
        let is_gemini = is_gemini_summary_group(&group);
        let usage = usage_from_summary_group(group);

        if is_gemini && usage.is_some() {
            return usage;
        }
        if fallback.is_none() {
            fallback = usage;
        }
    }

    fallback
}

fn usage_from_summary_group(group: QuotaSummaryGroup) -> Option<UsageData> {
    let mut data = UsageData::default();
    let mut has_quota = false;

    for bucket in group.buckets.unwrap_or_default() {
        let Some(section) = section_from_summary_bucket(&bucket) else {
            continue;
        };
        match bucket.window.as_deref() {
            Some(window) if window.eq_ignore_ascii_case("5h") => {
                data.session = section;
                has_quota = true;
            }
            Some(window) if window.eq_ignore_ascii_case("weekly") => {
                data.weekly = section;
                has_quota = true;
            }
            _ => {}
        }
    }

    has_quota.then_some(data)
}

fn is_gemini_summary_group(group: &QuotaSummaryGroup) -> bool {
    let mentions_gemini = |value: &Option<String>| {
        value
            .as_deref()
            .is_some_and(|text| text.to_ascii_lowercase().contains("gemini"))
    };

    mentions_gemini(&group.display_name)
        || mentions_gemini(&group.description)
        || group.buckets.as_ref().is_some_and(|buckets| {
            buckets.iter().any(|bucket| {
                bucket
                    .bucket_id
                    .as_deref()
                    .is_some_and(|id| id.to_ascii_lowercase().starts_with("gemini-"))
                    || mentions_gemini(&bucket.display_name)
            })
        })
}

/// Without a quota summary the closest thing to "how used up am I" is the
/// most-consumed model quota.
fn best_section<I: IntoIterator<Item = UsageSection>>(sections: I) -> Option<UsageSection> {
    sections.into_iter().max_by(|a, b| {
        a.percentage
            .partial_cmp(&b.percentage)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.resets_at.cmp(&b.resets_at))
    })
}

fn is_display_model(model: &str) -> bool {
    ["gemini", "claude", "gpt", "image", "imagen"]
        .iter()
        .any(|prefix| model.starts_with(prefix))
}

fn auth_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = non_empty_environment(AUTH_FILE_ENV).map(PathBuf::from) {
        paths.push(path);
    }
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".antigravity").join("auth.json"));
    }
    if let Some(config) = dirs::config_dir() {
        paths.push(config.join("Antigravity").join("auth.json"));
    }
    paths
}

fn read_credentials() -> Option<TokenData> {
    let from_file = auth_paths()
        .into_iter()
        .find_map(|path| std::fs::read_to_string(path).ok());
    let content = match from_file {
        Some(content) => content,
        None => secret_store::read_secret(CREDENTIAL_TARGET)?,
    };
    parse_credentials(&content)
}

fn parse_credentials(content: &str) -> Option<TokenData> {
    let auth: AuthFile = serde_json::from_str(content).ok()?;
    (!auth.token.access_token.is_empty()).then_some(auth.token)
}

pub(super) fn credential_watch_snapshot() -> Vec<String> {
    let mut snapshot: Vec<String> = auth_paths()
        .iter()
        .map(|path| super::file_watch_signature(path))
        .collect();
    snapshot.push(secret_store::watch_signature(CREDENTIAL_TARGET));
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_fraction_becomes_consumed_percentage() {
        let section = section_from_quota(QuotaInfo {
            remaining_fraction: Some(0.25),
            reset_time: Some("2026-08-25T19:27:24Z".into()),
        })
        .unwrap();
        assert_eq!(section.percentage, 75.0);
        assert!(section.resets_at.is_some());
    }

    #[test]
    fn the_gemini_group_outranks_an_earlier_group() {
        let response: QuotaSummaryResponse = serde_json::from_str(
            r#"{"groups":[
                {"displayName":"Other","buckets":[
                    {"window":"5h","remainingFraction":0.9}
                ]},
                {"displayName":"Gemini 3 Pro","buckets":[
                    {"window":"5h","remainingFraction":0.5},
                    {"window":"weekly","remainingFraction":0.2}
                ]}
            ]}"#,
        )
        .unwrap();

        let data = usage_from_summary(response).unwrap();
        assert_eq!(data.session.percentage, 50.0);
        assert_eq!(data.weekly.percentage, 80.0);
    }

    #[test]
    fn a_non_gemini_group_is_still_better_than_nothing() {
        let response: QuotaSummaryResponse = serde_json::from_str(
            r#"{"groups":[{"displayName":"Other","buckets":[{"window":"weekly","remainingFraction":0.4}]}]}"#,
        )
        .unwrap();
        assert_eq!(
            usage_from_summary(response).unwrap().weekly.percentage,
            60.0
        );
    }

    #[test]
    fn groups_without_recognised_windows_report_no_usage() {
        let response: QuotaSummaryResponse = serde_json::from_str(
            r#"{"groups":[{"displayName":"Gemini","buckets":[{"window":"daily","remainingFraction":0.4}]}]}"#,
        )
        .unwrap();
        assert!(usage_from_summary(response).is_none());
    }

    #[test]
    fn the_most_consumed_model_quota_wins() {
        let section = best_section([
            UsageSection::new(10.0, None),
            UsageSection::new(80.0, Some(5)),
            UsageSection::new(40.0, None),
        ])
        .unwrap();
        assert_eq!(section.percentage, 80.0);
        assert!(!is_display_model("internal-router"));
        assert!(is_display_model("gemini-3-pro"));
    }

    #[test]
    fn credentials_need_a_non_empty_access_token() {
        assert!(parse_credentials(r#"{"token":{"access_token":""}}"#).is_none());
        assert_eq!(
            parse_credentials(r#"{"token":{"access_token":"abc"}}"#)
                .unwrap()
                .access_token,
            "abc"
        );
    }
}

use super::*;
use crate::models::UsageSection;
use crate::providers::ProviderId;

#[test]
fn iso8601_timestamps_parse_with_and_without_offsets() {
    // The Anthropic usage endpoint's format, with fractional seconds.
    assert_eq!(
        parse_iso8601(Some("2026-03-05T08:00:00.321598+00:00")),
        Some(1_772_697_600)
    );
    assert_eq!(
        parse_iso8601(Some("2026-03-05T08:00:00Z")),
        Some(1_772_697_600)
    );
    // An offset shifts the instant back to UTC.
    assert_eq!(
        parse_iso8601(Some("2026-03-05T10:00:00+02:00")),
        Some(1_772_697_600)
    );
    assert_eq!(
        parse_iso8601(Some("2026-03-05T06:00:00-0200")),
        Some(1_772_697_600)
    );
}

#[test]
fn epoch_and_leap_days_land_where_they_should() {
    assert_eq!(parse_iso8601(Some("1970-01-01T00:00:00Z")), Some(0));
    assert_eq!(
        parse_iso8601(Some("2024-02-29T00:00:00Z")),
        Some(1_709_164_800)
    );
}

#[test]
fn malformed_timestamps_are_rejected_rather_than_guessed() {
    assert_eq!(parse_iso8601(None), None);
    assert_eq!(parse_iso8601(Some("")), None);
    assert_eq!(parse_iso8601(Some("2026-03-05")), None);
    assert_eq!(parse_iso8601(Some("2026-13-05T00:00:00Z")), None);
    assert_eq!(parse_iso8601(Some("not a timestamp")), None);
    // Before the epoch there is nothing this monitor can express.
    assert_eq!(parse_iso8601(Some("1969-12-31T23:59:59Z")), None);
}

#[test]
fn every_enabled_provider_reports_its_own_outcome() {
    let providers =
        ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex, ProviderId::Cursor]);

    let polls = poll_with(providers, |provider| match provider {
        ProviderId::Claude => Ok(UsageData {
            session: UsageSection::new(12.0, None),
            ..Default::default()
        }),
        ProviderId::Codex => Err(PollError::NoCredentials),
        _ => Err(PollError::RequestFailed),
    });

    assert_eq!(polls.len(), 3);
    assert_eq!(polls[0].provider, ProviderId::Claude);
    assert!(polls[0].result.is_ok());
    assert_eq!(polls[1].result, Err(PollError::NoCredentials));
    assert_eq!(polls[2].result, Err(PollError::RequestFailed));
}

#[test]
fn polling_nothing_produces_nothing() {
    assert!(poll_with(ProviderSet::empty(), |_| Ok(UsageData::default())).is_empty());
}

#[test]
fn poll_errors_carry_a_stable_key_and_a_readable_hint() {
    assert_eq!(PollError::NoCredentials.key(), "no_credentials");
    assert_eq!(PollError::AuthRequired.key(), "auth_required");
    assert_eq!(PollError::TokenExpired.key(), "token_expired");
    assert_eq!(PollError::RequestFailed.key(), "request_failed");
    assert!(PollError::NoCredentials
        .hint(ProviderId::Claude)
        .contains("Claude Code"));
}

#[test]
fn timed_out_children_are_killed_rather_than_waited_on() {
    let output = run_with_timeout(
        std::process::Command::new("sleep").arg("30"),
        Duration::from_millis(200),
    );
    assert!(output.is_none());
}

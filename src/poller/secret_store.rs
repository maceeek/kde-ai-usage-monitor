//! Reading tokens out of the desktop's secret store.
//!
//! Upstream reads Windows Credential Manager through `CredReadW`. The Linux
//! equivalents are libsecret (the freedesktop Secret Service, which is what
//! Electron's `safeStorage`/`keytar` writes to) and KWallet. Both are queried
//! through their shipped CLIs so the binary keeps zero link-time dependencies
//! on a desktop library — a KDE box has `kwallet-query`, a GNOME-flavoured one
//! has `secret-tool`, and a box with neither simply reports no credentials.

use std::process::{Command, Stdio};
use std::time::Duration;

use crate::diagnose;
use crate::poller::run_with_timeout;

const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_WALLET: &str = "kdewallet";

/// Look one secret up by its Credential-Manager-style target name.
///
/// Returns the first non-empty match. Errors are logged, never propagated: a
/// missing helper is indistinguishable from a missing secret as far as the
/// caller is concerned, and both mean "this provider has no credentials".
pub(super) fn read_secret(target: &str) -> Option<String> {
    secret_tool_lookup(target)
        .or_else(|| kwallet_lookup(target))
        .filter(|secret| !secret.is_empty())
}

/// A cheap fingerprint of the secret's presence, used to notice a re-login
/// without holding the secret itself in memory between polls.
pub(super) fn watch_signature(target: &str) -> String {
    match read_secret(target) {
        Some(secret) => format!("secret:{target}|present|{}", secret.len()),
        None => format!("secret:{target}|missing"),
    }
}

fn secret_tool_lookup(target: &str) -> Option<String> {
    // Different writers file the same logical credential under different
    // attribute names, so try the ones Electron apps actually use.
    for attribute in ["service", "account", "application"] {
        let output = run_with_timeout(
            Command::new("secret-tool")
                .arg("lookup")
                .arg(attribute)
                .arg(target)
                .stdout(Stdio::piped())
                .stderr(Stdio::null()),
            LOOKUP_TIMEOUT,
        )?;
        if output.status.success() {
            let secret = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !secret.is_empty() {
                diagnose::log(format!(
                    "resolved secret {target} via secret-tool/{attribute}"
                ));
                return Some(secret);
            }
        }
    }
    None
}

fn kwallet_lookup(target: &str) -> Option<String> {
    // KWallet namespaces entries by folder; Electron apps store theirs under
    // either the app's own folder or the shared password folder.
    for folder in ["Passwords", "Chromium Safe Storage", target] {
        let output = run_with_timeout(
            Command::new("kwallet-query")
                .arg("-f")
                .arg(folder)
                .arg("-r")
                .arg(target)
                .arg(DEFAULT_WALLET)
                .stdout(Stdio::piped())
                .stderr(Stdio::null()),
            LOOKUP_TIMEOUT,
        )?;
        if output.status.success() {
            let secret = String::from_utf8_lossy(&output.stdout).trim().to_string();
            // kwallet-query reports a miss on stdout rather than by exit code.
            if !secret.is_empty() && !secret.starts_with("Failed to read entry") {
                diagnose::log(format!(
                    "resolved secret {target} via kwallet folder {folder}"
                ));
                return Some(secret);
            }
        }
    }
    None
}

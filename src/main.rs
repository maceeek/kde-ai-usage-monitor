//! `kde-ai-usage-monitor` — AI coding-assistant usage, as JSON for the Plasma
//! applet or as a line of text for a terminal.
//!
//! An AI-assisted Linux/KDE fork of CodeZeno/Claude-Code-Usage-Monitor (MIT),
//! which is a Win32 desktop widget. Here the polling lives in this binary and
//! the drawing lives in QML, because a Plasma applet cannot host a Win32 window
//! and should not host an HTTP client either.

mod cache;
mod config;
mod diagnose;
mod format;
mod linux_sqlite;
mod models;
mod poller;
mod providers;
mod snapshot;

use std::io::Write;

use crate::cache::Cache;
use crate::config::Config;
use crate::poller::{PollOptions, ProviderPoll};
use crate::providers::{ProviderId, ProviderSet, PROVIDER_DESCRIPTORS};
use crate::snapshot::Snapshot;

const USAGE: &str = "\
kde-ai-usage-monitor — AI coding-assistant usage for KDE Plasma

USAGE:
    kde-ai-usage-monitor [OPTIONS]

OPTIONS:
    -f, --format <json|text>  Output format (default: json)
    -p, --providers <LIST>    Comma-separated provider keys to poll, overriding
                              the config file (e.g. claude,codex)
    -w, --watch               Keep polling, printing one JSON document per line
    -i, --interval <SECONDS>  Seconds between polls in --watch mode
        --refresh-tokens      Let a provider's CLI refresh an expired token
        --no-cache            Ignore and do not update the last-known cache
        --list-providers      List the providers this build knows about
        --config              Print the config file path and its contents
        --save-config         Write the effective settings to the config file
                              and exit
        --diagnose            Log what the poller is doing to stderr
        --diagnose-append     As --diagnose, appending to the log file
    -h, --help                Show this help
    -V, --version             Show the version

The JSON document is the contract with the Plasma applet; see README.md.";

#[derive(Debug, PartialEq)]
enum Format {
    Json,
    Text,
}

#[derive(Debug)]
struct Args {
    format: Format,
    providers: Option<ProviderSet>,
    watch: bool,
    interval_secs: Option<u64>,
    refresh_tokens: bool,
    use_cache: bool,
}

enum Mode {
    Run(Args),
    ListProviders,
    ShowConfig,
    SaveConfig(Args),
    Help,
    Version,
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let mode = match parse_arguments(&arguments) {
        Ok(mode) => mode,
        Err(message) => {
            eprintln!("kde-ai-usage-monitor: {message}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    match mode {
        Mode::Help => println!("{USAGE}"),
        Mode::Version => println!("kde-ai-usage-monitor {}", env!("CARGO_PKG_VERSION")),
        Mode::ListProviders => list_providers(),
        Mode::ShowConfig => show_config(),
        Mode::SaveConfig(args) => save_config(args),
        Mode::Run(args) => run(args),
    }
}

fn parse_arguments(arguments: &[String]) -> Result<Mode, String> {
    let mut args = Args {
        format: Format::Json,
        providers: None,
        watch: false,
        interval_secs: None,
        refresh_tokens: false,
        use_cache: true,
    };
    let mut diagnose_mode = None;
    let mut mode = None;
    let mut save_config = false;

    let mut remaining = arguments.iter();
    while let Some(argument) = remaining.next() {
        let mut value = || {
            remaining
                .next()
                .cloned()
                .ok_or_else(|| format!("{argument} needs a value"))
        };

        match argument.as_str() {
            "-h" | "--help" => mode = Some(Mode::Help),
            "-V" | "--version" => mode = Some(Mode::Version),
            "--list-providers" => mode = Some(Mode::ListProviders),
            "--config" => mode = Some(Mode::ShowConfig),
            "--save-config" => save_config = true,
            "-w" | "--watch" => args.watch = true,
            "--refresh-tokens" => args.refresh_tokens = true,
            "--no-cache" => args.use_cache = false,
            "--diagnose" => diagnose_mode = Some(false),
            "--diagnose-append" => diagnose_mode = Some(true),
            "--json" => args.format = Format::Json,
            "--text" => args.format = Format::Text,
            "-f" | "--format" => {
                args.format = match value()?.as_str() {
                    "json" => Format::Json,
                    "text" => Format::Text,
                    other => return Err(format!("unknown format {other}")),
                }
            }
            "-p" | "--providers" => args.providers = Some(parse_provider_list(&value()?)?),
            "-i" | "--interval" => {
                args.interval_secs = Some(
                    value()?
                        .parse()
                        .map_err(|_| "--interval needs a whole number of seconds".to_string())?,
                )
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }

    if let Some(append) = diagnose_mode {
        let path = diagnose::init(append);
        diagnose::log(format!("startup args={arguments:?} log_path={path:?}"));
    }

    Ok(match (mode, save_config) {
        (Some(mode), _) => mode,
        (None, true) => Mode::SaveConfig(args),
        (None, false) => Mode::Run(args),
    })
}

fn parse_provider_list(list: &str) -> Result<ProviderSet, String> {
    let mut providers = ProviderSet::empty();
    for key in list.split(',').map(str::trim).filter(|key| !key.is_empty()) {
        let provider = ProviderId::from_key(key)
            .ok_or_else(|| format!("unknown provider {key}; try --list-providers"))?;
        providers.set(provider, true);
    }
    if providers.is_empty() {
        return Err("no providers selected".to_string());
    }
    Ok(providers)
}

fn run(args: Args) {
    let config = Config::load();
    let providers = args.providers.unwrap_or_else(|| config.provider_set());
    let options = PollOptions {
        refresh_tokens: args.refresh_tokens || config.refresh_tokens,
    };
    let interval = args
        .interval_secs
        .map(|seconds| std::time::Duration::from_secs(seconds.max(config::MIN_INTERVAL_SECS)))
        .unwrap_or_else(|| config.interval());

    loop {
        let polls = poller::poll(providers, options);
        emit(&polls, &args, interval);

        if !args.watch {
            // A one-shot run reports a failure the caller can act on; the applet
            // reads the JSON either way, so the numbers still went out first.
            if polls.iter().all(|poll| poll.result.is_err()) {
                std::process::exit(1);
            }
            return;
        }
        wait_for_next_poll(interval, providers);
    }
}

/// Sleep until the next poll, cutting the wait short when a provider's
/// credentials change — signing in should light the widget up straight away
/// rather than at the end of a five-minute interval.
fn wait_for_next_poll(interval: std::time::Duration, providers: ProviderSet) {
    const SLICE: std::time::Duration = std::time::Duration::from_secs(5);

    let signature = poller::credential_watch_snapshot(providers);
    let deadline = std::time::Instant::now() + interval;
    while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
        std::thread::sleep(remaining.min(SLICE));
        if poller::credential_watch_snapshot(providers) != signature {
            diagnose::log("credentials changed; polling early");
            return;
        }
    }
}

fn emit(polls: &[ProviderPoll], args: &Args, interval: std::time::Duration) {
    let mut cache = if args.use_cache {
        Cache::load()
    } else {
        Cache::default()
    };
    let snapshot = Snapshot::build(polls, &mut cache);
    if args.use_cache {
        cache.save();
    }

    match args.format {
        Format::Json => print_json(&snapshot, args.watch),
        Format::Text => print_text(&snapshot),
    }

    diagnose::log(format!(
        "emitted {} providers, next poll in {}s",
        snapshot.providers.len(),
        interval.as_secs()
    ));
}

fn print_json(snapshot: &Snapshot, watch: bool) {
    // Watch mode is newline-delimited JSON so a reader can consume it a line at
    // a time; a one-shot run is pretty-printed for human eyes.
    let body = if watch {
        serde_json::to_string(snapshot)
    } else {
        serde_json::to_string_pretty(snapshot)
    };
    match body {
        Ok(body) => {
            println!("{body}");
            let _ = std::io::stdout().flush();
        }
        Err(error) => eprintln!("kde-ai-usage-monitor: unable to serialise snapshot: {error}"),
    }
}

fn print_text(snapshot: &Snapshot) {
    for provider in &snapshot.providers {
        let mut line = format!("{:<14}", provider.name);
        match (&provider.session, &provider.weekly) {
            (Some(session), Some(weekly)) => {
                line.push_str(&format!("{} {:>5}", session.label, session.text));
                if let Some(resets_in) = &session.resets_in_text {
                    line.push_str(&format!(" (resets in {resets_in})"));
                }
                line.push_str(&format!("   {} {:>5}", weekly.label, weekly.text));
                if let Some(resets_in) = &weekly.resets_in_text {
                    line.push_str(&format!(" (resets in {resets_in})"));
                }
                if provider.stale {
                    line.push_str("   [stale]");
                }
            }
            _ => line.push_str(
                provider
                    .message
                    .as_deref()
                    .unwrap_or("no usage data available"),
            ),
        }
        println!("{line}");
    }
}

fn list_providers() {
    let enabled = Config::load().provider_set();
    for descriptor in PROVIDER_DESCRIPTORS {
        let mark = if enabled.contains(descriptor.id) {
            "*"
        } else {
            " "
        };
        println!(
            "{mark} {:<12} {:<14} {}",
            descriptor.key, descriptor.display_name, descriptor.description
        );
    }
    println!("\n* = enabled in the config file");
}

/// Persist the settings this invocation was given, so the next run — and the
/// applet, which reads the same file — starts from them.
fn save_config(args: Args) {
    let existing = Config::load();
    let config = Config {
        providers: args
            .providers
            .unwrap_or_else(|| existing.provider_set())
            .iter()
            .map(|provider| provider.key().to_string())
            .collect(),
        interval_secs: args.interval_secs.unwrap_or(existing.interval_secs),
        refresh_tokens: args.refresh_tokens || existing.refresh_tokens,
    };

    match config.save() {
        Ok(path) => println!("wrote {}", path.display()),
        Err(error) => {
            eprintln!("kde-ai-usage-monitor: unable to write the config file: {error}");
            std::process::exit(1);
        }
    }
}

fn show_config() {
    match Config::path() {
        Some(path) => {
            println!("# {}", path.display());
            let config = Config::load();
            match serde_json::to_string_pretty(&config) {
                Ok(body) => println!("{body}"),
                Err(error) => eprintln!("kde-ai-usage-monitor: {error}"),
            }
        }
        None => eprintln!("kde-ai-usage-monitor: no XDG config directory available"),
    }
}

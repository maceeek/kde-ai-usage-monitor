# kde-ai-usage-monitor

AI coding-assistant usage in your KDE Plasma panel: how much of your Claude
Code, Codex, GitHub Copilot, Cursor, OpenCode, or Antigravity allowance is gone,
and when the window rolls over.

> **This is an AI-generated fork.** The code was ported by an AI agent (Claude)
> from [CodeZeno/Claude-Code-Usage-Monitor][upstream], a Windows desktop widget
> by Craig Constable / Code Zeno Pty Ltd, MIT-licensed. The provider-polling
> layer is upstream's work, adapted; the Plasma applet, the CLI, and the
> packaging are new. See [NOTICE](NOTICE) for a file-by-file breakdown. If you
> are on Windows, use the original — it is the better tool there.

[upstream]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor

## How it is put together

Two pieces, talking over stdout:

| Piece | What it does |
| --- | --- |
| `kde-ai-usage-monitor` (Rust) | Finds each provider's credentials, calls its usage API, prints one JSON document |
| Plasma 6 applet (QML) | Runs that binary on a timer and draws the result |

Splitting them keeps OAuth, TLS, and SQLite out of QML, and means the backend is
useful on its own — in a script, a status bar, or a terminal.

## Install

### Arch / CachyOS with paru

paru can install and update the monitor straight from this repository, like a
package from the AUR. Add the repository's PKGBUILDs to `/etc/paru.conf`:

```ini
[kde-ai-usage-monitor]
Url = https://github.com/maceeek/kde-ai-usage-monitor.git
Path = packaging/arch
```

Then install it once with paru, so paru knows where it comes from. Pick one of
the two packages:

```bash
paru -Sya
paru -S kde-ai-usage-monitor       # tagged releases
paru -S kde-ai-usage-monitor-git   # every commit on main
```

From then on `paru -Syu` updates it with the rest of the system:

- **`kde-ai-usage-monitor`** follows releases. A release is a version bump on
  `main`; paru sees the new `pkgver` and rebuilds.
- **`kde-ai-usage-monitor-git`** follows `main`. As with any `-git` package,
  paru looks for new commits with `paru -Syu --devel`, or on every plain
  `paru -Syu` after adding `Devel` to the `[options]` of `/etc/paru.conf`.

Both build locally, run the test suite, and install the binary to `/usr/bin`
and the applet to `/usr/share/plasma/plasmoids`. They conflict with each other,
so switching is a plain `paru -S` of the other one. After an update, restart
Plasma (below) or remove and re-add the widget to load the new applet.

### Arch / CachyOS by hand

```bash
git clone https://github.com/maceeek/kde-ai-usage-monitor.git
cd kde-ai-usage-monitor/packaging/arch/kde-ai-usage-monitor   # or …-git
makepkg -si
```

To update, `git pull` and run `makepkg -si` again in the same directory.

### Any other distribution

```bash
git clone https://github.com/maceeek/kde-ai-usage-monitor.git
cd kde-ai-usage-monitor
./packaging/install.sh
```

Installs into `~/.local` — no root. `./packaging/install.sh --uninstall` undoes
it. You need `cargo` (`pacman -S rust`, `dnf install cargo`, or rustup).

### Adding the widget

Right-click your panel → **Add Widgets…** → search for **AI Usage**. If it does
not appear, restart Plasma:

```bash
kquitapp6 plasmashell && kstart plasmashell
```

Then open the widget's settings and tick the providers you use. Only Claude Code
is on by default.

### What the widget shows

- **Panel:** the window closest to running out (or the one you pick), coloured
  by your amber/red thresholds, with an optional bar. Tick *Appearance → 5h
  reset* to show when the 5h window resets next to the percentage, as time
  left (`1h 12m`) or as a clock time (`17:40`).
- **Hover tooltip:** every provider at a glance — plan, every window with a
  bar, percentage, time left and the clock time of the reset, per-model caps
  (Claude's Opus/Sonnet weekly limits, Copilot's chat allowance), extra usage
  and credits, and why a provider is stale or rate limited. *Appearance →
  Tooltip* switches back to the one-line text tooltip.
- **Popup (click):** the same detail as cards, with refresh and settings.

## Providers

Each provider is read from whatever its own tooling already wrote to disk — the
monitor never asks you for a password and never stores one.

| Provider | Where credentials come from | Windows shown |
| --- | --- | --- |
| Claude Code | `~/.claude/.credentials.json`, or `$CLAUDE_CREDENTIALS_FILE`, or the desktop keyring | 5h session, 7d |
| Codex | `~/.codex/auth.json`, or `$CODEX_HOME/auth.json` | 5h, weekly |
| Cursor | Cursor's `state.vscdb`, or `$CURSOR_SESSION_TOKEN` | Auto %, API % (both reset at the billing cycle) |
| OpenCode | `$OPENCODE_GO_WORKSPACE_ID` + `$OPENCODE_GO_AUTH_COOKIE`, or `~/.config/opencode-go/config.json` | rolling, 7d or 30d |
| Antigravity | `~/.antigravity/auth.json`, `$ANTIGRAVITY_AUTH_FILE`, or the `gemini:antigravity` keyring entry | 5h, weekly |
| GitHub Copilot | `$COPILOT_GITHUB_TOKEN`, `$GH_TOKEN`, `$GITHUB_TOKEN`, the Copilot editor plugins' `~/.config/github-copilot/apps.json`, the GitHub CLI's `hosts.yml`, or `gh auth token` | 30d premium requests, plus metered chat/completions |

Alongside the headline windows the monitor passes through whatever else the
provider reports: Claude's per-model weekly caps and extra-usage spend, Codex's
monthly spend-control limit and credit balance, and the plan name.

A provider that answers `429 Too Many Requests` (or a 5xx with `Retry-After`)
is left alone until the time it asked for has passed — capped at an hour — and
shows its last known numbers in the meantime.

The keyring is read through `secret-tool` (libsecret) or `kwallet-query`
(KWallet), whichever is installed. A provider with no credentials reports
`no_credentials` and the widget says so rather than showing a zero.

**Antigravity is the least tested path.** Upstream reads its token from Windows
Credential Manager; the Linux equivalent is inferred, not verified against a
real install. If it does not work for you, run with `--diagnose` and open an
issue with the log.

## Command line

```console
$ kde-ai-usage-monitor --format text
Claude Code     5h   12% (resets in 3h 40m)   7d   58% (resets in 4d 2h)
                Opus 7d   20% (resets in 4d 2h)
                Sonnet 7d   41% (resets in 4d 2h)
Codex           5h    0% (resets in 5h)   7d  3.0% (resets in 6d 1h)
GitHub Copilot  30d   25% (resets in 19d 9h)
```

```
-f, --format <json|text>  Output format (default: json)
-p, --providers <LIST>    Providers to poll, e.g. claude,codex
-w, --watch               Keep polling, one JSON document per line
-i, --interval <SECONDS>  Seconds between polls in --watch mode
    --refresh-tokens      Let a provider's CLI refresh an expired token
    --no-cache            Ignore and do not update the last-known cache
    --list-providers      List the providers this build knows about
    --config              Print the config file path and its contents
    --save-config         Write the effective settings to the config file
    --diagnose            Log what the poller is doing to stderr
```

Settings live in `~/.config/kde-ai-usage-monitor/config.json` and last-known
usage in `~/.cache/kde-ai-usage-monitor/usage.json`. The applet keeps its own
copy of the provider list in its Plasma configuration and passes it on the
command line, so the two never fight over the file.

`--watch` re-polls early when a credentials file changes, so signing in lights
the widget up straight away instead of at the end of the interval.

### `--refresh-tokens`

Off by default. When on, an expired token makes the monitor run the provider's
own CLI (`claude -p .`, `codex exec .`) to refresh it — which starts a process
you did not ask for, from a panel widget. Turn it on only if usage stops
updating after a token expires.

## The JSON contract

The applet and the backend agree on this document, versioned by `schema`:

```json
{
  "schema": 1,
  "version": "0.2.0",
  "generated_at": 1787167371,
  "summary": {
    "provider": "claude", "name": "Claude Code", "label": "7d",
    "percentage": 58.0, "text": "58%"
  },
  "providers": [
    {
      "id": "claude", "name": "Claude Code", "plan": "max",
      "state": "ok", "ok": true, "stale": false, "past_reset": false,
      "polled_at": 1787167371,
      "session": {
        "label": "5h", "percentage": 12.0, "text": "12%",
        "resets_at": 1787180571, "resets_in": 13200, "resets_in_text": "3h 40m"
      },
      "weekly": { "label": "7d", "percentage": 58.0, "text": "58%" },
      "limits": [
        { "key": "seven_day_opus", "label": "Opus 7d", "percentage": 20.0, "text": "20%" }
      ],
      "credits": {
        "used": 12.5, "limit": 50.0, "remaining": 37.5,
        "percentage": 25.0, "text": "$12.50 of $50.00"
      }
    }
  ]
}
```

- `state` is `ok` or one of `no_credentials`, `auth_required`, `token_expired`,
  `request_failed`, `rate_limited`; a failure also carries a human-readable
  `message`, and `rate_limited` a `retry_at` (Unix seconds) when known.
- `stale: true` means the poll failed and these are cached numbers.
- `session`, `weekly` and `monthly` are each present only when the provider
  reported that window — Copilot has no `session`, and `monthly` appears only
  alongside a weekly window (Codex spend-control limits, OpenCode).
- `limits` lists secondary quotas (`active: true` marks the one holding usage
  back), `credits` paid usage past the plan, `plan` the subscription name. All
  three are omitted when the provider has nothing to say.
- `summary` is the headline window closest to running out across every
  provider that answered — what the panel shows when it has room for one
  number. Secondary `limits` never count towards it.

Exit code is `1` when no provider answered, `2` for a bad argument, `0`
otherwise. The JSON is printed either way.

## Releasing

A release is a version bump merged to `main`:

1. Set the new version in `Cargo.toml`, `plasmoid/package/metadata.json`, and
   `pkgver` in `packaging/arch/kde-ai-usage-monitor/PKGBUILD` (reset `pkgrel`
   to 1), and run `cargo build` so `Cargo.lock` follows.
   `python3 scripts/check-version.py` checks they agree; CI runs it too.
2. Merge to `main`. The release workflow sees a version with no `v<version>`
   tag yet, runs the tests, tags the commit, and publishes the tarball.
3. paru users get it on their next `paru -Syu`.

## What changed from upstream

**Kept:** the usage endpoints, OAuth beta headers, rate-limit header handling,
dashboard scraping, quota-summary selection, and credential file formats for all
six providers — brought up to date with upstream v2.18.1: the current Haiku
alias for Claude's header probe, no Messages-API fallback while the usage
endpoint is rate limited, Claude's per-model limits and extra-usage spend,
Codex windows placed by length plus its monthly spend-control limit and
credits, `Retry-After` back-offs, stricter timestamp validation, and the GitHub
Copilot provider.

**Rewritten:** credential discovery (Linux paths, libsecret/KWallet instead of
Windows Credential Manager, no WSL probing); SQLite access (`dlopen` on
`libsqlite3.so.0` instead of `winsqlite3.dll`); and the result shape — upstream
collapses a poll into a single error, while this one reports every provider
separately so a widget can draw one row each.

**Dropped:** the Win32 window, theme engine, theme studio, tray icon, updater,
and localisation layer — Plasma provides all of that. Also dropped is upstream's
Claude-desktop token cache, which is DPAPI-encrypted and Windows-only; sign in
with the Claude Code CLI instead.

**Not yet ported:** Grok Build, multiple accounts per provider, and
Antigravity's OAuth refresh (it reads client credentials out of the Windows
install).

## Development

```bash
cargo test                          # no network access needed
cargo clippy --all-targets -- -D warnings
cargo fmt --all
python3 scripts/check-plasmoid.py   # static checks on the applet package
python3 scripts/check-version.py    # version agrees across the three files
```

The Cursor tests read a real SQLite file from `tests/fixtures/`, so they need
`libsqlite3.so.0`; they say so and skip if it is missing.

`check-plasmoid.py` is what stands in for a QML test harness: it verifies that
every `cfg_*` binding in a config page has a matching entry in `config/main.xml`
(Plasma ignores the ones it cannot match, so a typo means a setting that never
saves), that config categories point at files that exist, that the metadata is
well-formed, and that brackets balance. It needs no Qt, which is why CI can run
it. Actual rendering was reviewed by reading, not by running — bug reports with
`--diagnose` output are welcome.

## License

MIT, as upstream. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

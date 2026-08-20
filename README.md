# kde-ai-usage-monitor

AI coding-assistant usage in your KDE Plasma panel: how much of your Claude
Code, Codex, Cursor, OpenCode, or Antigravity allowance is gone, and when the
window rolls over.

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

### Arch / CachyOS

```bash
git clone https://github.com/maceeek/kde-ai-usage-monitor.git
cd kde-ai-usage-monitor/packaging/arch/kde-ai-usage-monitor-git
makepkg -si
```

This builds from the main branch, runs the test suite, and installs the binary
to `/usr/bin` and the applet to `/usr/share/plasma/plasmoids`. For a tagged
release, use `packaging/arch/kde-ai-usage-monitor/PKGBUILD` instead (bump
`pkgver`, run `updpkgsums`, then `makepkg -si`).

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
Claude Code   5h   12% (resets in 3h 40m)   7d   58% (resets in 4d 2h)
Codex         5h    0%   7d  3.0%
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
  "version": "0.1.0",
  "generated_at": 1787167371,
  "summary": {
    "provider": "claude", "name": "Claude Code", "label": "7d",
    "percentage": 58.0, "text": "58%"
  },
  "providers": [
    {
      "id": "claude", "name": "Claude Code",
      "state": "ok", "ok": true, "stale": false, "past_reset": false,
      "polled_at": 1787167371,
      "session": {
        "label": "5h", "percentage": 12.0, "text": "12%",
        "resets_at": 1787180571, "resets_in": 13200, "resets_in_text": "3h 40m"
      },
      "weekly": { "label": "7d", "percentage": 58.0, "text": "58%" }
    }
  ]
}
```

- `state` is `ok` or one of `no_credentials`, `auth_required`, `token_expired`,
  `request_failed`; a failure also carries a human-readable `message`.
- `stale: true` means the poll failed and these are cached numbers.
- `summary` is the window closest to running out across every provider that
  answered — what the panel shows when it has room for one number.

Exit code is `1` when no provider answered, `2` for a bad argument, `0`
otherwise. The JSON is printed either way.

## What changed from upstream

**Kept:** the usage endpoints, OAuth beta headers, rate-limit header handling,
dashboard scraping, quota-summary selection, and credential file formats for all
five providers.

**Rewritten:** credential discovery (Linux paths, libsecret/KWallet instead of
Windows Credential Manager, no WSL probing); SQLite access (`dlopen` on
`libsqlite3.so.0` instead of `winsqlite3.dll`); and the result shape — upstream
collapses a poll into a single error, while this one reports every provider
separately so a widget can draw one row each.

**Dropped:** the Win32 window, theme engine, theme studio, tray icon, updater,
and localisation layer — Plasma provides all of that. Also dropped is upstream's
Claude-desktop token cache, which is DPAPI-encrypted and Windows-only; sign in
with the Claude Code CLI instead.

## Development

```bash
cargo test                          # 53 tests, no network access needed
cargo clippy --all-targets -- -D warnings
cargo fmt --all
python3 scripts/check-plasmoid.py   # static checks on the applet package
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

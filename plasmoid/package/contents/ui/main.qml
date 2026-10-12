import QtQuick
import org.kde.kirigami as Kirigami
import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.plasma5support as P5Support
import org.kde.plasma.plasmoid
import "../code/format.js" as Format

/// AI usage monitor for Plasma 6.
///
/// The applet owns no polling logic of its own: it runs the
/// `kde-ai-usage-monitor` binary, which speaks the JSON schema documented in
/// the project README, and draws whatever comes back. Keeping HTTP, OAuth, and
/// SQLite out of QML is the whole reason the backend is a separate process.
PlasmoidItem {
    id: root

    /// Latest decoded snapshot, or null before the first successful run.
    property var snapshot: null
    /// Set when the backend could not be run at all, as opposed to a provider
    /// failing to report — those travel inside the snapshot.
    property string backendError: ""
    property bool polling: false
    /// Ticks so countdowns stay honest between polls.
    property double clock: Date.now()

    readonly property var providerList: snapshot ? snapshot.providers : []
    readonly property var summary: snapshot ? snapshot.summary : null
    readonly property bool hasData: providerList.length > 0

    // Representations live in their own files, which cannot see this file's ids,
    // so the applet hands itself over explicitly.
    preferredRepresentation: compactRepresentation
    compactRepresentation: CompactRepresentation { monitor: root }
    fullRepresentation: FullRepresentation { monitor: root }

    Plasmoid.status: {
        if (backendError.length > 0) {
            return PlasmaCore.Types.NeedsAttentionStatus;
        }
        if (summary && summary.percentage >= Plasmoid.configuration.criticalThreshold) {
            return PlasmaCore.Types.NeedsAttentionStatus;
        }
        return hasData ? PlasmaCore.Types.ActiveStatus : PlasmaCore.Types.PassiveStatus;
    }

    // The detailed tooltip lays every window out with bars and reset times;
    // the plain text below is what Plasma shows when it is switched off.
    toolTipItem: Plasmoid.configuration.detailedTooltip ? detailedToolTip : null
    property Item detailedToolTip: ToolTipContent {
        monitor: root
    }

    toolTipMainText: summary
        ? i18n("%1 · %2 %3", summary.name, summary.label, summary.text)
        : i18n("AI Usage Monitor")
    toolTipSubText: {
        if (backendError.length > 0) {
            return backendError;
        }
        if (!hasData) {
            return i18n("Waiting for the first poll…");
        }
        return providerList.map(function (provider) {
            var sections = Format.headlineSections(provider);
            if (sections.length === 0) {
                return i18n("%1: %2", provider.name, provider.message || i18n("no data"));
            }
            return provider.name + ":   " + sections.map(function (section) {
                var reset = root.secondsUntil(section.resets_at);
                return section.label + " " + Format.percentage(section.percentage)
                       + (reset !== undefined ? " (" + Format.duration(reset) + ")" : "");
            }).join("   ") + (provider.stale ? i18n(" (stale)") : "");
        }).join("\n");
    }

    /// Colour for a usage percentage, following the configured thresholds.
    function usageColor(percentage) {
        if (percentage >= Plasmoid.configuration.criticalThreshold) {
            return Kirigami.Theme.negativeTextColor;
        }
        if (percentage >= Plasmoid.configuration.warningThreshold) {
            return Kirigami.Theme.neutralTextColor;
        }
        return Kirigami.Theme.positiveTextColor;
    }

    /// Seconds left in a window, recomputed against the ticking clock rather
    /// than the value baked in at poll time.
    function secondsUntil(resetsAt) {
        return Format.secondsUntil(resetsAt, clock);
    }

    /// The section the panel shows, honouring the compact-content setting.
    function compactSection() {
        if (!hasData) {
            return null;
        }

        var configured = Plasmoid.configuration.compactProvider;
        var candidates = providerList.filter(function (provider) {
            return Format.headlineSections(provider).length > 0
                && (configured === "" || provider.id === configured);
        });
        if (candidates.length === 0) {
            return null;
        }

        var mode = Plasmoid.configuration.compactContent;
        if (mode === 1 || mode === 2) {
            var key = mode === 1 ? "session" : "weekly";
            return candidates.map(function (provider) {
                return { provider: provider, section: provider[key] };
            }).filter(function (entry) {
                return !!entry.section;
            }).sort(function (a, b) {
                return b.section.percentage - a.section.percentage;
            })[0] || null;
        }

        // "Highest": whichever window across the selected providers is closest
        // to running out, which is the number that actually matters.
        var best = null;
        for (var i = 0; i < candidates.length; ++i) {
            var provider = candidates[i];
            Format.headlineSections(provider).forEach(function (section) {
                if (!best || section.percentage > best.section.percentage) {
                    best = { provider: provider, section: section };
                }
            });
        }
        return best;
    }

    /// Short status shown next to a provider's name in the popup and tooltip:
    /// empty when the numbers are live, otherwise why they are not.
    function providerStatus(provider) {
        if (provider.ok) {
            return "";
        }
        var retry = secondsUntil(provider.retry_at);
        if (provider.state === "rate_limited") {
            return retry ? i18n("rate limited · retrying in %1", Format.duration(retry))
                         : i18n("rate limited");
        }
        if (provider.stale) {
            var age = Math.max(0, Math.floor(clock / 1000) - provider.polled_at);
            return i18n("stale · from %1 ago", Format.duration(age));
        }
        return "";
    }

    /// "resets in 1h 12m · 17:40", or nothing when the window has no reset.
    function resetLabel(section) {
        var summary = Format.resetSummary(section.resets_at, clock);
        return summary.length > 0 ? i18n("resets in %1", summary) : "";
    }

    /// Text for the 5h countdown next to the panel percentage: the short
    /// window of whichever provider the panel is showing, or of the configured
    /// one. Empty when that provider has no short window or no reset time.
    function panelResetText(entry) {
        if (!entry || !entry.provider.session) {
            return "";
        }
        var resetsAt = entry.provider.session.resets_at;
        var remaining = secondsUntil(resetsAt);
        if (remaining === undefined) {
            return "";
        }
        return Plasmoid.configuration.panelResetFormat === 1
            ? Format.resetClock(resetsAt, clock)
            : Format.duration(remaining);
    }

    function shellQuote(value) {
        return "'" + String(value).replace(/'/g, "'\\''") + "'";
    }

    function pollCommand() {
        var binary = Plasmoid.configuration.binaryPath || "kde-ai-usage-monitor";
        var providers = Plasmoid.configuration.providers.join(",");
        var command = shellQuote(binary) + " --format json";
        if (providers.length > 0) {
            command += " --providers " + shellQuote(providers);
        }
        if (Plasmoid.configuration.refreshTokens) {
            command += " --refresh-tokens";
        }
        return command;
    }

    function refresh() {
        if (polling) {
            return;
        }
        polling = true;
        backend.connectSource(pollCommand());
    }

    P5Support.DataSource {
        id: backend
        engine: "executable"
        connectedSources: []

        onNewData: function (sourceName, data) {
            disconnectSource(sourceName);
            root.polling = false;

            var stdout = (data["stdout"] || "").trim();
            if (stdout.length === 0) {
                // A non-zero exit with no output means the binary is missing or
                // could not start; anything else is reported inside the JSON.
                root.backendError = (data["stderr"] || "").trim()
                    || i18n("Could not run %1", Plasmoid.configuration.binaryPath);
                return;
            }

            try {
                root.snapshot = JSON.parse(stdout);
                root.backendError = "";
            } catch (error) {
                root.backendError = i18n("The backend returned output this applet cannot read");
            }
        }
    }

    Timer {
        id: pollTimer
        interval: Math.max(30, Plasmoid.configuration.intervalSeconds) * 1000
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: root.refresh()
    }

    Timer {
        // Only needs to be fine-grained enough for a minute-resolution
        // countdown to look alive — in the panel, the popup, and the tooltip.
        interval: 20000
        running: root.expanded || Plasmoid.formFactor !== PlasmaCore.Types.Planar
                 || Plasmoid.configuration.showResetInPanel
        repeat: true
        triggeredOnStart: true
        onTriggered: root.clock = Date.now()
    }

    // A window that has rolled over is showing numbers for a window that no
    // longer exists, so fetch the new ones instead of waiting out the interval.
    Timer {
        interval: 60000
        running: root.hasData
        repeat: true
        onTriggered: {
            var pastReset = root.providerList.some(function (provider) {
                return provider.past_reset;
            });
            if (pastReset) {
                root.refresh();
            }
        }
    }

    Plasmoid.contextualActions: [
        PlasmaCore.Action {
            text: i18n("Refresh Now")
            icon.name: "view-refresh"
            onTriggered: root.refresh()
        }
    ]
}

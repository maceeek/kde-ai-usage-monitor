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

    preferredRepresentation: compactRepresentation
    compactRepresentation: CompactRepresentation {}
    fullRepresentation: FullRepresentation {}

    Plasmoid.status: {
        if (backendError.length > 0) {
            return PlasmaCore.Types.NeedsAttentionStatus;
        }
        if (summary && summary.percentage >= plasmoid.configuration.criticalThreshold) {
            return PlasmaCore.Types.NeedsAttentionStatus;
        }
        return hasData ? PlasmaCore.Types.ActiveStatus : PlasmaCore.Types.PassiveStatus;
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
            if (!provider.session) {
                return i18n("%1: %2", provider.name, provider.message || i18n("no data"));
            }
            var weekly = provider.weekly ? "   " + provider.weekly.label + " "
                                           + Format.percentage(provider.weekly.percentage) : "";
            return provider.name + ":   " + provider.session.label + " "
                   + Format.percentage(provider.session.percentage) + weekly
                   + (provider.stale ? i18n(" (stale)") : "");
        }).join("\n");
    }

    /// Colour for a usage percentage, following the configured thresholds.
    function usageColor(percentage) {
        if (percentage >= plasmoid.configuration.criticalThreshold) {
            return Kirigami.Theme.negativeTextColor;
        }
        if (percentage >= plasmoid.configuration.warningThreshold) {
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

        var configured = plasmoid.configuration.compactProvider;
        var candidates = providerList.filter(function (provider) {
            return provider.session && (configured === "" || provider.id === configured);
        });
        if (candidates.length === 0) {
            return null;
        }

        var mode = plasmoid.configuration.compactContent;
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
            [provider.session, provider.weekly].forEach(function (section) {
                if (section && (!best || section.percentage > best.section.percentage)) {
                    best = { provider: provider, section: section };
                }
            });
        }
        return best;
    }

    function shellQuote(value) {
        return "'" + String(value).replace(/'/g, "'\\''") + "'";
    }

    function pollCommand() {
        var binary = plasmoid.configuration.binaryPath || "kde-ai-usage-monitor";
        var providers = plasmoid.configuration.providers.join(",");
        var command = shellQuote(binary) + " --format json";
        if (providers.length > 0) {
            command += " --providers " + shellQuote(providers);
        }
        if (plasmoid.configuration.refreshTokens) {
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
                    || i18n("Could not run %1", plasmoid.configuration.binaryPath);
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
        interval: Math.max(30, plasmoid.configuration.intervalSeconds) * 1000
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: root.refresh()
    }

    Timer {
        // Only needs to be fine-grained enough for a minute-resolution
        // countdown to look alive.
        interval: 20000
        running: root.expanded || plasmoid.formFactor !== PlasmaCore.Types.Planar
        repeat: true
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

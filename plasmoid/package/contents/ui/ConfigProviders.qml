import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.kcmutils as KCM

KCM.SimpleKCM {
    id: page

    // The applet's own copy of the provider list; the checkboxes below write
    // into it and the config system saves it as a string list.
    property alias cfg_binaryPath: binaryPath.text
    property alias cfg_intervalSeconds: interval.value
    property alias cfg_refreshTokens: refreshTokens.checked
    property var cfg_providers: []

    /// Keys must match `ProviderId::key` in the Rust crate.
    readonly property var knownProviders: [
        { key: "claude", name: i18n("Claude Code"), hint: i18n("Anthropic") },
        { key: "codex", name: i18n("Codex"), hint: i18n("OpenAI") },
        { key: "antigravity", name: i18n("Antigravity"), hint: i18n("Google") },
        { key: "opencode", name: i18n("OpenCode"), hint: i18n("OpenCode Go") },
        { key: "cursor", name: i18n("Cursor"), hint: i18n("Cursor") }
    ]

    function toggleProvider(key, enabled) {
        var providers = (cfg_providers || []).slice();
        var index = providers.indexOf(key);
        if (enabled && index === -1) {
            providers.push(key);
        } else if (!enabled && index !== -1) {
            // Something has to stay selected, or the backend has nothing to do.
            if (providers.length === 1) {
                return;
            }
            providers.splice(index, 1);
        }
        cfg_providers = providers;
    }

    Kirigami.FormLayout {
        anchors.fill: parent

        Repeater {
            model: page.knownProviders

            QQC2.CheckBox {
                required property var modelData

                Kirigami.FormData.label: modelData === page.knownProviders[0]
                    ? i18n("Poll:") : ""
                text: modelData.name + " — " + modelData.hint
                checked: (page.cfg_providers || []).indexOf(modelData.key) !== -1
                onToggled: page.toggleProvider(modelData.key, checked)
            }
        }

        Item { Kirigami.FormData.isSection: true }

        QQC2.TextField {
            id: binaryPath
            Kirigami.FormData.label: i18n("Backend command:")
            Layout.fillWidth: true
            placeholderText: "kde-ai-usage-monitor"
        }

        QQC2.SpinBox {
            id: interval
            Kirigami.FormData.label: i18n("Poll every:")
            from: 30
            to: 3600
            stepSize: 30
            textFromValue: function (value) {
                return i18n("%1 seconds", value);
            }
            valueFromText: function (text) {
                return parseInt(text, 10) || 300;
            }
        }

        QQC2.CheckBox {
            id: refreshTokens
            Kirigami.FormData.label: i18n("Expired tokens:")
            text: i18n("Let the provider's CLI refresh them")
        }

        QQC2.Label {
            Layout.maximumWidth: Kirigami.Units.gridUnit * 20
            font: Kirigami.Theme.smallFont
            opacity: 0.7
            wrapMode: Text.WordWrap
            text: i18n("Refreshing runs the provider's own command-line tool (for example `claude -p .`) in the background, which starts a process you did not ask for. Leave this off unless usage stops updating after a token expires.")
        }
    }
}

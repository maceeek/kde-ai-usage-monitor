import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.kcmutils as KCM

KCM.SimpleKCM {
    id: page

    property alias cfg_compactContent: compactContent.currentIndex
    property alias cfg_showBarInPanel: showBar.checked
    property alias cfg_showResetInPanel: showReset.checked
    property alias cfg_panelResetFormat: resetFormat.currentIndex
    property alias cfg_detailedTooltip: detailedTooltip.checked
    property alias cfg_warningThreshold: warning.value
    property alias cfg_criticalThreshold: critical.value
    property string cfg_compactProvider: ""

    readonly property var providerChoices: [
        { key: "", name: i18n("Whichever is highest") },
        { key: "claude", name: i18n("Claude Code") },
        { key: "codex", name: i18n("Codex") },
        { key: "antigravity", name: i18n("Antigravity") },
        { key: "opencode", name: i18n("OpenCode") },
        { key: "cursor", name: i18n("Cursor") },
        { key: "copilot", name: i18n("GitHub Copilot") }
    ]

    Kirigami.FormLayout {
        anchors.fill: parent

        QQC2.ComboBox {
            id: compactContent
            Kirigami.FormData.label: i18n("Panel shows:")
            model: [
                i18n("The window closest to running out"),
                i18n("The short (session) window"),
                i18n("The long (weekly) window")
            ]
        }

        QQC2.ComboBox {
            Kirigami.FormData.label: i18n("From provider:")
            model: page.providerChoices.map(function (choice) {
                return choice.name;
            })
            currentIndex: Math.max(0, page.providerChoices.findIndex(function (choice) {
                return choice.key === page.cfg_compactProvider;
            }))
            onActivated: page.cfg_compactProvider = page.providerChoices[currentIndex].key
        }

        QQC2.CheckBox {
            id: showBar
            Kirigami.FormData.label: i18n("Panel bar:")
            text: i18n("Show a usage bar under the percentage")
        }

        QQC2.CheckBox {
            id: showReset
            Kirigami.FormData.label: i18n("5h reset:")
            text: i18n("Show when the 5h window resets next to the percentage")
        }

        QQC2.ComboBox {
            id: resetFormat
            Kirigami.FormData.label: i18n("Show it as:")
            enabled: showReset.checked
            model: [
                i18n("Time left (1h 12m)"),
                i18n("Clock time (17:40)")
            ]
        }

        QQC2.Label {
            Layout.maximumWidth: Kirigami.Units.gridUnit * 20
            font: Kirigami.Theme.smallFont
            opacity: 0.7
            wrapMode: Text.WordWrap
            text: i18n("The countdown belongs to the provider the panel is showing. Providers without a short window, such as GitHub Copilot, show none.")
        }

        QQC2.CheckBox {
            id: detailedTooltip
            Kirigami.FormData.label: i18n("Tooltip:")
            text: i18n("Show every window, limit and reset time on hover")
        }

        Item { Kirigami.FormData.isSection: true }

        QQC2.SpinBox {
            id: warning
            Kirigami.FormData.label: i18n("Amber above:")
            from: 1
            to: 100
            // Amber must not overtake red, or the colours stop meaning anything.
            onValueChanged: if (value > critical.value) {
                critical.value = value;
            }
            textFromValue: function (value) {
                return value + "%";
            }
            valueFromText: function (text) {
                return parseInt(text, 10) || 70;
            }
        }

        QQC2.SpinBox {
            id: critical
            Kirigami.FormData.label: i18n("Red above:")
            from: 1
            to: 100
            onValueChanged: if (value < warning.value) {
                warning.value = value;
            }
            textFromValue: function (value) {
                return value + "%";
            }
            valueFromText: function (text) {
                return parseInt(text, 10) || 90;
            }
        }
    }
}

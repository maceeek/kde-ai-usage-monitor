import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents
import org.kde.plasma.extras as PlasmaExtras
import org.kde.plasma.plasmoid
import "../code/format.js" as Format

/// The popup: every polled provider, one card each.
PlasmaExtras.Representation {
    id: full

    Layout.minimumWidth: Kirigami.Units.gridUnit * 18
    Layout.minimumHeight: Kirigami.Units.gridUnit * 12
    Layout.preferredWidth: Kirigami.Units.gridUnit * 22
    Layout.preferredHeight: Kirigami.Units.gridUnit * 20

    collapseMarginsHint: true

    header: PlasmaExtras.PlasmoidHeading {
        RowLayout {
            anchors.fill: parent
            spacing: Kirigami.Units.smallSpacing

            PlasmaExtras.Heading {
                level: 4
                text: i18n("AI Usage")
            }

            Item { Layout.fillWidth: true }

            PlasmaComponents.BusyIndicator {
                running: root.polling
                visible: running
                implicitWidth: Kirigami.Units.iconSizes.small
                implicitHeight: implicitWidth
            }

            PlasmaComponents.ToolButton {
                icon.name: "view-refresh"
                display: PlasmaComponents.AbstractButton.IconOnly
                text: i18n("Refresh Now")
                enabled: !root.polling
                onClicked: root.refresh()

                PlasmaComponents.ToolTip {
                    text: parent.text
                }
            }

            PlasmaComponents.ToolButton {
                icon.name: "configure"
                display: PlasmaComponents.AbstractButton.IconOnly
                text: i18n("Configure…")
                onClicked: plasmoid.internalAction("configure").trigger()

                PlasmaComponents.ToolTip {
                    text: parent.text
                }
            }
        }
    }

    // Three states share this popup: a backend that would not run, a first poll
    // that has not landed yet, and actual data.
    PlasmaExtras.PlaceholderMessage {
        anchors.centerIn: parent
        width: parent.width - Kirigami.Units.gridUnit * 2
        visible: root.backendError.length > 0
        iconName: "dialog-error"
        text: i18n("The usage backend did not run")
        explanation: root.backendError
        helpfulAction: Kirigami.Action {
            icon.name: "view-refresh"
            text: i18n("Try Again")
            onTriggered: root.refresh()
        }
    }

    PlasmaExtras.PlaceholderMessage {
        anchors.centerIn: parent
        width: parent.width - Kirigami.Units.gridUnit * 2
        visible: root.backendError.length === 0 && !root.hasData
        iconName: "utilities-system-monitor"
        text: i18n("Waiting for the first poll…")
    }

    contentItem: PlasmaComponents.ScrollView {
        visible: root.hasData && root.backendError.length === 0

        ColumnLayout {
            width: full.contentItem.availableWidth
            spacing: Kirigami.Units.largeSpacing

            Repeater {
                model: root.providerList

                ProviderItem {
                    required property var modelData

                    Layout.fillWidth: true
                    Layout.topMargin: Kirigami.Units.smallSpacing
                    provider: modelData
                }
            }
        }
    }

    footer: PlasmaExtras.PlasmoidHeading {
        position: PlasmaComponents.ToolBar.Footer

        RowLayout {
            anchors.fill: parent

            PlasmaComponents.Label {
                Layout.fillWidth: true
                font: Kirigami.Theme.smallFont
                opacity: 0.7
                elide: Text.ElideRight
                text: root.snapshot
                    ? i18n("Updated %1 ago", Format.duration(
                        Math.max(0, Math.floor(root.clock / 1000) - root.snapshot.generated_at)))
                    : ""
            }
        }
    }
}

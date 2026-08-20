import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents
import org.kde.plasma.extras as PlasmaExtras
import org.kde.plasma.plasmoid
import "../code/format.js" as Format

/// The popup: every polled provider, one card each.
PlasmaExtras.Representation {
    id: full

    /// The applet root — passed in, because ids do not cross files.
    required property var monitor

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
                running: full.monitor.polling
                visible: running
                implicitWidth: Kirigami.Units.iconSizes.small
                implicitHeight: implicitWidth
            }

            PlasmaComponents.ToolButton {
                icon.name: "view-refresh"
                display: PlasmaComponents.AbstractButton.IconOnly
                text: i18n("Refresh Now")
                enabled: !full.monitor.polling
                onClicked: full.monitor.refresh()

                QQC2.ToolTip.text: text
                QQC2.ToolTip.visible: hovered
                QQC2.ToolTip.delay: Kirigami.Units.toolTipDelay
            }

            PlasmaComponents.ToolButton {
                icon.name: "configure"
                display: PlasmaComponents.AbstractButton.IconOnly
                text: i18n("Configure…")
                onClicked: Plasmoid.internalAction("configure").trigger()

                QQC2.ToolTip.text: text
                QQC2.ToolTip.visible: hovered
                QQC2.ToolTip.delay: Kirigami.Units.toolTipDelay
            }
        }
    }

    // Three states share this popup: a backend that would not run, a first poll
    // that has not landed yet, and actual data.
    PlasmaExtras.PlaceholderMessage {
        anchors.centerIn: parent
        width: parent.width - Kirigami.Units.gridUnit * 2
        visible: full.monitor.backendError.length > 0
        iconName: "dialog-error"
        text: i18n("The usage backend did not run")
        explanation: full.monitor.backendError
        helpfulAction: Kirigami.Action {
            icon.name: "view-refresh"
            text: i18n("Try Again")
            onTriggered: full.monitor.refresh()
        }
    }

    PlasmaExtras.PlaceholderMessage {
        anchors.centerIn: parent
        width: parent.width - Kirigami.Units.gridUnit * 2
        visible: full.monitor.backendError.length === 0 && !full.monitor.hasData
        iconName: "utilities-system-monitor"
        text: i18n("Waiting for the first poll…")
    }

    PlasmaComponents.ScrollView {
        id: scroll
        anchors.fill: parent
        visible: full.monitor.hasData && full.monitor.backendError.length === 0

        ColumnLayout {
            width: scroll.availableWidth
            spacing: Kirigami.Units.largeSpacing

            Repeater {
                model: full.monitor.providerList

                ProviderItem {
                    required property var modelData

                    Layout.fillWidth: true
                    Layout.topMargin: Kirigami.Units.smallSpacing
                    monitor: full.monitor
                    provider: modelData
                }
            }
        }
    }

    footer: PlasmaExtras.PlasmoidHeading {
        position: PlasmaComponents.ToolBar.Footer

        PlasmaComponents.Label {
            anchors.fill: parent
            verticalAlignment: Text.AlignVCenter
            font: Kirigami.Theme.smallFont
            opacity: 0.7
            elide: Text.ElideRight
            text: full.monitor.snapshot
                ? i18n("Updated %1 ago", Format.duration(
                    Math.max(0, Math.floor(full.monitor.clock / 1000)
                        - full.monitor.snapshot.generated_at)))
                : ""
        }
    }
}

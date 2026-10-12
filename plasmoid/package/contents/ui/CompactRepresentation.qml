import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents
import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.plasmoid
import "../code/format.js" as Format

/// What sits in the panel: one percentage, optionally with a bar and the time
/// left until the 5h window resets, coloured by the configured thresholds.
MouseArea {
    id: compact

    /// The applet root — passed in, because ids do not cross files.
    required property var monitor

    readonly property var entry: monitor.compactSection()
    readonly property bool vertical: Plasmoid.formFactor === PlasmaCore.Types.Vertical
    readonly property color valueColor: entry ? monitor.usageColor(entry.section.percentage)
                                              : Kirigami.Theme.disabledTextColor
    readonly property string resetText: Plasmoid.configuration.showResetInPanel
                                        ? monitor.panelResetText(entry) : ""
    readonly property real valuePixelSize: Math.max(Kirigami.Units.gridUnit * 0.7,
        Math.round(height * (vertical ? 0.3 : 0.45)))

    Layout.minimumWidth: vertical ? 0 : contentRow.implicitWidth + Kirigami.Units.smallSpacing * 2
    Layout.minimumHeight: vertical ? contentRow.implicitHeight + Kirigami.Units.smallSpacing * 2 : 0
    Layout.preferredWidth: Layout.minimumWidth
    Layout.preferredHeight: Layout.minimumHeight

    // Stale numbers stay readable but visibly second-hand.
    opacity: (entry && entry.provider.stale) ? 0.6 : 1.0

    acceptedButtons: Qt.LeftButton | Qt.MiddleButton
    onClicked: function (mouse) {
        if (mouse.button === Qt.MiddleButton) {
            compact.monitor.refresh();
        } else {
            compact.monitor.expanded = !compact.monitor.expanded;
        }
    }

    // A horizontal panel has width to spare, so the countdown sits beside the
    // number; a vertical one stacks it underneath.
    GridLayout {
        id: contentRow
        anchors.centerIn: parent
        columns: compact.vertical ? 1 : 3
        rowSpacing: 0
        columnSpacing: Kirigami.Units.smallSpacing

        Kirigami.Icon {
            source: "utilities-system-monitor"
            visible: !compact.entry
            implicitWidth: Kirigami.Units.iconSizes.small
            implicitHeight: implicitWidth
        }

        ColumnLayout {
            spacing: 1
            visible: !!compact.entry

            PlasmaComponents.Label {
                id: valueLabel
                Layout.alignment: Qt.AlignHCenter
                text: compact.entry ? Format.percentage(compact.entry.section.percentage) : ""
                color: compact.valueColor
                font.bold: true
                font.pixelSize: compact.valuePixelSize
            }

            PlasmaComponents.Label {
                Layout.alignment: Qt.AlignHCenter
                visible: compact.vertical && compact.resetText.length > 0
                text: compact.resetText
                font.pixelSize: Math.max(Kirigami.Units.gridUnit * 0.55,
                                         Math.round(compact.valuePixelSize * 0.8))
                opacity: 0.8
            }

            // The bar carries the same reading as the number for anyone who
            // reads shapes faster than digits.
            Rectangle {
                Layout.alignment: Qt.AlignHCenter
                // Sized off the number rather than the applet in a horizontal
                // panel, or the countdown beside it would stretch the bar.
                Layout.preferredWidth: compact.vertical
                    ? Math.max(Kirigami.Units.gridUnit * 2, compact.width * 0.7)
                    : Math.max(Kirigami.Units.gridUnit * 2, valueLabel.implicitWidth)
                Layout.preferredHeight: Math.max(2, Math.round(Kirigami.Units.gridUnit / 6))
                visible: Plasmoid.configuration.showBarInPanel && !!compact.entry
                radius: height / 2
                color: Qt.rgba(Kirigami.Theme.textColor.r,
                               Kirigami.Theme.textColor.g,
                               Kirigami.Theme.textColor.b, 0.2)

                Rectangle {
                    anchors.left: parent.left
                    anchors.top: parent.top
                    anchors.bottom: parent.bottom
                    width: parent.width * Math.min(1, (compact.entry
                        ? compact.entry.section.percentage : 0) / 100)
                    radius: parent.radius
                    color: compact.valueColor

                    Behavior on width {
                        NumberAnimation {
                            duration: Kirigami.Units.longDuration
                            easing.type: Easing.OutCubic
                        }
                    }
                }
            }
        }

        PlasmaComponents.Label {
            Layout.alignment: Qt.AlignVCenter
            visible: !compact.vertical && !!compact.entry && compact.resetText.length > 0
            text: compact.resetText
            font.pixelSize: Math.max(Kirigami.Units.gridUnit * 0.55,
                                     Math.round(compact.valuePixelSize * 0.8))
            opacity: 0.8
        }
    }
}

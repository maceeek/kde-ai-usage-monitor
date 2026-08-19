import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents
import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.plasmoid
import "../code/format.js" as Format

/// What sits in the panel: one percentage, optionally with a bar, coloured by
/// the configured thresholds.
MouseArea {
    id: compact

    readonly property var entry: root.compactSection()
    readonly property bool vertical: plasmoid.formFactor === PlasmaCore.Types.Vertical
    readonly property color valueColor: entry ? root.usageColor(entry.section.percentage)
                                              : Kirigami.Theme.disabledTextColor

    Layout.minimumWidth: vertical ? 0 : contentRow.implicitWidth + Kirigami.Units.smallSpacing * 2
    Layout.minimumHeight: vertical ? contentRow.implicitHeight + Kirigami.Units.smallSpacing * 2 : 0
    Layout.preferredWidth: Layout.minimumWidth
    Layout.preferredHeight: Layout.minimumHeight

    acceptedButtons: Qt.LeftButton | Qt.MiddleButton
    onClicked: function (mouse) {
        if (mouse.button === Qt.MiddleButton) {
            root.refresh();
        } else {
            root.expanded = !root.expanded;
        }
    }

    RowLayout {
        id: contentRow
        anchors.centerIn: parent
        spacing: Kirigami.Units.smallSpacing

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
                Layout.alignment: Qt.AlignHCenter
                text: compact.entry ? Format.percentage(compact.entry.section.percentage) : ""
                color: compact.valueColor
                font.bold: true
                font.pixelSize: Math.max(Kirigami.Units.gridUnit * 0.7,
                                         Math.round(compact.height * (compact.vertical ? 0.3 : 0.45)))
            }

            // The bar doubles as the "which window is this" cue when the panel
            // is too short for a label.
            Rectangle {
                Layout.alignment: Qt.AlignHCenter
                Layout.preferredWidth: Math.max(Kirigami.Units.gridUnit * 2, compact.width * 0.7)
                Layout.preferredHeight: Math.max(2, Math.round(Kirigami.Units.gridUnit / 6))
                visible: plasmoid.configuration.showBarInPanel && !!compact.entry
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
    }

    // Stale numbers stay readable but visibly second-hand.
    opacity: (compact.entry && compact.entry.provider.stale) ? 0.6 : 1.0
}

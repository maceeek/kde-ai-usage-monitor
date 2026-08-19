import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents

/// One labelled usage window: "5h  42%  · resets in 1h 12m".
ColumnLayout {
    id: root

    required property string label
    required property real percentage
    property string resetText: ""
    property color barColor: Kirigami.Theme.highlightColor
    property bool dimmed: false

    spacing: Kirigami.Units.smallSpacing / 2

    RowLayout {
        Layout.fillWidth: true
        spacing: Kirigami.Units.smallSpacing

        PlasmaComponents.Label {
            text: root.label
            font: Kirigami.Theme.smallFont
            opacity: 0.75
        }

        Item { Layout.fillWidth: true }

        PlasmaComponents.Label {
            text: root.resetText
            font: Kirigami.Theme.smallFont
            opacity: 0.6
            visible: text.length > 0
        }

        PlasmaComponents.Label {
            text: Math.round(root.percentage) + "%"
            font.bold: true
        }
    }

    // Drawn by hand rather than with a ProgressBar so the fill can carry the
    // threshold colour without fighting the widget style.
    Rectangle {
        Layout.fillWidth: true
        implicitHeight: Math.round(Kirigami.Units.gridUnit / 3)
        radius: height / 2
        color: Kirigami.Theme.backgroundColor
        border.width: 1
        border.color: Qt.rgba(Kirigami.Theme.textColor.r,
                              Kirigami.Theme.textColor.g,
                              Kirigami.Theme.textColor.b, 0.15)
        opacity: root.dimmed ? 0.5 : 1.0

        Rectangle {
            anchors.left: parent.left
            anchors.top: parent.top
            anchors.bottom: parent.bottom
            anchors.margins: 1
            width: Math.max(0, Math.min(1, root.percentage / 100))
                   * (parent.width - 2)
            radius: parent.radius
            color: root.barColor

            Behavior on width {
                NumberAnimation {
                    duration: Kirigami.Units.longDuration
                    easing.type: Easing.OutCubic
                }
            }
        }
    }
}

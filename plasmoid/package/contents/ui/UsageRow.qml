import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents

/// One window on a single line, for the tooltip: "5h  ▓▓▓░░  42%  1h 12m · 17:40".
/// Columns have fixed widths so the rows of every provider line up.
RowLayout {
    id: row

    required property string label
    required property real percentage
    property string resetText: ""
    property color barColor: Kirigami.Theme.highlightColor
    property bool dimmed: false
    /// Secondary quotas are drawn a little quieter than the headline windows.
    property bool secondary: false
    property bool emphasised: false

    spacing: Kirigami.Units.smallSpacing
    opacity: dimmed ? 0.6 : 1.0

    PlasmaComponents.Label {
        Layout.preferredWidth: Kirigami.Units.gridUnit * 6
        text: row.label
        elide: Text.ElideRight
        font.pointSize: row.secondary ? Kirigami.Theme.smallFont.pointSize
                                      : Kirigami.Theme.defaultFont.pointSize
        font.bold: row.emphasised
        opacity: row.secondary ? 0.8 : 1.0
    }

    Rectangle {
        Layout.preferredWidth: Kirigami.Units.gridUnit * 6
        Layout.alignment: Qt.AlignVCenter
        implicitHeight: Math.max(4, Math.round(Kirigami.Units.gridUnit / (row.secondary ? 4 : 3)))
        radius: height / 2
        color: Qt.rgba(Kirigami.Theme.textColor.r,
                       Kirigami.Theme.textColor.g,
                       Kirigami.Theme.textColor.b, 0.15)

        Rectangle {
            anchors.left: parent.left
            anchors.top: parent.top
            anchors.bottom: parent.bottom
            width: Math.max(0, Math.min(1, row.percentage / 100)) * parent.width
            radius: parent.radius
            color: row.barColor
        }
    }

    PlasmaComponents.Label {
        Layout.preferredWidth: Kirigami.Units.gridUnit * 2.5
        horizontalAlignment: Text.AlignRight
        text: Math.round(row.percentage) + "%"
        font.bold: !row.secondary
        font.pointSize: row.secondary ? Kirigami.Theme.smallFont.pointSize
                                      : Kirigami.Theme.defaultFont.pointSize
    }

    PlasmaComponents.Label {
        Layout.preferredWidth: Kirigami.Units.gridUnit * 7
        text: row.resetText
        font: Kirigami.Theme.smallFont
        opacity: 0.7
        elide: Text.ElideRight
    }
}

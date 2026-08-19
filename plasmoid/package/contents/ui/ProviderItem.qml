import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents
import "../code/format.js" as Format

/// One provider's card in the popup: its name, its state, and its two windows.
ColumnLayout {
    id: card

    required property var provider

    spacing: Kirigami.Units.smallSpacing

    RowLayout {
        Layout.fillWidth: true
        spacing: Kirigami.Units.smallSpacing

        PlasmaComponents.Label {
            text: card.provider.name
            font.bold: true
        }

        Kirigami.Icon {
            source: card.provider.stale ? "documentinfo" : "dialog-warning"
            visible: !card.provider.ok
            color: Kirigami.Theme.neutralTextColor
            implicitWidth: Kirigami.Units.iconSizes.small
            implicitHeight: implicitWidth

            PlasmaComponents.ToolTip {
                text: card.provider.message || ""
                visible: parent.hovered
            }
        }

        Item { Layout.fillWidth: true }

        PlasmaComponents.Label {
            text: card.provider.stale ? i18n("stale") : ""
            font: Kirigami.Theme.smallFont
            opacity: 0.6
        }
    }

    // Without a session block there is nothing to draw but the reason why.
    PlasmaComponents.Label {
        Layout.fillWidth: true
        visible: !card.provider.session
        text: card.provider.message || i18n("No usage data available")
        font: Kirigami.Theme.smallFont
        opacity: 0.75
        wrapMode: Text.WordWrap
    }

    Repeater {
        model: card.provider.session
            ? [card.provider.session, card.provider.weekly].filter(function (section) {
                return !!section;
            })
            : []

        UsageBar {
            required property var modelData

            Layout.fillWidth: true
            label: modelData.label
            percentage: modelData.percentage
            barColor: root.usageColor(modelData.percentage)
            dimmed: card.provider.stale
            resetText: {
                var remaining = root.secondsUntil(modelData.resets_at);
                return remaining === undefined ? "" : i18n("resets in %1", Format.duration(remaining));
            }
        }
    }
}

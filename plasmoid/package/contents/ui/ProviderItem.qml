import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents
import "../code/format.js" as Format

/// One provider's card in the popup: its name, its state, and its two windows.
ColumnLayout {
    id: card

    /// The applet root, for colours and the shared clock.
    required property var monitor
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
            implicitWidth: Kirigami.Units.iconSizes.small
            implicitHeight: implicitWidth

            HoverHandler {
                id: stateHover
            }

            QQC2.ToolTip.text: card.provider.message || ""
            QQC2.ToolTip.visible: stateHover.hovered && QQC2.ToolTip.text.length > 0
            QQC2.ToolTip.delay: Kirigami.Units.toolTipDelay
        }

        Item { Layout.fillWidth: true }

        PlasmaComponents.Label {
            text: i18n("stale")
            visible: card.provider.stale
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
            barColor: card.monitor.usageColor(modelData.percentage)
            dimmed: card.provider.stale
            resetText: {
                var remaining = card.monitor.secondsUntil(modelData.resets_at);
                return remaining === undefined
                    ? ""
                    : i18n("resets in %1", Format.duration(remaining));
            }
        }
    }
}

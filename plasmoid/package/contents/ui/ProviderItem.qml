import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents
import "../code/format.js" as Format

/// One provider's card in the popup: its name and plan, its state, every
/// window it reported, its per-model caps, and any paid credits.
ColumnLayout {
    id: card

    /// The applet root, for colours and the shared clock.
    required property var monitor
    required property var provider

    readonly property var sections: Format.headlineSections(provider)
    readonly property var limits: provider.limits || []
    readonly property var credits: provider.credits || null
    readonly property string status: monitor.providerStatus(provider)

    spacing: Kirigami.Units.smallSpacing

    RowLayout {
        Layout.fillWidth: true
        spacing: Kirigami.Units.smallSpacing

        PlasmaComponents.Label {
            text: card.provider.name
            font.bold: true
        }

        PlasmaComponents.Label {
            visible: text.length > 0
            text: Format.planName(card.provider.plan)
            font: Kirigami.Theme.smallFont
            opacity: 0.6
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
            text: card.status
            visible: text.length > 0
            font: Kirigami.Theme.smallFont
            opacity: 0.6
        }
    }

    // Without any window there is nothing to draw but the reason why.
    PlasmaComponents.Label {
        Layout.fillWidth: true
        visible: card.sections.length === 0
        text: card.provider.message || i18n("No usage data available")
        font: Kirigami.Theme.smallFont
        opacity: 0.75
        wrapMode: Text.WordWrap
    }

    Repeater {
        model: card.sections

        UsageBar {
            required property var modelData

            Layout.fillWidth: true
            label: modelData.label
            percentage: modelData.percentage
            barColor: card.monitor.usageColor(modelData.percentage)
            dimmed: card.provider.stale
            resetText: card.monitor.resetLabel(modelData)
        }
    }

    // Per-model caps and other quotas: smaller, under their own heading, so the
    // headline windows above stay the first thing the eye lands on.
    PlasmaComponents.Label {
        visible: card.limits.length > 0
        Layout.topMargin: Kirigami.Units.smallSpacing
        text: i18n("Other limits")
        font: Kirigami.Theme.smallFont
        opacity: 0.6
    }

    Repeater {
        model: card.limits

        UsageBar {
            required property var modelData

            Layout.fillWidth: true
            label: modelData.active ? i18n("%1 (limiting)", modelData.label) : modelData.label
            percentage: modelData.percentage
            barColor: card.monitor.usageColor(modelData.percentage)
            dimmed: card.provider.stale
            compact: true
            resetText: card.monitor.resetLabel(modelData)
        }
    }

    UsageBar {
        Layout.fillWidth: true
        Layout.topMargin: Kirigami.Units.smallSpacing
        visible: !!card.credits && card.credits.percentage !== undefined
        label: i18n("Extra usage")
        percentage: card.credits && card.credits.percentage !== undefined ? card.credits.percentage : 0
        barColor: card.monitor.usageColor(percentage)
        compact: true
        resetText: card.credits ? card.credits.text : ""
    }

    PlasmaComponents.Label {
        visible: !!card.credits && card.credits.percentage === undefined && card.credits.text.length > 0
        text: card.credits ? i18n("Credits: %1", card.credits.text) : ""
        font: Kirigami.Theme.smallFont
        opacity: 0.75
    }
}

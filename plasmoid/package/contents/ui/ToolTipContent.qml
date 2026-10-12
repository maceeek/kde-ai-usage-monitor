import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents
import "../code/format.js" as Format

/// The hover tooltip: every provider with every window it reported — the
/// headline ones, per-model caps, credits — each with a bar, the percentage,
/// and when it resets. A read-only digest of the popup that needs no click.
Item {
    id: tip

    /// The applet root — passed in, because ids do not cross files.
    required property var monitor

    readonly property int margin: Kirigami.Units.largeSpacing

    implicitWidth: content.implicitWidth + margin * 2
    implicitHeight: content.implicitHeight + margin * 2
    // The tooltip dialog sizes itself from these.
    Layout.minimumWidth: implicitWidth
    Layout.maximumWidth: implicitWidth
    Layout.minimumHeight: implicitHeight
    Layout.maximumHeight: implicitHeight

    ColumnLayout {
        id: content
        anchors.fill: parent
        anchors.margins: tip.margin
        spacing: Kirigami.Units.largeSpacing

        RowLayout {
            Layout.fillWidth: true
            spacing: Kirigami.Units.largeSpacing

            Kirigami.Heading {
                level: 4
                text: i18n("AI Usage")
            }

            Item { Layout.fillWidth: true }

            PlasmaComponents.Label {
                visible: !!tip.monitor.snapshot
                text: tip.monitor.snapshot
                    ? i18n("updated %1 ago", Format.duration(Math.max(0,
                        Math.floor(tip.monitor.clock / 1000) - tip.monitor.snapshot.generated_at)))
                    : ""
                font: Kirigami.Theme.smallFont
                opacity: 0.6
            }
        }

        PlasmaComponents.Label {
            visible: tip.monitor.backendError.length > 0 || !tip.monitor.hasData
            Layout.maximumWidth: Kirigami.Units.gridUnit * 22
            wrapMode: Text.WordWrap
            text: tip.monitor.backendError.length > 0 ? tip.monitor.backendError
                                                      : i18n("Waiting for the first poll…")
        }

        Repeater {
            model: tip.monitor.providerList

            ColumnLayout {
                id: providerBlock

                required property var modelData
                readonly property var sections: Format.headlineSections(modelData)
                readonly property string status: tip.monitor.providerStatus(modelData)

                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing / 2

                RowLayout {
                    Layout.fillWidth: true
                    spacing: Kirigami.Units.smallSpacing

                    PlasmaComponents.Label {
                        text: providerBlock.modelData.name
                        font.bold: true
                    }

                    PlasmaComponents.Label {
                        visible: text.length > 0
                        text: Format.planName(providerBlock.modelData.plan)
                        font: Kirigami.Theme.smallFont
                        opacity: 0.6
                    }

                    Item { Layout.fillWidth: true }

                    PlasmaComponents.Label {
                        visible: text.length > 0
                        text: providerBlock.status
                        font: Kirigami.Theme.smallFont
                        color: Kirigami.Theme.neutralTextColor
                    }
                }

                // A provider with nothing to draw says why instead.
                PlasmaComponents.Label {
                    visible: providerBlock.sections.length === 0
                    Layout.maximumWidth: Kirigami.Units.gridUnit * 22
                    wrapMode: Text.WordWrap
                    text: providerBlock.modelData.message || i18n("No usage data available")
                    font: Kirigami.Theme.smallFont
                    opacity: 0.75
                }

                // Numbers from an earlier poll carry the reason this one failed.
                PlasmaComponents.Label {
                    visible: providerBlock.sections.length > 0 && !providerBlock.modelData.ok
                             && providerBlock.modelData.state !== "rate_limited"
                             && !!providerBlock.modelData.message
                    Layout.maximumWidth: Kirigami.Units.gridUnit * 22
                    wrapMode: Text.WordWrap
                    text: providerBlock.modelData.message || ""
                    font: Kirigami.Theme.smallFont
                    color: Kirigami.Theme.neutralTextColor
                }

                Repeater {
                    model: providerBlock.sections

                    UsageRow {
                        required property var modelData

                        label: modelData.label
                        percentage: modelData.percentage
                        barColor: tip.monitor.usageColor(modelData.percentage)
                        dimmed: providerBlock.modelData.stale
                        resetText: Format.resetSummary(modelData.resets_at, tip.monitor.clock)
                    }
                }

                Repeater {
                    model: providerBlock.modelData.limits || []

                    UsageRow {
                        required property var modelData

                        label: modelData.label
                        percentage: modelData.percentage
                        barColor: tip.monitor.usageColor(modelData.percentage)
                        dimmed: providerBlock.modelData.stale
                        secondary: true
                        emphasised: !!modelData.active
                        resetText: Format.resetSummary(modelData.resets_at, tip.monitor.clock)
                    }
                }

                // Credits with a cap get a bar like any window; a bare balance
                // is just a line of text.
                UsageRow {
                    readonly property var credits: providerBlock.modelData.credits

                    visible: !!credits && credits.percentage !== undefined
                    label: i18n("Extra usage")
                    percentage: credits && credits.percentage !== undefined ? credits.percentage : 0
                    barColor: tip.monitor.usageColor(percentage)
                    secondary: true
                    resetText: credits ? credits.text : ""
                }

                PlasmaComponents.Label {
                    readonly property var credits: providerBlock.modelData.credits

                    visible: !!credits && credits.percentage === undefined && credits.text.length > 0
                    text: credits ? i18n("Credits: %1", credits.text) : ""
                    font: Kirigami.Theme.smallFont
                    opacity: 0.8
                }
            }
        }
    }
}

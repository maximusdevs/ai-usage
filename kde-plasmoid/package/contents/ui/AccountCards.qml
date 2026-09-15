pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as QQC2
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents
import "../code/plasmoid-logic.mjs" as Logic

ColumnLayout {
    id: root

    required property var applet

    property int pageSize: 3
    property int currentPage: 0

    readonly property var allAccounts: root.applet.accounts || []
    readonly property int totalPages: Math.max(1, Math.ceil(allAccounts.length / pageSize))
    readonly property var pagedAccounts: {
        const start = root.currentPage * root.pageSize;
        return root.allAccounts.slice(start, start + root.pageSize);
    }

    spacing: Kirigami.Units.smallSpacing

    // Pagination Header (if more than 1 page)
    RowLayout {
        Layout.fillWidth: true
        visible: root.totalPages > 1
        spacing: Kirigami.Units.smallSpacing

        PlasmaComponents.Button {
            text: i18n("◀ Anterior")
            enabled: root.currentPage > 0
            onClicked: root.currentPage = Math.max(0, root.currentPage - 1)
        }

        PlasmaComponents.Label {
            Layout.fillWidth: true
            horizontalAlignment: Text.AlignHCenter
            font: Kirigami.Theme.smallFont
            opacity: 0.8
            text: i18n("Página %1 de %2 (%3 contas)", root.currentPage + 1, root.totalPages, root.allAccounts.length)
            textFormat: Text.PlainText
        }

        PlasmaComponents.Button {
            text: i18n("Próxima ▶")
            enabled: root.currentPage < root.totalPages - 1
            onClicked: root.currentPage = Math.min(root.totalPages - 1, root.currentPage + 1)
        }
    }

    Repeater {
        model: root.pagedAccounts

        delegate: Rectangle {
            id: acctCard
            required property var modelData
            required property int index

            Layout.fillWidth: true
            implicitHeight: cardCol.implicitHeight + Kirigami.Units.smallSpacing * 2
            radius: Kirigami.Units.cornerRadius
            color: acctCard.modelData.active
                ? Qt.alpha(Kirigami.Theme.highlightColor, 0.12)
                : Qt.alpha(Kirigami.Theme.textColor, 0.05)
            border.width: 1
            border.color: acctCard.modelData.active
                ? Kirigami.Theme.highlightColor
                : Qt.alpha(Kirigami.Theme.textColor, 0.2)

            ColumnLayout {
                id: cardCol
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.margins: Kirigami.Units.smallSpacing
                spacing: Kirigami.Units.smallSpacing / 2

                // Header
                RowLayout {
                    Layout.fillWidth: true
                    spacing: Kirigami.Units.smallSpacing

                    Kirigami.Icon {
                        source: "user-identity"
                        implicitWidth: Kirigami.Units.iconSizes.small
                        implicitHeight: Kirigami.Units.iconSizes.small
                        color: acctCard.modelData.active
                            ? Kirigami.Theme.highlightColor
                            : Kirigami.Theme.textColor
                    }

                    PlasmaComponents.Label {
                        Layout.fillWidth: true
                        font.bold: true
                        text: Logic.formatAccount(acctCard.modelData.label, root.applet.showFullEmail)
                        elide: Text.ElideRight
                        textFormat: Text.PlainText
                    }

                    Rectangle {
                        visible: acctCard.modelData.active
                        implicitWidth: activeLbl.implicitWidth + Kirigami.Units.smallSpacing * 2
                        implicitHeight: activeLbl.implicitHeight + Kirigami.Units.smallSpacing / 2
                        radius: height / 2
                        color: Kirigami.Theme.highlightColor

                        PlasmaComponents.Label {
                            id: activeLbl
                            anchors.centerIn: parent
                            text: i18n("Ativa")
                            font.bold: true
                            font.pointSize: Kirigami.Theme.smallFont.pointSize
                            color: Kirigami.Theme.highlightedTextColor
                        }
                    }

                    PlasmaComponents.Button {
                        visible: !acctCard.modelData.active
                        text: i18n("Ativar")
                        display: PlasmaComponents.AbstractButton.TextOnly
                        onClicked: root.applet.switchAccount(acctCard.modelData.label)
                    }
                }

                // Providers & Metrics
                Repeater {
                    model: (acctCard.modelData.providers && acctCard.modelData.providers.length > 0)
                        ? acctCard.modelData.providers : []

                    delegate: ColumnLayout {
                        id: provCol
                        required property var modelData
                        Layout.fillWidth: true
                        spacing: 2

                        RowLayout {
                            Layout.fillWidth: true
                            spacing: Kirigami.Units.smallSpacing

                            PlasmaComponents.Label {
                                text: provCol.modelData.name || provCol.modelData.id
                                font.bold: true
                                font.pointSize: Kirigami.Theme.smallFont.pointSize
                                color: Kirigami.Theme.highlightColor
                                textFormat: Text.PlainText
                            }
                        }

                        Repeater {
                            model: provCol.modelData.metrics || []

                            delegate: RowLayout {
                                id: metricRow
                                required property var modelData
                                Layout.fillWidth: true
                                spacing: Kirigami.Units.smallSpacing

                                PlasmaComponents.Label {
                                    text: "↳ " + metricRow.modelData.label
                                    font: Kirigami.Theme.smallFont
                                    opacity: 0.85
                                    Layout.minimumWidth: Kirigami.Units.gridUnit * 6
                                    textFormat: Text.PlainText
                                }

                                UsageBar {
                                    visible: metricRow.modelData.percent !== null
                                    pct: metricRow.modelData.percent ?? 0
                                    severity: metricRow.modelData.severity || "low"
                                    colors: root.applet.colors
                                    implicitWidth: Kirigami.Units.gridUnit * 5
                                    Layout.alignment: Qt.AlignVCenter
                                }

                                PlasmaComponents.Label {
                                    text: metricRow.modelData.value || (metricRow.modelData.percent + "%")
                                    font.pointSize: Kirigami.Theme.smallFont.pointSize
                                    font.bold: (metricRow.modelData.percent ?? 0) >= 90
                                    color: Logic.severityColor(metricRow.modelData.severity, root.applet.colors) ?? Kirigami.Theme.textColor
                                    textFormat: Text.PlainText
                                }
                            }
                        }
                    }
                }

                PlasmaComponents.Label {
                    visible: !acctCard.modelData.providers || acctCard.modelData.providers.length === 0
                    text: i18n("Nenhum provedor consultado ainda nesta conta.")
                    font: Kirigami.Theme.smallFont
                    opacity: 0.6
                    textFormat: Text.PlainText
                }
            }
        }
    }
}

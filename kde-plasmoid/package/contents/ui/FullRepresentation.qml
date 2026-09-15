// The popup. Layout follows the native Omarchy panel (omarchy/Panel.qml) so the
// two native frontends read the same: header, provider tabs, a status surface
// when something is wrong, the usage rows under one heading, and a footer
// saying how old the data is.
//
// Only the STRUCTURE is ported. Omarchy draws with its shell's own theme
// tokens; everything here sizes off Kirigami.Units and colours off
// Kirigami.Theme, so the widget follows whatever Plasma colour scheme the user
// runs.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as QQC2
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents
import "../code/plasmoid-logic.mjs" as Logic

Item {
    id: full

    required property var applet

    readonly property var entry: full.applet.entry
    readonly property string status: full.applet.statusMessage()
    readonly property var rows: Logic.detailRows(full.entry)

    function sharedPoolPercent() {
        if (!full.entry || !full.entry.sections) return null;
        for (const s of full.entry.sections) {
            if (s.type === "metric" && /claude|gpt/i.test(s.label)) {
                return s.percent;
            }
        }
        return null;
    }

    function displayedExtraModels() {
        if (!full.applet.showExtraModels || !full.entry || !full.entry.extraModels)
            return [];
        const selected = Array.from(full.applet.selectedExtraModels || []);
        if (selected.length === 0)
            return full.entry.extraModels;
        return full.entry.extraModels.filter(m => selected.indexOf(m) !== -1);
    }

    // An Item defaults to implicitHeight 0 and the popup sizes itself from the
    // implicit size, so without this the buttons render off-canvas. The
    // maximum is what makes the popup SHRINK again when a smaller vendor is
    // selected rather than keeping the tallest height it ever had.
    readonly property int contentHeight: column.implicitHeight + Kirigami.Units.largeSpacing * 2
    implicitWidth: Kirigami.Units.gridUnit * 22
    implicitHeight: full.contentHeight
    Layout.minimumHeight: full.contentHeight
    Layout.preferredHeight: full.contentHeight
    Layout.maximumHeight: full.contentHeight

    ColumnLayout {
        id: column
        // Deliberately not anchors.fill: the column must drive the height, not
        // be stretched by it, or contentHeight feeds back on itself.
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: Kirigami.Units.largeSpacing
        spacing: Kirigami.Units.smallSpacing

        // --- header --------------------------------------------------------
        RowLayout {
            Layout.fillWidth: true
            spacing: Kirigami.Units.smallSpacing

            // isMask is explicit on purpose: Kirigami only infers it for some
            // icons, and without it `color` is ignored and the full-colour
            // artwork renders — a little monitor with a chart painted on it,
            // which reads as a picture rather than as this widget's mark and
            // ignores the colour scheme entirely.
            Kirigami.Icon {
                source: "speedometer-symbolic"
                isMask: true
                implicitWidth: Kirigami.Units.iconSizes.medium
                implicitHeight: Kirigami.Units.iconSizes.medium
                color: full.applet.statusIsUrgent()
                    ? Kirigami.Theme.negativeTextColor : Kirigami.Theme.textColor
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 0

                Kirigami.Heading {
                    Layout.fillWidth: true
                    level: 3
                    elide: Text.ElideRight
                    text: full.entry ? full.entry.label : i18n("AI Usage Bar")
                    textFormat: Text.PlainText
                }

                PlasmaComponents.Label {
                    Layout.fillWidth: true
                    elide: Text.ElideRight
                    font: Kirigami.Theme.smallFont
                    opacity: 0.7
                    visible: text !== ""
                    text: {
                        if (!full.entry)
                            return "";
                        const plan = full.entry.plan;
                        return full.entry.stale ? i18n("%1 · cached", plan) : plan;
                    }
                    textFormat: Text.PlainText
                }
            }

            PlasmaComponents.ToolButton {
                icon.name: "view-refresh-symbolic"
                display: PlasmaComponents.AbstractButton.IconOnly
                enabled: full.applet.pendingCommand === ""
                text: i18n("Refresh now")
                PlasmaComponents.ToolTip.text: text
                PlasmaComponents.ToolTip.visible: hovered
                PlasmaComponents.ToolTip.delay: Kirigami.Units.toolTipDelay
                onClicked: full.applet.refresh(true)
            }

            PlasmaComponents.ToolButton {
                icon.name: "utilities-terminal-symbolic"
                display: PlasmaComponents.AbstractButton.IconOnly
                text: i18n("Open TUI")
                PlasmaComponents.ToolTip.text: text
                PlasmaComponents.ToolTip.visible: hovered
                PlasmaComponents.ToolTip.delay: Kirigami.Units.toolTipDelay
                onClicked: full.applet.launchTui()
            }
        }

        // --- account selector ----------------------------------------------
        RowLayout {
            Layout.fillWidth: true
            visible: full.applet.accounts.length > 0
            spacing: Kirigami.Units.smallSpacing

            PlasmaComponents.Label {
                text: i18n("Account:")
                font: Kirigami.Theme.smallFont
                opacity: 0.7
                textFormat: Text.PlainText
            }

            QQC2.ComboBox {
                id: accountCombo
                Layout.fillWidth: true
                model: full.applet.accounts
                displayText: {
                    const acc = full.applet.accounts[currentIndex];
                    if (!acc) return "";
                    return Logic.formatAccount(acc.label, full.applet.showFullEmail);
                }
                currentIndex: {
                    for (let i = 0; i < full.applet.accounts.length; i++) {
                        if (full.applet.activeAccountOverride) {
                            if (full.applet.accounts[i].label === full.applet.activeAccountOverride)
                                return i;
                        } else if (full.applet.accounts[i].active) {
                            return i;
                        }
                    }
                    return 0;
                }
                delegate: QQC2.ItemDelegate {
                    required property var modelData
                    required property int index
                    width: accountCombo.width
                    text: Logic.formatAccount(modelData.label, full.applet.showFullEmail)
                    highlighted: accountCombo.highlightedIndex === index
                }
                onActivated: index => {
                    const acc = full.applet.accounts[index];
                    if (acc) full.applet.switchAccount(acc.label);
                }
            }
        }

        // --- provider tabs -------------------------------------------------
        // Every configured vendor, including the ones currently failing. Hiding
        // a broken vendor is what made "not configured" indistinguishable from
        // "configured and erroring". Only in the tab layout; the cards layout
        // shows every entry side by side instead.
        Flow {
            Layout.fillWidth: true
            Layout.topMargin: Kirigami.Units.smallSpacing
            visible: full.applet.viewMode === 0 && full.applet.tabs.length > 1
            spacing: Kirigami.Units.smallSpacing

            Repeater {
                model: full.applet.tabs

                PlasmaComponents.Button {
                    required property var modelData

                    text: modelData.label
                    checkable: true
                    checked: modelData.active
                    icon.name: modelData.failing ? "dialog-warning" : ""
                    onClicked: full.applet.selectVendor(modelData.id)
                }
            }
        }

        // --- Renewal Notice Alert Banner / Card -----------------------------
        Rectangle {
            id: renewalCard
            Layout.fillWidth: true
            Layout.topMargin: Kirigami.Units.smallSpacing
            visible: full.applet.activeRenewals.length > 0
            implicitHeight: renewalColumn.implicitHeight + Kirigami.Units.smallSpacing * 2
            radius: Kirigami.Units.cornerRadius
            color: Qt.alpha(Kirigami.Theme.highlightColor, 0.12)
            border.width: 1
            border.color: Kirigami.Theme.highlightColor

            ColumnLayout {
                id: renewalColumn
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.margins: Kirigami.Units.smallSpacing
                spacing: Kirigami.Units.smallSpacing / 2

                RowLayout {
                    Layout.fillWidth: true
                    spacing: Kirigami.Units.smallSpacing

                    Kirigami.Icon {
                        source: "notifications"
                        implicitWidth: Kirigami.Units.iconSizes.small
                        implicitHeight: Kirigami.Units.iconSizes.small
                        color: Kirigami.Theme.highlightColor
                        Layout.alignment: Qt.AlignVCenter
                    }

                    PlasmaComponents.Label {
                        Layout.fillWidth: true
                        font.bold: true
                        text: i18n("Cotas Renovadas (%1 conta(s))", full.applet.activeRenewals.length)
                        color: Kirigami.Theme.highlightColor
                        textFormat: Text.PlainText
                    }

                    PlasmaComponents.Button {
                        text: i18n("✕ Dispensar")
                        display: PlasmaComponents.AbstractButton.TextOnly
                        onClicked: full.applet.dismissActiveRenewals()
                    }
                }

                Repeater {
                    model: full.applet.activeRenewals

                    delegate: RowLayout {
                        id: renewalItem
                        required property var modelData
                        Layout.fillWidth: true
                        spacing: Kirigami.Units.smallSpacing

                        Kirigami.Icon {
                            source: "dialog-ok-apply"
                            implicitWidth: Kirigami.Units.iconSizes.small
                            implicitHeight: Kirigami.Units.iconSizes.small
                            color: Kirigami.Theme.positiveTextColor
                            Layout.alignment: Qt.AlignVCenter
                        }

                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 0

                            PlasmaComponents.Label {
                                Layout.fillWidth: true
                                font.bold: true
                                text: Logic.formatAccount(renewalItem.modelData.account_label, full.applet.showFullEmail)
                                    + " (" + (renewalItem.modelData.provider_name || renewalItem.modelData.provider_id) + ")"
                                textFormat: Text.PlainText
                            }

                            PlasmaComponents.Label {
                                Layout.fillWidth: true
                                font: Kirigami.Theme.smallFont
                                opacity: 0.85
                                text: "↳ " + renewalItem.modelData.metric_label
                                    + " (" + (renewalItem.modelData.window_type || "5h") + ")"
                                    + " · Renovado pronto para uso!"
                                textFormat: Text.PlainText
                            }
                        }
                    }
                }
            }
        }

        // --- status surface -------------------------------------------------
        Rectangle {
            Layout.fillWidth: true
            Layout.topMargin: Kirigami.Units.smallSpacing
            visible: full.status !== ""
            implicitHeight: visible ? statusLabel.implicitHeight + Kirigami.Units.largeSpacing : 0
            radius: Kirigami.Units.cornerRadius
            color: Qt.alpha(full.applet.statusIsUrgent()
                ? Kirigami.Theme.negativeTextColor : Kirigami.Theme.textColor, 0.09)
            border.width: 1
            border.color: Qt.alpha(full.applet.statusIsUrgent()
                ? Kirigami.Theme.negativeTextColor : Kirigami.Theme.textColor, 0.35)

            PlasmaComponents.Label {
                id: statusLabel
                anchors.fill: parent
                anchors.margins: Kirigami.Units.smallSpacing
                wrapMode: Text.WordWrap
                font: Kirigami.Theme.smallFont
                text: full.status
                textFormat: Text.PlainText
            }
        }

        // --- usage rows (tab layout) ----------------------------------------
        Kirigami.Separator {
            Layout.fillWidth: true
            Layout.topMargin: Kirigami.Units.smallSpacing
            visible: full.applet.viewMode === 0 && full.rows.length > 0
        }

        PlasmaComponents.Label {
            Layout.fillWidth: true
            visible: full.applet.viewMode === 0 && full.rows.length > 0
            font: Kirigami.Theme.smallFont
            opacity: 0.6
            text: i18n("USAGE & BALANCE")
            textFormat: Text.PlainText
        }

        Repeater {
            model: full.applet.viewMode === 0 ? full.rows : []

            UsageRow {
                required property var modelData

                Layout.fillWidth: true
                row: modelData
                colors: full.applet.colors
                resetText: full.applet.resetText(modelData.resetAt)
                showBar: true
            }
        }

        Kirigami.Separator {
            Layout.fillWidth: true
            Layout.topMargin: Kirigami.Units.smallSpacing
            visible: full.applet.viewMode === 0 && full.displayedExtraModels().length > 0
        }

        PlasmaComponents.Label {
            Layout.fillWidth: true
            visible: full.applet.viewMode === 0 && full.displayedExtraModels().length > 0
            font: Kirigami.Theme.smallFont
            opacity: 0.6
            text: i18n("AVAILABLE MODELS (ANTIGRAVITY POOL)")
            textFormat: Text.PlainText
        }

        Repeater {
            model: full.applet.viewMode === 0 ? full.displayedExtraModels() : []

            delegate: RowLayout {
                id: extraModelRow
                required property string modelData
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing

                readonly property var poolPct: full.sharedPoolPercent()
                readonly property bool isExhausted: poolPct !== null && poolPct >= 100

                Kirigami.Icon {
                    source: extraModelRow.isExhausted ? "dialog-warning" : "emblem-favorite-symbolic"
                    implicitWidth: Kirigami.Units.iconSizes.small
                    implicitHeight: Kirigami.Units.iconSizes.small
                    color: extraModelRow.isExhausted
                        ? Kirigami.Theme.negativeTextColor
                        : Kirigami.Theme.positiveTextColor
                }

                PlasmaComponents.Label {
                    Layout.fillWidth: true
                    text: "↳ " + extraModelRow.modelData
                    textFormat: Text.PlainText
                    font: Kirigami.Theme.smallFont
                }

                PlasmaComponents.Label {
                    text: {
                        if (extraModelRow.poolPct === null) return "";
                        if (extraModelRow.isExhausted) return i18n("Sem crédito (100%)");
                        return i18n("Compartilhado (%1%)", extraModelRow.poolPct);
                    }
                    textFormat: Text.PlainText
                    font: Kirigami.Theme.smallFont
                    color: extraModelRow.isExhausted
                        ? Kirigami.Theme.negativeTextColor
                        : (extraModelRow.poolPct >= 75 ? Kirigami.Theme.neutralTextColor : Kirigami.Theme.positiveTextColor)
                }
            }
        }

        PlasmaComponents.Label {
            Layout.fillWidth: true
            Layout.topMargin: Kirigami.Units.smallSpacing
            visible: !full.entry && full.status === "" && full.applet.viewMode === 0
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            opacity: 0.6
            text: i18n("No configured provider reported usage.")
            textFormat: Text.PlainText
        }

        // --- cards (alternative layout) -------------------------------------
        // The #142 card design, reworked: one card per entry the report
        // returned, gauges per window, failures and staleness inline. Sourced
        // from the same aggregate report as the tab view, so switching the
        // layout never refetches.
        VendorCards {
            Layout.fillWidth: true
            visible: full.applet.viewMode === 1
            applet: full.applet
        }

        // --- footer ---------------------------------------------------------
        PlasmaComponents.Label {
            Layout.fillWidth: true
            Layout.topMargin: Kirigami.Units.smallSpacing
            visible: text !== ""
            horizontalAlignment: Text.AlignHCenter
            font: Kirigami.Theme.smallFont
            opacity: 0.6
            text: full.applet.updatedText()
            textFormat: Text.PlainText
        }
    }
}

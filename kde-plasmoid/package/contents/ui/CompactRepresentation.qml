pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts
import org.kde.plasma.plasmoid
import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami as Kirigami
import "../code/plasmoid-logic.mjs" as Logic

// Providing our own compactRepresentation means we own click behaviour
// entirely: Plasma's "click expands the popup" is not framework magic, it is
// plain QML in DefaultCompactRepresentation.qml which is used only when an
// applet supplies nothing. So there is nothing to suppress — we simply choose.
MouseArea {
    id: root

    required property var applet

    // Right button is deliberately NOT accepted, so it falls through to
    // Plasma's own context menu (Configure, Remove, …).
    acceptedButtons: Qt.LeftButton | Qt.MiddleButton
    hoverEnabled: true

    readonly property bool vertical: Plasmoid.formFactor === PlasmaCore.Types.Vertical

    // Effective visibility logic:
    // If all (icon, percent, bar, name) are turned off, fallback to showing icon.
    readonly property bool showIconCfg: Plasmoid.configuration.showIcon
    readonly property bool showNameCfg: Plasmoid.configuration.showName
    readonly property bool showPercentCfg: Plasmoid.configuration.showPercent
    readonly property bool showBarCfg: Plasmoid.configuration.showBars

    readonly property bool effectiveShowIcon: showIconCfg || (!showPercentCfg && !showBarCfg && !showNameCfg)
    readonly property bool effectiveShowName: showNameCfg
    readonly property bool effectiveShowValue: showPercentCfg
    readonly property bool effectiveShowBar: showBarCfg

    readonly property int effectivePointSize: {
        const custom = Plasmoid.configuration.fontSize || 0;
        return custom > 0 ? custom : Kirigami.Theme.defaultFont.pointSize;
    }

    Layout.minimumWidth: root.vertical ? 0 : content.implicitWidth
    Layout.preferredWidth: root.vertical ? 0 : content.implicitWidth
    Layout.minimumHeight: root.vertical ? content.implicitHeight : 0
    Layout.preferredHeight: root.vertical ? content.implicitHeight : 0

    // --- click -------------------------------------------------------------
    // Captured on press: clicking outside an open popup dismisses it *before*
    // the click lands, so without this a click-to-close immediately reopens.
    property bool wasExpanded: false
    onPressed: root.wasExpanded = root.applet.expanded
    onClicked: mouse => {
        if (mouse.button === Qt.MiddleButton || Plasmoid.configuration.leftClickAction === 1) {
            root.applet.launchTui();
            return;
        }
        root.applet.expanded = !root.wasExpanded;
    }

    // --- wheel: cycle the vendor ring --------------------------------------
    // angleDelta is in eighths of a degree; 120 units == 15deg == one detent.
    // `inverted` honours natural scrolling, the angleDelta.x fallback covers
    // horizontal wheels and touchpads, and the loops (not ifs) matter because a
    // fast flick delivers a delta well over 120 in a single event.
    property int wheelAccumulator: 0
    onWheel: wheel => {
        const d = wheel.angleDelta.y !== 0 ? wheel.angleDelta.y : wheel.angleDelta.x;
        root.wheelAccumulator += (wheel.inverted ? -1 : 1) * d;
        while (root.wheelAccumulator >= 120) {
            root.wheelAccumulator -= 120;
            root.applet.cycleVendor(1);   // scroll up == --cycle-next, as in Waybar
        }
        while (root.wheelAccumulator <= -120) {
            root.wheelAccumulator += 120;
            root.applet.cycleVendor(-1);
        }
    }

    // Keyboard/global-shortcut parity. Plasmoid.activated() is NOT emitted by
    // mouse clicks — only by shortcut, Enter/Space and accessibility.
    Keys.onPressed: event => {
        switch (event.key) {
        case Qt.Key_Space:
        case Qt.Key_Enter:
        case Qt.Key_Return:
        case Qt.Key_Select:
            Plasmoid.activated();
            event.accepted = true;
            break;
        }
    }

    GridLayout {
        id: content
        anchors.centerIn: parent
        flow: root.vertical ? GridLayout.TopToBottom : GridLayout.LeftToRight
        columnSpacing: Kirigami.Units.smallSpacing * 3
        rowSpacing: Kirigami.Units.smallSpacing

        // Initial loading or overall failure
        PlasmaComponents.Label {
            visible: root.applet.compactEntries.length === 0
            text: root.applet.failure ? "⚠ ai" : "..."
            color: root.applet.failure ? Kirigami.Theme.negativeTextColor : Kirigami.Theme.textColor
            textFormat: Text.PlainText
            font.pointSize: root.effectivePointSize
            Layout.alignment: Qt.AlignVCenter
        }

        Repeater {
            model: root.applet.compactEntries

            delegate: RowLayout {
                id: entryRow
                required property var modelData
                required property int index
                spacing: Kirigami.Units.smallSpacing
                Layout.alignment: Qt.AlignVCenter

                // Clean separator between providers
                Kirigami.Separator {
                    visible: entryRow.index > 0
                    Layout.fillHeight: true
                    Layout.topMargin: Kirigami.Units.smallSpacing
                    Layout.bottomMargin: Kirigami.Units.smallSpacing
                    opacity: 0.35
                }

                // Provider Icon
                RowLayout {
                    visible: root.effectiveShowIcon
                    spacing: Kirigami.Units.smallSpacing
                    Layout.alignment: Qt.AlignVCenter

                    Image {
                        id: providerIcon
                        source: entryRow.modelData.id ? Qt.resolvedUrl("../icons/" + entryRow.modelData.id + ".svg") : ""
                        sourceSize.width: Kirigami.Units.iconSizes.small
                        sourceSize.height: Kirigami.Units.iconSizes.small
                        Layout.preferredWidth: Kirigami.Units.iconSizes.small
                        Layout.preferredHeight: Kirigami.Units.iconSizes.small
                        fillMode: Image.PreserveAspectFit
                        Layout.alignment: Qt.AlignVCenter
                        visible: status === Image.Ready
                    }

                    Kirigami.Icon {
                        source: "speedometer"
                        visible: providerIcon.status !== Image.Ready
                        implicitWidth: Kirigami.Units.iconSizes.small
                        implicitHeight: Kirigami.Units.iconSizes.small
                        Layout.alignment: Qt.AlignVCenter
                    }
                }

                // Provider Name if no metrics or when value is off
                PlasmaComponents.Label {
                    visible: root.effectiveShowName && (entryRow.modelData.cells.length === 0 || !root.effectiveShowValue)
                    text: entryRow.modelData.label
                    textFormat: Text.PlainText
                    font.bold: true
                    font.pointSize: root.effectivePointSize
                    Layout.alignment: Qt.AlignVCenter
                }

                // Error state for this specific provider
                PlasmaComponents.Label {
                    visible: entryRow.modelData.cells.length === 0 && (root.effectiveShowValue || root.effectiveShowName)
                    text: entryRow.modelData.failure ? "⚠ error" : "ready"
                    color: entryRow.modelData.failure ? Kirigami.Theme.negativeTextColor : Kirigami.Theme.textColor
                    textFormat: Text.PlainText
                    font.pointSize: root.effectivePointSize
                    Layout.alignment: Qt.AlignVCenter
                }

                // Metric Cells for this provider
                Repeater {
                    model: entryRow.modelData.cells

                    delegate: RowLayout {
                        id: cell
                        required property var modelData
                        required property int index
                        spacing: cell.isExtra ? (Kirigami.Units.smallSpacing / 2) : Kirigami.Units.smallSpacing
                        visible: root.effectiveShowName || root.effectiveShowValue || root.effectiveShowBar
                        Layout.alignment: Qt.AlignVCenter

                        readonly property bool isExtra: entryRow.modelData.id === "antigravity" && cell.index > 0
                        readonly property bool isExhausted: cell.isExtra && cell.modelData.percent !== null && cell.modelData.percent >= 100

                        PlasmaComponents.Label {
                            visible: text !== "" && root.effectiveShowName
                            text: cell.isExtra ? ("↳ " + cell.modelData.label) : cell.modelData.label
                            textFormat: Text.PlainText
                            opacity: cell.isExtra ? 0.9 : 0.85
                            font.italic: cell.isExtra
                            font.bold: cell.isExhausted
                            font.pointSize: root.effectivePointSize
                            color: cell.isExhausted
                                ? Kirigami.Theme.negativeTextColor
                                : Kirigami.Theme.textColor
                            Layout.alignment: Qt.AlignVCenter
                        }

                        PlasmaComponents.Label {
                            visible: root.effectiveShowValue
                            text: cell.modelData.text
                            textFormat: Text.PlainText
                            font.bold: cell.isExhausted
                            font.pointSize: root.effectivePointSize
                            color: cell.isExhausted
                                ? Kirigami.Theme.negativeTextColor
                                : (Logic.severityColor(cell.modelData.severity, root.applet.colors) ?? Kirigami.Theme.textColor)
                            Layout.alignment: Qt.AlignVCenter
                        }

                        UsageBar {
                            visible: root.effectiveShowBar && cell.modelData.percent !== null
                            pct: cell.modelData.percent ?? 0
                            severity: cell.modelData.severity
                            colors: root.applet.colors
                            implicitWidth: Kirigami.Units.gridUnit * Plasmoid.configuration.barWidth / (cell.isExtra ? 5 : 4)
                            Layout.alignment: Qt.AlignVCenter
                        }
                    }
                }
            }
        }

        // Small Renewal Notice badge in panel ("algo como notificacao pequeno")
        RowLayout {
            id: renewalBadgeRow
            visible: root.applet.activeRenewals.length > 0
            spacing: Kirigami.Units.smallSpacing / 2
            Layout.alignment: Qt.AlignVCenter

            Rectangle {
                id: notifBadge
                Layout.preferredWidth: Math.max(notifRow.implicitWidth + Kirigami.Units.smallSpacing * 3, Kirigami.Units.gridUnit * 1.6)
                Layout.preferredHeight: Math.max(notifRow.implicitHeight + Kirigami.Units.smallSpacing, Kirigami.Units.gridUnit * 1.2)
                implicitWidth: Layout.preferredWidth
                implicitHeight: Layout.preferredHeight
                radius: height / 2
                color: notifMouse.containsMouse
                    ? Kirigami.Theme.highlightColor
                    : Qt.alpha(Kirigami.Theme.highlightColor, 0.3)
                border.width: 1
                border.color: Kirigami.Theme.highlightColor

                RowLayout {
                    id: notifRow
                    anchors.centerIn: parent
                    spacing: Kirigami.Units.smallSpacing / 2

                    Kirigami.Icon {
                        source: "notifications"
                        implicitWidth: Kirigami.Units.iconSizes.small
                        implicitHeight: Kirigami.Units.iconSizes.small
                        color: notifMouse.containsMouse
                            ? Kirigami.Theme.highlightedTextColor
                            : Kirigami.Theme.highlightColor
                        Layout.alignment: Qt.AlignVCenter
                    }

                    PlasmaComponents.Label {
                        text: root.applet.activeRenewals.length.toString()
                        font.bold: true
                        font.pointSize: root.effectivePointSize
                        color: notifMouse.containsMouse
                            ? Kirigami.Theme.highlightedTextColor
                            : Kirigami.Theme.highlightColor
                        Layout.alignment: Qt.AlignVCenter
                        textFormat: Text.PlainText
                    }
                }

                MouseArea {
                    id: notifMouse
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    z: 2
                    onClicked: {
                        root.applet.triggerRenewalNoticeToggle();
                    }
                }

                PlasmaComponents.ToolTip {
                    visible: notifMouse.containsMouse
                    text: i18n("%1 conta(s) renovaram cota! Clique para ver aviso.", root.applet.activeRenewals.length)
                }
            }
        }
    }
}

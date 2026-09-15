import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as QQC2
import org.kde.plasma.plasma5support as Plasma5Support
import org.kde.kirigami as Kirigami
import org.kde.kcmutils as KCM
import "../code/plasmoid-logic.mjs" as Logic

// The cfg_<key> properties ARE the mechanism: Plasma copies each config value
// onto a property of that exact name here, and reads them back on Apply. A typo
// in the suffix fails silently — the setting simply never persists.
KCM.SimpleKCM {
    id: page

    property alias cfg_interval: intervalSpin.value
    property alias cfg_commandTimeout: commandTimeoutSpin.value
    property alias cfg_binaryPath: binaryField.text
    property alias cfg_terminalCommand: terminalField.text
    property alias cfg_useThemeColors: themeColorsCheck.checked
    property alias cfg_showBars: showBarsCheck.checked
    property alias cfg_barWidth: barWidthSpin.value
    property alias cfg_fontSize: fontSizeSpin.value
    property alias cfg_showPercent: showPercentCheck.checked
    property alias cfg_showIcon: showIconCheck.checked
    property alias cfg_showName: showNameCheck.checked
    property alias cfg_showAllProviders: showAllProvidersCheck.checked
    property alias cfg_showFullEmail: showFullEmailCheck.checked
    property alias cfg_showExtraModels: showExtraModelsCheck.checked
    property alias cfg_notifyResets: notifyResetsCheck.checked
    property var cfg_selectedExtraModels: ["Claude Sonnet 4.6 (Thinking)", "Claude Opus 4.6 (Thinking)", "GPT-OSS 120B (Medium)"]
    property alias cfg_colorLow: lowSwatch.hex
    property alias cfg_colorMid: midSwatch.hex
    property alias cfg_colorHigh: highSwatch.hex
    property alias cfg_colorCritical: criticalSwatch.hex
    property alias cfg_colorEmpty: emptySwatch.hex
    // A ComboBox cannot use `property alias` to currentIndex: the alias would be
    // write-only from Plasma's side at load time, so the saved value never shows.
    property int cfg_leftClickAction: 0
    property int cfg_viewMode: 0
    property int cfg_compactDisplayMode: 0
    property string cfg_vendor: ""
    property var cfg_vendorRing: []
    property var cfg_dismissedRenewals: []

    function hasExtraModel(modelName) {
        return Array.from(page.cfg_selectedExtraModels || []).indexOf(modelName) !== -1;
    }

    function toggleExtraModel(modelName, on) {
        const list = Array.from(page.cfg_selectedExtraModels || []).filter(m => m !== modelName);
        if (on) list.push(modelName);
        page.cfg_selectedExtraModels = list;
    }

    // Which vendors exist and whether they currently work. Sourced from
    // `ai-usagebar usage --json`, which reports every entry enabled in
    // config.toml plus its plan or its error — so you can see that Codex has no
    // credentials *before* putting it in the scroll ring.
    property var vendorList: []
    property var allProvidersList: []
    property bool probing: true

    Plasma5Support.DataSource {
        id: prober
        engine: "executable"
        connectedSources: []
        onNewData: (sourceName, data) => {
            disconnectSource(sourceName);
            page.probing = false;
            const report = Logic.parseReport(data["stdout"] || "");
            page.vendorList = report.entries;
        }
    }

    Plasma5Support.DataSource {
        id: providerRunner
        engine: "executable"
        connectedSources: []
        onNewData: (sourceName, data) => {
            disconnectSource(sourceName);
            const list = Logic.parseProviders(data["stdout"] || "");
            if (list.length > 0)
                page.allProvidersList = list;
        }
        function exec(cmd) {
            if (connectedSources.indexOf(cmd) === -1)
                connectSource(cmd);
        }
    }

    function toggleProvider(id, enable) {
        providerRunner.exec(Logic.buildProviderToggleCommand(page.cfg_binaryPath, id, enable));
        // Re-probe after toggle
        prober.connectSource(Logic.buildCommand(page.cfg_binaryPath, page.cfg_commandTimeout));
        providerRunner.exec(Logic.buildProvidersListCommand(page.cfg_binaryPath));
    }

    Component.onCompleted: {
        prober.connectSource(Logic.buildCommand(
            page.cfg_binaryPath, page.cfg_commandTimeout));
        providerRunner.exec(Logic.buildProvidersListCommand(page.cfg_binaryPath));
    }

    // The report owns the canonical display name, so there is no second table
    // here to drift out of sync with it. Falls back to the raw id while the
    // probe is still running or when the vendor left config.toml.
    function labelFor(id) {
        for (const entry of page.vendorList)
            if (entry.id === id)
                return entry.label;
        return Logic.safeText(id, 60);
    }

    function ringHas(id) {
        return Array.from(page.cfg_vendorRing || []).indexOf(id) !== -1;
    }

    function setRing(id, on) {
        const next = Array.from(page.cfg_vendorRing || []).filter(v => v !== id);
        if (on)
            next.push(id);
        page.cfg_vendorRing = next;
        // Never leave the applet pointing at a vendor no longer in the ring.
        if (next.length > 0 && next.indexOf(page.cfg_vendor) === -1)
            page.cfg_vendor = next[0];
    }

    Kirigami.FormLayout {
        anchors.fill: parent

        QQC2.Label {
            Kirigami.FormData.label: i18n("Vendors:")
            text: page.probing
                ? i18n("Checking which vendors are configured…")
                : i18n("Tick the ones the scroll gesture cycles through. Status comes from your config.toml.")
            font: Kirigami.Theme.smallFont
            opacity: 0.7
            wrapMode: Text.WordWrap
            Layout.maximumWidth: Kirigami.Units.gridUnit * 24
            textFormat: Text.PlainText
        }

        Repeater {
            model: page.vendorList

            delegate: RowLayout {
                id: choice
                required property var modelData
                spacing: Kirigami.Units.smallSpacing

                QQC2.CheckBox {
                    text: choice.modelData.label || choice.modelData.id
                    checked: page.ringHas(choice.modelData.id)
                    onToggled: page.setRing(choice.modelData.id, checked)
                }

                // The plan when the vendor answers, its own error when it does
                // not — so "no API key" is visible here, before the vendor goes
                // into the scroll ring, rather than as a ⚠ in the panel later.
                QQC2.Label {
                    readonly property bool failing: choice.modelData.status === "error"
                        || !!choice.modelData.error

                    text: failing ? "✗ " + Logic.errorMessage(choice.modelData.error)
                                  : "✓ " + (choice.modelData.plan || choice.modelData.status)
                    color: failing ? Kirigami.Theme.negativeTextColor
                                   : Kirigami.Theme.positiveTextColor
                    font: Kirigami.Theme.smallFont
                    elide: Text.ElideRight
                    Layout.fillWidth: true
                    Layout.maximumWidth: Kirigami.Units.gridUnit * 20
                    textFormat: Text.PlainText
                }
            }
        }

        QQC2.ComboBox {
            id: currentVendorCombo
            Kirigami.FormData.label: i18n("Current vendor:")
            model: Array.from(page.cfg_vendorRing || [])
            textRole: ""
            displayText: page.labelFor(page.cfg_vendor)
            onActivated: page.cfg_vendor = model[currentIndex]
            delegate: QQC2.ItemDelegate {
                required property var modelData
                required property int index
                width: currentVendorCombo.width
                text: page.labelFor(modelData)
                highlighted: currentVendorCombo.highlightedIndex === index
            }
        }

        Item { Kirigami.FormData.isSection: true }

        QQC2.SpinBox {
            id: intervalSpin
            Kirigami.FormData.label: i18n("Refresh interval (s):")
            // Matches <min>30</min> in config/main.xml and the floor main.qml
            // clamps to. One report covers every configured vendor, so the
            // panel does not need a per-vendor cadence — the countdowns tick
            // locally from reset_at between fetches.
            from: 30
            to: 3600
            stepSize: 30
        }

        QQC2.SpinBox {
            id: commandTimeoutSpin
            Kirigami.FormData.label: i18n("Command timeout (s):")
            from: 60
            to: 3600
            stepSize: 60
        }

        QQC2.ComboBox {
            id: clickCombo
            Kirigami.FormData.label: i18n("Left click:")
            model: [i18n("Open the panel popup"), i18n("Open the TUI")]
            currentIndex: page.cfg_leftClickAction
            onActivated: page.cfg_leftClickAction = currentIndex
        }

        QQC2.ComboBox {
            id: viewCombo
            Kirigami.FormData.label: i18n("Popup layout:")
            model: [i18n("Provider tabs"), i18n("One card per vendor")]
            currentIndex: page.cfg_viewMode
            onActivated: page.cfg_viewMode = currentIndex
        }

        QQC2.TextField {
            id: terminalField
            Kirigami.FormData.label: i18n("Terminal:")
            placeholderText: i18n("Automatic (konsole, x-terminal-emulator…)")
        }

        Item { Kirigami.FormData.isSection: true }

        QQC2.Label {
            Kirigami.FormData.label: i18n("Providers:")
            text: i18n("Enable or disable providers installed on this system:")
            font: Kirigami.Theme.smallFont
            opacity: 0.7
            wrapMode: Text.WordWrap
            Layout.maximumWidth: Kirigami.Units.gridUnit * 24
            textFormat: Text.PlainText
        }

        Repeater {
            model: page.allProvidersList

            delegate: RowLayout {
                id: provChoice
                required property var modelData
                spacing: Kirigami.Units.smallSpacing

                QQC2.CheckBox {
                    text: provChoice.modelData.name || provChoice.modelData.id
                    checked: provChoice.modelData.enabled
                    onToggled: page.toggleProvider(provChoice.modelData.id, checked)
                }

                QQC2.Label {
                    text: provChoice.modelData.configured ? i18n("✓ Configured") : i18n("Needs credentials")
                    color: provChoice.modelData.configured ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.neutralTextColor
                    font: Kirigami.Theme.smallFont
                    elide: Text.ElideRight
                    Layout.fillWidth: true
                    Layout.maximumWidth: Kirigami.Units.gridUnit * 20
                    textFormat: Text.PlainText
                }
            }
        }

        Item { Kirigami.FormData.isSection: true }

        QQC2.CheckBox {
            id: showAllProvidersCheck
            Kirigami.FormData.label: i18n("Panel providers:")
            text: i18n("Show all active providers side by side in panel")
        }

        QQC2.ComboBox {
            id: compactDisplayModeCombo
            Kirigami.FormData.label: i18n("Display mode:")
            model: [
                i18n("All (Icon + Name + % + Bar)"),
                i18n("Icon only"),
                i18n("Icon and Percentage"),
                i18n("Icon and Progress Bar"),
                i18n("Name and Percentage"),
                i18n("Name only"),
                i18n("Percentage only"),
                i18n("Percentage and Progress Bar")
            ]
            currentIndex: page.cfg_compactDisplayMode
            onActivated: {
                page.cfg_compactDisplayMode = currentIndex;
                if (currentIndex === 0) {
                    showIconCheck.checked = true;
                    showNameCheck.checked = true;
                    showPercentCheck.checked = true;
                    showBarsCheck.checked = true;
                } else if (currentIndex === 1) {
                    showIconCheck.checked = true;
                    showNameCheck.checked = false;
                    showPercentCheck.checked = false;
                    showBarsCheck.checked = false;
                } else if (currentIndex === 2) {
                    showIconCheck.checked = true;
                    showNameCheck.checked = false;
                    showPercentCheck.checked = true;
                    showBarsCheck.checked = false;
                } else if (currentIndex === 3) {
                    showIconCheck.checked = true;
                    showNameCheck.checked = false;
                    showPercentCheck.checked = false;
                    showBarsCheck.checked = true;
                } else if (currentIndex === 4) {
                    showIconCheck.checked = false;
                    showNameCheck.checked = true;
                    showPercentCheck.checked = true;
                    showBarsCheck.checked = false;
                } else if (currentIndex === 5) {
                    showIconCheck.checked = false;
                    showNameCheck.checked = true;
                    showPercentCheck.checked = false;
                    showBarsCheck.checked = false;
                } else if (currentIndex === 6) {
                    showIconCheck.checked = false;
                    showNameCheck.checked = false;
                    showPercentCheck.checked = true;
                    showBarsCheck.checked = false;
                } else if (currentIndex === 7) {
                    showIconCheck.checked = false;
                    showNameCheck.checked = false;
                    showPercentCheck.checked = true;
                    showBarsCheck.checked = true;
                }
            }
        }

        QQC2.CheckBox {
            id: showIconCheck
            text: i18n("Show icon")
        }

        QQC2.CheckBox {
            id: showNameCheck
            text: i18n("Show provider/metric name")
        }

        QQC2.CheckBox {
            id: showPercentCheck
            text: i18n("Show percentage/value")
        }

        QQC2.CheckBox {
            id: showBarsCheck
            text: i18n("Show progress bars in panel")
        }

        QQC2.SpinBox {
            id: fontSizeSpin
            Kirigami.FormData.label: i18n("Panel font size:")
            from: 0
            to: 32
            stepSize: 1
            textFromValue: function(value) {
                return value === 0 ? i18n("System default") : value + " pt";
            }
            valueFromText: function(text) {
                const n = parseInt(text, 10);
                return isNaN(n) ? 0 : n;
            }
        }

        QQC2.CheckBox {
            id: showFullEmailCheck
            Kirigami.FormData.label: i18n("Accounts:")
            text: i18n("Show full email address (off = username only)")
        }

        QQC2.CheckBox {
            id: showExtraModelsCheck
            Kirigami.FormData.label: i18n("Extra models:")
            text: i18n("Show extra models (Claude & GPT in Antigravity)")
        }

        Repeater {
            model: showExtraModelsCheck.checked ? [
                "Claude Sonnet 4.6 (Thinking)",
                "Claude Opus 4.6 (Thinking)",
                "GPT-OSS 120B (Medium)"
            ] : []

            delegate: RowLayout {
                id: extraChoice
                required property string modelData
                spacing: Kirigami.Units.smallSpacing

                QQC2.CheckBox {
                    text: extraChoice.modelData
                    checked: page.hasExtraModel(extraChoice.modelData)
                    onToggled: page.toggleExtraModel(extraChoice.modelData, checked)
                }

                QQC2.Label {
                    text: i18n("Shared quota pool")
                    font: Kirigami.Theme.smallFont
                    opacity: 0.6
                    textFormat: Text.PlainText
                }
            }
        }

        RowLayout {
            Kirigami.FormData.label: i18n("Notifications:")
            spacing: Kirigami.Units.smallSpacing

            QQC2.CheckBox {
                id: notifyResetsCheck
                text: i18n("Desktop notifications when account quotas reset")
            }

            QQC2.Button {
                text: i18n("Test Notification")
                icon.name: "preferences-system-notifications"
                onClicked: {
                    providerRunner.exec(Logic.buildMonitorTestCommand(page.cfg_binaryPath));
                }
            }

            QQC2.Button {
                text: i18n("Simulate Renewal Alert")
                icon.name: "dialog-information"
                onClicked: {
                    providerRunner.exec(Logic.buildSimulateRenewalCommand(page.cfg_binaryPath));
                }
            }
        }

        QQC2.CheckBox {
            id: themeColorsCheck
            Kirigami.FormData.label: i18n("Colors:")
            text: i18n("Follow the Plasma colour scheme")
        }

        QQC2.SpinBox {
            id: barWidthSpin
            Kirigami.FormData.label: i18n("Width of each panel bar (cells):")
            from: 4
            to: 20
            enabled: showBarsCheck.checked
        }

        Item { Kirigami.FormData.isSection: true }

        ColorSwatch {
            id: lowSwatch
            Kirigami.FormData.label: i18n("Low (<50%):")
            enabled: !themeColorsCheck.checked
        }

        ColorSwatch {
            id: midSwatch
            Kirigami.FormData.label: i18n("Medium (50-74%):")
            enabled: !themeColorsCheck.checked
        }

        ColorSwatch {
            id: highSwatch
            Kirigami.FormData.label: i18n("High (75-89%):")
            enabled: !themeColorsCheck.checked
        }

        ColorSwatch {
            id: criticalSwatch
            Kirigami.FormData.label: i18n("Critical (>=90%):")
            enabled: !themeColorsCheck.checked
        }

        ColorSwatch {
            id: emptySwatch
            Kirigami.FormData.label: i18n("Empty (bar background):")
            enabled: !themeColorsCheck.checked
        }

        QQC2.Label {
            text: i18n("Used when \"Follow the Plasma colour scheme\" is off")
            font: Kirigami.Theme.smallFont
            opacity: 0.7
            textFormat: Text.PlainText
        }

        Item { Kirigami.FormData.isSection: true }

        QQC2.TextField {
            id: binaryField
            Kirigami.FormData.label: i18n("Binary path (empty = auto):")
            placeholderText: i18n("empty = look on PATH")
        }

        QQC2.Label {
            text: i18n("plasmashell does not inherit your shell PATH, so a cargo\ninstall may need the full path here.")
            font: Kirigami.Theme.smallFont
            opacity: 0.7
            textFormat: Text.PlainText
        }
    }
}

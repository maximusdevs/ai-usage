pragma ComponentBehavior: Bound

import QtQuick
import org.kde.plasma.plasmoid
import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.plasma5support as Plasma5Support
import org.kde.kirigami as Kirigami
import "../code/plasmoid-logic.mjs" as Logic

PlasmoidItem {
    id: root

    // --- state -------------------------------------------------------------
    property var report: null       // parsed `usage --json`, or null
    property string failure: ""     // our failure: spawn, timeout, unparseable

    // The command currently in flight, "" when idle. Also the watchdog's handle
    // on which source to drop.
    property string pendingCommand: ""

    // Ticked locally so every "resets in" and "updated N ago" counts down
    // between fetches. Without it the popup would look frozen for minutes at a
    // time, which is what makes a slow refresh interval acceptable at all.
    property double nowMs: 0

    // A report fetch can visit several providers sequentially, each with its
    // own network timeout. Keep this configurable and bounded rather than
    // killing a healthy multi-provider report after one minute.
    readonly property int fetchTimeoutSecs: Logic.timeoutSeconds(
        Plasmoid.configuration.commandTimeout)

    // The floor the settings actually offer: config/main.xml declares <min>5</min>
    // and the spinbox steps from 5 or 10. One report covers every configured vendor.
    readonly property int minIntervalSecs: 5

    readonly property string vendor: Plasmoid.configuration.vendor
    readonly property var entry: Logic.entryFor(root.report, root.vendor)
    readonly property var tabs: Logic.vendorTabs(root.report, root.vendor)
    readonly property var compactCells: Logic.panelCells(root.entry, {max: 2})
    readonly property var accounts: (root.report && root.report.accounts) || []
    property string activeAccountOverride: ""
    readonly property var rawRenewals: (root.report && root.report.renewals) || []
    property var dismissedRenewalIds: Plasmoid.configuration.dismissedRenewals || []
    property bool showRenewalNotice: false

    readonly property var activeRenewals: Logic.filterActiveRenewals(
        root.rawRenewals, root.dismissedRenewalIds)

    function dismissActiveRenewals() {
        var set = [];
        var i;
        var existing = root.dismissedRenewalIds || [];
        for (i = 0; i < existing.length; i++) {
            set.push(existing[i]);
        }
        var raw = root.rawRenewals || [];
        for (i = 0; i < raw.length; i++) {
            var r = raw[i];
            if (r && r.id && set.indexOf(r.id) === -1) {
                set.push(r.id);
            }
        }
        root.dismissedRenewalIds = set;
        Plasmoid.configuration.dismissedRenewals = set;
        root.showRenewalNotice = false;
    }

    function triggerRenewalNoticeToggle() {
        if (root.showRenewalNotice) {
            dismissActiveRenewals();
        } else {
            root.showRenewalNotice = true;
            root.expanded = true;
        }
    }

    readonly property bool showBars: Plasmoid.configuration.showBars
    readonly property bool showPercent: Plasmoid.configuration.showPercent
    readonly property bool showIcon: Plasmoid.configuration.showIcon
    readonly property bool showName: Plasmoid.configuration.showName
    readonly property bool showAllProviders: Plasmoid.configuration.showAllProviders
    readonly property bool multiAccount: Plasmoid.configuration.multiAccount
    readonly property bool showFullEmail: Plasmoid.configuration.showFullEmail
    readonly property bool showExtraModels: Plasmoid.configuration.showExtraModels
    readonly property var selectedExtraModels: Plasmoid.configuration.selectedExtraModels || []
    readonly property int compactDisplayMode: Plasmoid.configuration.compactDisplayMode

    readonly property var compactEntries: {
        const entries = (root.report && root.report.entries) || [];
        if (!root.showAllProviders) {
            const single = root.entry;
            if (!single) return [];
            return [{
                id: single.id,
                label: single.label,
                cells: root.compactCells,
                status: single.status,
                failure: single.status === "error"
            }];
        }
        return entries.map(e => ({
            id: e.id,
            label: e.label,
            cells: Logic.panelCells(e, {max: 2}),
            status: e.status,
            failure: e.status === "error"
        }));
    }

    // Read back as the integer index of the Enum choice (0 = tabs, 1 = cards).
    readonly property int viewMode: Plasmoid.configuration.viewMode
    // The cards view projects every entry the report returned; it needs no
    // per-vendor selection and never refetches when toggled.
    readonly property var cards: Logic.cardModel(root.report)

    // The user's own five colours, defaulting to One Dark exactly as in
    // src/theme.rs, the GNOME extension and the macOS app.
    readonly property var fixedColors: ({
        low: Plasmoid.configuration.colorLow,
        mid: Plasmoid.configuration.colorMid,
        high: Plasmoid.configuration.colorHigh,
        critical: Plasmoid.configuration.colorCritical,
        empty: Plasmoid.configuration.colorEmpty,
    })

    // Referencing the Kirigami roles inside the binding is what makes this
    // reactive: they are notifying properties, so a theme switch recolours with
    // no restart. Assigning imperatively anywhere would freeze the old colours.
    readonly property var colors: Plasmoid.configuration.useThemeColors
        ? Logic.paletteFromTheme({
            textColor: Kirigami.Theme.textColor,
            neutralTextColor: Kirigami.Theme.neutralTextColor,
            negativeTextColor: Kirigami.Theme.negativeTextColor,
            positiveTextColor: Kirigami.Theme.positiveTextColor,
            disabledTextColor: Kirigami.Theme.disabledTextColor,
        })
        : root.fixedColors

    // --- applet plumbing ---------------------------------------------------
    Plasmoid.icon: "speedometer"

    switchWidth: Kirigami.Units.gridUnit * 14
    switchHeight: Kirigami.Units.gridUnit * 10

    // When left click launches the TUI, the global shortcut and Enter/Space
    // must not still open a popup we have decided not to use.
    activationTogglesExpanded: Plasmoid.configuration.leftClickAction === 0
    Plasmoid.onActivated: {
        if (Plasmoid.configuration.leftClickAction === 1)
            root.launchTui();
    }

    compactRepresentation: CompactRepresentation { applet: root }
    fullRepresentation: FullRepresentation { applet: root }

    toolTipMainText: root.entry ? root.entry.label : i18n("AI Usage Bar")
    toolTipSubText: root.failure ? root.failure
        : (root.entry ? (root.entry.plan || root.entry.status) : i18n("Loading…"))
    toolTipItem: UsageToolTip { applet: root }

    // Wording deliberately identical to the GNOME dropdown and the macOS menu.
    Plasmoid.contextualActions: [
        PlasmaCore.Action {
            text: i18n("Refresh now")
            icon.name: "view-refresh"
            onTriggered: root.refresh(true)
        },
        PlasmaCore.Action {
            text: i18n("Open TUI")
            icon.name: "utilities-terminal"
            onTriggered: root.launchTui()
        }
    ]

    // --- words -------------------------------------------------------------
    // The pure layer returns milliseconds; the sentences are built here so they
    // go through i18n() like the rest of the applet's chrome.
    function resetText(resetAt) {
        const ms = Logic.resetRemainingMs(resetAt, root.nowMs);
        if (ms === null)
            return "";
        return ms > 0 ? i18n("Resets in %1", Logic.formatDuration(ms))
                      : i18n("Reset due");
    }

    function updatedText() {
        if (!root.entry)
            return "";
        const ms = Logic.updatedAgeMs(root.entry.fetchedAt, root.nowMs);
        if (ms === null)
            return i18n("Updated time unavailable");
        const base = ms < 60000 ? i18n("Updated just now")
                                : i18n("Updated %1 ago", Logic.formatDuration(ms));
        return root.pendingCommand !== "" ? base + i18n(" · refreshing…") : base;
    }

    // The one line that explains a non-normal state, "" when all is well.
    function statusMessage() {
        if (root.failure)
            return root.failure;
        if (!root.report)
            return "";
        if (!root.entry)
            return i18n("No configured provider reported usage.");
        if (root.entry.status === "error" || root.entry.error)
            return Logic.errorMessage(root.entry.error);
        if (root.entry.stale)
            return i18n("Cached data · the provider could not supply a fresh response.");
        return "";
    }

    function statusIsUrgent() {
        return root.failure !== "" || (!!root.entry && root.entry.status === "error");
    }

    // --- data --------------------------------------------------------------
    Plasma5Support.DataSource {
        id: reader
        engine: "executable"
        connectedSources: []

        onNewData: (sourceName, data) => {
            // MANDATORY: the source name IS the command string and stays
            // connected, re-running on the engine's own interval. Disconnecting
            // here turns every connectSource into a strict one-shot.
            disconnectSource(sourceName);
            if (sourceName !== root.pendingCommand)
                return;
            root.pendingCommand = "";
            watchdog.stop();
            root.consume(data);
        }

        function exec(cmd) {
            // Connecting an already-connected source is a no-op, so a slow
            // command cannot pile up duplicate runs.
            if (connectedSources.indexOf(cmd) === -1)
                connectSource(cmd);
        }
    }

    // Separate source for fire-and-forget launches: sharing `reader` would feed
    // the terminal's output into consume() and clobber the report.
    Plasma5Support.DataSource {
        id: launcher
        engine: "executable"
        connectedSources: []
        onNewData: sourceName => disconnectSource(sourceName)
        function exec(cmd) {
            if (connectedSources.indexOf(cmd) === -1)
                connectSource(cmd);
        }
    }

    function currentCommand(options) {
        const opts = options || {};
        if (root.activeAccountOverride && !opts.account)
            opts.account = root.activeAccountOverride;
        return Logic.buildCommand(Plasmoid.configuration.binaryPath, root.fetchTimeoutSecs, opts);
    }

    function refresh(forced) {
        if (forced || !root.multiAccount) {
            root.activeAccountOverride = "";
        }
        const cmd = root.currentCommand(forced ? { refresh: true } : {});
        if (!Logic.shouldStartFetch(root.pendingCommand, cmd))
            return;
        root.pendingCommand = cmd;
        watchdog.restart();
        reader.exec(cmd);
    }

    function switchAccount(label) {
        if (!root.multiAccount || !label) return;
        root.activeAccountOverride = label;
        launcher.exec(Logic.buildAccountSwitchCommand(Plasmoid.configuration.binaryPath, label));
        root.refresh(true);
    }

    // Backstop only. timeout(1) in the spawned command is what bounds and kills
    // a hung binary (see buildArgv); this covers the remaining case where the
    // data engine never reports back at all. Fires after timeout(1) would have,
    // so the process-level kill is what the user normally sees, not this.
    Timer {
        id: watchdog
        interval: (root.fetchTimeoutSecs + Logic.TIMEOUT_KILL_GRACE_SECS + 5) * 1000
        repeat: false
        onTriggered: {
            if (root.pendingCommand === "")
                return;
            reader.disconnectSource(root.pendingCommand);
            root.pendingCommand = "";
            root.failure = i18n("ai-usagebar took too long (>%1s)", root.fetchTimeoutSecs);
        }
    }

    function consume(data) {
        const exitCode = data["exit code"];
        const stdout = data["stdout"] || "";
        const stderr = data["stderr"] || "";

        // timeout(1) killed it. Report that as a timeout rather than letting it
        // fall through to "invalid output", which is what a half-written stdout
        // would otherwise look like.
        if (exitCode === Logic.EXIT_TIMED_OUT || exitCode === Logic.EXIT_KILLED) {
            root.failure = i18n("ai-usagebar took too long (>%1s)", root.fetchTimeoutSecs);
            return;
        }

        const parsed = Logic.parseReport(stdout);
        if (!parsed.ok) {
            // Keep the ORIGINAL output as the detail line. A bare "invalid
            // output" throws away the only actionable thing the user has. A
            // vendor-side failure never lands here — it arrives as an entry
            // with status "error" — so this really is a missing or broken
            // binary.
            const headline = exitCode !== 0
                ? (stderr.trim() ? i18n("ai-usagebar failed") : i18n("ai-usagebar exited with %1", exitCode))
                : i18n("invalid output");
            const detail = Logic.safeText(
                (exitCode !== 0 ? stderr.trim() : parsed.raw) || "", 300);
            root.failure = detail ? headline + "\n" + detail.substring(0, 300) : headline;
            return;
        }

        root.failure = "";
        root.report = parsed;
        if (parsed.account && parsed.account.label) {
            root.activeAccountOverride = parsed.account.label;
        }
    }

    function selectVendor(id) {
        if (id && id !== root.vendor)
            Plasmoid.configuration.vendor = id;
    }

    function cycleVendor(delta) {
        // Scroll walks the ring the user configured; the tab strip in the popup
        // is the discoverable way to reach a vendor that is not in it.
        const next = Logic.nextVendor(Plasmoid.configuration.vendorRing, root.vendor, delta);
        root.selectVendor(next);
    }

    function launchTui() {
        launcher.exec(Logic.buildTuiCommand(Plasmoid.configuration.terminalCommand));
    }

    // One report covers every vendor, so switching only re-picks an entry that
    // is already in hand — no refetch, and the popup repaints instantly.
    Timer {
        interval: Math.max(root.minIntervalSecs, Plasmoid.configuration.interval) * 1000
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: root.refresh()
    }

    // Cheap local tick for the countdowns. Deliberately not a fetch.
    Timer {
        interval: 30000
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: root.nowMs = Date.now()
    }
}

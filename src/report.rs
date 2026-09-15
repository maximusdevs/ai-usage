//! `ai-usagebar usage` — quota and time-to-reset for everything in the config,
//! in one pass.
//!
//! The widget answers "how is *this* vendor doing" one process at a time, which
//! is what a status bar needs and what a person checking on four Claude
//! accounts does not. This walks the same tab set the TUI builds — every
//! enabled vendor, plus one entry per named Claude account — and prints what
//! each one has left.
//!
//! Deliberately thin: [`crate::tui::app::tabs_from_config`] already decides
//! what is configured, [`crate::tui::app::refresh_one`] already fetches and
//! parses it, and [`crate::tui::panels::sections_for`] already projects any
//! vendor's snapshot into labelled sections carrying every reported value.
//! So this file only enumerates, projects, and formats — no vendor
//! ever needs to know it exists.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;

use crate::config::Config;
use crate::tui::app::{TabId, TabSource, TabState, tabs_with_desktop};
use crate::tui::panels::{Section, sections_with_metadata_for};

/// Matches the widget's `--pace-tolerance` default; only affects the pacing
/// note appended to a metric's detail line.
const PACE_TOLERANCE: u32 = 5;

/// Version of the tolerant, machine-readable `usage --json` contract.
/// Increment only when an incompatible change cannot be represented by adding
/// or omitting fields.
const USAGE_SCHEMA_VERSION: u8 = 1;

pub use crate::account_store::{ReportEntry as Entry, ReportSection};

/// Snapshot every configured vendor as the JSON `usage --json` prints.
///
/// Account info displayed in report headers and JSON output.
#[derive(Debug, Clone, Serialize)]
pub struct AccountHeaderInfo {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_time: Option<DateTime<Utc>>,
}

/// A frontend hosted in the same process calls this instead of spawning a
/// console subprocess. Errors are the same user-facing strings `usage` would
/// print.
pub async fn collect_json() -> std::result::Result<String, String> {
    let (entries, primary, account) = collect_entries(None, false).await?;
    Ok(render_json_with_account(
        &entries,
        primary,
        account.as_ref(),
    ))
}

/// Snapshot one configured vendor or account — the entry whose `id` is
/// `entry_id`, as `collect_json` would have labelled it — as `{"entries": [..]}`.
///
/// A frontend's per-provider "Refresh" calls this so re-fetching one vendor
/// does not hit every other one. No `primary` key: the caller already knows
/// which entry it asked for.
pub async fn collect_entry_json(entry_id: &str) -> std::result::Result<String, String> {
    let config = Config::load().map_err(|error| error.user_message())?;
    let client = crate::widget::run::http_client().map_err(|error| error.user_message())?;
    let tabs = tabs_matching(&tabs_with_desktop(&config), entry_id);
    if tabs.is_empty() {
        return Err(format!("no enabled provider matches {entry_id}"));
    }
    let entries = collect_entries_for(&client, &config, &tabs, false).await;
    Ok(render_json_entries(&entries))
}

/// The tabs whose report entry would carry `entry_id` — at most one, since
/// [`tab_id`] is unique across a tab list, but kept as a slice so the caller
/// runs the same loop as the full report.
fn tabs_matching(tabs: &[TabId], entry_id: &str) -> Vec<TabId> {
    tabs.iter()
        .filter(|tab| tab_id(tab) == entry_id)
        .cloned()
        .collect()
}

async fn collect_entries(
    account_override: Option<&str>,
    refresh: bool,
) -> std::result::Result<(Vec<Entry>, Option<&'static str>, Option<AccountHeaderInfo>), String> {
    let config = Config::load().map_err(|error| error.user_message())?;
    let client = crate::widget::run::http_client().map_err(|error| error.user_message())?;
    let all_tabs = tabs_with_desktop(&config);
    if all_tabs.is_empty() {
        return Err(format!(
            "no vendors enabled in {}",
            crate::config::config_path_hint()
        ));
    }

    // Check if the live session on disk has switched to a different account (e.g. user logged into a new account on Antigravity / OpenAI).
    let live_detected_email = all_tabs.iter().find_map(|tab| {
        let prov_id = tab_id(tab);
        crate::account_store::detect_provider_email(&prov_id, &config)
    });

    let effective_override = if refresh {
        if let Some(live) = &live_detected_email {
            if let Some(target) = account_override {
                if !crate::account_store::accounts_match_or_prefix(live, target) {
                    let _ = crate::active::write_account(live);
                    Some(live.as_str())
                } else {
                    account_override
                }
            } else {
                let _ = crate::active::write_account(live);
                Some(live.as_str())
            }
        } else {
            account_override
        }
    } else {
        if let Some(target) = account_override {
            let has_snap = crate::account_store::get_snapshot(target).is_some();
            if !has_snap && let Some(live) = &live_detected_email {
                let _ = crate::active::write_account(live);
                Some(live.as_str())
            } else {
                account_override
            }
        } else {
            account_override
        }
    };

    // If an explicit account override was requested, check if it's currently active.
    // If not active and not forced refresh, try loading its saved snapshot first.
    if let Some(target_label) = effective_override {
        let is_currently_active =
            config.resolve_active_account(None).as_deref() == Some(target_label);
        if !is_currently_active
            && !refresh
            && let Some(mut snap) = crate::account_store::get_snapshot(target_label)
        {
            crate::account_store::update_snapshot_entries(&mut snap.entries, Utc::now());
            for e in &mut snap.entries {
                crate::account_store::sanitize_snapshot_entry(e);
            }
            let info = AccountHeaderInfo {
                label: snap.account_label,
                user: snap.user,
                providers: snap.providers,
                snapshot_time: Some(snap.saved_at),
            };
            return Ok((
                snap.entries,
                config.ui.primary.map(|vendor| vendor.slug()),
                Some(info),
            ));
        }
    }

    let active_label = config.resolve_active_account(effective_override);
    let (tabs, account_info) = match active_label.as_deref() {
        Some(label) => {
            if let Some(acct) = config.find_account(label) {
                let user = crate::account::resolve_user_for_account(&config, acct);
                let snapshots = crate::account_store::all_snapshots();
                let filtered: Vec<TabId> = all_tabs
                    .into_iter()
                    .filter(|tab| {
                        let provider_matches = match &tab.source {
                            TabSource::Builtin(v) => {
                                if !acct.providers.is_empty() {
                                    acct.has_provider(v.slug())
                                } else if let Some(target_user) =
                                    user.as_deref().or(acct.user.as_deref())
                                {
                                    crate::account_store::provider_matches_user(
                                        v.slug(),
                                        target_user,
                                        &config,
                                        &snapshots,
                                    )
                                } else {
                                    true
                                }
                            }
                            TabSource::Custom { id, .. } => {
                                if !acct.providers.is_empty() {
                                    acct.has_provider(id)
                                } else if let Some(target_user) =
                                    user.as_deref().or(acct.user.as_deref())
                                {
                                    crate::account_store::provider_matches_user(
                                        id,
                                        target_user,
                                        &config,
                                        &snapshots,
                                    )
                                } else {
                                    true
                                }
                            }
                        };
                        let account_matches = tab
                            .account
                            .as_deref()
                            .map(|a| a.eq_ignore_ascii_case(&acct.label))
                            .unwrap_or(false);
                        provider_matches || account_matches
                    })
                    .collect();
                if filtered.is_empty() {
                    // Check if a saved snapshot exists for this account:
                    if let Some(mut snap) = crate::account_store::get_snapshot(label) {
                        crate::account_store::update_snapshot_entries(
                            &mut snap.entries,
                            Utc::now(),
                        );
                        let info = AccountHeaderInfo {
                            label: snap.account_label,
                            user: snap.user,
                            providers: snap.providers,
                            snapshot_time: Some(snap.saved_at),
                        };
                        return Ok((
                            snap.entries,
                            config.ui.primary.map(|vendor| vendor.slug()),
                            Some(info),
                        ));
                    }
                    return Err(format!(
                        "account '{label}' has no matching enabled providers or saved snapshots"
                    ));
                }
                let effective_providers = if !acct.providers.is_empty() {
                    acct.providers.clone()
                } else {
                    filtered
                        .iter()
                        .map(|t| match &t.source {
                            TabSource::Builtin(v) => v.slug().to_string(),
                            TabSource::Custom { id, .. } => id.clone(),
                        })
                        .collect()
                };
                let info = AccountHeaderInfo {
                    label: acct.label.clone(),
                    user,
                    providers: effective_providers,
                    snapshot_time: None,
                };
                (filtered, Some(info))
            } else if account_override.is_some() {
                // An explicit account was passed via CLI but is not a global account; check vendor-specific account tabs
                let filtered: Vec<TabId> = all_tabs
                    .into_iter()
                    .filter(|tab| {
                        tab.account
                            .as_deref()
                            .map(|a| a.eq_ignore_ascii_case(label))
                            .unwrap_or(false)
                    })
                    .collect();
                if filtered.is_empty() {
                    if let Some(mut snap) = crate::account_store::get_snapshot(label) {
                        crate::account_store::update_snapshot_entries(
                            &mut snap.entries,
                            Utc::now(),
                        );
                        let info = AccountHeaderInfo {
                            label: snap.account_label,
                            user: snap.user,
                            providers: snap.providers,
                            snapshot_time: Some(snap.saved_at),
                        };
                        return Ok((
                            snap.entries,
                            config.ui.primary.map(|vendor| vendor.slug()),
                            Some(info),
                        ));
                    }
                    return Err(format!("no account or enabled provider matches '{label}'"));
                }
                let info = AccountHeaderInfo {
                    label: label.to_string(),
                    user: None,
                    providers: filtered
                        .iter()
                        .filter_map(|t| t.vendor_id().map(|v| v.slug().to_string()))
                        .collect(),
                    snapshot_time: None,
                };
                (filtered, Some(info))
            } else {
                (all_tabs, None)
            }
        }
        None => (all_tabs, None),
    };

    let target_label_str = active_label.as_deref().unwrap_or("");
    let target_user_str = account_info
        .as_ref()
        .and_then(|info| info.user.as_deref())
        .unwrap_or(target_label_str);

    let mut entries = Vec::with_capacity(tabs.len());
    for tab in &tabs {
        let prov_id = tab_id(tab);
        let live_email = crate::account_store::detect_provider_email(&prov_id, &config);

        // Does this provider's live session on disk belong to the target account?
        let is_live_for_target = match &live_email {
            Some(live) => {
                if !target_user_str.is_empty() {
                    crate::account_store::accounts_match_or_prefix(live, target_user_str)
                } else {
                    true
                }
            }
            None => true, // Vendor doesn't have account-scoped logins (e.g. static api key)
        };

        if is_live_for_target {
            let mut entry = entry_for(&client, &config, tab, refresh).await;
            let has_auth_error = entry.sections.iter().any(|s| match s {
                ReportSection::Text { label, value } => {
                    let lbl = label.trim();
                    let val = value.trim();
                    lbl.starts_with("HTTP 5")
                        || lbl.starts_with("HTTP 401")
                        || val.starts_with('{')
                        || val.contains("not logged into")
                        || val.contains("error getting token")
                }
                _ => false,
            });

            if (entry.error.is_some() || has_auth_error) && !target_label_str.is_empty() {
                if let Some(snap) = crate::account_store::get_snapshot(target_label_str) {
                    if let Some(snap_e) = snap.entries.iter().find(|e| e.id == entry.id && e.error.is_none()) {
                        let mut se = snap_e.clone();
                        crate::account_store::update_snapshot_entries(std::slice::from_mut(&mut se), Utc::now());
                        crate::account_store::sanitize_snapshot_entry(&mut se);
                        entry = se;
                    }
                }
            } else if !target_label_str.is_empty() {
                let _ = crate::account_store::record_provider_entry(
                    target_label_str,
                    Some(target_user_str),
                    &entry,
                );
            }
            // Sanitize entry sections so no internal failure or raw JSON is exposed
            entry.sections.retain(|s| match s {
                ReportSection::Text { label, value } => {
                    let lbl = label.trim();
                    let val = value.trim();
                    !(lbl.starts_with("HTTP ")
                        || val.starts_with('{')
                        || val.contains("not logged into")
                        || val.contains("error getting token"))
                }
                _ => true,
            });
            while matches!(entry.sections.last(), Some(ReportSection::Spacer)) {
                entry.sections.pop();
            }
            if entry.sections.iter().any(|s| matches!(s, ReportSection::Metric { .. })) {
                entry.stale = false;
            }
            entries.push(entry);
        } else {
            // Live session belongs to ANOTHER account on disk!
            // Do NOT fetch live, do NOT overwrite target account's snapshot.
            // Load this provider from target account's saved snapshot:
            let snap = if !target_label_str.is_empty() {
                crate::account_store::get_snapshot(target_label_str)
                    .or_else(|| {
                        if !target_user_str.is_empty() {
                            crate::account_store::get_snapshot(target_user_str)
                        } else {
                            None
                        }
                    })
            } else {
                None
            };

            if let Some(snap) = snap
                && let Some(mut snap_e) = snap.entries.into_iter().find(|e| e.id == prov_id)
            {
                crate::account_store::update_snapshot_entries(
                    std::slice::from_mut(&mut snap_e),
                    Utc::now(),
                );
                crate::account_store::sanitize_snapshot_entry(&mut snap_e);
                entries.push(snap_e);
            } else {
                let zeroed = zeroed_entry_for_tab(&config, tab);
                entries.push(zeroed);
            }
        }
    }

    Ok((
        entries,
        config.ui.primary.map(|vendor| vendor.slug()),
        account_info,
    ))
}

async fn collect_entries_for(
    client: &reqwest::Client,
    config: &Config,
    tabs: &[TabId],
    refresh: bool,
) -> Vec<Entry> {
    // Sequential on purpose: several of these share a per-vendor cache lock,
    // and firing every account at Anthropic at once is a good way to get
    // rate-limited for no gain on a handful of entries.
    let mut entries = Vec::with_capacity(tabs.len());
    for tab in tabs {
        entries.push(entry_for(client, config, tab, refresh).await);
    }
    entries
}

pub async fn run(json: bool, account_override: Option<&str>, refresh: bool) -> i32 {
    let (entries, primary, account) = match collect_entries(account_override, refresh).await {
        Ok(res) => res,
        Err(message) => {
            eprintln!("ai-usagebar usage: {message}");
            return 1;
        }
    };

    // Automatically check quota resets and notify user if any quota window renewed
    let config = crate::config::Config::load().unwrap_or_default();
    if config.ui.notify_resets() {
        if let Ok(notif_path) = crate::monitor::notifications_path() {
            let _ = crate::monitor::check_and_notify_resets(&notif_path, Utc::now());
        }
    }

    if json {
        println!(
            "{}",
            render_json_with_account(&entries, primary, account.as_ref())
        );
    } else {
        print!("{}", render_text_with_account(&entries, account.as_ref()));
    }
    report_exit_code(&entries)
}

async fn entry_for(client: &reqwest::Client, config: &Config, tab: &TabId, refresh: bool) -> Entry {
    let ttl = if refresh {
        std::time::Duration::ZERO
    } else {
        crate::cache::DEFAULT_TTL
    };
    let state = crate::tui::app::refresh_one_with_ttl(client, config, tab, ttl).await;
    entry_from_state_with_config(config, tab, &state, Utc::now())
}

pub fn zeroed_entry_for_tab(config: &Config, tab: &TabId) -> Entry {
    let mut entry = Entry {
        id: tab_id(tab),
        name: tab_name(tab),
        display_name: tab_display_name(tab),
        short_name: tab_short_name(tab),
        icon: tab_icon(tab),
        brand: tab_brand(tab),
        plan: None,
        sections: Vec::new(),
        error: None,
        stale: false,
        fetched_at: Some(Utc::now()),
    };

    if let TabSource::Custom { id, .. } = &tab.source {
        entry.brand = config
            .custom_by_id(id)
            .and_then(|provider| provider.brand.clone());
    }

    match &tab.source {
        TabSource::Builtin(crate::vendor::VendorId::Antigravity) => {
            entry.plan = Some("Google AI Pro".into());
            entry.sections = vec![
                ReportSection::Spacer,
                ReportSection::Text {
                    label: "Session".into(),
                    value: "".into(),
                },
                ReportSection::Spacer,
                ReportSection::Metric {
                    label: "Gemini".into(),
                    percent: 0,
                    value: "0%".into(),
                    detail: "0% used".into(),
                    severity: "low".into(),
                    reset_at: None,
                    window_secs: Some(18000),
                },
                ReportSection::Spacer,
                ReportSection::Metric {
                    label: "Claude & GPT OSS".into(),
                    percent: 0,
                    value: "0%".into(),
                    detail: "0% used".into(),
                    severity: "low".into(),
                    reset_at: None,
                    window_secs: Some(18000),
                },
                ReportSection::Spacer,
                ReportSection::Text {
                    label: "Weekly".into(),
                    value: "".into(),
                },
                ReportSection::Spacer,
                ReportSection::Metric {
                    label: "Gemini".into(),
                    percent: 0,
                    value: "0%".into(),
                    detail: "0% used".into(),
                    severity: "low".into(),
                    reset_at: None,
                    window_secs: Some(604800),
                },
                ReportSection::Spacer,
                ReportSection::Metric {
                    label: "Claude & GPT OSS".into(),
                    percent: 0,
                    value: "0%".into(),
                    detail: "0% used".into(),
                    severity: "low".into(),
                    reset_at: None,
                    window_secs: Some(604800),
                },
            ];
        }
        TabSource::Builtin(crate::vendor::VendorId::Openai) => {
            entry.plan = Some("ChatGPT Free".into());
            entry.sections = vec![
                ReportSection::Spacer,
                ReportSection::Metric {
                    label: "Codex 5h".into(),
                    percent: 0,
                    value: "0%".into(),
                    detail: "0% used".into(),
                    severity: "low".into(),
                    reset_at: None,
                    window_secs: Some(18000),
                },
                ReportSection::Spacer,
            ];
        }
        TabSource::Builtin(crate::vendor::VendorId::Anthropic) => {
            entry.plan = Some("Claude Free".into());
            entry.sections = vec![
                ReportSection::Spacer,
                ReportSection::Metric {
                    label: "Session (5h)".into(),
                    percent: 0,
                    value: "0%".into(),
                    detail: "0% used".into(),
                    severity: "low".into(),
                    reset_at: None,
                    window_secs: Some(18000),
                },
                ReportSection::Spacer,
                ReportSection::Metric {
                    label: "Weekly".into(),
                    percent: 0,
                    value: "0%".into(),
                    detail: "0% used".into(),
                    severity: "low".into(),
                    reset_at: None,
                    window_secs: Some(604800),
                },
            ];
        }
        TabSource::Builtin(crate::vendor::VendorId::Cursor) => {
            entry.plan = Some("Hobby".into());
            entry.sections = vec![
                ReportSection::Spacer,
                ReportSection::Metric {
                    label: "Cursor Models".into(),
                    percent: 0,
                    value: "0%".into(),
                    detail: "0% used".into(),
                    severity: "low".into(),
                    reset_at: None,
                    window_secs: None,
                },
                ReportSection::Spacer,
                ReportSection::Metric {
                    label: "Other Models".into(),
                    percent: 0,
                    value: "0%".into(),
                    detail: "0% used".into(),
                    severity: "low".into(),
                    reset_at: None,
                    window_secs: None,
                },
            ];
        }
        TabSource::Builtin(v) => {
            entry.plan = Some(v.display_name().to_string());
            entry.sections = vec![
                ReportSection::Spacer,
                ReportSection::Metric {
                    label: "Usage".into(),
                    percent: 0,
                    value: "0%".into(),
                    detail: "0% used".into(),
                    severity: "low".into(),
                    reset_at: None,
                    window_secs: None,
                },
            ];
        }
        TabSource::Custom { name, .. } => {
            entry.plan = Some(name.clone());
            entry.sections = vec![
                ReportSection::Spacer,
                ReportSection::Metric {
                    label: "Usage".into(),
                    percent: 0,
                    value: "0%".into(),
                    detail: "0% used".into(),
                    severity: "low".into(),
                    reset_at: None,
                    window_secs: None,
                },
            ];
        }
    }

    entry
}

pub(crate) fn entry_from_state_with_config(
    config: &Config,
    tab: &TabId,
    state: &TabState,
    now: chrono::DateTime<Utc>,
) -> Entry {
    let mut entry = entry_from_state(tab, state, now);
    if let TabSource::Custom { id, .. } = &tab.source {

        entry.brand = config
            .custom_by_id(id)
            .and_then(|provider| provider.brand.clone());
    }
    entry
}

pub(crate) fn entry_from_state(tab: &TabId, state: &TabState, now: chrono::DateTime<Utc>) -> Entry {
    if let TabState::Snapshot(snap) = state {
        return snap.entry.clone();
    }
    let mut entry = Entry {
        id: tab_id(tab),
        name: tab_name(tab),
        display_name: tab_display_name(tab),
        // #164's naming (a custom tab has no VendorId) with #162's plan
        // (an error card still names the plan from the OAuth blob).
        short_name: tab_short_name(tab),
        icon: tab_icon(tab),
        brand: tab_brand(tab),
        plan: match state {
            TabState::Error { plan, .. } => plan
                .as_deref()
                .map(crate::display::sanitize_untrusted_field)
                .filter(|plan| !plan.is_empty()),
            _ => None,
        },
        sections: Vec::new(),
        error: match state {
            TabState::Error { message, .. } => {
                Some(crate::display::sanitize_untrusted_field(message))
            }
            _ => None,
        },
        stale: matches!(state, TabState::Ready(ready) if ready.stale),
        fetched_at: match state {
            TabState::Ready(ready) => ready.fetched_at,
            _ => None,
        },
    };
    // The error is already a first-class entry field. Do not duplicate the
    // TUI's interactive retry instructions as report data.
    if entry.error.is_some() {
        return entry;
    }
    for projected in sections_with_metadata_for(state, now, PACE_TOLERANCE) {
        match projected.section {
            Section::Title { left, .. } => entry.plan = Some(left),
            Section::Metric {
                label,
                pct,
                value_label,
                footnote,
                severity,
                ..
            } => {
                entry.sections.push(ReportSection::Metric {
                    label,
                    percent: pct,
                    value: value_label,
                    detail: footnote,
                    severity: severity.as_str().into(),
                    reset_at: projected.reset_at,
                    window_secs: projected
                        .window
                        .map(|window| window.num_seconds().max(0) as u64),
                });
            }
            Section::Text { label, value } => {
                entry.sections.push(ReportSection::Text { label, value });
            }
            Section::Block { label, body } => {
                entry.sections.push(ReportSection::Block { label, body });
            }
            Section::Spacer => entry.sections.push(ReportSection::Spacer),
        }
    }
    entry
}

fn report_exit_code(entries: &[Entry]) -> i32 {
    i32::from(entries.iter().all(|entry| entry.error.is_some()))
}

/// Stable machine id shared by aggregate views and the macOS menu bar:
/// `<vendor>@<label>` for named accounts, `custom:<id>` for a `[[custom]]`
/// provider (which never has accounts).
fn tab_id(tab: &TabId) -> String {
    match &tab.source {
        TabSource::Custom { id, .. } => format!("custom:{id}"),
        TabSource::Builtin(vendor) => match &tab.account {
            Some(account) => format!("{}@{account}", vendor.slug()),
            None => vendor.slug().to_string(),
        },
    }
}

fn tab_name(tab: &TabId) -> String {
    match &tab.source {
        TabSource::Builtin(vendor) => format_tab_name(tab, vendor.slug()),
        TabSource::Custom { name, .. } => format_tab_name(tab, name),
    }
}

fn tab_display_name(tab: &TabId) -> String {
    match &tab.source {
        TabSource::Builtin(vendor) => format_tab_name(tab, vendor.display_name()),
        TabSource::Custom { name, .. } => format_tab_name(tab, name),
    }
}

/// The `{vendor_short}` code: the vendor's own for a built-in, the configured
/// `short_name` for a custom provider.
fn tab_short_name(tab: &TabId) -> String {
    match &tab.source {
        TabSource::Builtin(vendor) => vendor.short_name().to_string(),
        TabSource::Custom { short_name, .. } => {
            crate::display::sanitize_untrusted_field(short_name)
        }
    }
}

/// The bar glyph: the vendor's own for a built-in. A custom provider has no
/// glyph of its own, so its `short_name` stands in, as Zai and Kimi's do.
fn tab_icon(tab: &TabId) -> String {
    match &tab.source {
        TabSource::Builtin(vendor) => vendor.bar_icon().to_string(),
        TabSource::Custom { short_name, .. } => {
            crate::display::sanitize_untrusted_field(short_name)
        }
    }
}

/// The provider whose mark draws this entry. Built-in tabs carry everything
/// needed to derive it; a custom tab's optional brand stays in `Config` rather
/// than widening the public `TabSource` enum and is attached by
/// `entry_from_state_with_config`.
fn tab_brand(tab: &TabId) -> Option<String> {
    match &tab.source {
        TabSource::Builtin(vendor) => Some(vendor.slug().to_string()),
        TabSource::Custom { .. } => None,
    }
}

fn format_tab_name(tab: &TabId, vendor_name: &str) -> String {
    let name = match &tab.account {
        // Mark a Desktop-sourced account so a mixed CLI+Desktop setup is legible;
        // for a Desktop-only user every Claude row simply reads "· <label> (desktop)".
        Some(account) if tab.desktop => format!("{vendor_name} · {account} (desktop)"),
        Some(account) => format!("{vendor_name} · {account}"),
        None => vendor_name.to_string(),
    };
    crate::display::sanitize_untrusted_field(&name)
}

#[allow(dead_code)]
fn render_json_for_primary(entries: &[Entry], primary: Option<&str>) -> String {
    render_json_with_account(entries, primary, None)
}

fn render_json_with_account(
    entries: &[Entry],
    primary: Option<&str>,
    account: Option<&AccountHeaderInfo>,
) -> String {
    let mut val = json!({
        "schema_version": USAGE_SCHEMA_VERSION,
        "primary": primary,
        "entries": json_rows(entries),
    });
    if let Some(acct) = account
        && let Some(obj) = val.as_object_mut()
    {
        obj.insert("account".to_string(), json!(acct));
    }
    if let Ok(cfg) = Config::load() {
        let active_lbl = cfg.resolve_active_account(account.map(|a| a.label.as_str()));
        let mut acct_list = Vec::new();
        for a in &cfg.accounts {
            let is_active = active_lbl
                .as_deref()
                .map(|al| al.eq_ignore_ascii_case(&a.label))
                .unwrap_or(false);
            acct_list.push(json!({
                "label": a.label,
                "user": a.user,
                "active": is_active,
            }));
        }
        for snap in crate::account_store::all_snapshots() {
            if !acct_list.iter().any(|item| item["label"].as_str() == Some(&snap.account_label)) {
                let is_active = active_lbl
                    .as_deref()
                    .map(|al| al.eq_ignore_ascii_case(&snap.account_label))
                    .unwrap_or(false);
                acct_list.push(json!({
                    "label": snap.account_label,
                    "user": snap.user,
                    "active": is_active,
                }));
            }
        }
        if let Some(obj) = val.as_object_mut() {
            obj.insert("accounts".to_string(), json!(acct_list));
        }
    }
    let renewals = crate::monitor::detect_all_renewals(Utc::now());
    if let Some(obj) = val.as_object_mut() {
        obj.insert("renewals".to_string(), json!(renewals));
    }
    val.to_string()
}

/// The single-entry shape: the same rows, without a `primary` the caller
/// did not ask about.
fn render_json_entries(entries: &[Entry]) -> String {
    json!({
        "schema_version": USAGE_SCHEMA_VERSION,
        "entries": json_rows(entries),
    })
    .to_string()
}

fn json_rows(entries: &[Entry]) -> Vec<serde_json::Value> {
    entries
        .iter()
        .map(|entry| {
            let metrics = entry
                .sections
                .iter()
                .filter_map(|section| match section {
                    ReportSection::Metric {
                        label,
                        percent,
                        value,
                        detail,
                        severity,
                        reset_at,
                        window_secs,
                    } => {
                        let mut metric = json!({
                            "label": label,
                            "percent": percent,
                            "value": value,
                            "detail": detail,
                            "severity": severity,
                            "reset_at": reset_at,
                        });
                        // Same rule as the `sections` serializer: absent, not null.
                        if let Some(secs) = window_secs {
                            metric["window_secs"] = json!(secs);
                        }
                        Some(metric)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let mut row = json!({
                "id": entry.id,
                "name": entry.name,
                "display_name": entry.display_name,
                "short_name": entry.short_name,
                "icon": entry.icon,
                "plan": entry.plan,
                "status": if entry.error.is_some() { "error" } else { "ready" },
                "error": entry.error,
                "stale": entry.stale,
                "fetched_at": entry.fetched_at,
                "metrics": metrics,
                "sections": entry.sections,
            });
            // Optional additive fields are absent, not null, so older and
            // newer producers keep the documented tolerant JSON contract.
            if let Some(brand) = &entry.brand {
                row["brand"] = json!(brand);
            }
            if entry.id == "antigravity" || entry.name == "antigravity" {
                row["extra_models"] = json!(crate::antigravity::vendor::ANTIGRAVITY_THIRD_PARTY_MODELS);
            }
            row
        })
        .collect()
}

#[allow(dead_code)]
fn render_text(entries: &[Entry]) -> String {
    render_text_with_account(entries, None)
}

fn render_text_with_account(entries: &[Entry], account: Option<&AccountHeaderInfo>) -> String {
    // Widest label across every entry, so the value column lines up down the
    // whole report rather than per-section.
    let width = entries
        .iter()
        .flat_map(|entry| entry.sections.iter())
        .filter_map(ReportSection::label)
        .map(|label| label.chars().count())
        .max()
        .unwrap_or(0);

    let mut out = String::new();
    if let Some(acct) = account {
        let user_str = acct
            .user
            .as_deref()
            .map(|u| format!(" ({u})"))
            .unwrap_or_default();
        let snap_str = acct
            .snapshot_time
            .map(|t| format!(" [Snapshot from {}]", t.format("%Y-%m-%d %H:%M UTC")))
            .unwrap_or_default();
        out.push_str(&format!(
            "Account: {}{}{}\n",
            acct.label, user_str, snap_str
        ));
        if !acct.providers.is_empty() {
            out.push_str(&format!("Providers: {}\n", acct.providers.join(", ")));
        }
        out.push_str("──────────────────────────────────────────────────\n\n");
    }
    for entry in entries {
        out.push_str(&entry.name);
        if let Some(plan) = &entry.plan {
            out.push_str(&format!("   {plan}"));
        }
        out.push('\n');
        if let Some(error) = &entry.error {
            out.push_str(&format!("  ! {error}\n\n"));
            continue;
        }
        if !entry
            .sections
            .iter()
            .any(|section| !matches!(section, ReportSection::Spacer))
        {
            out.push_str("  (nothing reported)\n\n");
            continue;
        }
        let mut body = String::new();
        let mut pending_spacer = false;
        for section in &entry.sections {
            if matches!(section, ReportSection::Spacer) {
                pending_spacer |= !body.is_empty();
                continue;
            }
            if pending_spacer {
                body.push('\n');
                pending_spacer = false;
            }
            match section {
                ReportSection::Metric {
                    label,
                    value,
                    detail,
                    ..
                } => {
                    let label = format!("{label:width$}");
                    let value = format!("{value:>9}");
                    if detail.is_empty() {
                        body.push_str(&format!("  {label}  {value}\n"));
                    } else {
                        body.push_str(&format!("  {label}  {value}   {detail}\n"));
                    }
                }
                ReportSection::Text { label, value } => {
                    if label.is_empty() {
                        body.push_str(&format!("  {}\n", value.trim_start()));
                    } else if value.is_empty() {
                        body.push_str(&format!("  {label}\n"));
                    } else {
                        body.push_str(&format!("  {label:width$}  {value}\n"));
                    }
                }
                ReportSection::Block { label, body: lines } => {
                    body.push_str(&format!("  {label}\n"));
                    for line in lines {
                        body.push_str(&format!("    {line}\n"));
                    }
                }
                ReportSection::Spacer => unreachable!(),
            }
        }
        out.push_str(&body);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::ReadyTab;
    use crate::usage::{
        DeepseekSnapshot, KimiSnapshot, KiroSnapshot, OpenRouterSnapshot, VendorSnapshot,
    };
    use crate::vendor::VendorId;

    fn entry(name: &str, sections: Vec<ReportSection>) -> Entry {
        Entry {
            id: name.into(),
            name: name.into(),
            display_name: name.into(),
            short_name: VendorId::Anthropic.short_name().into(),
            icon: VendorId::Anthropic.bar_icon().into(),
            brand: Some(VendorId::Anthropic.slug().into()),
            plan: Some("Claude Max 20x".into()),
            sections,
            error: None,
            stale: false,
            fetched_at: None,
        }
    }

    fn metric(label: &str, percent: u16, value: &str, detail: &str) -> ReportSection {
        ReportSection::Metric {
            label: label.into(),
            percent,
            value: value.into(),
            detail: detail.into(),
            severity: "mid".into(),
            reset_at: None,
            window_secs: None,
        }
    }

    #[test]
    fn accounts_get_a_stable_id_and_a_readable_name() {
        let account = TabId::account("gmail");
        assert_eq!(tab_id(&account), "anthropic@gmail");
        assert_eq!(tab_name(&account), "anthropic · gmail");
        assert_eq!(tab_display_name(&account), "Claude · gmail");

        let plain = TabId::vendor(VendorId::Cursor);
        assert_eq!(tab_id(&plain), "cursor");
        assert_eq!(tab_name(&plain), "cursor");
        assert_eq!(tab_display_name(&plain), "Cursor");

        let openrouter = TabId::account_for(VendorId::Openrouter, "work");
        assert_eq!(tab_id(&openrouter), "openrouter@work");
        assert_eq!(tab_name(&openrouter), "openrouter · work");
        assert_eq!(tab_display_name(&openrouter), "OpenRouter · work");
    }

    /// The Omarchy bar can be set to draw the provider tag and nothing else,
    /// so every entry — a failed one included — has to carry a code, and every
    /// account of one vendor has to carry that vendor's code rather than a
    /// per-account one.
    #[test]
    fn every_entry_carries_its_vendor_short_code() {
        let now = Utc::now();
        let failed = TabState::error("not signed in");

        let account = entry_from_state(&TabId::account("gmail"), &failed, now);
        assert_eq!(account.short_name, "cld");
        let other_account = entry_from_state(&TabId::account("work"), &failed, now);
        assert_eq!(other_account.short_name, account.short_name);

        let cursor = entry_from_state(&TabId::vendor(VendorId::Cursor), &failed, now);
        assert_eq!(cursor.short_name, "cur");
        assert_eq!(cursor.icon, VendorId::Cursor.bar_icon());
        // A built-in vendor is its own brand, so a frontend never has to map
        // the entry id back to a provider to pick the mark.
        assert_eq!(cursor.brand.as_deref(), Some(VendorId::Cursor.slug()));

        let rendered = render_json_for_primary(&[cursor], None);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["entries"][0]["short_name"], "cur");
        assert_eq!(value["entries"][0]["icon"], VendorId::Cursor.bar_icon());
        assert_eq!(value["entries"][0]["brand"], VendorId::Cursor.slug());
    }

    #[test]
    fn every_metric_reports_its_quota_and_its_reset() {
        let text = render_text(&[entry(
            "anthropic · gmail",
            vec![
                metric("Session (5h)", 29, "29%", "Resets in 0h 50m"),
                metric("Weekly (7d)", 32, "32%", "Resets in 4d 2h"),
            ],
        )]);

        assert!(
            text.contains("anthropic · gmail   Claude Max 20x"),
            "{text}"
        );
        assert!(text.contains("29%   Resets in 0h 50m"), "{text}");
        assert!(text.contains("32%   Resets in 4d 2h"), "{text}");
    }

    /// Labels are padded to one width across the whole report, so the columns
    /// still line up when a later entry has a longer label than the first.
    #[test]
    fn value_columns_align_across_entries() {
        let text = render_text(&[
            entry("a", vec![metric("S", 1, "1%", "")]),
            entry("b", vec![metric("A very long label", 2, "2%", "")]),
        ]);
        let columns: Vec<usize> = text
            .lines()
            .filter(|line| line.starts_with("  ") && line.contains('%'))
            .map(|line| line.find('%').unwrap())
            .collect();
        assert_eq!(columns.len(), 2);
        assert_eq!(columns[0], columns[1], "{text}");
    }

    /// One dead vendor must not hide the others — it reports inline and the
    /// rest still print.
    #[test]
    fn a_failing_entry_is_reported_without_dropping_the_rest() {
        let mut broken = entry("openai", Vec::new());
        broken.error = Some("credentials error: not signed in".into());
        let text = render_text(&[broken, entry("cursor", vec![metric("Auto", 5, "5%", "")])]);

        assert!(
            text.contains("! credentials error: not signed in"),
            "{text}"
        );
        assert!(text.contains("cursor"), "{text}");
        assert!(text.contains("5%"), "{text}");
    }

    #[test]
    fn json_carries_the_percentage_as_a_number() {
        let rendered = render_json_for_primary(
            &[entry(
                "anthropic · gmail",
                vec![metric("Session (5h)", 29, "29%", "Resets in 0h 50m")],
            )],
            None,
        );
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let first = &value["entries"][0];
        assert_eq!(first["plan"], "Claude Max 20x");
        assert_eq!(first["display_name"], "anthropic · gmail");
        assert_eq!(first["short_name"], "cld");
        assert_eq!(first["metrics"][0]["percent"], 29);
        assert_eq!(first["metrics"][0]["detail"], "Resets in 0h 50m");
        assert!(first["error"].is_null());
        assert_eq!(first["status"], "ready");
        assert_eq!(first["stale"], false);
        assert!(first["fetched_at"].is_null());
        assert_eq!(first["metrics"][0]["severity"], "mid");
        assert!(first["metrics"][0]["reset_at"].is_null());
        assert!(value["primary"].is_null());
    }

    #[test]
    fn json_carries_the_configured_primary_without_reordering_entries() {
        let rendered = render_json_for_primary(
            &[entry("anthropic", Vec::new()), entry("openai", Vec::new())],
            Some("openai"),
        );
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["primary"], "openai");
        assert_eq!(value["entries"][0]["id"], "anthropic");
        assert_eq!(value["entries"][1]["id"], "openai");
    }

    #[test]
    fn every_json_report_declares_its_schema_version() {
        let aggregate: serde_json::Value = serde_json::from_str(&render_json_for_primary(
            &[entry("anthropic", Vec::new())],
            Some("anthropic"),
        ))
        .unwrap();
        assert_eq!(aggregate["schema_version"], 1);

        let single: serde_json::Value =
            serde_json::from_str(&render_json_entries(&[entry("anthropic", Vec::new())])).unwrap();
        assert_eq!(single["schema_version"], 1);
    }

    #[test]
    fn json_exposes_absolute_resets_and_cache_freshness_additively() {
        let fetched_at = Utc::now() - chrono::Duration::minutes(3);
        let reset_at = Utc::now() + chrono::Duration::days(1);
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Kiro(KiroSnapshot {
                plan: "KIRO POWER".into(),
                used: 4_000.0,
                limit: 10_000.0,
                reset_at: Some(reset_at),
            }),
            stale: true,
            last_error: None,
            fetched_at: Some(fetched_at),
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Kiro), &state, Utc::now());
        let rendered = render_json_for_primary(&[projected], None);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let first = &value["entries"][0];

        assert_eq!(first["stale"], true);
        let fetched_rfc3339 = fetched_at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true);
        let reset_rfc3339 = reset_at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true);
        assert_eq!(first["fetched_at"], fetched_rfc3339);
        assert_eq!(first["metrics"][0]["reset_at"], reset_rfc3339);
        assert_eq!(first["sections"][1]["reset_at"], reset_rfc3339);
        assert_eq!(first["metrics"][0]["severity"], "low");
        // Kiro states a reset but not how long its window is; a frontend
        // must not be handed a length to pace against.
        assert!(first["metrics"][0]["window_secs"].is_null());
        assert!(first["sections"][1].get("window_secs").is_none());
    }

    /// A rolling window's exact length rides along with its row, in both the
    /// ordered `sections` and the `metrics` convenience view, and is omitted
    /// (not `null`) for a metric that has none.
    #[test]
    fn json_carries_the_window_length_only_for_exact_windows() {
        use crate::usage::{AnthropicSnapshot, UsageWindow};

        let now = Utc::now();
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Anthropic(AnthropicSnapshot {
                plan: "Claude Max 20x".into(),
                session: UsageWindow {
                    utilization_pct: 29,
                    resets_at: Some(now + chrono::Duration::minutes(50)),
                    window_duration: chrono::Duration::hours(5),
                },
                weekly: UsageWindow {
                    utilization_pct: 32,
                    resets_at: Some(now + chrono::Duration::days(4)),
                    window_duration: chrono::Duration::days(7),
                },
                sonnet: None,
                scoped: vec![],
                extra: None,
            }),
            stale: false,
            last_error: None,
            fetched_at: None,
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Anthropic), &state, now);
        let rendered = render_json_for_primary(&[projected], None);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let first = &value["entries"][0];

        let window_of = |label: &str| {
            first["sections"]
                .as_array()
                .unwrap()
                .iter()
                .find(|section| section["label"] == label)
                .map(|section| section["window_secs"].clone())
                .unwrap_or_else(|| panic!("no section labelled {label}"))
        };
        assert_eq!(window_of("Session (5h)"), 18_000);
        assert_eq!(window_of("Weekly (7d)"), 604_800);
        assert_eq!(first["metrics"][0]["label"], "Session (5h)");
        assert_eq!(first["metrics"][0]["window_secs"], 18_000);
        assert_eq!(first["metrics"][1]["window_secs"], 604_800);

        // Same shape from the single-entry collector a frontend refreshes one provider with.
        let single: serde_json::Value =
            serde_json::from_str(&render_json_entries(&[entry_from_state(
                &TabId::vendor(VendorId::Anthropic),
                &state,
                now,
            )]))
            .unwrap();
        assert!(single.get("primary").is_none());
        assert_eq!(single["entries"][0]["metrics"][0]["window_secs"], 18_000);

        // A hand-built metric with no window serializes without the key at all.
        let bare =
            render_json_for_primary(&[entry("cursor", vec![metric("Auto", 5, "5%", "")])], None);
        let bare: serde_json::Value = serde_json::from_str(&bare).unwrap();
        assert!(
            bare["entries"][0]["metrics"][0]
                .get("window_secs")
                .is_none()
        );
        assert!(
            bare["entries"][0]["sections"][0]
                .get("window_secs")
                .is_none()
        );
    }

    /// A per-provider refresh narrows the configured tab list to the
    /// one whose report id it was shown; anything else yields nothing rather
    /// than a fallback to the whole report.
    #[test]
    fn tabs_matching_selects_exactly_the_entry_with_that_id() {
        use crate::tui::app::tabs_from_config;

        // Flip the flags both ways rather than trusting any vendor's default,
        // so the match below is this test's doing and the miss is a real one.
        let mut config = Config::default();
        config.zai.enabled = false;
        config.deepseek.enabled = false;
        assert!(tabs_matching(&tabs_from_config(&config), "zai").is_empty());
        assert!(tabs_matching(&tabs_from_config(&config), "deepseek").is_empty());
        config.zai.enabled = true;
        config.deepseek.enabled = true;
        let tabs = tabs_from_config(&config);

        assert_eq!(
            tabs_matching(&tabs, "zai"),
            vec![TabId::vendor(VendorId::Zai)]
        );
        assert_eq!(
            tabs_matching(&tabs, "deepseek"),
            vec![TabId::vendor(VendorId::Deepseek)]
        );
        assert!(tabs_matching(&tabs, "not-a-vendor").is_empty());
        assert!(tabs_matching(&tabs, "zai@work").is_empty());
        assert!(tabs_matching(&tabs, "").is_empty());
    }

    /// Named accounts are addressed by the same `<vendor>@<label>` id the
    /// report prints, so a frontend can refresh one Claude account by itself.
    #[test]
    fn tabs_matching_addresses_named_accounts_by_report_id() {
        let tabs = vec![
            TabId::vendor(VendorId::Anthropic),
            TabId::account("gmail"),
            TabId::account("work"),
        ];
        assert_eq!(
            tabs_matching(&tabs, "anthropic@work"),
            vec![TabId::account("work")]
        );
        assert_eq!(
            tabs_matching(&tabs, "anthropic"),
            vec![TabId::vendor(VendorId::Anthropic)]
        );
        assert!(tabs_matching(&tabs, "anthropic@nobody").is_empty());
    }

    #[test]
    fn report_reset_metadata_follows_multi_metric_order() {
        let weekly_reset = Utc::now() + chrono::Duration::days(3);
        let window_reset = Utc::now() + chrono::Duration::hours(2);
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Kimi(KimiSnapshot {
                plan: Some("Kimi Code".into()),
                weekly_limit: 1_000,
                weekly_used: 200,
                weekly_remaining: 800,
                weekly_reset_at: Some(weekly_reset),
                window_limit: 100,
                window_used: 40,
                window_remaining: 60,
                window_reset_at: Some(window_reset),
            }),
            stale: false,
            last_error: None,
            fetched_at: None,
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Kimi), &state, Utc::now());
        // Pair each reset with its own row rather than pinning the row order —
        // that order is `panels::kimi_sections`' to choose, and pinning it here
        // would only duplicate the test that owns it.
        let resets: Vec<_> = projected
            .sections
            .iter()
            .filter_map(|section| match section {
                ReportSection::Metric {
                    label, reset_at, ..
                } => Some((label.as_str(), *reset_at)),
                _ => None,
            })
            .collect();

        assert_eq!(
            resets
                .iter()
                .copied()
                .collect::<std::collections::HashMap<_, _>>(),
            std::collections::HashMap::from([
                ("Weekly quota", Some(weekly_reset)),
                ("Rolling window (5h)", Some(window_reset)),
            ])
        );
    }

    #[test]
    fn json_preserves_non_metric_sections_without_fabricating_percentages() {
        let rendered = render_json_for_primary(
            &[entry(
                "openrouter",
                vec![
                    metric("Credit balance", 25, "$75.00", "$25.00 used"),
                    ReportSection::Spacer,
                    ReportSection::Text {
                        label: "Resets".into(),
                        value: "in 9d".into(),
                    },
                    ReportSection::Block {
                        label: "Usage by period".into(),
                        body: vec!["today $1.00 · week $5.00".into()],
                    },
                ],
            )],
            None,
        );
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let first = &value["entries"][0];
        assert_eq!(first["metrics"].as_array().unwrap().len(), 1);
        assert_eq!(first["sections"][1]["type"], "spacer");
        assert_eq!(first["sections"][2]["type"], "text");
        assert!(first["sections"][2].get("percent").is_none());
        assert_eq!(first["sections"][3]["type"], "block");
        assert_eq!(first["sections"][3]["body"][0], "today $1.00 · week $5.00");
    }

    #[test]
    fn real_panel_projection_keeps_openrouter_blocks() {
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Openrouter(OpenRouterSnapshot {
                label: "OR".into(),
                total_credits: 100.0,
                total_usage: 25.0,
                usage_daily: 1.0,
                usage_weekly: 5.0,
                usage_monthly: 25.0,
                is_free_tier: false,
                limit: None,
                limit_remaining: None,
            }),
            stale: false,
            last_error: None,
            fetched_at: None,
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Openrouter), &state, Utc::now());
        assert!(projected.sections.iter().any(|section| matches!(
            section,
            ReportSection::Block { label, .. } if label == "Usage by period"
        )));
        assert!(projected.sections.iter().any(|section| matches!(
            section,
            ReportSection::Block { label, .. } if label == "Tier"
        )));
        let text = render_text(&[projected]);
        assert!(text.contains("Usage by period"), "{text}");
        assert!(
            text.contains("today $1.00 · week $5.00 · month $25.00"),
            "{text}"
        );
    }

    #[test]
    fn real_balance_text_is_not_exposed_as_a_percentage_metric() {
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Deepseek(DeepseekSnapshot {
                is_available: true,
                balance: 12.5,
                granted: 2.5,
                topped_up: 10.0,
                currency: "USD".into(),
            }),
            stale: false,
            last_error: None,
            fetched_at: None,
        }));
        let projected = entry_from_state(&TabId::vendor(VendorId::Deepseek), &state, Utc::now());
        assert!(projected.sections.iter().any(|section| matches!(
            section,
            ReportSection::Text { label, value } if label == "Balance" && value == "$12.50"
        )));
        assert!(
            !projected
                .sections
                .iter()
                .any(|section| matches!(section, ReportSection::Metric { .. }))
        );
    }

    #[test]
    fn failed_entries_do_not_duplicate_tui_retry_rows() {
        let failed = entry_from_state(
            &TabId::vendor(VendorId::Openai),
            &TabState::error("not \x1b[31msigned in\u{202e}"),
            Utc::now(),
        );
        assert_eq!(failed.error.as_deref(), Some("not [31msigned in"));
        assert!(failed.sections.is_empty());
    }

    #[test]
    fn anthropic_error_keeps_oauth_plan_without_inventing_gauges() {
        let failed = TabState::error_with_plan(
            "HTTP 401: authentication rejected — credentials may be missing, expired, or invalid",
            Some("Claude Max 5x".into()),
        );
        let entry = entry_from_state(&TabId::vendor(VendorId::Anthropic), &failed, Utc::now());
        assert_eq!(entry.plan.as_deref(), Some("Claude Max 5x"));
        assert!(entry.sections.is_empty());
        assert!(entry.error.as_deref().unwrap().contains("401"));
        let rendered = render_json_for_primary(&[entry], None);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["entries"][0]["plan"], "Claude Max 5x");
        assert_eq!(value["entries"][0]["status"], "error");
        assert_eq!(value["entries"][0]["sections"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn exit_is_nonzero_only_when_every_entry_failed() {
        let mut failed = entry("openai", Vec::new());
        failed.error = Some("not signed in".into());
        assert_eq!(report_exit_code(&[failed]), 1);

        let mut failed = entry("openai", Vec::new());
        failed.error = Some("not signed in".into());
        assert_eq!(report_exit_code(&[failed, entry("cursor", Vec::new())]), 0);
    }

    fn custom_spec(id: &str, enabled: bool) -> crate::config::CustomProviderConfig {
        crate::config::CustomProviderConfig {
            id: id.into(),
            name: "My Tool".into(),
            short_name: "myt".into(),
            enabled,
            ..Default::default()
        }
    }

    /// A custom provider can choose a built-in vendor's mark with `brand`; the
    /// report relays that slug without copying config-only data into `TabId`.
    #[test]
    fn a_custom_entry_relays_the_brand_it_borrowed() {
        let spec = crate::config::CustomProviderConfig {
            brand: Some("opencode-go".into()),
            ..custom_spec("oc-second", true)
        };
        let config = Config {
            custom: vec![spec],
            ..Default::default()
        };
        let tab = TabId::custom(&config.custom[0]);
        let entry =
            entry_from_state_with_config(&config, &tab, &TabState::error("HTTP 500"), Utc::now());
        assert_eq!(entry.brand.as_deref(), Some("opencode-go"));
        // The tag is untouched: the mark is artwork, not the bar label.
        assert_eq!(entry.short_name, "myt");

        let value: serde_json::Value =
            serde_json::from_str(&render_json_for_primary(&[entry], None)).unwrap();
        assert_eq!(value["entries"][0]["brand"], "opencode-go");
    }

    /// A `[[custom]]` provider is one more report entry after the built-ins,
    /// addressed as `custom:<id>`; a disabled one is absent.
    #[test]
    fn enabled_custom_providers_are_listed_after_builtins_by_custom_id() {
        use crate::tui::app::tabs_from_config;

        let mut config = Config {
            custom: vec![custom_spec("mytool", true)],
            ..Default::default()
        };
        let tabs = tabs_from_config(&config);
        let ids: Vec<String> = tabs.iter().map(tab_id).collect();
        assert_eq!(ids.last().map(String::as_str), Some("custom:mytool"));
        assert!(
            ids[..ids.len() - 1]
                .iter()
                .all(|id| !id.starts_with("custom:"))
        );
        assert_eq!(tabs.last(), Some(&TabId::custom(&config.custom[0])));

        config.custom[0].enabled = false;
        assert!(
            tabs_from_config(&config)
                .iter()
                .all(|tab| tab_id(tab) != "custom:mytool")
        );
    }

    #[test]
    fn custom_entries_carry_their_configured_names_and_projected_windows() {
        use crate::custom::types::{CustomMetric, CustomSnapshot, CustomText};

        let now = Utc::now();
        let spec = custom_spec("mytool", true);
        let tab = TabId::custom(&spec);
        assert_eq!(tab_id(&tab), "custom:mytool");
        assert_eq!(tab_name(&tab), "My Tool");
        assert_eq!(tab_display_name(&tab), "My Tool");

        let session_reset = now + chrono::Duration::hours(2);
        let state = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Custom(CustomSnapshot {
                plan: Some("Team".into()),
                metrics: vec![
                    CustomMetric {
                        label: "Session".into(),
                        pct: 40,
                        footnote: "40 of 100".into(),
                        resets_at: Some(session_reset),
                        window_secs: Some(18_000),
                    },
                    CustomMetric {
                        label: "Monthly".into(),
                        pct: 10,
                        footnote: String::new(),
                        resets_at: None,
                        window_secs: None,
                    },
                ],
                texts: vec![CustomText {
                    label: "Region".into(),
                    value: "eu".into(),
                }],
            }),
            stale: false,
            last_error: None,
            fetched_at: Some(now),
        }));
        let projected = entry_from_state(&tab, &state, now);
        assert_eq!(projected.id, "custom:mytool");
        assert_eq!(projected.display_name, "My Tool");
        assert_eq!(projected.short_name, "myt");
        assert_eq!(projected.plan.as_deref(), Some("Team"));

        let rendered = render_json_for_primary(&[projected], None);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let first = &value["entries"][0];
        assert_eq!(first["short_name"], "myt");
        assert_eq!(first["icon"], "myt");
        // No `brand`, so the optional field is absent and the frontend keeps
        // drawing the short_name tag.
        assert!(first.get("brand").is_none());
        assert_eq!(first["metrics"][0]["label"], "Session");
        assert_eq!(first["metrics"][0]["percent"], 40);
        assert_eq!(first["metrics"][0]["window_secs"], 18_000);
        assert_eq!(
            first["metrics"][0]["reset_at"],
            session_reset.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
        );
        assert_eq!(first["metrics"][1]["label"], "Monthly");
        assert!(first["metrics"][1].get("window_secs").is_none());
        assert!(first["sections"].as_array().unwrap().iter().any(|section| {
            section["type"] == "text" && section["label"] == "Region" && section["value"] == "eu"
        }));

        // A failed custom entry still carries its code and names.
        let failed = entry_from_state(&tab, &TabState::error("HTTP 500"), now);
        assert_eq!(failed.short_name, "myt");
        assert_eq!(failed.display_name, "My Tool");
        assert!(failed.sections.is_empty());
    }
}

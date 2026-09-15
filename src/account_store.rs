//! Persistent multi-account snapshot storage and cross-provider email linking.
//!
//! When an account is active, its live usage is captured and saved as a snapshot
//! in `~/.cache/ai-usagebar/account_snapshots.json`. When switching accounts,
//! previous accounts remain safely preserved on disk. Querying an inactive account
//! displays its last saved snapshot along with real-time calculated quota renewal.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::{AccountConfig, Config};
use crate::error::{AppError, Result};

/// Lossless machine-readable projection of a TUI panel row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReportSection {
    Metric {
        label: String,
        percent: u16,
        value: String,
        detail: String,
        severity: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reset_at: Option<DateTime<Utc>>,
        /// Full length of the reset window in seconds, present only when the
        /// vendor states it exactly (rolling 5h/7d windows).
        #[serde(skip_serializing_if = "Option::is_none")]
        window_secs: Option<u64>,
    },
    Text {
        label: String,
        value: String,
    },
    Block {
        label: String,
        body: Vec<String>,
    },
    Spacer,
}

impl ReportSection {
    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Metric { label, .. } | Self::Text { label, .. } | Self::Block { label, .. } => {
                Some(label)
            }
            Self::Spacer => None,
        }
    }
}

/// One configured vendor or account entry within a report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportEntry {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub short_name: String,
    pub icon: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brand: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    pub sections: Vec<ReportSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<DateTime<Utc>>,
}

/// Frozen snapshot of an account's quota state across its providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSnapshot {
    pub account_label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    pub saved_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
    pub entries: Vec<ReportEntry>,
}

/// Container for all persisted snapshots.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotStore {
    #[serde(default)]
    pub snapshots: BTreeMap<String, AccountSnapshot>,
    /// Last detected user/email identity per provider (e.g. "openai" -> "user@example.com").
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub active_sessions: BTreeMap<String, String>,
}

/// Path to the persisted account snapshots file in the user's cache directory.
pub fn snapshots_path() -> Result<PathBuf> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| AppError::Other("could not resolve XDG cache dir".into()))?;
    Ok(base
        .cache_dir()
        .join("ai-usagebar")
        .join("account_snapshots.json"))
}

/// Load the snapshot store from the default user cache location.
pub fn load_store() -> SnapshotStore {
    match snapshots_path() {
        Ok(path) => load_store_from(&path),
        Err(_) => SnapshotStore::default(),
    }
}

/// Check if two account labels or user strings refer to the same account.
/// Matches case-insensitively, or matches if one is the username prefix of an email
/// (e.g. "s2.luan2009" and "s2.luan2009@gmail.com").
pub fn accounts_match_or_prefix(a: &str, b: &str) -> bool {
    let a_clean = a.trim();
    let b_clean = b.trim();
    if a_clean.eq_ignore_ascii_case(b_clean) {
        return true;
    }
    if let Some((user_a, _)) = a_clean.split_once('@') {
        if user_a.eq_ignore_ascii_case(b_clean) {
            return true;
        }
    }
    if let Some((user_b, _)) = b_clean.split_once('@') {
        if user_b.eq_ignore_ascii_case(a_clean) {
            return true;
        }
    }
    false
}

/// Sanitize snapshot entry to strip out transient HTTP error sections or raw JSON failures.
pub fn sanitize_snapshot_entry(entry: &mut ReportEntry) {
    entry.sections.retain(|s| match s {
        ReportSection::Text { label, value } => {
            let lbl = label.trim();
            let val = value.trim();
            if lbl.starts_with("HTTP ")
                || val.starts_with('{')
                || val.contains("not logged into")
                || val.contains("error getting token")
            {
                false
            } else {
                true
            }
        }
        _ => true,
    });
    while matches!(entry.sections.last(), Some(ReportSection::Spacer)) {
        entry.sections.pop();
    }
    if entry.sections.iter().any(|s| matches!(s, ReportSection::Metric { .. })) {
        entry.stale = false;
    }
}

/// Normalize snapshot store so that account labels are canonical full emails where known,
/// and duplicate snapshots where one label was just the username prefix are merged.
pub fn normalize_store(store: &mut SnapshotStore) {
    let mut canonical_map: BTreeMap<String, AccountSnapshot> = BTreeMap::new();
    for (_k, snap) in std::mem::take(&mut store.snapshots) {
        // Check if canonical_map already has a matching snapshot
        if let Some(existing) = canonical_map.values_mut().find(|s| {
            accounts_match_or_prefix(&s.account_label, &snap.account_label)
                || (s.user.is_some()
                    && snap.user.is_some()
                    && accounts_match_or_prefix(
                        s.user.as_deref().unwrap(),
                        snap.user.as_deref().unwrap(),
                    ))
        }) {
            // Merge providers
            for p in snap.providers {
                if !existing.providers.iter().any(|ep| ep.eq_ignore_ascii_case(&p)) {
                    existing.providers.push(p);
                }
            }
            // Merge entries
            for entry in snap.entries {
                if let Some(pos) = existing
                    .entries
                    .iter()
                    .position(|e| e.id.eq_ignore_ascii_case(&entry.id))
                {
                    if snap.saved_at > existing.saved_at {
                        existing.entries[pos] = entry;
                    }
                } else {
                    existing.entries.push(entry);
                }
            }
            if snap.saved_at > existing.saved_at {
                existing.saved_at = snap.saved_at;
            }
            if existing.user.is_none() && snap.user.is_some() {
                existing.user = snap.user;
            }
            if !existing.account_label.contains('@') && snap.account_label.contains('@') {
                existing.account_label = snap.account_label;
            }
        } else {
            canonical_map.insert(snap.account_label.clone(), snap);
        }
    }

    for snap in canonical_map.values_mut() {
        for entry in &mut snap.entries {
            sanitize_snapshot_entry(entry);
        }
    }

    // Re-key canonical_map by account_label
    store.snapshots = canonical_map
        .into_iter()
        .map(|(_, s)| (s.account_label.clone(), s))
        .collect();

    // Also canonicalize active_sessions
    for (_, session_email) in store.active_sessions.iter_mut() {
        if !session_email.contains('@') {
            if let Some(snap) = store
                .snapshots
                .values()
                .find(|s| accounts_match_or_prefix(&s.account_label, session_email))
            {
                if snap.account_label.contains('@') {
                    *session_email = snap.account_label.clone();
                }
            }
        }
    }
}

/// Load the snapshot store from an explicit path (used by tests).
pub fn load_store_from(path: &Path) -> SnapshotStore {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return SnapshotStore::default(),
    };
    let mut store: SnapshotStore = serde_json::from_str(&raw).unwrap_or_default();
    normalize_store(&mut store);
    store
}

/// Save the snapshot store to the default user cache location.
pub fn save_store(store: &SnapshotStore) -> Result<()> {
    let path = snapshots_path()?;
    save_store_to(&path, store)
}

/// Save the snapshot store to an explicit path atomically (used by tests).
pub fn save_store_to(path: &Path, store: &SnapshotStore) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_vec_pretty(store)?;
    crate::cache::atomic_write(path, &data)
}

/// Record or update an account snapshot in the default user cache.
pub fn record_snapshot(
    account_label: &str,
    user: Option<&str>,
    providers: &[String],
    entries: &[ReportEntry],
) -> Result<()> {
    let path = snapshots_path()?;
    record_snapshot_at(&path, account_label, user, providers, entries)
}

/// Record or update an account snapshot at an explicit path.
pub fn record_snapshot_at(
    path: &Path,
    account_label: &str,
    user: Option<&str>,
    providers: &[String],
    entries: &[ReportEntry],
) -> Result<()> {
    let mut store = load_store_from(path);
    let snapshot = AccountSnapshot {
        account_label: account_label.to_string(),
        user: user.map(str::to_string),
        saved_at: Utc::now(),
        providers: providers.to_vec(),
        entries: entries.to_vec(),
    };
    store.snapshots.insert(account_label.to_string(), snapshot);
    save_store_to(path, &store)
}

/// Update or insert a single provider entry in the snapshot for `account_label`.
pub fn record_provider_entry(
    account_label: &str,
    user: Option<&str>,
    entry: &ReportEntry,
) -> Result<()> {
    let path = snapshots_path()?;
    record_provider_entry_at(&path, account_label, user, entry)
}

/// Update or insert a single provider entry at an explicit path.
pub fn record_provider_entry_at(
    path: &Path,
    account_label: &str,
    user: Option<&str>,
    entry: &ReportEntry,
) -> Result<()> {
    let mut store = load_store_from(path);
    let snap = store
        .snapshots
        .entry(account_label.to_string())
        .or_insert_with(|| AccountSnapshot {
            account_label: account_label.to_string(),
            user: user.map(str::to_string),
            saved_at: Utc::now(),
            providers: Vec::new(),
            entries: Vec::new(),
        });

    if user.is_some() && snap.user.is_none() {
        snap.user = user.map(str::to_string);
    }
    snap.saved_at = Utc::now();
    if !snap
        .providers
        .iter()
        .any(|p| p.eq_ignore_ascii_case(&entry.id))
    {
        snap.providers.push(entry.id.clone());
    }

    let mut clean_entry = entry.clone();
    sanitize_snapshot_entry(&mut clean_entry);
    if let Some(pos) = snap.entries.iter().position(|e| e.id == entry.id) {
        snap.entries[pos] = clean_entry;
    } else {
        snap.entries.push(clean_entry);
    }

    save_store_to(path, &store)
}

/// Retrieve a saved snapshot by account label or user email from the default cache.
pub fn get_snapshot(account_or_user: &str) -> Option<AccountSnapshot> {
    let path = snapshots_path().ok()?;
    get_snapshot_at(&path, account_or_user)
}

/// Retrieve a saved snapshot by account label or user email from an explicit path.
pub fn get_snapshot_at(path: &Path, account_or_user: &str) -> Option<AccountSnapshot> {
    let store = load_store_from(path);
    // 1. Match on account_label (case-insensitive or prefix)
    if let Some(snap) = store
        .snapshots
        .values()
        .find(|s| accounts_match_or_prefix(&s.account_label, account_or_user))
    {
        return Some(snap.clone());
    }
    // 2. Match on user / email (case-insensitive or prefix)
    store
        .snapshots
        .values()
        .find(|s| {
            s.user
                .as_deref()
                .map(|u| accounts_match_or_prefix(u, account_or_user))
                .unwrap_or(false)
        })
        .cloned()
}

/// Return all saved snapshots from the default cache.
pub fn all_snapshots() -> Vec<AccountSnapshot> {
    match snapshots_path() {
        Ok(path) => all_snapshots_at(&path),
        Err(_) => Vec::new(),
    }
}

/// Return all saved snapshots from an explicit path.
pub fn all_snapshots_at(path: &Path) -> Vec<AccountSnapshot> {
    let store = load_store_from(path);
    store.snapshots.into_values().collect()
}

/// Update countdown and renewal messages dynamically for a snapshot's entries
/// relative to the current timestamp (`now`).
pub fn update_snapshot_entries(entries: &mut [ReportEntry], now: DateTime<Utc>) {
    for entry in entries.iter_mut() {
        sanitize_snapshot_entry(entry);
        for section in &mut entry.sections {
            if let ReportSection::Metric {
                reset_at: Some(reset_at),
                detail,
                ..
            } = section
            {
                if now >= *reset_at {
                    *detail = format!("Renewed at {} (ready to use)", reset_at.format("%H:%M UTC"));
                } else {
                    *detail = format!(
                        "resets in {} ({})",
                        crate::countdown::format(Some(*reset_at), now),
                        reset_at.format("%H:%M UTC")
                    );
                }
            }
        }
    }
}

/// Attempt to detect the local user/email identity associated with a provider.
pub fn detect_provider_email(provider_slug: &str, config: &Config) -> Option<String> {
    let slug = provider_slug.trim().to_lowercase();
    match slug.as_str() {
        "openai" => {
            let path = config
                .openai
                .resolve_auth_path(None)
                .or_else(|_| crate::openai::creds::default_path());
            if let Ok(p) = path
                && let Ok(auth) = crate::openai::creds::read_from(&p)
            {
                return auth.tokens.email_from_id_token();
            }
        }
        "antigravity" => {
            if let Ok(cache) = crate::cache::Cache::for_vendor("antigravity") {
                let is_logged_out = if let Ok(err) = fs::read_to_string(cache.last_error_path()) {
                    err.contains("not logged into") || err.contains("unauthenticated")
                } else {
                    false
                };
                if !is_logged_out
                    && let Ok(raw) = fs::read_to_string(cache.payload_path())
                    && let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw)
                    && let Some(email) = v["user_email"].as_str()
                {
                    let trimmed = email.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_string());
                    }
                }
            }
            if let Ok(Some(raw)) = crate::antigravity::credential::read() {
                let trimmed = raw.trim();
                let json = match trimmed.strip_prefix(crate::antigravity::credential::GO_KEYRING_PREFIX) {
                    Some(enc) => {
                        use base64::Engine;
                        base64::engine::general_purpose::STANDARD
                            .decode(enc.trim())
                            .ok()
                            .and_then(|b| String::from_utf8(b).ok())
                            .unwrap_or_default()
                    }
                    None => trimmed.to_string(),
                };
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
                    if let Some(email) = v.get("email").and_then(|e| e.as_str()) {
                        let trimmed = email.trim();
                        if !trimmed.is_empty() {
                            return Some(trimmed.to_string());
                        }
                    }
                }
            }
        }
        "anthropic" => {
            let cred_path = config
                .anthropic
                .credentials_path
                .clone()
                .or_else(|| crate::anthropic::creds::default_path().ok());
            if let Some(path) = cred_path
                && let Some(parent) = path.parent()
            {
                let marker = crate::anthropic::cli_account::marker_path(parent);
                if let Some(email) = crate::anthropic::cli_account::account_email_in(&marker) {
                    return Some(email);
                }
            }
        }
        _ => {}
    }
    None
}

/// Check if a provider matches a target user email, considering credentials,
/// cache, or saved snapshots.
pub fn provider_matches_user(
    provider_slug: &str,
    target_user: &str,
    config: &Config,
    snapshots: &[AccountSnapshot],
) -> bool {
    let target = target_user.trim();
    if target.is_empty() {
        return true;
    }

    // 1. Direct email detection from active credentials/cache:
    if let Some(email) = detect_provider_email(provider_slug, config) {
        return email.eq_ignore_ascii_case(target);
    }

    // 2. Check in saved snapshots:
    for snap in snapshots {
        if snap
            .user
            .as_deref()
            .map(|u| u.eq_ignore_ascii_case(target))
            .unwrap_or(false)
            && snap
                .providers
                .iter()
                .any(|p| p.eq_ignore_ascii_case(provider_slug))
        {
            return true;
        }
    }

    false
}

/// Generate a clean, readable account label from a user email (e.g. "maximusdev58@gmail.com").
pub fn suggest_account_label(email: &str, existing: &[AccountConfig]) -> String {
    let clean: String = email
        .trim()
        .chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\' && *c != ':')
        .collect();
    let base = if clean.is_empty() {
        "account".to_string()
    } else {
        clean.to_lowercase()
    };

    if !existing.iter().any(|a| a.label.eq_ignore_ascii_case(&base)) {
        return base;
    }

    let mut i = 2;
    loop {
        let candidate = format!("{base}-{i}");
        if !existing
            .iter()
            .any(|a| a.label.eq_ignore_ascii_case(&candidate))
        {
            return candidate;
        }
        i += 1;
    }
}


/// Detect active provider sessions across the machine, handle account switches by
/// invalidating stale vendor caches, and auto-register newly detected accounts in
/// `config.accounts` (and `config.toml` when `config_path` is given).
pub fn detect_and_sync_account_session(
    config: &mut Config,
    config_path: Option<&Path>,
) -> Result<bool> {
    let path = snapshots_path()?;
    detect_and_sync_account_session_at(&path, config, config_path)
}

/// Test-friendly variant taking explicit snapshot store path.
pub fn detect_and_sync_account_session_at(
    store_path: &Path,
    config: &mut Config,
    config_path: Option<&Path>,
) -> Result<bool> {
    let mut store = load_store_from(store_path);
    let mut store_dirty = false;
    let mut config_dirty = false;

    // Prune dummy accounts if present:
    let dummy_keys: Vec<String> = store
        .snapshots
        .iter()
        .filter(|(k, s)| {
            *k == "personal" && s.user.as_deref() == Some("maximus@personal.dev")
                || *k == "work" && s.user.as_deref() == Some("maximus@work.corp")
                || *k == "openai_auto"
        })
        .map(|(k, _)| k.clone())
        .collect();
    for k in dummy_keys {
        store.snapshots.remove(&k);
        store_dirty = true;
    }

    let providers = [
        (crate::vendor::VendorId::Openai, "openai"),
        (crate::vendor::VendorId::Antigravity, "antigravity"),
        (crate::vendor::VendorId::Anthropic, "anthropic"),
    ];
    for (v, p) in providers {
        if !config.is_enabled(v) {
            continue;
        }
        if let Some(curr) = detect_provider_email(p, config) {
            let last = store.active_sessions.get(p).cloned();
            if let Some(ref prev) = last {
                if !prev.eq_ignore_ascii_case(&curr) {
                    // Account switch detected!
                    // Invalidate cache for provider p:
                    if let Ok(cache) = crate::cache::Cache::for_vendor(p) {
                        let _ = fs::remove_file(cache.payload_path());
                    }
                    store.active_sessions.insert(p.to_string(), curr.clone());
                    store_dirty = true;
                }
            } else {
                // First session record
                store.active_sessions.insert(p.to_string(), curr.clone());
                store_dirty = true;
            }

            // Check if curr is registered in config.accounts (full match or prefix)
            let matching_pos = config.accounts.iter().position(|a| {
                accounts_match_or_prefix(&a.label, &curr)
                    || a.user
                        .as_deref()
                        .map(|u| accounts_match_or_prefix(u, &curr))
                        .unwrap_or(false)
            });

            if let Some(pos) = matching_pos {
                let acct = &mut config.accounts[pos];
                if !acct.label.contains('@') && curr.contains('@') {
                    acct.label = curr.clone();
                    config_dirty = true;
                }
                if acct.user.is_none() {
                    acct.user = Some(curr.clone());
                    config_dirty = true;
                }
                if !acct.has_provider(p) {
                    acct.providers.push(p.to_string());
                    config_dirty = true;
                }
            } else {
                let label = suggest_account_label(&curr, &config.accounts);
                let new_account = AccountConfig {
                    label: label.clone(),
                    user: Some(curr.clone()),
                    providers: vec![p.to_string()],
                };
                config.accounts.push(new_account);
                config_dirty = true;
            }
        }
    }

    // Also sync existing snapshots from disk into config.accounts if not present:
    for snap in store.snapshots.values() {
        let exists = config.accounts.iter().any(|a| {
            accounts_match_or_prefix(&a.label, &snap.account_label)
                || (snap.user.is_some()
                    && a.user
                        .as_deref()
                        .map(|u| accounts_match_or_prefix(u, snap.user.as_deref().unwrap()))
                        .unwrap_or(false))
        });
        if !exists {
            let label = if crate::config::validate_account_label(&snap.account_label).is_ok()
                && !config
                    .accounts
                    .iter()
                    .any(|a| accounts_match_or_prefix(&a.label, &snap.account_label))
            {
                snap.account_label.clone()
            } else if let Some(ref u) = snap.user {
                suggest_account_label(u, &config.accounts)
            } else {
                continue;
            };

            let providers = if snap.providers.is_empty() {
                snap.entries.iter().map(|e| e.id.clone()).collect()
            } else {
                snap.providers.clone()
            };

            config.accounts.push(AccountConfig {
                label,
                user: snap.user.clone(),
                providers,
            });
            config_dirty = true;
        }
    }

    // Deduplicate config.accounts to eliminate any legacy username vs full email duplicates:
    let mut deduplicated: Vec<AccountConfig> = Vec::new();
    for acct in std::mem::take(&mut config.accounts) {
        if let Some(existing) = deduplicated.iter_mut().find(|a| {
            accounts_match_or_prefix(&a.label, &acct.label)
                || (a.user.is_some()
                    && acct.user.is_some()
                    && accounts_match_or_prefix(
                        a.user.as_deref().unwrap(),
                        acct.user.as_deref().unwrap(),
                    ))
        }) {
            if !existing.label.contains('@') && acct.label.contains('@') {
                existing.label = acct.label.clone();
                config_dirty = true;
            }
            if existing.user.is_none() && acct.user.is_some() {
                existing.user = acct.user.clone();
                config_dirty = true;
            }
            for p in acct.providers {
                if !existing.has_provider(&p) {
                    existing.providers.push(p);
                    config_dirty = true;
                }
            }
        } else {
            deduplicated.push(acct);
        }
    }
    config.accounts = deduplicated;

    if config_dirty
        && let Some(cp) = config_path
        && let Ok(mut doc) = crate::config::read_config_document(cp)
    {
        if crate::config::sync_global_accounts(&mut doc, &config.accounts).is_ok() {
            let _ = crate::config::write_config_document(cp, &doc);
        }
    }

    if store_dirty {
        save_store_to(store_path, &store)?;
    }

    Ok(config_dirty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_entries() -> Vec<ReportEntry> {
        vec![ReportEntry {
            id: "antigravity".into(),
            name: "Google Antigravity".into(),
            display_name: "Google Antigravity".into(),
            short_name: "AG".into(),
            icon: "󰧑".into(),
            brand: Some("antigravity".into()),
            plan: Some("Google AI Pro".into()),
            sections: vec![
                ReportSection::Metric {
                    label: "Gemini 5-Hour Limit".into(),
                    percent: 43,
                    value: "43%".into(),
                    detail: "resets in 2h".into(),
                    severity: "neutral".into(),
                    reset_at: Some(Utc::now() + chrono::Duration::hours(2)),
                    window_secs: Some(5 * 3600),
                },
                ReportSection::Spacer,
            ],
            error: None,
            stale: false,
            fetched_at: Some(Utc::now()),
        }]
    }

    #[test]
    fn snapshot_store_roundtrip_save_and_load() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("snapshots.json");

        let entries = sample_entries();
        let providers = vec!["antigravity".to_string()];
        record_snapshot_at(
            &path,
            "personal",
            Some("personal@example.com"),
            &providers,
            &entries,
        )
        .unwrap();

        let loaded = get_snapshot_at(&path, "personal").expect("must find by label");
        assert_eq!(loaded.account_label, "personal");
        assert_eq!(loaded.user.as_deref(), Some("personal@example.com"));
        assert_eq!(loaded.providers, vec!["antigravity"]);
        assert_eq!(loaded.entries.len(), 1);

        // Also accessible by email
        let by_email = get_snapshot_at(&path, "personal@example.com").expect("must find by email");
        assert_eq!(by_email.account_label, "personal");
    }

    #[test]
    fn update_snapshot_entries_shows_renewed_when_expired() {
        let mut entries = sample_entries();
        let past = Utc::now() - chrono::Duration::minutes(5);
        if let ReportSection::Metric { reset_at, .. } = &mut entries[0].sections[0] {
            *reset_at = Some(past);
        }

        update_snapshot_entries(&mut entries, Utc::now());
        if let ReportSection::Metric { detail, .. } = &entries[0].sections[0] {
            assert!(detail.contains("Renewed at"));
            assert!(detail.contains("ready to use"));
        } else {
            panic!("expected Metric section");
        }
    }

    #[test]
    fn update_snapshot_entries_shows_countdown_when_pending() {
        let mut entries = sample_entries();
        let future = Utc::now() + chrono::Duration::minutes(45);
        if let ReportSection::Metric { reset_at, .. } = &mut entries[0].sections[0] {
            *reset_at = Some(future);
        }

        update_snapshot_entries(&mut entries, Utc::now());
        if let ReportSection::Metric { detail, .. } = &entries[0].sections[0] {
            assert!(detail.contains("resets in"));
        } else {
            panic!("expected Metric section");
        }
    }

    #[test]
    fn suggest_account_label_derives_clean_unique_labels() {
        let existing = vec![AccountConfig {
            label: "personal@gmail.com".into(),
            user: Some("personal@gmail.com".into()),
            providers: vec![],
        }];

        // Unique email becomes full email label
        assert_eq!(
            suggest_account_label("work@company.com", &existing),
            "work@company.com"
        );
        // Collides with existing label, adds numeric suffix
        assert_eq!(
            suggest_account_label("personal@gmail.com", &existing),
            "personal@gmail.com-2"
        );
        let existing2 = vec![
            AccountConfig {
                label: "personal@gmail.com".into(),
                user: Some("personal@gmail.com".into()),
                providers: vec![],
            },
            AccountConfig {
                label: "personal@gmail.com-2".into(),
                user: Some("personal@gmail.com".into()),
                providers: vec![],
            },
        ];
        assert_eq!(
            suggest_account_label("personal@gmail.com", &existing2),
            "personal@gmail.com-3"
        );
    }


    #[test]
    fn record_provider_entry_at_updates_existing_and_preserves_other_providers() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("snapshots.json");

        let entry1 = ReportEntry {
            id: "openai".into(),
            name: "OpenAI".into(),
            display_name: "OpenAI".into(),
            short_name: "OAI".into(),
            icon: "󰧑".into(),
            brand: None,
            plan: Some("ChatGPT Plus".into()),
            sections: vec![],
            error: None,
            stale: false,
            fetched_at: Some(Utc::now()),
        };

        let entry2 = ReportEntry {
            id: "antigravity".into(),
            name: "Antigravity".into(),
            display_name: "Antigravity".into(),
            short_name: "AG".into(),
            icon: "󰧑".into(),
            brand: None,
            plan: Some("Google AI Pro".into()),
            sections: vec![],
            error: None,
            stale: false,
            fetched_at: Some(Utc::now()),
        };

        record_provider_entry_at(&path, "personal", Some("user@gmail.com"), &entry1).unwrap();
        record_provider_entry_at(&path, "personal", Some("user@gmail.com"), &entry2).unwrap();

        let snap = get_snapshot_at(&path, "personal").expect("snapshot exists");
        assert_eq!(snap.entries.len(), 2);
        assert_eq!(snap.providers, vec!["openai", "antigravity"]);

        // Update openai entry
        let mut updated_entry1 = entry1.clone();
        updated_entry1.plan = Some("ChatGPT Team".into());
        record_provider_entry_at(&path, "personal", Some("user@gmail.com"), &updated_entry1)
            .unwrap();

        let snap2 = get_snapshot_at(&path, "personal").expect("snapshot exists");
        assert_eq!(snap2.entries.len(), 2);
        let oai = snap2.entries.iter().find(|e| e.id == "openai").unwrap();
        assert_eq!(oai.plan.as_deref(), Some("ChatGPT Team"));
    }

    #[test]
    fn detect_and_sync_account_session_auto_registers_and_persists_config() {
        use base64::Engine;
        let dir = tempdir().unwrap();
        let store_path = dir.path().join("snapshots.json");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "# user config\n").unwrap();

        // Create dummy openai auth file
        let codex_dir = dir.path().join("codex");
        fs::create_dir_all(&codex_dir).unwrap();
        let auth_path = codex_dir.join("auth.json");

        // Helper JWT with email claims
        let jwt_payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"https://api.openai.com/profile":{"email":"testuser@workcorp.com"},"exp":1999999999}"#);
        let dummy_jwt = format!("eyJhbGciOiJub25lIn0.{jwt_payload}.");
        let auth_json = format!(
            r#"{{"tokens":{{"id_token":"{dummy_jwt}","access_token":"at","refresh_token":"rt"}}}}"#
        );
        fs::write(&auth_path, auth_json).unwrap();

        let mut config = Config::default();
        config.antigravity.enabled = false;
        config.anthropic.enabled = false;
        config.openai.enabled = true;
        config.openai.codex_auth_path = Some(auth_path.clone());

        // First run: discovers testuser@workcorp.com, auto-registers account
        let changed =
            detect_and_sync_account_session_at(&store_path, &mut config, Some(&config_path))
                .unwrap();
        assert!(changed);
        assert_eq!(config.accounts.len(), 1);
        assert_eq!(
            config.accounts[0].user.as_deref(),
            Some("testuser@workcorp.com")
        );
        assert_eq!(config.accounts[0].label, "testuser@workcorp.com");

        // Config file was written
        let saved_cfg = fs::read_to_string(&config_path).unwrap();
        assert!(saved_cfg.contains("[[accounts]]"));
        assert!(saved_cfg.contains("label = \"testuser@workcorp.com\""));
        assert!(saved_cfg.contains("user = \"testuser@workcorp.com\""));

        // Store active session was recorded
        let store = load_store_from(&store_path);
        assert_eq!(
            store.active_sessions.get("openai").map(String::as_str),
            Some("testuser@workcorp.com")
        );

        // Running again with same session makes no changes
        let changed2 =
            detect_and_sync_account_session_at(&store_path, &mut config, Some(&config_path))
                .unwrap();
        assert!(!changed2);
        assert_eq!(config.accounts.len(), 1);
    }

    #[test]
    fn test_deduplicate_and_canonicalize_prefix_accounts() {
        let dir = tempdir().unwrap();
        let store_path = dir.path().join("snapshots.json");
        let config_path = dir.path().join("config.toml");

        // Write an initial config that has legacy prefix label "s2.luan2009"
        let initial_cfg = r#"
[[accounts]]
label = "s2.luan2009"
user = "s2.luan2009@gmail.com"
providers = ["antigravity"]
"#;
        fs::write(&config_path, initial_cfg).unwrap();

        // Write a codex auth.json with full email "s2.luan2009@gmail.com"
        let auth_path = dir.path().join("auth.json");
        use base64::Engine;
        let jwt_payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"https://api.openai.com/profile":{"email":"s2.luan2009@gmail.com"},"exp":1999999999}"#);
        let dummy_jwt = format!("eyJhbGciOiJub25lIn0.{jwt_payload}.");
        let auth_json = format!(
            r#"{{"tokens":{{"id_token":"{dummy_jwt}","access_token":"at","refresh_token":"rt"}}}}"#
        );
        fs::write(&auth_path, auth_json).unwrap();

        let mut config = Config::default();
        config.antigravity.enabled = false;
        config.anthropic.enabled = false;
        config.openai.enabled = true;
        config.openai.codex_auth_path = Some(auth_path.clone());
        config.accounts.push(AccountConfig {
            label: "s2.luan2009".into(),
            user: Some("s2.luan2009@gmail.com".into()),
            providers: vec!["antigravity".into()],
        });

        // Run sync
        let changed = detect_and_sync_account_session_at(
            &store_path,
            &mut config,
            Some(&config_path),
        ).unwrap();

        assert!(changed);
        // Must NOT create a second account! Must upgrade the existing one
        assert_eq!(config.accounts.len(), 1);
        assert_eq!(config.accounts[0].label, "s2.luan2009@gmail.com");
        assert_eq!(config.accounts[0].user.as_deref(), Some("s2.luan2009@gmail.com"));
        assert!(config.accounts[0].has_provider("antigravity"));
        assert!(config.accounts[0].has_provider("openai"));

        // Also test SnapshotStore merging duplicate snapshots:
        let mut store = SnapshotStore::default();
        store.snapshots.insert(
            "s2.luan2009".into(),
            AccountSnapshot {
                account_label: "s2.luan2009".into(),
                user: Some("s2.luan2009@gmail.com".into()),
                saved_at: Utc::now() - chrono::Duration::hours(1),
                providers: vec!["antigravity".into()],
                entries: vec![],
            },
        );
        store.snapshots.insert(
            "s2.luan2009@gmail.com".into(),
            AccountSnapshot {
                account_label: "s2.luan2009@gmail.com".into(),
                user: Some("s2.luan2009@gmail.com".into()),
                saved_at: Utc::now(),
                providers: vec!["openai".into()],
                entries: vec![],
            },
        );

        normalize_store(&mut store);
        assert_eq!(store.snapshots.len(), 1);
        assert!(store.snapshots.contains_key("s2.luan2009@gmail.com"));
        let merged = &store.snapshots["s2.luan2009@gmail.com"];
        assert!(merged.providers.contains(&"antigravity".to_string()));
        assert!(merged.providers.contains(&"openai".to_string()));
    }
}

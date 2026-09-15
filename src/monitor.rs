//! Background quota renewal monitor and desktop notification daemon.
//!
//! Tracks quota reset timestamps across active and stored account snapshots.
//! When a 5-hour or weekly quota window expires (renews), it triggers a native
//! desktop notification (e.g. via `notify-send` on KDE Plasma / Wayland) so the
//! user knows the account is ready to be used again.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::account_store::{ReportSection, all_snapshots, snapshots_path};
use crate::error::{AppError, Result};

/// Track notifications that have already been sent to prevent duplicates.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotificationStore {
    #[serde(default)]
    pub sent: BTreeMap<String, DateTime<Utc>>,
}

/// Represents an account quota window that has renewed (reset).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaRenewal {
    pub id: String,
    pub account_label: String,
    pub user: Option<String>,
    pub provider_id: String,
    pub provider_name: String,
    pub metric_label: String,
    pub window_type: String,
    pub reset_at: DateTime<Utc>,
    pub percent_before: u64,
}

/// Path to the persisted notifications-sent cache file.
pub fn notifications_path() -> Result<PathBuf> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| AppError::Other("could not resolve XDG cache dir".into()))?;
    Ok(base
        .cache_dir()
        .join("ai-usagebar")
        .join("notifications_sent.json"))
}

/// Path to the simulated test renewals file.
pub fn test_renewals_path() -> Result<PathBuf> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| AppError::Other("could not resolve XDG cache dir".into()))?;
    Ok(base
        .cache_dir()
        .join("ai-usagebar")
        .join("test_renewals.json"))
}

/// Simulate a test quota renewal notice for the widget.
pub fn simulate_test_renewal() -> Result<()> {
    let path = test_renewals_path()?;
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    let now = Utc::now();
    let items = vec![
        QuotaRenewal {
            id: format!("test:{}:antigravity:claude", now.timestamp()),
            account_label: "maximusdev58@gmail.com".into(),
            user: Some("maximusdev58@gmail.com".into()),
            provider_id: "antigravity".into(),
            provider_name: "Antigravity".into(),
            metric_label: "Claude & GPT OSS".into(),
            window_type: "5h".into(),
            reset_at: now - chrono::Duration::minutes(15),
            percent_before: 100,
        },
        QuotaRenewal {
            id: format!("test:{}:openai:5h", now.timestamp()),
            account_label: "s2.luan2009@gmail.com".into(),
            user: Some("s2.luan2009@gmail.com".into()),
            provider_id: "openai".into(),
            provider_name: "Codex".into(),
            metric_label: "Codex 5h".into(),
            window_type: "5h".into(),
            reset_at: now - chrono::Duration::minutes(35),
            percent_before: 80,
        },
    ];
    let data = serde_json::to_vec_pretty(&items)?;
    crate::cache::atomic_write(&path, &data)
}

/// Clear any simulated test renewal notices.
pub fn clear_test_renewals() -> Result<()> {
    let path = test_renewals_path()?;
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

/// Detect all renewals across snapshots where now >= reset_at.
pub fn detect_all_renewals(now: DateTime<Utc>) -> Vec<QuotaRenewal> {
    let snaps = all_snapshots();
    let mut results = Vec::new();
    for snap in snaps {
        for entry in &snap.entries {
            for section in &entry.sections {
                if let ReportSection::Metric {
                    label,
                    reset_at: Some(reset_at),
                    percent,
                    window_secs,
                    ..
                } = section
                {
                    // If reset_at has passed and happened within the last 72h
                    if now >= *reset_at && (now - *reset_at).num_hours() <= 72 {
                        let window_type = match window_secs {
                            Some(s) if *s <= 21600 => "5h".to_string(),
                            Some(s) if *s >= 500000 && *s <= 700000 => "Semanal".to_string(),
                            _ => {
                                let l = label.to_lowercase();
                                if l.contains("week") {
                                    "Semanal".to_string()
                                } else if l.contains("5h") || l.contains("sess") {
                                    "5h".to_string()
                                } else {
                                    "5h".to_string()
                                }
                            }
                        };
                        let id = format!("{}:{}:{}:{}", snap.account_label, entry.id, label, reset_at.to_rfc3339());
                        results.push(QuotaRenewal {
                            id,
                            account_label: snap.account_label.clone(),
                            user: snap.user.clone(),
                            provider_id: entry.id.clone(),
                            provider_name: entry.display_name.clone(),
                            metric_label: label.clone(),
                            window_type,
                            reset_at: *reset_at,
                            percent_before: u64::from(*percent),
                        });
                    }
                }
            }
        }
    }
    if let Ok(path) = test_renewals_path() {
        if let Ok(content) = fs::read_to_string(path) {
            if let Ok(items) = serde_json::from_str::<Vec<QuotaRenewal>>(&content) {
                results.extend(items);
            }
        }
    }
    results
}

pub fn load_notification_store_from(path: &Path) -> NotificationStore {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return NotificationStore::default(),
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn save_notification_store_to(path: &Path, store: &NotificationStore) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_vec_pretty(store)?;
    crate::cache::atomic_write(path, &data)
}

/// Send a native desktop notification across Linux, macOS, and Windows.
pub fn send_desktop_notification(summary: &str, body: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            body.replace('\\', "\\\\").replace('"', "\\\""),
            summary.replace('\\', "\\\\").replace('"', "\\\""),
        );
        let mut cmd = std::process::Command::new("osascript");
        cmd.args(["-e", &script]);
        cmd.status()?;
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        let escaped_summary = summary.replace('\'', "''");
        let escaped_body = body.replace('\'', "''");
        let script = format!(
            "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null; \
             $template = [Windows.UI.Notifications.ToastTemplateType]::ToastText02; \
             $xml = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent($template); \
             $text = $xml.GetElementsByTagName('text'); \
             $text[0].AppendChild($xml.CreateTextNode('{escaped_summary}')) > $null; \
             $text[1].AppendChild($xml.CreateTextNode('{escaped_body}')) > $null; \
             $toast = [Windows.UI.Notifications.ToastNotification]::new($xml); \
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('AI Usage Bar').Show($toast);"
        );
        let mut cmd = std::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-Command", &script]);
        cmd.status()?;
        return Ok(());
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let mut cmd = std::process::Command::new("notify-send");
        cmd.args([
            "-a",
            "AI Usage Bar",
            "-i",
            "dialog-information",
            "-u",
            "normal",
            summary,
            body,
        ]);
        cmd.status()?;
        Ok(())
    }
}

/// Check all snapshots for expired reset windows and fire notifications.
/// Accepts a custom notifier function for hermetic testability.
pub fn check_resets_with<F>(
    notif_path: &Path,
    snapshots: &[crate::account_store::AccountSnapshot],
    now: DateTime<Utc>,
    mut notify: F,
) -> usize
where
    F: FnMut(&str, &str),
{
    let mut store = load_notification_store_from(notif_path);
    let mut notified_count = 0;

    for snap in snapshots {
        for entry in &snap.entries {
            for section in &entry.sections {
                if let ReportSection::Metric {
                    label,
                    reset_at: Some(reset_at),
                    percent,
                    ..
                } = section
                {
                    // Only notify if now >= reset_at and there was some recorded usage
                    if now >= *reset_at && *percent > 0 {
                        let key = format!(
                            "{}:{}:{}:{}",
                            snap.account_label,
                            entry.id,
                            label,
                            reset_at.to_rfc3339()
                        );

                        if let std::collections::btree_map::Entry::Vacant(e) = store.sent.entry(key)
                        {
                            let summary =
                                format!("AI Usage Bar — Cota Renovada ({})", entry.display_name);
                            let user_display = snap
                                .user
                                .as_deref()
                                .map(|u| format!(" ({u})"))
                                .unwrap_or_default();
                            let body = format!(
                                "A conta '{}{}' ({}) renovou a janela de '{}' e já pode ser utilizada!",
                                snap.account_label, user_display, entry.display_name, label
                            );

                            notify(&summary, &body);
                            e.insert(now);
                            notified_count += 1;
                        }
                    }
                }
            }
        }
    }

    if notified_count > 0 {
        let _ = save_notification_store_to(notif_path, &store);
    }

    notified_count
}

/// Check all resets from the default snapshots store and send real desktop notifications.
pub fn check_and_notify_resets(notif_path: &Path, now: DateTime<Utc>) -> usize {
    let snaps = all_snapshots();
    check_resets_with(notif_path, &snaps, now, |summary, body| {
        println!(
            "[{}] NOTIFICATION: {summary} - {body}",
            Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        );
        if let Err(err) = send_desktop_notification(summary, body) {
            eprintln!("ai-usagebar monitor: failed to invoke notify-send: {err}");
        }
    })
}

/// Run the background monitor loop.
pub async fn run(
    interval_secs: u64,
    once: bool,
    test: bool,
    simulate_renewal: bool,
    clear_renewals: bool,
) -> i32 {
    if simulate_renewal {
        if let Err(e) = simulate_test_renewal() {
            eprintln!("ai-usagebar monitor: falha ao simular renovação: {e}");
            return 1;
        }
        println!("Aviso de renovação simulado com sucesso em test_renewals.json!");
        return 0;
    }
    if clear_renewals {
        if let Err(e) = clear_test_renewals() {
            eprintln!("ai-usagebar monitor: falha ao limpar avisos: {e}");
            return 1;
        }
        println!("Avisos de renovação simulados limpos com sucesso!");
        return 0;
    }
    if test {
        let summary = "AI Usage Bar — Cota Renovada (Google Antigravity)";
        let body = "A conta 'maximusdev58@gmail.com' (Google Antigravity) renovou a janela de 'Claude & GPT OSS' e já pode ser utilizada!";
        println!("Enviando notificação desktop de teste...");
        println!("  Título:   {summary}");
        println!("  Mensagem: {body}");
        if let Err(err) = send_desktop_notification(summary, body) {
            eprintln!("ai-usagebar monitor: falha ao enviar notificação: {err}");
            return 1;
        }
        println!("Notificação enviada com sucesso para o desktop!");
        return 0;
    }

    let interval = std::time::Duration::from_secs(interval_secs.max(5));
    let notif_path = match notifications_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ai-usagebar monitor: {e}");
            return 1;
        }
    };

    println!(
        "AI Usage Bar quota monitor started (check interval: {}s)...",
        interval.as_secs()
    );
    if let Ok(snaps_path) = snapshots_path() {
        println!("Monitoring snapshots from {}", snaps_path.display());
    }

    loop {
        let config = crate::config::Config::load().unwrap_or_default();
        if config.ui.notify_resets() {
            let count = check_and_notify_resets(&notif_path, Utc::now());
            if count > 0 {
                println!("  -> {count} quota renewal notification(s) sent.");
            }
        }
        if once {
            break;
        }
        tokio::select! {
            _ = tokio::time::sleep(interval) => {},
            _ = tokio::signal::ctrl_c() => {
                println!("\nAI Usage Bar monitor stopped.");
                break;
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account_store::{AccountSnapshot, ReportEntry, ReportSection};
    use tempfile::tempdir;

    #[test]
    fn check_resets_notifies_and_deduplicates() {
        let dir = tempdir().unwrap();
        let notif_path = dir.path().join("notifications.json");

        let past = Utc::now() - chrono::Duration::minutes(10);
        let snapshots = vec![AccountSnapshot {
            account_label: "personal".into(),
            user: Some("personal@dev".into()),
            saved_at: Utc::now(),
            providers: vec!["antigravity".into()],
            entries: vec![ReportEntry {
                id: "antigravity".into(),
                name: "Google Antigravity".into(),
                display_name: "Google Antigravity".into(),
                short_name: "AG".into(),
                icon: "󰧑".into(),
                brand: None,
                plan: None,
                sections: vec![ReportSection::Metric {
                    label: "5-Hour Limit".into(),
                    percent: 50,
                    value: "50%".into(),
                    detail: "resets in 0m".into(),
                    severity: "neutral".into(),
                    reset_at: Some(past),
                    window_secs: Some(5 * 3600),
                }],
                error: None,
                stale: false,
                fetched_at: None,
            }],
        }];

        let mut captured = Vec::new();
        let count1 = check_resets_with(&notif_path, &snapshots, Utc::now(), |s, b| {
            captured.push((s.to_string(), b.to_string()));
        });
        assert_eq!(count1, 1);
        assert_eq!(captured.len(), 1);
        assert!(captured[0].0.contains("Cota Renovada"));
        assert!(captured[0].1.contains("personal"));

        // Second pass at same or later time must NOT send duplicate notification
        let mut captured2 = Vec::new();
        let count2 = check_resets_with(&notif_path, &snapshots, Utc::now(), |s, b| {
            captured2.push((s.to_string(), b.to_string()));
        });
        assert_eq!(count2, 0);
        assert_eq!(captured2.len(), 0);
    }
}

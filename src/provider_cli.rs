//! CLI command handler for managing AI providers (`ai-usagebar provider [list|enable|disable]`).

use crate::config::{Config, read_config_document, resolved_path, set_bool, write_config_document};
use crate::vendor::VendorId;
use crate::widget::cli::ProviderAction;

/// Find a `VendorId` by slug, short name, or display name (case-insensitive, flexible spacing/hyphens/aliases).
pub fn find_vendor(input: &str) -> Option<VendorId> {
    let clean = input.trim().to_lowercase().replace(['-', '_', ' '], "");
    if clean.is_empty() {
        return None;
    }
    if clean.contains("antigravity") || clean == "agy" {
        return Some(VendorId::Antigravity);
    }
    VendorId::all().iter().copied().find(|v| {
        let v_slug = v.slug().replace(['-', '_'], "");
        let v_display = v.display_name().to_lowercase().replace(['-', '_', ' '], "");
        v.slug().eq_ignore_ascii_case(input.trim())
            || v.short_name().eq_ignore_ascii_case(input.trim())
            || v.display_name().eq_ignore_ascii_case(input.trim())
            || v_slug == clean
            || v_display == clean
            || clean.contains(&v_slug)
            || clean.contains(&v_display)
    })
}

/// Run a provider command and return its exit code.
pub fn run(action: &ProviderAction) -> i32 {
    match action {
        ProviderAction::List { json } => list(*json),
        ProviderAction::Enable { name } => toggle(name, true),
        ProviderAction::Disable { name } => toggle(name, false),
    }
}

fn list(json: bool) -> i32 {
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ai-usagebar: failed to load config: {e}");
            return 1;
        }
    };
    let statuses = crate::catalog::statuses(&cfg);
    if json {
        match serde_json::to_string_pretty(&statuses) {
            Ok(s) => {
                println!("{s}");
                0
            }
            Err(e) => {
                eprintln!("ai-usagebar: failed to serialize json: {e}");
                1
            }
        }
    } else {
        println!(
            "{:<24} {:<16} {:<10} CREDENTIALS",
            "PROVIDER", "SLUG", "STATUS"
        );
        println!(
            "{:<24} {:<16} {:<10} ───────────",
            "────────", "────", "──────"
        );
        for s in &statuses {
            let status = if s.enabled { "enabled" } else { "disabled" };
            let cred = if s.configured {
                if !s.needs_credential {
                    "ready (local)"
                } else {
                    "configured"
                }
            } else {
                "missing"
            };
            let cred_detail = if !s.configured && !s.env.is_empty() {
                format!("{cred} ({})", s.env)
            } else {
                cred.to_string()
            };
            println!("{:<24} {:<16} {:<10} {}", s.name, s.id, status, cred_detail);
        }
        println!();
        println!("Tip: Enable a provider with 'ai-usagebar provider enable <SLUG>'");
        println!("     Disable a provider with 'ai-usagebar provider disable <SLUG>'");
        0
    }
}

fn toggle(name: &str, enable: bool) -> i32 {
    let Some(vendor) = find_vendor(name) else {
        eprintln!("ai-usagebar: unknown provider '{name}'.");
        eprintln!("Run 'ai-usagebar provider list' to see all supported providers.");
        return 1;
    };

    let path = match resolved_path() {
        Some(p) => p,
        None => {
            eprintln!("ai-usagebar: could not resolve config path");
            return 1;
        }
    };

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let mut doc = match read_config_document(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("ai-usagebar: failed to read {}: {e}", path.display());
            return 1;
        }
    };

    if let Err(e) = set_bool(&mut doc, vendor.config_section(), "enabled", enable) {
        eprintln!("ai-usagebar: failed to update config document: {e}");
        return 1;
    }

    if let Some(accounts) = doc.get_mut("accounts").and_then(|a| a.as_array_of_tables_mut()) {
        let slug = vendor.slug();
        for acct in accounts.iter_mut() {
            if let Some(providers) = acct.get_mut("providers").and_then(|p| p.as_array_mut()) {
                if enable {
                    let already_has = providers.iter().any(|v| v.as_str() == Some(slug));
                    if !already_has {
                        providers.push(slug);
                    }
                } else {
                    let mut i = 0;
                    while i < providers.len() {
                        if providers.get(i).and_then(|v| v.as_str()) == Some(slug) {
                            providers.remove(i);
                        } else {
                            i += 1;
                        }
                    }
                }
            }
        }
    }

    if let Err(e) = write_config_document(&path, &doc) {
        eprintln!("ai-usagebar: failed to write {}: {e}", path.display());
        return 1;
    }

    crate::waybar::request_refresh();

    let action_str = if enable { "enabled" } else { "disabled" };
    println!(
        "✓ Provider '{}' ({}) {action_str} in {}",
        vendor.display_name(),
        vendor.slug(),
        path.display()
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_vendor_variations() {
        assert_eq!(find_vendor("antigravity"), Some(VendorId::Antigravity));
        assert_eq!(
            find_vendor("Google Antigravity"),
            Some(VendorId::Antigravity)
        );
        assert_eq!(find_vendor("agy"), Some(VendorId::Antigravity));
        assert_eq!(find_vendor("deepseek"), Some(VendorId::Deepseek));
        assert_eq!(find_vendor("dsk"), Some(VendorId::Deepseek));
        assert_eq!(find_vendor("kimi"), Some(VendorId::Kimi));
        assert_eq!(find_vendor("opencode_go"), Some(VendorId::OpenCodeGo));
        assert_eq!(find_vendor("opencode-go"), Some(VendorId::OpenCodeGo));
        assert_eq!(find_vendor("unknown_nonexistent"), None);
    }
}

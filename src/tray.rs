//! Tray icon, menu construction and UI updates.

#![cfg(windows)]

use chrono::{DateTime, Local, Utc};
use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::config::Config;
use crate::state::{format_tooltip, format_window_value, AppState, ProviderKind, ProviderStatus};

pub const MENU_ID_REFRESH: &str = "refresh";
pub const MENU_ID_SETTINGS: &str = "settings";
pub const MENU_ID_AUTOSTART: &str = "autostart";
pub const MENU_ID_QUIT: &str = "quit";
pub const MENU_ID_OPEN_PREFIX: &str = "open:";

pub struct TrayUi {
    tray: TrayIcon,
}

fn status_text(status: &ProviderStatus) -> Option<String> {
    Some(match status {
        ProviderStatus::NotConfigured(reason) => format!("not configured: {reason}"),
        ProviderStatus::Loading => "loading".to_string(),
        ProviderStatus::Ready => return None,
        ProviderStatus::Stale(reason) => format!("stale: {reason}"),
        ProviderStatus::SignInRequired(reason) => format!("sign-in required: {reason}"),
        ProviderStatus::RateLimited(reason) => format!("rate limited: {reason}"),
        ProviderStatus::Unsupported(reason) => format!("unavailable: {reason}"),
        ProviderStatus::Error(reason) => format!("error: {reason}"),
    })
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max_chars.saturating_sub(3)).collect();
        format!("{cut}...")
    }
}

fn local_time(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local)
        .format("%a %d %b %Y %H:%M")
        .to_string()
}

/// Build the context menu from current state. Rebuilt on every update; the
/// item count varies with the available windows.
fn build_menu(state: &AppState, autostart: bool) -> Menu {
    let menu = Menu::new();

    for kind in ProviderKind::ALL {
        let Some(snap) = state.snapshot(kind) else {
            continue;
        };
        for w in &snap.windows {
            let mut text = match format_window_value(w) {
                Some(v) => format!("{}: {}", w.label, v),
                None => format!("{}: no data", w.label),
            };
            if w.allowance_is_configured {
                text.push_str(" (configured total)");
            }
            if let Some(overage) = w.overage_units.filter(|o| *o > 0.0) {
                text.push_str(&format!(", {overage:.0} overage"));
            }
            if let Some(r) = w.resets_at {
                text.push_str(&format!(", resets {}", local_time(r)));
            }
            if matches!(snap.status, ProviderStatus::Stale(_)) {
                text.push_str(" [stale]");
            }
            menu.append(&MenuItem::new(truncate(&text, 90), false, None))
                .ok();
        }
        if let Some(status) = status_text(&snap.status) {
            let text = format!("{}: {}", kind.display_name(), status);
            menu.append(&MenuItem::new(truncate(&text, 90), false, None))
                .ok();
        }
        if let Some(collected) = snap.collected_at {
            menu.append(&MenuItem::new(
                format!("{} updated {}", kind.display_name(), local_time(collected)),
                false,
                None,
            ))
            .ok();
        }
    }

    menu.append(&PredefinedMenuItem::separator()).ok();
    menu.append(&MenuItem::with_id(
        MENU_ID_REFRESH,
        "Refresh now",
        true,
        None,
    ))
    .ok();

    let open = Submenu::new("Open usage pages", true);
    for kind in ProviderKind::ALL {
        open.append(&MenuItem::with_id(
            format!("{}{}", MENU_ID_OPEN_PREFIX, kind.key()),
            kind.display_name(),
            true,
            None,
        ))
        .ok();
    }
    menu.append(&open).ok();

    menu.append(&MenuItem::with_id(MENU_ID_SETTINGS, "Settings", true, None))
        .ok();
    menu.append(&CheckMenuItem::with_id(
        MENU_ID_AUTOSTART,
        "Start with Windows",
        true,
        autostart,
        None,
    ))
    .ok();
    menu.append(&PredefinedMenuItem::separator()).ok();
    menu.append(&MenuItem::with_id(MENU_ID_QUIT, "Quit", true, None))
        .ok();
    menu
}

impl TrayUi {
    pub fn new(state: &AppState, config: &Config, autostart: bool) -> Result<Self, String> {
        let icon = Icon::from_resource(1, Some((32, 32)))
            .map_err(|e| format!("could not load tray icon resource: {e}"))?;
        let tooltip = format_tooltip(state, &config.tooltip.windows, Utc::now());
        let tray = TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip(&tooltip)
            .with_menu(Box::new(build_menu(state, autostart)))
            .build()
            .map_err(|e| format!("could not create tray icon: {e}"))?;
        Ok(TrayUi { tray })
    }

    pub fn update(&self, state: &AppState, config: &Config, autostart: bool) {
        let tooltip = format_tooltip(state, &config.tooltip.windows, Utc::now());
        let _ = self.tray.set_tooltip(Some(tooltip));
        self.tray
            .set_menu(Some(Box::new(build_menu(state, autostart))));
    }
}

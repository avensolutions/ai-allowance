//! Tray icon, menu construction and UI updates.

#![cfg(windows)]

use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Local, Utc};
use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    PostMessageW, WM_APP, WM_ENTERMENULOOP, WM_EXITMENULOOP,
};

use crate::config::Config;
use crate::state::{format_window_value, tooltip_lines, AppState, ProviderKind, ProviderStatus};

pub const MENU_ID_REFRESH: &str = "refresh";
pub const MENU_ID_SETTINGS: &str = "settings";
pub const MENU_ID_AUTOSTART: &str = "autostart";
pub const MENU_ID_ABOUT: &str = "about";
pub const MENU_ID_QUIT: &str = "quit";
pub const MENU_ID_OPEN_PREFIX: &str = "open:";

/// True while the context menu is open. Replacing the menu then would drop
/// the open popup's items, and muda would discard the click on any of them,
/// Quit included. Fetch results and countdown ticks arrive while it is open.
static MENU_OPEN: AtomicBool = AtomicBool::new(false);
const MENU_WATCH_ID: usize = 1;
/// Posted when the menu loop ends, so it is queued behind the WM_COMMAND for
/// the chosen item and the open menu is kept until that command is handled.
const WM_MENU_CLOSED: u32 = WM_APP + 0x10;

pub struct TrayUi {
    tooltip: crate::tooltip::Tooltip,
    tray: TrayIcon,
    /// The latest menu, held back while the current one is open.
    pending_menu: Option<Menu>,
}

/// Subclass procedure on tray-icon's hidden window, which owns the menu loop.
unsafe extern "system" fn menu_watch_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    match msg {
        WM_ENTERMENULOOP => MENU_OPEN.store(true, Ordering::SeqCst),
        WM_EXITMENULOOP => {
            if PostMessageW(hwnd, WM_MENU_CLOSED, 0, 0) == 0 {
                MENU_OPEN.store(false, Ordering::SeqCst);
            }
        }
        WM_MENU_CLOSED => {
            MENU_OPEN.store(false, Ordering::SeqCst);
            return 0;
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

pub(crate) fn status_text(status: &ProviderStatus) -> Option<String> {
    Some(match status {
        ProviderStatus::NotConfigured(reason) => format!("not configured: {reason}"),
        ProviderStatus::Loading => "loading".to_string(),
        ProviderStatus::Ready => return None,
        ProviderStatus::Stale(reason) => format!("refresh failed: {reason}"),
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
fn build_menu(state: &AppState, autostart: Option<bool>) -> Menu {
    let menu = Menu::new();
    let now = Utc::now();

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
            if snap.is_stale(now) {
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
        autostart.is_some(),
        autostart.unwrap_or(false),
        None,
    ))
    .ok();
    menu.append(&MenuItem::with_id(
        MENU_ID_ABOUT,
        "About AI Allowance",
        true,
        None,
    ))
    .ok();
    menu.append(&PredefinedMenuItem::separator()).ok();
    menu.append(&MenuItem::with_id(MENU_ID_QUIT, "Quit", true, None))
        .ok();
    menu
}

impl TrayUi {
    pub fn new(state: &AppState, config: &Config, autostart: Option<bool>) -> Result<Self, String> {
        let icon = Icon::from_resource(1, Some((32, 32)))
            .map_err(|e| format!("could not load tray icon resource: {e}"))?;
        let tooltip = tooltip_lines(state, &config.tooltip.windows, Utc::now());
        let tray = TrayIconBuilder::new()
            .with_icon(icon)
            .with_menu(Box::new(build_menu(state, autostart)))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|e| format!("could not create tray icon: {e}"))?;
        let mut tooltip = crate::tooltip::Tooltip::new(tray.window_handle(), tooltip)?;
        tooltip.follow_shell_hover(tray.window_handle());
        let watched = unsafe {
            SetWindowSubclass(
                tray.window_handle(),
                Some(menu_watch_proc),
                MENU_WATCH_ID,
                0,
            )
        };
        if watched == 0 {
            crate::eprint_line("AI Allowance: could not watch the tray menu; menu clicks may be lost while it updates");
        }
        Ok(TrayUi {
            tray,
            tooltip,
            pending_menu: None,
        })
    }

    pub fn update(&mut self, state: &AppState, config: &Config, autostart: Option<bool>) {
        let tooltip = tooltip_lines(state, &config.tooltip.windows, Utc::now());
        self.tooltip.update(tooltip);
        self.pending_menu = Some(build_menu(state, autostart));
        self.apply_pending_menu();
    }

    fn apply_pending_menu(&mut self) {
        if !MENU_OPEN.load(Ordering::SeqCst) {
            if let Some(menu) = self.pending_menu.take() {
                self.tray.set_menu(Some(Box::new(menu)));
            }
        }
    }

    pub fn handle_event(&mut self, event: &tray_icon::TrayIconEvent) {
        // Hovering precedes every right-click, so a menu held back while the
        // previous one was open is in place before the next one opens.
        self.apply_pending_menu();
        match event {
            // The shell's hover notifications open and close the popup.
            tray_icon::TrayIconEvent::Enter { .. } | tray_icon::TrayIconEvent::Move { .. }
                if self.tooltip.follows_shell_hover() => {}
            tray_icon::TrayIconEvent::Enter { position, .. }
            | tray_icon::TrayIconEvent::Move { position, .. } => {
                self.tooltip.show(position.x as i32, position.y as i32)
            }
            _ => self.tooltip.hide(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    #[test]
    fn menu_counts_as_open_until_the_chosen_command_is_handled() {
        unsafe {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!hwnd.is_null());
            assert_ne!(
                SetWindowSubclass(hwnd, Some(menu_watch_proc), MENU_WATCH_ID, 0),
                0
            );
            SendMessageW(hwnd, WM_ENTERMENULOOP, 1, 0);
            assert!(MENU_OPEN.load(Ordering::SeqCst));
            // TrackPopupMenu posts the chosen item's WM_COMMAND as the loop ends.
            PostMessageW(hwnd, WM_COMMAND, 42, 0);
            SendMessageW(hwnd, WM_EXITMENULOOP, 1, 0);
            let mut msg = MSG::default();
            assert_ne!(PeekMessageW(&mut msg, hwnd, 0, 0, PM_REMOVE), 0);
            assert_eq!(msg.message, WM_COMMAND);
            DispatchMessageW(&msg);
            assert!(
                MENU_OPEN.load(Ordering::SeqCst),
                "the menu could be replaced before its command was handled"
            );
            assert_ne!(PeekMessageW(&mut msg, hwnd, 0, 0, PM_REMOVE), 0);
            DispatchMessageW(&msg);
            assert!(!MENU_OPEN.load(Ordering::SeqCst));
            DestroyWindow(hwnd);
        }
    }
}

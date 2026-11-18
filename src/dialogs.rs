//! Native, modeless settings and usage windows, owned by the tray UI thread.

#![cfg(windows)]

use std::cell::Cell;
use std::rc::Rc;

use chrono::{Local, Utc};
use tao::event_loop::EventLoopProxy;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::app::UserEvent;
use crate::config::{self, Config};
use crate::state::{format_countdown, format_window_value, AppState, ProviderKind};

const CLASS: &str = "AIAllowance.Dialog";
const SAVE: usize = 1;
const CANCEL: usize = 2;
const PAGE_GENERAL: usize = 10;
const PAGE_PROVIDERS: usize = 11;
const PAGE_BROWSER: usize = 12;
const POLL: usize = 100;
const INTERVAL: usize = 101;
const NOTIFY: usize = 102;
const TOOLTIP_AUTO: usize = 103;
const CLAUDE: usize = 110;
const HELPER: usize = 111;
const NODE: usize = 112;
const CODEX: usize = 113;
const COMMAND: usize = 114;
const COPILOT: usize = 115;
const GH_COMMAND: usize = 116;
const CHROME: usize = 120;
const CLAUDE_PROFILE: usize = 121;
const CODEX_PROFILE: usize = 122;
const COPILOT_PROFILE: usize = 123;
const TOOLTIP_ROW: usize = 200;
const BROWSE: usize = 1000;
const EDIT_HEIGHT: i32 = 32;
const LOGO_SIZE: i32 = 24;
const MAX_TOOLTIP_CHECKS: usize = 12;
const SS_CENTERIMAGE: u32 = 0x200;
const SETTINGS_HEIGHT: i32 = 640;
const SETTINGS_WIDTH: i32 = 760;

// Claude's settings dialog uses 14px body text, 12px captions and 15px
// semibold headings. Its anthropic-sans face is proprietary, so this uses
// Segoe UI, which is Claude's own Windows fallback.
const TEXT: COLORREF = rgb(31, 31, 30);
const MUTED: COLORREF = rgb(107, 107, 104);
const BAR: COLORREF = rgb(106, 171, 241);
const TRACK: COLORREF = rgb(228, 239, 251);
const INPUT_BORDER: COLORREF = rgb(214, 214, 210);
const INPUT_FOCUS: COLORREF = rgb(54, 125, 222);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Browse {
    Folder,
    Program,
    ChromeProfile,
}

fn browse_kind(id: usize) -> Option<Browse> {
    match id {
        HELPER => Some(Browse::Folder),
        NODE | COMMAND | GH_COMMAND | CHROME => Some(Browse::Program),
        CLAUDE_PROFILE | CODEX_PROFILE | COPILOT_PROFILE => Some(Browse::ChromeProfile),
        _ => None,
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn show_error(message: &str) {
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            wide(message).as_ptr(),
            wide("AI Allowance").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[derive(Default)]
pub struct Dialogs {
    settings: Rc<Cell<HWND>>,
    usage: Rc<Cell<HWND>>,
}

struct Control {
    hwnd: HWND,
    page: usize,
    y: i32,
    x: i32,
    width: i32,
    height: i32,
    color: COLORREF,
    background: COLORREF,
}

struct ProviderCard {
    kind: ProviderKind,
    y: i32,
    height: i32,
}

struct UsageBar {
    y: i32,
    left: f64,
}

/// A selectable tray tooltip row. `default` marks rows the automatic choice shows.
struct TooltipRow {
    id: String,
    label: String,
    default: bool,
    selected: bool,
}

/// The rounded frame painted around a borderless edit, in physical pixels.
struct FieldBox {
    page: usize,
    rect: RECT,
    edit: HWND,
}

struct WindowData {
    owner: Rc<Cell<HWND>>,
    config: Option<Config>,
    proxy: EventLoopProxy<UserEvent>,
    controls: Vec<Control>,
    font: HFONT,
    heading_font: HFONT,
    small_font: HFONT,
    line_height: i32,
    logos: [HICON; 3],
    cards: Vec<ProviderCard>,
    bars: Vec<UsageBar>,
    fields: Vec<FieldBox>,
    tooltip_rows: Vec<TooltipRow>,
    scale: f64,
    page: usize,
    scroll: i32,
    content_height: i32,
}

impl Drop for WindowData {
    fn drop(&mut self) {
        self.owner.set(std::ptr::null_mut());
        unsafe {
            DeleteObject(self.font);
            DeleteObject(self.heading_font);
            DeleteObject(self.small_font);
            for icon in self.logos {
                DestroyIcon(icon);
            }
        }
    }
}

impl Dialogs {
    /// Tao's message hook supplies native Tab, Shift+Tab and dialog-key handling.
    pub fn process_message(msg: *const std::ffi::c_void) -> bool {
        unsafe {
            let msg = msg.cast::<MSG>();
            let root = GetAncestor((*msg).hwnd, GA_ROOT);
            let mut class = [0u16; 64];
            let len = GetClassNameW(root, class.as_mut_ptr(), class.len() as i32);
            if len > 0 && String::from_utf16_lossy(&class[..len as usize]) == CLASS {
                return IsDialogMessageW(root, msg) != 0;
            }
        }
        false
    }

    pub fn settings(
        &self,
        cfg: &Config,
        state: &AppState,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Result<(), String> {
        if focus_existing(self.settings.get()) {
            return Ok(());
        }
        let hwnd = create_window(
            "AI Allowance Settings",
            self.settings.clone(),
            Some(cfg.clone()),
            proxy,
            SETTINGS_HEIGHT,
        )?;
        let result = unsafe {
            data(hwnd).tooltip_rows = tooltip_rows(state, &cfg.tooltip.windows);
            data(hwnd).build_settings(hwnd)
        };
        finish_build(hwnd, result)
    }

    pub fn usage(&self, state: &AppState, proxy: EventLoopProxy<UserEvent>) -> Result<(), String> {
        if focus_existing(self.usage.get()) {
            return Ok(());
        }
        let hwnd = create_window("AI Allowance Usage", self.usage.clone(), None, proxy, 700)?;
        let result = unsafe { data(hwnd).build_usage(hwnd, state) };
        finish_build(hwnd, result)
    }

    pub fn update_usage(&self, state: &AppState) {
        let hwnd = self.usage.get();
        if !hwnd.is_null() {
            let result = unsafe { data(hwnd).build_usage(hwnd, state) };
            if let Err(e) = result {
                show_error(&e);
                unsafe {
                    DestroyWindow(hwnd);
                }
            }
        }
    }

    pub fn close_all(&self) {
        for hwnd in [self.settings.get(), self.usage.get()] {
            if !hwnd.is_null() {
                unsafe {
                    DestroyWindow(hwnd);
                }
            }
        }
    }
}

impl Drop for Dialogs {
    fn drop(&mut self) {
        self.close_all();
    }
}

fn focus_existing(hwnd: HWND) -> bool {
    if hwnd.is_null() {
        return false;
    }
    unsafe {
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd);
    }
    true
}

fn finish_build(hwnd: HWND, result: Result<(), String>) -> Result<(), String> {
    if let Err(e) = result {
        unsafe {
            DestroyWindow(hwnd);
        }
        return Err(e);
    }
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        let first = GetNextDlgTabItem(hwnd, std::ptr::null_mut(), 0);
        SetFocus(if first.is_null() { hwnd } else { first });
    }
    Ok(())
}

fn create_window(
    title: &str,
    owner: Rc<Cell<HWND>>,
    config: Option<Config>,
    proxy: EventLoopProxy<UserEvent>,
    height: i32,
) -> Result<HWND, String> {
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let class = wide(CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hIcon: LoadImageW(
                instance,
                std::ptr::without_provenance(1),
                IMAGE_ICON,
                GetSystemMetrics(SM_CXICON),
                GetSystemMetrics(SM_CYICON),
                LR_SHARED,
            ) as HICON,
            hIconSm: LoadImageW(
                instance,
                std::ptr::without_provenance(1),
                IMAGE_ICON,
                GetSystemMetrics(SM_CXSMICON),
                GetSystemMetrics(SM_CYSMICON),
                LR_SHARED,
            ) as HICON,
            hbrBackground: GetStockObject(WHITE_BRUSH) as HBRUSH,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            return Err(format!(
                "Could not register native dialog: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut work = RECT::default();
        if SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut work as *mut RECT).cast(), 0) == 0 {
            return Err("Could not read the screen work area.".into());
        }
        let dc = GetDC(std::ptr::null_mut());
        let scale = if dc.is_null() {
            1.0
        } else {
            GetDeviceCaps(dc, LOGPIXELSX as i32) as f64 / 96.0
        };
        if !dc.is_null() {
            ReleaseDC(std::ptr::null_mut(), dc);
        }
        let scale = if config.is_some() {
            scale
                .min((work.bottom - work.top - 20) as f64 / height as f64)
                .min((work.right - work.left - 20) as f64 / SETTINGS_WIDTH as f64)
        } else {
            scale
        };
        let make_font = |size: f64, weight: i32| {
            CreateFontW(
                -(size * scale).round() as i32,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                0,
                0,
                CLEARTYPE_QUALITY as u32,
                0,
                wide("Segoe UI").as_ptr(),
            )
        };
        let font = make_font(14.0, FW_NORMAL as i32);
        let heading_font = make_font(15.0, FW_SEMIBOLD as i32);
        let small_font = make_font(12.0, FW_NORMAL as i32);
        // Load each logo at its exact drawn size so Windows picks the matching
        // image from the multi-size icon instead of stretching a 32px image.
        let logo = (LOGO_SIZE as f64 * scale).round() as i32;
        let logos = [2, 3, 4].map(|id| {
            LoadImageW(
                instance,
                std::ptr::without_provenance(id),
                IMAGE_ICON,
                logo,
                logo,
                LR_DEFAULTCOLOR,
            ) as HICON
        });
        if font.is_null()
            || heading_font.is_null()
            || small_font.is_null()
            || logos.iter().any(|icon| icon.is_null())
        {
            for f in [font, heading_font, small_font] {
                if !f.is_null() {
                    DeleteObject(f);
                }
            }
            for icon in logos {
                if !icon.is_null() {
                    DestroyIcon(icon);
                }
            }
            return Err("Could not load native dialog fonts or provider icons.".into());
        }
        let line_height = text_height(font);
        let settings = config.is_some();
        let boxed = Box::new(WindowData {
            owner,
            config,
            proxy,
            controls: Vec::new(),
            font,
            heading_font,
            small_font,
            line_height,
            logos,
            cards: Vec::new(),
            bars: Vec::new(),
            fields: Vec::new(),
            tooltip_rows: Vec::new(),
            scale,
            page: PAGE_GENERAL,
            scroll: 0,
            content_height: height,
        });
        let width = ((if settings {
            SETTINGS_WIDTH as f64
        } else {
            620.0
        }) * scale)
            .round() as i32;
        let height = ((height as f64 * scale).round() as i32).min(work.bottom - work.top - 20);
        let x = work.left + (work.right - work.left - width) / 2;
        let y = work.top + (work.bottom - work.top - height) / 2;
        let ptr = Box::into_raw(boxed);
        let hwnd = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            class.as_ptr(),
            wide(title).as_ptr(),
            WS_OVERLAPPED
                | WS_CLIPCHILDREN
                | WS_CAPTION
                | WS_SYSMENU
                | WS_MINIMIZEBOX
                | if settings { 0 } else { WS_VSCROLL },
            x,
            y,
            width,
            height,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            ptr.cast(),
        );
        // WM_NCCREATE transfers ownership to the window, including on failure.
        if hwnd.is_null() {
            return Err(format!(
                "Could not create native dialog: {}",
                std::io::Error::last_os_error()
            ));
        }
        data(hwnd).owner.set(hwnd);
        Ok(hwnd)
    }
}

unsafe fn data(hwnd: HWND) -> &'static mut WindowData {
    &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowData)
}

impl WindowData {
    fn px(&self, n: i32) -> i32 {
        (n as f64 * self.scale).round() as i32
    }

    #[allow(clippy::too_many_arguments)]
    fn control(
        &mut self,
        parent: HWND,
        class: &str,
        text: &str,
        id: usize,
        style: u32,
        page: usize,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Result<HWND, String> {
        unsafe {
            let hwnd = CreateWindowExW(
                0,
                wide(class).as_ptr(),
                wide(text).as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | style,
                self.px(x),
                self.px(y),
                self.px(width),
                self.px(height),
                parent,
                id as HMENU,
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return Err(format!(
                    "Could not create dialog control: {}",
                    std::io::Error::last_os_error()
                ));
            }
            SendMessageW(hwnd, WM_SETFONT, self.font as usize, 1);
            self.controls.push(Control {
                hwnd,
                page,
                x,
                y,
                width,
                height,
                color: TEXT,
                background: rgb(255, 255, 255),
            });
            Ok(hwnd)
        }
    }

    /// Muted 12px explanatory text.
    fn note(
        &mut self,
        hwnd: HWND,
        text: &str,
        page: usize,
        x: i32,
        y: i32,
        height: i32,
    ) -> Result<(), String> {
        let h = self.control(hwnd, "STATIC", text, 0, 0, page, x, y, 720 - x, height)?;
        unsafe {
            SendMessageW(h, WM_SETFONT, self.small_font as usize, 0);
        }
        self.controls.last_mut().expect("note").color = MUTED;
        Ok(())
    }

    fn heading(&mut self, hwnd: HWND, text: &str, page: usize, y: i32) -> Result<(), String> {
        let h = self.control(hwnd, "STATIC", text, 0, 0, page, 24, y, 696, 22)?;
        unsafe {
            SendMessageW(h, WM_SETFONT, self.heading_font as usize, 0);
        }
        Ok(())
    }

    /// A label, a rounded input box with a vertically centred borderless edit,
    /// and an Explorer Browse button for path fields.
    fn field(
        &mut self,
        hwnd: HWND,
        label: &str,
        text: &str,
        id: usize,
        page: usize,
        y: i32,
    ) -> Result<(), String> {
        self.control(
            hwnd,
            "STATIC",
            label,
            0,
            SS_CENTERIMAGE,
            page,
            24,
            y,
            228,
            EDIT_HEIGHT,
        )?;
        let browse = browse_kind(id).is_some();
        let box_width = if browse { 360 } else { 466 };
        let edit = self.control(
            hwnd,
            "EDIT",
            text,
            id,
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            page,
            264,
            y,
            box_width,
            EDIT_HEIGHT,
        )?;
        let rect = RECT {
            left: self.px(264),
            top: self.px(y),
            right: self.px(264 + box_width),
            bottom: self.px(y + EDIT_HEIGHT),
        };
        let pad = self.px(10);
        let edit_top = rect.top + (rect.bottom - rect.top - self.line_height) / 2;
        unsafe {
            MoveWindow(
                edit,
                rect.left + pad,
                edit_top,
                rect.right - rect.left - 2 * pad,
                self.line_height,
                0,
            );
            SendMessageW(
                edit,
                EM_SETMARGINS,
                (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
                0,
            );
        }
        self.fields.push(FieldBox { page, rect, edit });
        if browse {
            self.control(
                hwnd,
                "BUTTON",
                "Browse...",
                id + BROWSE,
                WS_TABSTOP,
                page,
                634,
                y,
                96,
                EDIT_HEIGHT,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn check(
        &mut self,
        hwnd: HWND,
        label: &str,
        checked: bool,
        id: usize,
        page: usize,
        x: i32,
        y: i32,
        width: i32,
    ) -> Result<HWND, String> {
        let h = self.control(
            hwnd,
            "BUTTON",
            label,
            id,
            WS_TABSTOP | BS_AUTOCHECKBOX as u32,
            page,
            x,
            y,
            width,
            28,
        )?;
        unsafe {
            SendMessageW(h, BM_SETCHECK, usize::from(checked), 0);
        }
        Ok(h)
    }

    fn build_settings(&mut self, hwnd: HWND) -> Result<(), String> {
        let c = self.config.clone().expect("settings config");
        for (index, (id, name)) in [
            (PAGE_GENERAL, "General"),
            (PAGE_PROVIDERS, "Providers"),
            (PAGE_BROWSER, "Browser profiles"),
        ]
        .into_iter()
        .enumerate()
        {
            self.control(
                hwnd,
                "BUTTON",
                name,
                id,
                WS_TABSTOP,
                0,
                24 + index as i32 * 180,
                16,
                170,
                EDIT_HEIGHT,
            )?;
        }

        self.check(
            hwnd,
            "Refresh in the background",
            c.general.poll_enabled,
            POLL,
            PAGE_GENERAL,
            24,
            68,
            696,
        )?;
        self.field(
            hwnd,
            "Refresh interval (seconds)",
            &c.general.poll_interval_secs.to_string(),
            INTERVAL,
            PAGE_GENERAL,
            102,
        )?;
        self.check(
            hwnd,
            "Notify at 20% and 10% left, and when an allowance resets",
            c.notifications.enabled,
            NOTIFY,
            PAGE_GENERAL,
            24,
            144,
            696,
        )?;
        self.note(
            hwnd,
            "Notifications need background refresh, otherwise data only updates when you hover over the tray icon.",
            PAGE_GENERAL,
            46,
            172,
            20,
        )?;
        self.heading(hwnd, "Tray tooltip", PAGE_GENERAL, 214)?;
        self.note(
            hwnd,
            "Choose the rows shown when you hover over the tray icon. Windows limits a tooltip to about four rows,\r\nso every row still appears in the usage window (double-click the icon) and the right-click menu.",
            PAGE_GENERAL,
            24,
            240,
            36,
        )?;
        let automatic = c.tooltip.windows.is_empty();
        self.check(
            hwnd,
            "Automatic: the first row for each provider, plus Claude Fable",
            automatic,
            TOOLTIP_AUTO,
            PAGE_GENERAL,
            24,
            284,
            696,
        )?;
        let mut y = 318;
        if self.tooltip_rows.is_empty() {
            self.note(
                hwnd,
                "Rows to choose from appear after the first successful refresh.",
                PAGE_GENERAL,
                46,
                y + 4,
                20,
            )?;
            y += 28;
        }
        let rows: Vec<_> = self
            .tooltip_rows
            .iter()
            .take(MAX_TOOLTIP_CHECKS)
            .map(|r| (r.label.clone(), r.default, r.selected))
            .collect();
        for (index, (label, default, selected)) in rows.into_iter().enumerate() {
            let column = (index % 2) as i32;
            let h = self.check(
                hwnd,
                &label,
                if automatic { default } else { selected },
                TOOLTIP_ROW + index,
                PAGE_GENERAL,
                46 + column * 340,
                y + (index / 2) as i32 * 30,
                330,
            )?;
            unsafe {
                EnableWindow(h, i32::from(!automatic));
            }
        }
        y += self.tooltip_rows.len().min(MAX_TOOLTIP_CHECKS).div_ceil(2) as i32 * 30;
        self.note(
            hwnd,
            &format!(
                "Settings file: {}",
                config::config_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            ),
            PAGE_GENERAL,
            24,
            y.max(500) + 8,
            36,
        )?;

        self.check(
            hwnd,
            "Claude (uses your existing Claude Code sign-in)",
            c.providers.claude.enabled,
            CLAUDE,
            PAGE_PROVIDERS,
            24,
            68,
            696,
        )?;
        self.field(
            hwnd,
            "Claude helper folder",
            &path_text(c.providers.claude.helper_dir.as_deref()),
            HELPER,
            PAGE_PROVIDERS,
            102,
        )?;
        self.field(
            hwnd,
            "Node executable",
            c.providers.claude.node_command.as_deref().unwrap_or(""),
            NODE,
            PAGE_PROVIDERS,
            142,
        )?;
        self.check(
            hwnd,
            "ChatGPT Work/Codex (uses your existing Codex sign-in)",
            c.providers.codex.enabled,
            CODEX,
            PAGE_PROVIDERS,
            24,
            196,
            696,
        )?;
        self.field(
            hwnd,
            "Codex executable",
            &path_text(c.providers.codex.command.as_deref()),
            COMMAND,
            PAGE_PROVIDERS,
            230,
        )?;
        self.check(
            hwnd,
            "GitHub Copilot (uses your existing GitHub CLI sign-in)",
            c.providers.copilot.enabled,
            COPILOT,
            PAGE_PROVIDERS,
            24,
            284,
            696,
        )?;
        self.field(
            hwnd,
            "GitHub CLI executable",
            &path_text(c.providers.copilot.command.as_deref()),
            GH_COMMAND,
            PAGE_PROVIDERS,
            318,
        )?;
        self.note(hwnd, "Leave a path blank to find the program on PATH or use the bundled helper.\r\nRun gh auth login if Copilot reports that sign-in is required. Copilot quota data is experimental.", PAGE_PROVIDERS, 24, 372, 36)?;

        self.note(hwnd, "Choose a Chrome profile per provider to open its usage page in that profile.\r\nLeave a profile blank to use your default browser.", PAGE_BROWSER, 24, 68, 36)?;
        self.field(
            hwnd,
            "Chrome executable",
            &path_text(c.browser.chrome_path.as_deref()),
            CHROME,
            PAGE_BROWSER,
            120,
        )?;
        self.field(
            hwnd,
            "Claude profile",
            c.browser.claude_profile.as_deref().unwrap_or(""),
            CLAUDE_PROFILE,
            PAGE_BROWSER,
            164,
        )?;
        self.field(
            hwnd,
            "ChatGPT profile",
            c.browser.codex_profile.as_deref().unwrap_or(""),
            CODEX_PROFILE,
            PAGE_BROWSER,
            208,
        )?;
        self.field(
            hwnd,
            "GitHub Copilot profile",
            c.browser.copilot_profile.as_deref().unwrap_or(""),
            COPILOT_PROFILE,
            PAGE_BROWSER,
            252,
        )?;
        self.note(hwnd, "Browse opens Chrome's User Data folder; pick the profile folder, such as Default or Profile 1.\r\nTo find it, open chrome://version in that profile and check the end of Profile Path.", PAGE_BROWSER, 24, 304, 36)?;
        self.control(
            hwnd,
            "BUTTON",
            "Save",
            SAVE,
            WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
            0,
            524,
            552,
            100,
            EDIT_HEIGHT,
        )?;
        self.control(
            hwnd,
            "BUTTON",
            "Cancel",
            CANCEL,
            WS_TABSTOP,
            0,
            632,
            552,
            98,
            EDIT_HEIGHT,
        )?;
        self.show_page(PAGE_GENERAL);
        Ok(())
    }

    /// Switch the tooltip row checkboxes between automatic and manual choice.
    fn set_tooltip_automatic(&mut self, hwnd: HWND, automatic: bool) {
        let count = self.tooltip_rows.len().min(MAX_TOOLTIP_CHECKS);
        unsafe {
            if automatic {
                for index in 0..count {
                    self.tooltip_rows[index].selected = self.checked(hwnd, TOOLTIP_ROW + index);
                }
            }
            let none_selected = !self.tooltip_rows.iter().any(|r| r.selected);
            for (index, row) in self.tooltip_rows.iter().enumerate().take(count) {
                let check = if automatic || none_selected {
                    row.default
                } else {
                    row.selected
                };
                let h = GetDlgItem(hwnd, (TOOLTIP_ROW + index) as i32);
                SendMessageW(h, BM_SETCHECK, usize::from(check), 0);
                EnableWindow(h, i32::from(!automatic));
            }
        }
    }

    fn show_page(&mut self, page: usize) {
        self.page = page;
        for c in &self.controls {
            unsafe {
                ShowWindow(
                    c.hwnd,
                    if c.page == 0 || c.page == page {
                        SW_SHOW
                    } else {
                        SW_HIDE
                    },
                );
            }
        }
        if let Some(c) = self.controls.first() {
            unsafe {
                RedrawWindow(
                    GetParent(c.hwnd),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
                );
            }
        }
    }

    fn text(&self, hwnd: HWND, id: usize) -> String {
        unsafe { window_text(GetDlgItem(hwnd, id as i32)) }
    }

    fn checked(&self, hwnd: HWND, id: usize) -> bool {
        unsafe { SendMessageW(GetDlgItem(hwnd, id as i32), BM_GETCHECK, 0, 0) == 1 }
    }

    fn read_settings(&self, hwnd: HWND) -> Result<Config, String> {
        let mut cfg = self.config.clone().expect("settings config");
        cfg.general.poll_enabled = self.checked(hwnd, POLL);
        cfg.general.poll_interval_secs = self
            .text(hwnd, INTERVAL)
            .parse()
            .map_err(|_| "Enter a whole number of seconds for the polling interval.")?;
        cfg.notifications.enabled = self.checked(hwnd, NOTIFY);
        cfg.tooltip.windows = if self.checked(hwnd, TOOLTIP_AUTO) {
            Vec::new()
        } else {
            let ids: Vec<String> = self
                .tooltip_rows
                .iter()
                .enumerate()
                .filter(|(index, row)| {
                    if *index < MAX_TOOLTIP_CHECKS {
                        self.checked(hwnd, TOOLTIP_ROW + index)
                    } else {
                        row.selected
                    }
                })
                .map(|(_, row)| row.id.clone())
                .collect();
            if ids.is_empty() {
                return Err("Choose at least one tooltip row, or select Automatic.".into());
            }
            ids
        };
        cfg.providers.claude.enabled = self.checked(hwnd, CLAUDE);
        cfg.providers.claude.helper_dir = optional(self.text(hwnd, HELPER)).map(Into::into);
        cfg.providers.claude.node_command = optional(self.text(hwnd, NODE));
        cfg.providers.codex.enabled = self.checked(hwnd, CODEX);
        cfg.providers.codex.command = optional(self.text(hwnd, COMMAND)).map(Into::into);
        cfg.providers.copilot.enabled = self.checked(hwnd, COPILOT);
        cfg.providers.copilot.command = optional(self.text(hwnd, GH_COMMAND)).map(Into::into);
        cfg.browser.chrome_path = optional(self.text(hwnd, CHROME)).map(Into::into);
        cfg.browser.claude_profile = optional(self.text(hwnd, CLAUDE_PROFILE));
        cfg.browser.codex_profile = optional(self.text(hwnd, CODEX_PROFILE));
        cfg.browser.copilot_profile = optional(self.text(hwnd, COPILOT_PROFILE));
        config::validate(&cfg)?;
        Ok(cfg)
    }

    fn save(&mut self, hwnd: HWND) -> Result<(), String> {
        let cfg = self.read_settings(hwnd)?;
        config::save(&cfg)?;
        self.proxy
            .send_event(UserEvent::SettingsSaved {
                config: Box::new(cfg),
            })
            .map_err(|_| "Settings saved, but the app is shutting down.".to_string())?;
        Ok(())
    }

    fn build_usage(&mut self, hwnd: HWND, state: &AppState) -> Result<(), String> {
        for c in self.controls.drain(..) {
            unsafe {
                DestroyWindow(c.hwnd);
            }
        }
        self.cards.clear();
        self.bars.clear();
        let now = Utc::now();
        let mut y = 18;
        for (kind, title) in [
            (ProviderKind::Claude, "Claude"),
            (ProviderKind::Codex, "ChatGPT"),
            (ProviderKind::Copilot, "GitHub Copilot"),
        ] {
            let top = y;
            let h = self.usage_text(hwnd, title, 68, y + 16, 480, 24, self.heading_font, false)?;
            unsafe {
                SetWindowLongPtrW(
                    h,
                    GWL_STYLE,
                    GetWindowLongPtrW(h, GWL_STYLE) | SS_CENTERIMAGE as isize,
                );
            }
            y += 52;
            if kind == ProviderKind::Codex {
                self.usage_text(
                    hwnd,
                    "Work/Codex allowance, not all ChatGPT features",
                    34,
                    y,
                    520,
                    18,
                    self.small_font,
                    true,
                )?;
                y += 26;
            }
            if let Some(snap) = state.snapshot(kind) {
                if let Some(status) = crate::tray::status_text(&snap.status) {
                    self.usage_text(hwnd, &status, 34, y, 520, 40, self.font, true)?;
                    y += 44;
                }
                for w in &snap.windows {
                    self.usage_text(hwnd, &w.label, 34, y, 360, 22, self.font, false)?;
                    let value = format_window_value(w).unwrap_or_else(|| "Unavailable".into());
                    let h = self.usage_text(hwnd, &value, 404, y, 150, 22, self.font, true)?;
                    unsafe {
                        SetWindowLongPtrW(h, GWL_STYLE, GetWindowLongPtrW(h, GWL_STYLE) | 2);
                    }
                    y += 26;
                    if let Some(left) = w.remaining_percent() {
                        self.bars.push(UsageBar { y, left });
                        y += 12;
                    } else {
                        self.usage_text(
                            hwnd,
                            "Allowance unavailable",
                            34,
                            y,
                            520,
                            18,
                            self.small_font,
                            true,
                        )?;
                        y += 18;
                    }
                    if let Some(reset) = w.resets_at {
                        self.usage_text(
                            hwnd,
                            &format!(
                                "Resets in {}  |  {}",
                                format_countdown(reset, now),
                                reset.with_timezone(&Local).format("%a %d %b, %H:%M")
                            ),
                            34,
                            y,
                            520,
                            18,
                            self.small_font,
                            true,
                        )?;
                        y += 22;
                    }
                    if let Some(overage) = w.overage_units.filter(|n| *n > 0.0) {
                        self.usage_text(
                            hwnd,
                            &format!("{overage:.1} overage"),
                            34,
                            y,
                            520,
                            18,
                            self.small_font,
                            true,
                        )?;
                        y += 22;
                    }
                    y += 6;
                }
                if let Some(at) = snap.collected_at {
                    self.usage_text(
                        hwnd,
                        &format!("Updated {}", at.with_timezone(&Local).format("%H:%M")),
                        34,
                        y,
                        520,
                        18,
                        self.small_font,
                        true,
                    )?;
                    y += 18;
                }
            } else {
                self.usage_text(hwnd, "Loading", 34, y, 520, 20, self.font, true)?;
                y += 24;
            }
            y += 14;
            self.cards.push(ProviderCard {
                kind,
                y: top,
                height: y - top,
            });
            y += 14;
        }
        self.usage_text(
            hwnd,
            "Bars show allowance left. Double-click the tray to reopen.",
            24,
            y,
            540,
            20,
            self.small_font,
            true,
        )?;
        self.controls.last_mut().expect("footer").background = rgb(248, 249, 251);
        self.content_height = y + 34;
        self.layout_scroll(hwnd);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn usage_text(
        &mut self,
        hwnd: HWND,
        text: &str,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        font: HFONT,
        muted: bool,
    ) -> Result<HWND, String> {
        let h = self.control(hwnd, "STATIC", text, 0, 0, 0, x, y, width, height)?;
        unsafe {
            SendMessageW(h, WM_SETFONT, font as usize, 0);
        }
        self.controls.last_mut().expect("new control").color = if muted { MUTED } else { TEXT };
        Ok(h)
    }

    unsafe fn paint(&self, hwnd: HWND, dc: HDC) {
        let mut rect = RECT::default();
        GetClientRect(hwnd, &mut rect);
        let background = CreateSolidBrush(if self.config.is_some() {
            rgb(255, 255, 255)
        } else {
            rgb(248, 249, 251)
        });
        FillRect(dc, &rect, background);
        DeleteObject(background);
        let focus = GetFocus();
        for field in self.fields.iter().filter(|f| f.page == self.page) {
            let r = field.rect;
            rounded_rect(
                dc,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                self.px(12),
                rgb(255, 255, 255),
                if focus == field.edit {
                    INPUT_FOCUS
                } else {
                    INPUT_BORDER
                },
            );
        }
        for card in &self.cards {
            let y = self.px(card.y) - self.scroll;
            rounded_rect(
                dc,
                self.px(18),
                y,
                self.px(558),
                self.px(card.height),
                self.px(12),
                rgb(255, 255, 255),
                rgb(226, 228, 232),
            );
            let index = match card.kind {
                ProviderKind::Claude => 0,
                ProviderKind::Codex => 1,
                ProviderKind::Copilot => 2,
            };
            DrawIconEx(
                dc,
                self.px(34),
                y + self.px(16),
                self.logos[index],
                self.px(LOGO_SIZE),
                self.px(LOGO_SIZE),
                0,
                std::ptr::null_mut(),
                DI_NORMAL,
            );
        }
        for bar in &self.bars {
            let y = self.px(bar.y) - self.scroll;
            let width = self.px(520);
            rounded_rect(
                dc,
                self.px(34),
                y,
                width,
                self.px(4),
                self.px(4),
                TRACK,
                TRACK,
            );
            let left = (width as f64 * bar.left / 100.0).round() as i32;
            if left > 0 {
                rounded_rect(dc, self.px(34), y, left, self.px(4), self.px(4), BAR, BAR);
            }
        }
    }

    fn layout_scroll(&mut self, hwnd: HWND) {
        unsafe {
            let mut rect = RECT::default();
            GetClientRect(hwnd, &mut rect);
            let max = (self.px(self.content_height) - rect.bottom).max(0);
            self.scroll = self.scroll.clamp(0, max);
            let info = SCROLLINFO {
                cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
                nMin: 0,
                nMax: self.px(self.content_height) - 1,
                nPage: rect.bottom.max(0) as u32,
                nPos: self.scroll,
                ..Default::default()
            };
            SetScrollInfo(hwnd, SB_VERT, &info, 1);
            for c in &self.controls {
                MoveWindow(
                    c.hwnd,
                    self.px(c.x),
                    self.px(c.y) - self.scroll,
                    self.px(c.width),
                    self.px(c.height),
                    1,
                );
            }
            RedrawWindow(
                hwnd,
                std::ptr::null(),
                std::ptr::null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
            );
        }
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
}

#[allow(clippy::too_many_arguments)]
unsafe fn rounded_rect(
    dc: HDC,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    radius: i32,
    fill: COLORREF,
    border: COLORREF,
) {
    let brush = CreateSolidBrush(fill);
    let pen = CreatePen(PS_SOLID, 1, border);
    let old_brush = SelectObject(dc, brush);
    let old_pen = SelectObject(dc, pen);
    RoundRect(dc, x, y, x + width, y + height, radius, radius);
    SelectObject(dc, old_pen);
    SelectObject(dc, old_brush);
    DeleteObject(pen);
    DeleteObject(brush);
}

fn path_text(path: Option<&std::path::Path>) -> String {
    path.map(|p| p.display().to_string()).unwrap_or_default()
}

fn optional(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn text_height(font: HFONT) -> i32 {
    unsafe {
        let dc = GetDC(std::ptr::null_mut());
        let old = SelectObject(dc, font);
        let mut metrics = TEXTMETRICW::default();
        GetTextMetricsW(dc, &mut metrics);
        SelectObject(dc, old);
        ReleaseDC(std::ptr::null_mut(), dc);
        metrics.tmHeight
    }
}

/// Every reported window, marked with the automatic default and the user's
/// saved choice. Saved ids that are not currently reported are kept.
fn tooltip_rows(state: &AppState, saved: &[String]) -> Vec<TooltipRow> {
    let defaults = crate::state::default_tooltip_ids(state);
    let mut rows: Vec<TooltipRow> = Vec::new();
    for kind in ProviderKind::ALL {
        if let Some(snap) = state.snapshot(kind) {
            for w in &snap.windows {
                rows.push(TooltipRow {
                    id: w.id.clone(),
                    label: w.label.clone(),
                    default: defaults.contains(&w.id),
                    selected: saved.contains(&w.id),
                });
            }
        }
    }
    for id in saved {
        if !rows.iter().any(|r| &r.id == id) {
            rows.push(TooltipRow {
                id: id.clone(),
                label: format!("{id} (not reported now)"),
                default: false,
                selected: true,
            });
        }
    }
    rows
}

unsafe fn window_text(hwnd: HWND) -> String {
    let len = GetWindowTextLengthW(hwnd);
    let mut buf = vec![0u16; len as usize + 1];
    let copied = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    String::from_utf16_lossy(&buf[..copied as usize])
        .trim()
        .to_string()
}

/// Fill a path field from a native Explorer file or folder dialog.
unsafe fn browse(hwnd: HWND, id: usize, kind: Browse) -> Result<(), String> {
    use crate::file_picker::{chrome_profile_name, chrome_user_data, pick, Pick};
    use std::path::{Path, PathBuf};
    let edit = GetDlgItem(hwnd, id as i32);
    let current = window_text(edit);
    let (mode, title, start): (Pick, &str, Option<PathBuf>) = match kind {
        Browse::Folder => (
            Pick::Folder,
            "Choose the Claude helper folder",
            Some(PathBuf::from(&current)),
        ),
        Browse::Program => (
            Pick::Program,
            "Choose a program",
            Path::new(&current).parent().map(Path::to_path_buf),
        ),
        Browse::ChromeProfile => (
            Pick::Folder,
            "Choose a Chrome profile folder",
            chrome_user_data(),
        ),
    };
    let Some(path) = pick(hwnd, mode, title, start.as_deref())? else {
        return Ok(());
    };
    let value = if kind == Browse::ChromeProfile {
        chrome_profile_name(&path)?
    } else {
        path.display().to_string()
    };
    SetWindowTextW(edit, wide(&value).as_ptr());
    SetFocus(edit);
    Ok(())
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lparam as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowData;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    match msg {
        WM_ERASEBKGND | WM_PRINTCLIENT => {
            (*ptr).paint(hwnd, wparam as HDC);
            1
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut paint);
            (*ptr).paint(hwnd, dc);
            EndPaint(hwnd, &paint);
            0
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let dc = wparam as HDC;
            let control = (*ptr).controls.iter().find(|c| c.hwnd == lparam as HWND);
            let color = control.map(|c| c.color).unwrap_or(TEXT);
            let background = control.map(|c| c.background).unwrap_or(rgb(255, 255, 255));
            SetTextColor(dc, color);
            SetBkColor(dc, background);
            SetBkMode(dc, OPAQUE as i32);
            SetDCBrushColor(dc, background);
            GetStockObject(DC_BRUSH) as LRESULT
        }
        WM_CTLCOLOREDIT => {
            let dc = wparam as HDC;
            SetTextColor(dc, TEXT);
            SetBkColor(dc, rgb(255, 255, 255));
            SetDCBrushColor(dc, rgb(255, 255, 255));
            GetStockObject(DC_BRUSH) as LRESULT
        }
        WM_LBUTTONDOWN if (*ptr).config.is_some() => {
            let point = POINT {
                x: (lparam & 0xffff) as i16 as i32,
                y: ((lparam >> 16) & 0xffff) as i16 as i32,
            };
            let d = &*ptr;
            if let Some(field) = d
                .fields
                .iter()
                .find(|f| f.page == d.page && PtInRect(&f.rect, point) != 0)
            {
                SetFocus(field.edit);
            }
            0
        }
        WM_COMMAND => {
            let id = wparam & 0xffff;
            let code = ((wparam >> 16) & 0xffff) as u32;
            if code == EN_SETFOCUS || code == EN_KILLFOCUS {
                if let Some(field) = (*ptr).fields.iter().find(|f| f.edit == lparam as HWND) {
                    InvalidateRect(hwnd, &field.rect, 0);
                }
                return 0;
            }
            match id {
                CANCEL => {
                    DestroyWindow(hwnd);
                }
                SAVE if (*ptr).config.is_some() => match (*ptr).save(hwnd) {
                    Ok(()) => {
                        DestroyWindow(hwnd);
                    }
                    Err(e) => {
                        show_error(&e);
                    }
                },
                PAGE_GENERAL | PAGE_PROVIDERS | PAGE_BROWSER => {
                    (*ptr).show_page(id);
                }
                TOOLTIP_AUTO => {
                    let automatic = (*ptr).checked(hwnd, TOOLTIP_AUTO);
                    (*ptr).set_tooltip_automatic(hwnd, automatic);
                }
                _ if id > BROWSE => {
                    if let Some(kind) = browse_kind(id - BROWSE) {
                        // Free function: the modal dialog may re-enter this
                        // window procedure, so hold no WindowData borrow.
                        if let Err(e) = browse(hwnd, id - BROWSE, kind) {
                            show_error(&e);
                        }
                    }
                }
                _ => {}
            }
            0
        }
        WM_VSCROLL if (*ptr).config.is_none() => {
            let d = &mut *ptr;
            match (wparam & 0xffff) as i32 {
                SB_LINEUP => d.scroll -= d.px(28),
                SB_LINEDOWN => d.scroll += d.px(28),
                SB_PAGEUP => d.scroll -= d.px(250),
                SB_PAGEDOWN => d.scroll += d.px(250),
                SB_TOP => d.scroll = 0,
                SB_BOTTOM => d.scroll = i32::MAX,
                SB_THUMBTRACK | SB_THUMBPOSITION => {
                    let mut info = SCROLLINFO {
                        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                        fMask: SIF_TRACKPOS,
                        ..Default::default()
                    };
                    GetScrollInfo(hwnd, SB_VERT, &mut info);
                    d.scroll = info.nTrackPos;
                }
                _ => {}
            }
            d.layout_scroll(hwnd);
            0
        }
        WM_MOUSEWHEEL if (*ptr).config.is_none() => {
            (*ptr).scroll -= (*ptr).px((wparam >> 16) as i16 as i32 / 120 * 70);
            (*ptr).layout_scroll(hwnd);
            0
        }
        WM_KEYDOWN if (*ptr).config.is_none() => {
            let command = match wparam as u16 {
                VK_UP => Some(SB_LINEUP),
                VK_DOWN => Some(SB_LINEDOWN),
                VK_PRIOR => Some(SB_PAGEUP),
                VK_NEXT => Some(SB_PAGEDOWN),
                VK_HOME => Some(SB_TOP),
                VK_END => Some(SB_BOTTOM),
                _ => None,
            };
            if let Some(command) = command {
                SendMessageW(hwnd, WM_VSCROLL, command as usize, 0);
                0
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(ptr));
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub fn initialize() -> Result<(), String> {
    let controls = INITCOMMONCONTROLSEX {
        dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_PROGRESS_CLASS,
    };
    if unsafe { InitCommonControlsEx(&controls) } == 0 {
        Err("Could not initialize native progress bars.".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{FetchOutcome, QuotaWindow};
    use tao::event_loop::EventLoopBuilder;
    use tao::platform::windows::EventLoopBuilderExtWindows;

    #[test]
    fn native_settings_and_usage_controls_preserve_values_and_missing_data() {
        initialize().unwrap();
        let event_loop = EventLoopBuilder::<UserEvent>::with_user_event()
            .with_any_thread(true)
            .build();
        let dialogs = Dialogs::default();
        let mut cfg = Config::default();
        cfg.browser.claude_profile = Some("Profile 1".into());
        let settings = create_window(
            "Settings test",
            dialogs.settings.clone(),
            Some(cfg.clone()),
            event_loop.create_proxy(),
            SETTINGS_HEIGHT,
        )
        .unwrap();
        unsafe {
            data(settings).build_settings(settings).unwrap();
            let d = data(settings);
            assert!(!d.fields.is_empty());
            for field in &d.fields {
                let mut edit = RECT::default();
                GetWindowRect(field.edit, &mut edit);
                let mut origin = POINT::default();
                ClientToScreen(settings, &mut origin);
                let edit_centre = (edit.top + edit.bottom) / 2 - origin.y;
                let box_centre = (field.rect.top + field.rect.bottom) / 2;
                assert!((edit_centre - box_centre).abs() <= 1, "edit not centred");
                let id = GetDlgCtrlID(field.edit) as usize;
                assert_eq!(
                    !GetDlgItem(settings, (id + BROWSE) as i32).is_null(),
                    browse_kind(id).is_some()
                );
            }
            let mut title = [0u16; 64];
            let len = GetWindowTextW(settings, title.as_mut_ptr(), title.len() as i32);
            assert_eq!(
                String::from_utf16_lossy(&title[..len as usize]),
                "Settings test"
            );
            assert_eq!(data(settings).read_settings(settings).unwrap(), cfg);
            SendMessageW(settings, WM_COMMAND, PAGE_PROVIDERS, 0);
            assert_eq!(data(settings).page, PAGE_PROVIDERS);
            SendMessageW(GetDlgItem(settings, COPILOT as i32), BM_SETCHECK, 0, 0);
            SetWindowTextW(
                GetDlgItem(settings, GH_COMMAND as i32),
                wide(r"C:\tools\gh.exe").as_ptr(),
            );
            let changed = data(settings).read_settings(settings).unwrap();
            assert!(!changed.providers.copilot.enabled);
            assert_eq!(
                changed.providers.copilot.command,
                Some(r"C:\tools\gh.exe".into())
            );
            assert_eq!(changed.browser.claude_profile.as_deref(), Some("Profile 1"));
            SetWindowTextW(GetDlgItem(settings, INTERVAL as i32), wide("0").as_ptr());
            assert!(data(settings).read_settings(settings).is_err());
            let mut client = RECT::default();
            GetClientRect(settings, &mut client);
            let d = data(settings);
            for c in &d.controls {
                assert!(
                    d.px(c.y + c.height) <= client.bottom,
                    "settings control below client area"
                );
            }
        }

        let mut state = AppState::default();
        let mut overall = QuotaWindow::new("claude:overall", "Claude");
        overall.used_percent = Some(25.0);
        let mut fable = QuotaWindow::new("claude:model:fable", "Claude Fable");
        fable.used_percent = Some(60.0);
        state.apply_success(
            ProviderKind::Claude,
            FetchOutcome {
                account_key: None,
                windows: vec![overall, fable],
            },
            Utc::now(),
        );
        let mut copilot = QuotaWindow::new("copilot:credits", "Copilot");
        copilot.used_units = Some(150.0);
        state.apply_success(
            ProviderKind::Copilot,
            FetchOutcome {
                account_key: None,
                windows: vec![copilot],
            },
            Utc::now(),
        );
        let usage = create_window(
            "Usage test",
            dialogs.usage.clone(),
            None,
            event_loop.create_proxy(),
            640,
        )
        .unwrap();
        unsafe {
            data(usage).build_usage(usage, &state).unwrap();
            let positions: Vec<_> = data(usage).bars.iter().map(|b| b.left).collect();
            let mut labels = Vec::new();
            for c in &data(usage).controls {
                let mut text = [0u16; 512];
                let len = GetWindowTextW(c.hwnd, text.as_mut_ptr(), text.len() as i32);
                labels.push(String::from_utf16_lossy(&text[..len as usize]));
            }
            assert_eq!(positions, [75.0, 40.0]);
            assert_eq!(data(usage).cards.len(), 3);
            for title in [
                "Claude",
                "ChatGPT",
                "GitHub Copilot",
                "150 used",
                "75% left",
                "40% left",
            ] {
                assert!(labels.iter().any(|label| label == title), "missing {title}");
            }
            assert!(labels.iter().any(|label| label == "Allowance unavailable"));
            let screen = GetDC(usage);
            let dc = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, 800, 800);
            ReleaseDC(usage, screen);
            assert!(!dc.is_null() && !bitmap.is_null());
            let old_bitmap = SelectObject(dc, bitmap);
            let old_font = SelectObject(dc, data(usage).heading_font);
            let mut face = [0u16; 64];
            let len = GetTextFaceW(dc, face.len() as i32, face.as_mut_ptr());
            assert!(len > 0);
            assert_eq!(
                String::from_utf16_lossy(&face[..len as usize - 1]),
                "Segoe UI"
            );
            SelectObject(dc, old_font);
            for _ in 0..2 {
                data(usage).build_usage(usage, &state).unwrap();
                SendMessageW(usage, WM_PRINTCLIENT, dc as usize, PRF_CLIENT as isize);
                assert_eq!(GetPixel(dc, 0, 0), rgb(248, 249, 251));
                let d = data(usage);
                assert_eq!(GetPixel(dc, d.px(40), d.px(d.bars[0].y + 2)), BAR);
                let brush = SendMessageW(
                    usage,
                    WM_CTLCOLORSTATIC,
                    dc as usize,
                    d.controls[0].hwnd as isize,
                ) as HBRUSH;
                let rect = RECT {
                    left: 0,
                    top: 0,
                    right: 20,
                    bottom: 20,
                };
                FillRect(dc, &rect, brush);
                assert_eq!(GetPixel(dc, 0, 0), rgb(255, 255, 255));
                assert_eq!(GetBkMode(dc), OPAQUE as i32);
            }
            SelectObject(dc, old_bitmap);
            DeleteObject(bitmap);
            DeleteDC(dc);
            let mut work = RECT::default();
            let mut rect = RECT::default();
            SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut work as *mut RECT).cast(), 0);
            GetWindowRect(usage, &mut rect);
            assert!(((rect.left + rect.right) - (work.left + work.right)).abs() <= 1);
            assert!(((rect.top + rect.bottom) - (work.top + work.bottom)).abs() <= 1);
            SendMessageW(usage, WM_VSCROLL, SB_BOTTOM as usize, 0);
            SendMessageW(usage, WM_COMMAND, CANCEL, 0);
            assert!(dialogs.usage.get().is_null());
            SendMessageW(settings, WM_COMMAND, CANCEL, 0);
            assert!(dialogs.settings.get().is_null());

            let tooltip = create_window(
                "Tooltip test",
                Rc::new(Cell::new(std::ptr::null_mut())),
                Some(Config::default()),
                event_loop.create_proxy(),
                SETTINGS_HEIGHT,
            )
            .unwrap();
            data(tooltip).tooltip_rows = tooltip_rows(&state, &[]);
            data(tooltip).build_settings(tooltip).unwrap();
            let row = |i: usize| GetDlgItem(tooltip, (TOOLTIP_ROW + i) as i32);
            assert!(data(tooltip).checked(tooltip, TOOLTIP_AUTO));
            assert_eq!(IsWindowEnabled(row(0)), 0);
            assert!(data(tooltip)
                .read_settings(tooltip)
                .unwrap()
                .tooltip
                .windows
                .is_empty());
            SendMessageW(GetDlgItem(tooltip, TOOLTIP_AUTO as i32), BM_SETCHECK, 0, 0);
            SendMessageW(tooltip, WM_COMMAND, TOOLTIP_AUTO, 0);
            assert_ne!(IsWindowEnabled(row(0)), 0);
            SendMessageW(row(0), BM_SETCHECK, 0, 0);
            SendMessageW(row(2), BM_SETCHECK, 0, 0);
            assert_eq!(
                data(tooltip)
                    .read_settings(tooltip)
                    .unwrap()
                    .tooltip
                    .windows,
                ["claude:model:fable"]
            );
            SendMessageW(row(1), BM_SETCHECK, 0, 0);
            assert!(data(tooltip).read_settings(tooltip).is_err());
            SendMessageW(tooltip, WM_COMMAND, CANCEL, 0);
        }
    }
}

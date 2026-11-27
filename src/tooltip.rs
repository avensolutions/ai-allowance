//! Tray hover popup. A custom-painted window replaces the shell tooltip so
//! each row can show a coloured traffic light; GDI text renders emoji
//! circles in monochrome.
//!
//! The popup follows the shell's own hover notifications. tray-icon registers
//! its icon with the legacy callback version, under which Windows 11 shows an
//! empty native tooltip box and reports no reliable hover end. `ShellHover`
//! subclasses tray-icon's hidden window, switches the icon to
//! `NOTIFYICON_VERSION_4` without `NIF_SHOWTIP` (no native tooltip), opens and
//! closes the popup on `NIN_POPUPOPEN`/`NIN_POPUPCLOSE`, and forwards every
//! other callback to tray-icon in the legacy format it expects.

#![cfg(windows)]

use std::cell::Cell;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconGetRect, Shell_NotifyIconW, NIM_SETVERSION, NIN_POPUPCLOSE, NIN_POPUPOPEN,
    NOTIFYICONDATAW, NOTIFYICONDATAW_0, NOTIFYICONIDENTIFIER, NOTIFYICON_VERSION_4,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::state::{Pace, TooltipLine};

const CLASS: &str = "AIAllowance.Tooltip";
const BACKGROUND: COLORREF = rgb(255, 255, 255);
const BORDER: COLORREF = rgb(204, 204, 204);
const TEXT: COLORREF = rgb(31, 31, 30);
const UNKNOWN: COLORREF = rgb(190, 190, 186);
// Supersampling factor for anti-aliased circles.
const SS: i32 = 4;
/// tray-icon's private notification callback message (`WM_USER_TRAYICON`).
const TRAY_CALLBACK: u32 = 6002;
const WM_POPUP_OPEN: u32 = WM_APP + 1;
const WM_POPUP_CLOSE: u32 = WM_APP + 2;
const WATCH_TIMER: usize = 1;
const WATCH_MS: u32 = 200;

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn pace_color(pace: Option<Pace>) -> COLORREF {
    match pace {
        Some(Pace::Green) => rgb(34, 160, 76),
        Some(Pace::Yellow) => rgb(230, 170, 0),
        Some(Pace::Red) => rgb(214, 48, 49),
        None => UNKNOWN,
    }
}

struct TipData {
    lines: Vec<TooltipLine>,
    font: HFONT,
    scale: f64,
    visible: Cell<bool>,
    /// Set when the shell hover notifications drive the popup.
    icon: Cell<Option<(HWND, u32)>>,
}

impl TipData {
    fn px(&self, n: i32) -> i32 {
        (n as f64 * self.scale).round() as i32
    }

    fn dot(&self) -> i32 {
        self.px(10)
    }

    /// Line height and the full popup size in physical pixels.
    unsafe fn measure(&self) -> (i32, SIZE) {
        let dc = GetDC(std::ptr::null_mut());
        let old = SelectObject(dc, self.font);
        let mut line_height = 0;
        let mut width = 0;
        for line in &self.lines {
            let text: Vec<u16> = line.text.encode_utf16().collect();
            let mut size = SIZE::default();
            GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32, &mut size);
            width = width.max(size.cx);
            line_height = line_height.max(size.cy);
        }
        SelectObject(dc, old);
        ReleaseDC(std::ptr::null_mut(), dc);
        let line_height = line_height + self.px(4);
        let size = SIZE {
            cx: self.px(10) + self.dot() + self.px(8) + width + self.px(12),
            cy: self.px(6) * 2 + line_height * self.lines.len() as i32,
        };
        (line_height, size)
    }

    fn dot_centre(&self, line_height: i32, index: usize) -> (i32, i32) {
        (
            self.px(10) + self.dot() / 2,
            self.px(6) + line_height * index as i32 + line_height / 2,
        )
    }

    unsafe fn paint(&self, hwnd: HWND, dc: HDC) {
        let mut rect = RECT::default();
        GetClientRect(hwnd, &mut rect);
        let brush = CreateSolidBrush(BACKGROUND);
        FillRect(dc, &rect, brush);
        DeleteObject(brush);
        let pen = CreatePen(PS_SOLID, 1, BORDER);
        let old_pen = SelectObject(dc, pen);
        let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
        Rectangle(dc, 0, 0, rect.right, rect.bottom);
        SelectObject(dc, old_brush);
        SelectObject(dc, old_pen);
        DeleteObject(pen);

        let (line_height, _) = self.measure();
        let old_font = SelectObject(dc, self.font);
        SetTextColor(dc, TEXT);
        SetBkMode(dc, TRANSPARENT as i32);
        for (index, line) in self.lines.iter().enumerate() {
            let (cx, cy) = self.dot_centre(line_height, index);
            draw_pace_dot(dc, cx, cy, self.dot(), line.pace, BACKGROUND);
            let mut text: Vec<u16> = line.text.encode_utf16().collect();
            let top = self.px(6) + line_height * index as i32;
            let mut r = RECT {
                left: cx + self.dot() / 2 + self.px(8),
                top,
                right: rect.right,
                bottom: top + line_height,
            };
            DrawTextW(
                dc,
                text.as_mut_ptr(),
                text.len() as i32,
                &mut r,
                DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
            );
        }
        SelectObject(dc, old_font);
    }
}

impl Drop for TipData {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.font);
        }
    }
}

/// Draw the traffic light for `pace`: a filled circle, or a hollow grey one
/// when pace is unknown. `background` must match the surface behind it.
pub(crate) unsafe fn draw_pace_dot(
    dc: HDC,
    cx: i32,
    cy: i32,
    size: i32,
    pace: Option<Pace>,
    background: COLORREF,
) {
    draw_dot(
        dc,
        cx,
        cy,
        size,
        pace_color(pace),
        pace.is_some(),
        background,
    );
}

/// Draw an anti-aliased circle by rendering it larger and halftoning down.
unsafe fn draw_dot(
    dc: HDC,
    cx: i32,
    cy: i32,
    size: i32,
    color: COLORREF,
    filled: bool,
    background: COLORREF,
) {
    let big = size * SS;
    let mem = CreateCompatibleDC(dc);
    let bitmap = CreateCompatibleBitmap(dc, big, big);
    let old_bitmap = SelectObject(mem, bitmap);
    let background = CreateSolidBrush(background);
    FillRect(
        mem,
        &RECT {
            left: 0,
            top: 0,
            right: big,
            bottom: big,
        },
        background,
    );
    DeleteObject(background);
    let pen = CreatePen(PS_SOLID, if filled { 1 } else { SS + SS / 2 }, color);
    let brush = if filled {
        CreateSolidBrush(color)
    } else {
        GetStockObject(NULL_BRUSH) as HBRUSH
    };
    let old_pen = SelectObject(mem, pen);
    let old_brush = SelectObject(mem, brush);
    let inset = if filled { 0 } else { SS };
    Ellipse(mem, inset, inset, big - inset, big - inset);
    SelectObject(mem, old_brush);
    SelectObject(mem, old_pen);
    DeleteObject(pen);
    if filled {
        DeleteObject(brush);
    }
    SetStretchBltMode(dc, HALFTONE);
    SetBrushOrgEx(dc, 0, 0, std::ptr::null_mut());
    StretchBlt(
        dc,
        cx - size / 2,
        cy - size / 2,
        size,
        size,
        mem,
        0,
        0,
        big,
        big,
        SRCCOPY,
    );
    SelectObject(mem, old_bitmap);
    DeleteObject(bitmap);
    DeleteDC(mem);
}

pub struct Tooltip {
    hwnd: HWND,
    data: Box<TipData>,
}

impl Tooltip {
    pub fn new(owner: HWND, lines: Vec<TooltipLine>) -> Result<Self, String> {
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let class = wide(CLASS);
            let wc = WNDCLASSW {
                style: CS_DROPSHADOW,
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return Err("Could not register the usage popup.".into());
            }
            let screen = GetDC(std::ptr::null_mut());
            let scale = if screen.is_null() {
                1.0
            } else {
                let dpi = GetDeviceCaps(screen, LOGPIXELSY as i32);
                ReleaseDC(std::ptr::null_mut(), screen);
                dpi as f64 / 96.0
            };
            let font = CreateFontW(
                -(12.0 * scale).round() as i32,
                0,
                0,
                0,
                FW_NORMAL as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                0,
                0,
                CLEARTYPE_QUALITY as u32,
                0,
                wide("Segoe UI").as_ptr(),
            );
            if font.is_null() {
                return Err("Could not create the usage popup font.".into());
            }
            let mut data = Box::new(TipData {
                lines,
                font,
                scale,
                visible: Cell::new(false),
                icon: Cell::new(None),
            });
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                owner,
                std::ptr::null_mut(),
                instance,
                (&mut *data as *mut TipData).cast(),
            );
            if hwnd.is_null() {
                return Err("Could not create the usage popup.".into());
            }
            Ok(Tooltip { hwnd, data })
        }
    }

    /// Let the shell's hover notifications for the tray icon owned by
    /// `tray_hwnd` open and close the popup. Returns false, leaving the
    /// tray-icon enter/leave events in charge, if the icon cannot be switched.
    pub fn follow_shell_hover(&mut self, tray_hwnd: HWND) -> bool {
        unsafe {
            let Some(id) = (0..1024).find(|&id| set_version_4(tray_hwnd, id)) else {
                return false;
            };
            ICON_ID.store(id, Ordering::SeqCst);
            POPUP.store(self.hwnd as isize, Ordering::SeqCst);
            TASKBAR_CREATED.store(
                RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
                Ordering::SeqCst,
            );
            let previous =
                SetWindowLongPtrW(tray_hwnd, GWLP_WNDPROC, shell_proc as *const () as isize);
            if previous == 0 {
                return false;
            }
            PREVIOUS_PROC.store(previous, Ordering::SeqCst);
            self.data.icon.set(Some((tray_hwnd, id)));
            true
        }
    }

    pub fn follows_shell_hover(&self) -> bool {
        self.data.icon.get().is_some()
    }

    pub fn update(&mut self, lines: Vec<TooltipLine>) {
        self.data.lines = lines;
        if self.data.visible.get() {
            unsafe {
                self.data.place(self.hwnd);
                InvalidateRect(self.hwnd, std::ptr::null(), 1);
            }
        }
    }

    /// Show near the given screen point.
    pub fn show(&mut self, x: i32, y: i32) {
        unsafe { self.data.show(self.hwnd, x, y) }
    }

    pub fn hide(&mut self) {
        unsafe { self.data.hide(self.hwnd) }
    }
}

static PREVIOUS_PROC: AtomicIsize = AtomicIsize::new(0);
static POPUP: AtomicIsize = AtomicIsize::new(0);
static ICON_ID: AtomicU32 = AtomicU32::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

unsafe fn set_version_4(hwnd: HWND, id: u32) -> bool {
    let nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: id,
        Anonymous: NOTIFYICONDATAW_0 {
            uVersion: NOTIFYICON_VERSION_4,
        },
        ..Default::default()
    };
    Shell_NotifyIconW(NIM_SETVERSION, &nid) != 0
}

unsafe fn icon_rect(hwnd: HWND, id: u32) -> Option<RECT> {
    let nid = NOTIFYICONIDENTIFIER {
        cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
        hWnd: hwnd,
        uID: id,
        ..Default::default()
    };
    let mut rect = RECT::default();
    (Shell_NotifyIconGetRect(&nid, &mut rect) >= 0).then_some(rect)
}

/// Replacement window procedure for tray-icon's hidden window.
unsafe extern "system" fn shell_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let previous: WNDPROC = std::mem::transmute(PREVIOUS_PROC.load(Ordering::SeqCst));
    let id = ICON_ID.load(Ordering::SeqCst);
    if msg == TRAY_CALLBACK {
        let event = (lparam as u32) & 0xffff;
        let popup = POPUP.load(Ordering::SeqCst) as HWND;
        match event {
            NIN_POPUPOPEN => {
                PostMessageW(popup, WM_POPUP_OPEN, 0, 0);
                return 0;
            }
            NIN_POPUPCLOSE => {
                PostMessageW(popup, WM_POPUP_CLOSE, 0, 0);
                return 0;
            }
            // Legacy format: icon id in wParam, mouse message in lParam.
            _ => return CallWindowProcW(previous, hwnd, msg, id as WPARAM, event as LPARAM),
        }
    }
    let result = CallWindowProcW(previous, hwnd, msg, wparam, lparam);
    // tray-icon re-adds the icon after an Explorer restart, which resets the
    // callback version.
    if msg != 0 && msg == TASKBAR_CREATED.load(Ordering::SeqCst) {
        set_version_4(hwnd, id);
    }
    result
}

impl TipData {
    unsafe fn place(&self, hwnd: HWND) {
        let mut cursor = POINT::default();
        GetCursorPos(&mut cursor);
        self.position(hwnd, cursor.x, cursor.y);
    }

    unsafe fn show(&self, hwnd: HWND, x: i32, y: i32) {
        self.position(hwnd, x, y);
        if !self.visible.replace(true) && self.icon.get().is_some() {
            SetTimer(hwnd, WATCH_TIMER, WATCH_MS, None);
        }
    }

    /// Place near the point, flipped above it and kept on its monitor.
    unsafe fn position(&self, hwnd: HWND, x: i32, y: i32) {
        let (_, size) = self.measure();
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(monitor, &mut info);
        let work = info.rcWork;
        let mut left = x + self.px(12);
        let mut top = y + self.px(22);
        if top + size.cy > work.bottom {
            top = y - size.cy - self.px(8);
        }
        left = left.min(work.right - size.cx).max(work.left);
        top = top.min(work.bottom - size.cy).max(work.top);
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            left,
            top,
            size.cx,
            size.cy,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }

    unsafe fn hide(&self, hwnd: HWND) {
        if self.visible.replace(false) {
            KillTimer(hwnd, WATCH_TIMER);
            ShowWindow(hwnd, SW_HIDE);
        }
    }

    /// Fallback for a missed NIN_POPUPCLOSE: hide once the cursor is no
    /// longer over the icon, or the icon is no longer on screen.
    unsafe fn watch(&self, hwnd: HWND) {
        let Some((tray, id)) = self.icon.get() else {
            return;
        };
        let mut cursor = POINT::default();
        let over = GetCursorPos(&mut cursor) != 0
            && icon_rect(tray, id).is_some_and(|r| {
                (r.left - 2..r.right + 2).contains(&cursor.x)
                    && (r.top - 2..r.bottom + 2).contains(&cursor.y)
            });
        if !over {
            self.hide(hwnd);
        }
    }
}

impl Drop for Tooltip {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
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
    let data = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TipData;
    if data.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    match msg {
        WM_ERASEBKGND => 1,
        WM_PRINTCLIENT => {
            (*data).paint(hwnd, wparam as HDC);
            0
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            (*data).paint(hwnd, dc);
            EndPaint(hwnd, &ps);
            0
        }
        WM_POPUP_OPEN => {
            let mut cursor = POINT::default();
            GetCursorPos(&mut cursor);
            (*data).show(hwnd, cursor.x, cursor.y);
            0
        }
        WM_POPUP_CLOSE => {
            (*data).hide(hwnd);
            0
        }
        WM_TIMER if wparam == WATCH_TIMER => {
            (*data).watch(hwnd);
            0
        }
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: COLORREF, b: COLORREF) -> bool {
        (0..3).all(|shift| {
            let x = ((a >> (shift * 8)) & 0xff) as i32;
            let y = ((b >> (shift * 8)) & 0xff) as i32;
            (x - y).abs() <= 24
        })
    }

    #[test]
    fn popup_paints_a_traffic_light_for_each_row() {
        let lines = vec![
            TooltipLine {
                pace: Some(Pace::Green),
                text: "Claude: 85% left | 5d".into(),
            },
            TooltipLine {
                pace: Some(Pace::Yellow),
                text: "Work/Codex: 32% left | 6d".into(),
            },
            TooltipLine {
                pace: Some(Pace::Red),
                text: "Copilot: 10% left | 12d".into(),
            },
            TooltipLine {
                pace: None,
                text: "Copilot Chat: 150 used".into(),
            },
        ];
        let mut tip = Tooltip::new(std::ptr::null_mut(), Vec::new()).unwrap();
        tip.update(lines.clone());
        tip.show(200, 200);
        unsafe {
            let (line_height, size) = tip.data.measure();
            let mut client = RECT::default();
            GetClientRect(tip.hwnd, &mut client);
            assert_eq!((client.right, client.bottom), (size.cx, size.cy));
            let screen = GetDC(tip.hwnd);
            let dc = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, size.cx, size.cy);
            ReleaseDC(tip.hwnd, screen);
            let old = SelectObject(dc, bitmap);
            SendMessageW(tip.hwnd, WM_PRINTCLIENT, dc as usize, PRF_CLIENT as isize);
            for (index, line) in lines.iter().enumerate().take(3) {
                let (x, y) = tip.data.dot_centre(line_height, index);
                let pixel = GetPixel(dc, x, y);
                assert!(
                    close(pixel, pace_color(line.pace)),
                    "row {index}: {pixel:06x}"
                );
            }
            let (x, y) = tip.data.dot_centre(line_height, 3);
            assert!(
                close(GetPixel(dc, x, y), BACKGROUND),
                "unknown pace is hollow"
            );
            SelectObject(dc, old);
            DeleteObject(bitmap);
            DeleteDC(dc);
        }
        tip.hide();
    }

    unsafe fn pump() {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    #[test]
    fn shell_hover_notifications_drive_the_popup_and_clicks_still_reach_tray_icon() {
        use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};

        let icon = tray_icon::Icon::from_rgba(vec![0x80; 16 * 16 * 4], 16, 16).unwrap();
        let tray = tray_icon::TrayIconBuilder::new()
            .with_icon(icon)
            .build()
            .unwrap();
        let tray_hwnd = tray.window_handle();
        let lines = vec![TooltipLine {
            pace: None,
            text: "Claude: 50% left | 3d".into(),
        }];
        let mut tip = Tooltip::new(tray_hwnd, lines).unwrap();
        assert!(tip.follow_shell_hover(tray_hwnd));
        let id = ICON_ID.load(Ordering::SeqCst);
        // Version 4 callback format: event in the low word, icon id in the high word.
        let notify = |event: u32| unsafe {
            SendMessageW(tray_hwnd, TRAY_CALLBACK, 0, ((id << 16) | event) as LPARAM);
            pump();
        };
        unsafe {
            notify(NIN_POPUPOPEN);
            assert_ne!(IsWindowVisible(tip.hwnd), 0, "popup opens on hover");
            notify(NIN_POPUPCLOSE);
            assert_eq!(IsWindowVisible(tip.hwnd), 0, "popup closes on hover end");

            if icon_rect(tray_hwnd, id).is_some() {
                while TrayIconEvent::receiver().try_recv().is_ok() {}
                notify(WM_LBUTTONUP);
                assert!(matches!(
                    TrayIconEvent::receiver().try_recv(),
                    Ok(TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    })
                ));
            }
        }
    }
}

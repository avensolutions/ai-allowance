//! Windows-specific integration: single instance, shell open, autostart,
//! console attachment for CLI usage and toast delivery.

#![cfg(windows)]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Acquire the single-instance mutex. Returns false when another instance
/// already owns it. The handle is intentionally leaked for process lifetime.
pub fn acquire_single_instance() -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name = wide("Local\\AIAllowance.SingleInstance");
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        // Could not create the mutex at all; do not block startup.
        return true;
    }
    unsafe { GetLastError() != ERROR_ALREADY_EXISTS }
}

/// Open a URL or file with the user's default handler, without a shell.
pub fn shell_open(target: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let op = wide("open");
    let t = wide(target);
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ptr(),
            t.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// Attach to the parent console so CLI flags can print when the binary is
/// built with the windows subsystem.
pub fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "AIAllowance";

pub fn autostart_enabled() -> bool {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    hkcu.open_subkey(RUN_KEY)
        .and_then(|k| k.get_value::<String, _>(RUN_VALUE))
        .is_ok()
}

pub fn set_autostart(enable: bool) -> Result<(), String> {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey(RUN_KEY)
        .map_err(|e| format!("could not open Run key: {e}"))?;
    if enable {
        let exe = std::env::current_exe().map_err(|e| format!("current exe: {e}"))?;
        let value = format!("\"{}\"", exe.display());
        key.set_value(RUN_VALUE, &value)
            .map_err(|e| format!("could not set Run value: {e}"))
    } else {
        match key.delete_value(RUN_VALUE) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("could not remove Run value: {e}")),
        }
    }
}

/// Deliver a toast notification. Failures are non-fatal.
pub fn show_toast(title: &str, body: &str) {
    let _ = notify_rust::Notification::new()
        .appname("AIAllowance")
        .summary(title)
        .body(body)
        .show();
}

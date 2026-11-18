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
pub fn shell_open(target: &str) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let op = wide("open");
    let t = wide(target);
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ptr(),
            t.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as isize <= 32 {
        Err(format!(
            "Could not open the requested page (Windows error {}).",
            result as isize
        ))
    } else {
        Ok(())
    }
}

pub fn open_usage(
    kind: crate::state::ProviderKind,
    cfg: &crate::config::Browser,
) -> Result<(), String> {
    use crate::state::ProviderKind;
    let profile = match kind {
        ProviderKind::Claude => &cfg.claude_profile,
        ProviderKind::Codex => &cfg.codex_profile,
        ProviderKind::Copilot => &cfg.copilot_profile,
    };
    let Some(profile) = profile else {
        return shell_open(kind.usage_url());
    };
    let chrome = cfg
        .chrome_path
        .clone()
        .or_else(|| {
            ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"]
                .into_iter()
                .filter_map(std::env::var_os)
                .map(|dir| {
                    std::path::PathBuf::from(dir).join(r"Google\Chrome\Application\chrome.exe")
                })
                .find(|path| path.is_file())
        })
        .ok_or("Chrome was not found. Set the Chrome executable in Settings.")?;
    std::process::Command::new(chrome)
        .arg(format!("--profile-directory={profile}"))
        .arg(kind.usage_url())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not start Chrome: {e}"))
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

pub fn autostart_enabled() -> Result<bool, String> {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    read_autostart(&hkcu, RUN_KEY)
        .map_err(|e| format!("Could not read the current user's Start with Windows setting: {e}"))
}

fn read_autostart(root: &winreg::RegKey, subkey: &str) -> std::io::Result<bool> {
    let result = root
        .open_subkey_with_flags(subkey, winreg::enums::KEY_QUERY_VALUE)
        .and_then(|key| key.get_value::<String, _>(RUN_VALUE));
    match result {
        Ok(value) if !value.is_empty() => Ok(true),
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "the startup command is empty",
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

pub fn set_autostart(enable: bool) -> Result<(), String> {
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let exe = if enable {
        Some(
            std::env::current_exe()
                .map_err(|e| format!("Could not locate this executable: {e}"))?,
        )
    } else {
        None
    };
    write_autostart(
        &hkcu,
        RUN_KEY,
        exe.as_deref().map(std::path::Path::as_os_str),
    )
    .map_err(|e| format!("Could not change the current user's Start with Windows setting: {e}"))
}

fn write_autostart(
    root: &winreg::RegKey,
    subkey: &str,
    executable: Option<&OsStr>,
) -> std::io::Result<()> {
    use winreg::enums::KEY_SET_VALUE;
    if let Some(executable) = executable {
        let (key, _) = root.create_subkey_with_flags(subkey, KEY_SET_VALUE)?;
        let mut command = std::ffi::OsString::from("\"");
        command.push(executable);
        command.push("\"");
        key.set_value(RUN_VALUE, &command)
    } else {
        let key = match root.open_subkey_with_flags(subkey, KEY_SET_VALUE) {
            Ok(key) => key,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        match key.delete_value(RUN_VALUE) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Deliver a toast notification. Failures are non-fatal.
pub fn show_toast(title: &str, body: &str) {
    let _ = notify_rust::Notification::new()
        .appname("AI Allowance")
        .summary(title)
        .body(body)
        .show();
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestKey(String);

    impl TestKey {
        fn new() -> Self {
            Self(format!(
                r"Software\AIAllowance\Tests\{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ))
        }

        fn root(&self) -> winreg::RegKey {
            winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        }
    }

    impl Drop for TestKey {
        fn drop(&mut self) {
            match self.root().delete_subkey_all(&self.0) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => panic!("could not clean up isolated test key: {e}"),
            }
        }
    }

    #[test]
    fn startup_round_trip_preserves_other_values_and_quotes_path() {
        let test = TestKey::new();
        let root = test.root();
        let (key, _) = root.create_subkey(&test.0).unwrap();
        key.set_value("UnrelatedApp", &"leave unchanged").unwrap();
        assert!(!read_autostart(&root, &test.0).unwrap());
        let exe = OsStr::new(r"C:\Users\Test User\AI Allowance\ai-allowance.exe");
        write_autostart(&root, &test.0, Some(exe)).unwrap();
        assert!(read_autostart(&root, &test.0).unwrap());
        assert_eq!(
            key.get_value::<String, _>(RUN_VALUE).unwrap(),
            r#""C:\Users\Test User\AI Allowance\ai-allowance.exe""#
        );
        write_autostart(&root, &test.0, None).unwrap();
        write_autostart(&root, &test.0, None).unwrap();
        assert!(!read_autostart(&root, &test.0).unwrap());
        assert_eq!(
            key.get_value::<String, _>("UnrelatedApp").unwrap(),
            "leave unchanged"
        );
        write_autostart(&root, &test.0, Some(exe)).unwrap();
        assert!(read_autostart(&root, &test.0).unwrap());
    }

    #[test]
    fn disabling_missing_startup_does_not_create_key() {
        let test = TestKey::new();
        let root = test.root();
        write_autostart(&root, &test.0, None).unwrap();
        assert!(!read_autostart(&root, &test.0).unwrap());
        assert_eq!(
            root.open_subkey(&test.0).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
    }

    #[test]
    fn malformed_startup_is_reported_instead_of_treated_as_disabled() {
        let test = TestKey::new();
        let root = test.root();
        let (key, _) = root.create_subkey(&test.0).unwrap();
        key.set_value(RUN_VALUE, &42_u32).unwrap();
        assert!(read_autostart(&root, &test.0).is_err());
        key.set_value(RUN_VALUE, &"").unwrap();
        assert!(read_autostart(&root, &test.0).is_err());
    }
}

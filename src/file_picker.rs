//! Native Windows Explorer file and folder picker (IFileOpenDialog).

use std::path::{Path, PathBuf};

use windows::core::{w, HSTRING};
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, FOS_FILEMUSTEXIST,
    FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Folder,
    Program,
}

/// Show a modal Explorer dialog. Returns `Ok(None)` when the user cancels.
pub fn pick(
    owner: *mut std::ffi::c_void,
    kind: Pick,
    title: &str,
    start: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    unsafe {
        // The UI thread is normally already an STA; S_FALSE still needs a
        // matching CoUninitialize, a mode change error does not.
        let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let result = show(owner, kind, title, start);
        if initialized.is_ok() {
            CoUninitialize();
        }
        result.map_err(|e| format!("Could not open the Explorer dialog: {}", e.message()))
    }
}

unsafe fn show(
    owner: *mut std::ffi::c_void,
    kind: Pick,
    title: &str,
    start: Option<&Path>,
) -> windows::core::Result<Option<PathBuf>> {
    let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
    let mut options = dialog.GetOptions()? | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST;
    options |= match kind {
        Pick::Folder => FOS_PICKFOLDERS,
        Pick::Program => FOS_FILEMUSTEXIST,
    };
    dialog.SetOptions(options)?;
    dialog.SetTitle(&HSTRING::from(title))?;
    if kind == Pick::Program {
        dialog.SetFileTypes(&[
            COMDLG_FILTERSPEC {
                pszName: w!("Programs (*.exe)"),
                pszSpec: w!("*.exe"),
            },
            COMDLG_FILTERSPEC {
                pszName: w!("All files (*.*)"),
                pszSpec: w!("*.*"),
            },
        ])?;
    }
    if let Some(folder) = start.filter(|p| p.is_dir()) {
        let item: windows::core::Result<IShellItem> =
            SHCreateItemFromParsingName(&HSTRING::from(folder.as_os_str()), None);
        if let Ok(item) = item {
            dialog.SetFolder(&item)?;
        }
    }
    if let Err(e) = dialog.Show(Some(HWND(owner))) {
        if e.code() == ERROR_CANCELLED.to_hresult() {
            return Ok(None);
        }
        return Err(e);
    }
    let name = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
    let path = name.to_string();
    CoTaskMemFree(Some(name.0 as *const _));
    Ok(Some(PathBuf::from(path?)))
}

/// Chrome's `--profile-directory` takes a folder name relative to its user
/// data folder, so a chosen profile must sit directly inside a folder that
/// has Chrome's `Local State` file.
pub fn chrome_profile_name(folder: &Path) -> Result<String, String> {
    let parent = folder.parent().filter(|p| p.join("Local State").is_file());
    match (parent, folder.file_name()) {
        (Some(_), Some(name)) => Ok(name.to_string_lossy().into_owned()),
        _ => Err("Choose a profile folder directly inside Chrome's User Data folder, such as Default or Profile 1.".into()),
    }
}

pub fn chrome_user_data() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("Google").join("Chrome").join("User Data"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_profiles_must_be_direct_children_of_user_data() {
        let root = std::env::temp_dir().join(format!("aiallowance-picker-{}", std::process::id()));
        let profile = root.join("Profile 3");
        std::fs::create_dir_all(profile.join("Nested")).unwrap();
        assert!(chrome_profile_name(&profile).is_err());
        std::fs::write(root.join("Local State"), "{}").unwrap();
        assert_eq!(chrome_profile_name(&profile).unwrap(), "Profile 3");
        assert!(chrome_profile_name(&profile.join("Nested")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

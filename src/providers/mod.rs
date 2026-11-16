//! Independent provider adapters.
//!
//! Each adapter exposes a tolerant pure parser (unit tested against synthetic
//! fixtures) plus an async `fetch` that gathers the raw payload. Fetches run
//! off the UI thread with bounded timeouts and never start a model turn or
//! redeem credits.

pub mod claude;
pub mod codex;
pub mod copilot;

/// Creation flag that hides child-process console windows on Windows.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
pub fn hide_console(cmd: &mut tokio::process::Command) {
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub fn hide_console(_cmd: &mut tokio::process::Command) {}

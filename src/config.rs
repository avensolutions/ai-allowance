//! User preferences and credential references.
//!
//! Configuration lives in the Windows user application-data directory.
//! Secrets are never stored here; the Copilot token lives in Windows
//! Credential Manager under service `AIAllowance`, user `github-copilot-pat`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const APP_DIR_NAME: &str = "AIAllowance";
pub const CONFIG_FILE_NAME: &str = "config.toml";
pub const KEYRING_SERVICE: &str = "AIAllowance";
pub const KEYRING_COPILOT_USER: &str = "github-copilot-pat";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub notifications: Notifications,
    pub tooltip: Tooltip,
    pub providers: Providers,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct General {
    /// Optional unattended polling; required for notifications to fire
    /// while the user is not hovering the tray icon.
    pub poll_enabled: bool,
    pub poll_interval_secs: u64,
}

impl Default for General {
    fn default() -> Self {
        General {
            poll_enabled: false,
            poll_interval_secs: 300,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Notifications {
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Tooltip {
    /// Window ids to show in the compact tooltip, in order. Empty means the
    /// first window of each enabled provider. Other windows stay in the menu.
    pub windows: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Providers {
    pub claude: ClaudeConfig,
    pub codex: CodexConfig,
    pub copilot: CopilotConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ClaudeConfig {
    pub enabled: bool,
    /// Directory containing the claude-usage helper (package.json and usage.ts).
    /// Defaults to `helpers/claude-usage` next to the executable, then the
    /// development checkout.
    pub helper_dir: Option<PathBuf>,
    /// Node.js command. Defaults to `node` on PATH.
    pub node_command: Option<String>,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        ClaudeConfig {
            enabled: true,
            helper_dir: None,
            node_command: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CodexConfig {
    pub enabled: bool,
    /// Full path to codex.exe. When unset, `codex` on PATH is used, then the
    /// newest bundled binary inside the ChatGPT VS Code extension.
    pub command: Option<PathBuf>,
}

impl Default for CodexConfig {
    fn default() -> Self {
        CodexConfig {
            enabled: true,
            command: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CopilotConfig {
    pub enabled: bool,
    /// GitHub username the personal Copilot plan belongs to.
    pub username: Option<String>,
    /// Total monthly included AI credits. GitHub's usage report does not
    /// return the allowance, so without this value the app shows credits used
    /// instead of inventing a percentage. Marked as configured in the UI.
    pub monthly_included_credits: Option<f64>,
    /// Billing products counted as Copilot usage (case-insensitive).
    /// An empty list counts every AI-credit item.
    pub products: Vec<String>,
}

impl Default for CopilotConfig {
    fn default() -> Self {
        CopilotConfig {
            enabled: false,
            username: None,
            monthly_included_credits: None,
            products: vec!["copilot".to_string()],
        }
    }
}

pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join(APP_DIR_NAME))
}

pub fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join(CONFIG_FILE_NAME))
}

/// Documented template written on first run.
pub const DEFAULT_CONFIG_TOML: &str = r#"# AIAllowance configuration.
# This file holds preferences only. Never put tokens here; the Copilot token
# is stored in Windows Credential Manager (service "AIAllowance", user
# "github-copilot-pat"). Set it with: aiallowance --set-copilot-token

[general]
# Unattended background polling. Required for notifications to fire while you
# are not hovering the tray icon.
poll_enabled = false
poll_interval_secs = 300

[notifications]
# Toast notifications at 20% and 10% remaining, plus confirmed resets.
enabled = false

[tooltip]
# Window ids shown in the compact tooltip, in order. Empty = first window of
# each provider. All windows stay visible in the tray menu. Examples:
# windows = ["claude:overall", "codex:codex:primary", "copilot:credits"]
windows = []

[providers.claude]
# Experimental: uses the Claude Agent SDK's unstable usage API via a local
# Node.js helper. Requires Node.js and an existing Claude Code sign-in.
enabled = true
# helper_dir = 'C:\path\to\helpers\claude-usage'
# node_command = "node"

[providers.codex]
# Uses the official local codex app-server with your ChatGPT sign-in.
enabled = true
# command = 'C:\path\to\codex.exe'

[providers.copilot]
# Personal GitHub Copilot AI-credit usage. Needs a fine-grained PAT with the
# "Plan" read permission stored via: aiallowance --set-copilot-token
enabled = false
# username = "your-github-username"
# Total monthly included credits for your plan (not exposed by the API).
# Without it the app shows credits used instead of a percentage.
# monthly_included_credits = 300.0
products = ["copilot"]
"#;

/// Load the configuration, writing the documented template on first run.
/// Returns the config plus an optional warning for display.
pub fn load_or_create() -> (Config, Option<String>) {
    let Some(path) = config_path() else {
        return (
            Config::default(),
            Some("could not resolve the user configuration directory".to_string()),
        );
    };
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&path, DEFAULT_CONFIG_TOML) {
            return (
                Config::default(),
                Some(format!("could not write default config: {e}")),
            );
        }
        return (Config::default(), None);
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => match toml::from_str::<Config>(&text) {
            Ok(cfg) => (cfg, None),
            Err(e) => (
                Config::default(),
                Some(format!("config.toml is invalid, using defaults: {e}")),
            ),
        },
        Err(e) => (
            Config::default(),
            Some(format!("could not read config.toml, using defaults: {e}")),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_template_parses_to_defaults() {
        let cfg: Config = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn partial_config_fills_defaults() {
        let cfg: Config = toml::from_str(
            r#"
[providers.copilot]
enabled = true
username = "someone"
monthly_included_credits = 300.0
"#,
        )
        .unwrap();
        assert!(cfg.providers.copilot.enabled);
        assert_eq!(cfg.providers.copilot.username.as_deref(), Some("someone"));
        assert_eq!(cfg.providers.copilot.monthly_included_credits, Some(300.0));
        assert!(cfg.providers.claude.enabled);
        assert_eq!(cfg.general.poll_interval_secs, 300);
    }
}

//! User preferences.
//!
//! Configuration lives next to the executable, migrating the legacy user file.
//! Provider authentication is managed by the installed provider CLIs.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const APP_DIR_NAME: &str = "AIAllowance";
pub const CONFIG_FILE_NAME: &str = "config.toml";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub notifications: Notifications,
    pub tooltip: Tooltip,
    pub providers: Providers,
    pub browser: Browser,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Browser {
    pub chrome_path: Option<PathBuf>,
    pub claude_profile: Option<String>,
    pub codex_profile: Option<String>,
    pub copilot_profile: Option<String>,
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
    /// first window of each provider plus Claude Fable. Other windows stay in the menu.
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
    /// Optional GitHub CLI executable. Defaults to gh on PATH.
    pub command: Option<PathBuf>,
}

impl Default for CopilotConfig {
    fn default() -> Self {
        CopilotConfig {
            enabled: true,
            command: None,
        }
    }
}

pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join(APP_DIR_NAME))
}

pub fn config_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(CONFIG_FILE_NAME)))
}

pub fn validate(cfg: &Config) -> Result<(), String> {
    if cfg.general.poll_interval_secs < 60 || cfg.general.poll_interval_secs > i64::MAX as u64 {
        return Err(
            "Polling interval must be at least 60 seconds and fit in a signed 64-bit integer."
                .into(),
        );
    }
    if cfg.notifications.enabled && !cfg.general.poll_enabled {
        return Err("Enable background polling to use notifications.".into());
    }
    for profile in [
        &cfg.browser.claude_profile,
        &cfg.browser.codex_profile,
        &cfg.browser.copilot_profile,
    ]
    .into_iter()
    .flatten()
    {
        if profile.is_empty()
            || profile.contains(['/', '\\', '\0'])
            || profile == "."
            || profile == ".."
        {
            return Err(
                "Chrome profiles must be directory names such as Default or Profile 1.".into(),
            );
        }
    }
    Ok(())
}

pub fn save(cfg: &Config) -> Result<(), String> {
    let path = config_path().ok_or("Could not locate the executable directory.")?;
    save_to_path(cfg, &path)
}

fn save_to_path(cfg: &Config, path: &std::path::Path) -> Result<(), String> {
    use std::io::Write;
    validate(cfg)?;
    let text =
        toml::to_string_pretty(cfg).map_err(|e| format!("Could not encode settings: {e}"))?;
    let temp = path.with_extension(format!("toml.{}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| format!("Could not create {}: {e}", temp.display()))?;
    let written = file
        .write_all(text.as_bytes())
        .and_then(|_| file.sync_all());
    drop(file);
    let result = written.and_then(|_| std::fs::rename(&temp, path));
    if let Err(e) = result {
        let cleanup = std::fs::remove_file(&temp);
        return Err(format!(
            "Could not save {}: {e}{}",
            path.display(),
            cleanup
                .err()
                .map(|e| format!("; temporary file cleanup failed: {e}"))
                .unwrap_or_default()
        ));
    }
    Ok(())
}

/// Documented template written on first run.
pub const DEFAULT_CONFIG_TOML: &str = r#"# AI Allowance configuration.
# Preferences only. Provider CLIs manage authentication.

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
# each provider plus Claude Fable. All windows stay visible in the tray menu.
# windows = ["claude:overall", "codex:codex:primary", "copilot:premium_interactions"]
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
# Experimental quota snapshots through your existing GitHub CLI sign-in.
enabled = true
# command = 'C:\Program Files\GitHub CLI\gh.exe'

[browser]
# Blank profiles use the default browser. Use Chrome directory names, not
# display names: for example "Default" or "Profile 1" from chrome://version.
# chrome_path = 'C:\Program Files\Google\Chrome\Application\chrome.exe'
# claude_profile = "Profile 1"
# codex_profile = "Default"
# copilot_profile = "Profile 2"
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
        let legacy = config_dir().map(|dir| dir.join(CONFIG_FILE_NAME));
        let result = if let Some(legacy) = legacy.filter(|p| p.exists()) {
            std::fs::copy(legacy, &path).map(|_| ())
        } else {
            std::fs::write(&path, DEFAULT_CONFIG_TOML)
        };
        if let Err(e) = result {
            return (
                Config::default(),
                Some(format!("could not create {}: {e}", path.display())),
            );
        }
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
command = 'C:\tools\gh.exe'
"#,
        )
        .unwrap();
        assert!(cfg.providers.copilot.enabled);
        assert_eq!(
            cfg.providers.copilot.command,
            Some(PathBuf::from(r"C:\tools\gh.exe"))
        );
        assert!(cfg.providers.claude.enabled);
        assert_eq!(cfg.general.poll_interval_secs, 300);
    }

    #[test]
    fn settings_round_trip_and_validate_optional_values() {
        let mut cfg = Config::default();
        cfg.browser.claude_profile = Some("Profile 1".into());
        cfg.browser.codex_profile = Some("Default".into());
        assert_eq!(
            toml::from_str::<Config>(&toml::to_string(&cfg).unwrap()).unwrap(),
            cfg
        );
        assert!(validate(&cfg).is_ok());
        cfg.browser.claude_profile = Some("..\\other".into());
        assert!(validate(&cfg).is_err());
    }

    #[test]
    fn saving_replaces_existing_config_and_rejects_invalid_edits() {
        let dir = std::env::temp_dir().join(format!(
            "aiallowance-config-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join(CONFIG_FILE_NAME);
        let mut cfg = Config::default();
        save_to_path(&cfg, &path).unwrap();
        cfg.browser.codex_profile = Some("Profile 2".into());
        save_to_path(&cfg, &path).unwrap();
        assert_eq!(
            toml::from_str::<Config>(&std::fs::read_to_string(&path).unwrap()).unwrap(),
            cfg
        );
        let saved = std::fs::read_to_string(&path).unwrap();
        cfg.general.poll_interval_secs = 0;
        assert!(save_to_path(&cfg, &path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}

//! Claude adapter (experimental).
//!
//! Uses the Claude Agent SDK's experimental usage API through a small pinned
//! Node.js helper (`helpers/claude-usage`). The helper opens an SDK session
//! with the user's existing Claude Code sign-in, calls the experimental usage
//! method without submitting any prompt, and prints one JSON document to
//! stdout. Response parsing is isolated here and tolerant of unknown fields;
//! missing fields are treated as unavailable, never as 0% or 100%.
//!
//! Validated on 2026-11-16 against @anthropic-ai/claude-agent-sdk 0.3.325:
//! `utilization`/`percent` are used percentages 0-100 and `resets_at` is an
//! ISO 8601 timestamp. See docs/provider-validation.md.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::config::ClaudeConfig;
use crate::providers::hide_console;
use crate::state::{FetchError, FetchOutcome, QuotaWindow, WEEK_SECS};

const FETCH_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Deserialize)]
struct HelperOutput {
    ok: bool,
    #[serde(default)]
    error: Option<HelperError>,
    #[serde(default)]
    data: Option<UsageData>,
}

#[derive(Deserialize)]
struct HelperError {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    retry_after_secs: Option<u64>,
}

#[derive(Deserialize)]
struct UsageData {
    #[serde(default)]
    subscription_type: Option<String>,
    #[serde(default)]
    rate_limits_available: Option<bool>,
    #[serde(default)]
    rate_limits: Option<RateLimits>,
}

#[derive(Deserialize)]
struct RateLimits {
    #[serde(default)]
    five_hour: Option<UsageWindow>,
    #[serde(default)]
    seven_day: Option<UsageWindow>,
    #[serde(default)]
    model_scoped: Option<Vec<ModelScoped>>,
    /// Server limits array observed in live responses; used as a fallback
    /// when the typed model_scoped field is absent.
    #[serde(default)]
    limits: Option<Vec<ServerLimit>>,
}

#[derive(Deserialize)]
struct UsageWindow {
    #[serde(default)]
    utilization: Option<f64>,
    #[serde(default)]
    resets_at: Option<String>,
}

#[derive(Deserialize)]
struct ModelScoped {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    utilization: Option<f64>,
    #[serde(default)]
    resets_at: Option<String>,
}

#[derive(Deserialize)]
struct ServerLimit {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    percent: Option<f64>,
    #[serde(default)]
    resets_at: Option<String>,
    #[serde(default)]
    scope: Option<LimitScope>,
}

#[derive(Deserialize)]
struct LimitScope {
    #[serde(default)]
    model: Option<LimitScopeModel>,
}

#[derive(Deserialize)]
struct LimitScopeModel {
    #[serde(default)]
    display_name: Option<String>,
}

fn parse_iso(ts: &Option<String>) -> Option<DateTime<Utc>> {
    ts.as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
}

fn slug(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// Parse helper JSON output into normalized windows.
pub fn parse_helper_output(json: &str) -> Result<FetchOutcome, FetchError> {
    let parsed: HelperOutput = serde_json::from_str(json)
        .map_err(|e| FetchError::Other(format!("invalid helper output: {e}")))?;
    if !parsed.ok {
        let err = parsed.error.unwrap_or(HelperError {
            kind: None,
            message: None,
            retry_after_secs: None,
        });
        let message = err.message.unwrap_or_else(|| "helper failed".to_string());
        return Err(match err.kind.as_deref() {
            Some("sign_in_required") => FetchError::SignInRequired(message),
            Some("rate_limited") => FetchError::RateLimited {
                retry_after_secs: err.retry_after_secs,
                message,
            },
            _ => classify_error_message(&message),
        });
    }
    let data = parsed
        .data
        .ok_or_else(|| FetchError::Other("helper output missing data".to_string()))?;
    if data.rate_limits_available == Some(false) {
        return Err(FetchError::Unsupported(
            "plan rate limits do not apply to this session (API key or non-subscription auth)"
                .to_string(),
        ));
    }
    // The SDK reports null rate limits when its read of the claude.ai usage
    // endpoint failed (timeout, network error or rate limit). Transient.
    let Some(limits) = data.rate_limits else {
        return Err(FetchError::Other(
            "Claude usage endpoint returned no data".to_string(),
        ));
    };

    let mut windows = Vec::new();

    if let Some(w) = limits.seven_day.as_ref() {
        if w.utilization.is_some() || w.resets_at.is_some() {
            let mut q = QuotaWindow::new("claude:overall", "Claude");
            q.window_secs = Some(WEEK_SECS);
            q.used_percent = w.utilization.filter(|u| u.is_finite());
            q.resets_at = parse_iso(&w.resets_at);
            windows.push(q);
        }
    } else if let Some(server) = limits.limits.as_ref() {
        // Fall back to the server limits array for the overall weekly window.
        if let Some(l) = server
            .iter()
            .find(|l| l.kind.as_deref() == Some("weekly_all"))
        {
            let mut q = QuotaWindow::new("claude:overall", "Claude");
            q.window_secs = Some(WEEK_SECS);
            q.used_percent = l.percent.filter(|u| u.is_finite());
            q.resets_at = parse_iso(&l.resets_at);
            windows.push(q);
        }
    }

    if let Some(w) = limits.five_hour.as_ref() {
        if w.utilization.is_some() || w.resets_at.is_some() {
            let mut q = QuotaWindow::new("claude:5h", "Claude 5h");
            q.window_secs = Some(5 * 60 * 60);
            q.used_percent = w.utilization.filter(|u| u.is_finite());
            q.resets_at = parse_iso(&w.resets_at);
            windows.push(q);
        }
    }

    // Model-scoped weekly windows (e.g. Fable). Prefer the typed field;
    // live responses have been observed carrying them only in limits[].
    let mut added_models = false;
    if let Some(models) = limits.model_scoped.as_ref() {
        for m in models {
            let Some(name) = m.display_name.as_deref().filter(|n| !n.is_empty()) else {
                continue;
            };
            let mut q = QuotaWindow::new(
                format!("claude:model:{}", slug(name)),
                format!("Claude {name}"),
            );
            q.window_secs = Some(WEEK_SECS);
            q.used_percent = m.utilization.filter(|u| u.is_finite());
            q.resets_at = parse_iso(&m.resets_at);
            windows.push(q);
            added_models = true;
        }
    }
    if !added_models {
        if let Some(server) = limits.limits.as_ref() {
            for l in server {
                if l.kind.as_deref() != Some("weekly_scoped") {
                    continue;
                }
                let Some(name) = l
                    .scope
                    .as_ref()
                    .and_then(|s| s.model.as_ref())
                    .and_then(|m| m.display_name.as_deref())
                    .filter(|n| !n.is_empty())
                else {
                    continue;
                };
                let mut q = QuotaWindow::new(
                    format!("claude:model:{}", slug(name)),
                    format!("Claude {name}"),
                );
                q.window_secs = Some(WEEK_SECS);
                q.used_percent = l.percent.filter(|u| u.is_finite());
                q.resets_at = parse_iso(&l.resets_at);
                windows.push(q);
            }
        }
    }

    if windows.is_empty() {
        return Err(FetchError::Other(
            "usage response contained no recognizable windows".to_string(),
        ));
    }
    Ok(FetchOutcome {
        account_key: data.subscription_type,
        windows,
    })
}

fn classify_error_message(message: &str) -> FetchError {
    let lower = message.to_ascii_lowercase();
    if lower.contains("auth")
        || lower.contains("login")
        || lower.contains("sign in")
        || lower.contains("sign-in")
        || lower.contains("api key")
        || lower.contains("credential")
    {
        FetchError::SignInRequired(message.to_string())
    } else if lower.contains("rate limit") || lower.contains("429") {
        FetchError::RateLimited {
            retry_after_secs: None,
            message: message.to_string(),
        }
    } else {
        FetchError::Other(message.to_string())
    }
}

fn helper_dir(cfg: &ClaudeConfig) -> Option<PathBuf> {
    if let Some(dir) = &cfg.helper_dir {
        return Some(dir.clone());
    }
    // Next to the executable (installed layout) ...
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("helpers").join("claude-usage");
            if candidate.join("usage.ts").exists() {
                return Some(candidate);
            }
        }
    }
    // ... or the development checkout.
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("helpers")
        .join("claude-usage");
    if dev.join("usage.ts").exists() {
        return Some(dev);
    }
    None
}

/// Run the pinned helper and parse its output.
pub async fn fetch(cfg: &ClaudeConfig) -> Result<FetchOutcome, FetchError> {
    let Some(dir) = helper_dir(cfg) else {
        return Err(FetchError::Unsupported(
            "claude-usage helper not found; set providers.claude.helper_dir".to_string(),
        ));
    };
    if !dir.join("node_modules").exists() {
        return Err(FetchError::Unsupported(format!(
            "claude-usage helper dependencies missing; run npm install in {}",
            dir.display()
        )));
    }
    let node = cfg
        .node_command
        .clone()
        .unwrap_or_else(|| "node".to_string());
    let mut cmd = tokio::process::Command::new(&node);
    cmd.arg("usage.ts")
        .current_dir(&dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    hide_console(&mut cmd);
    let child = cmd
        .spawn()
        .map_err(|e| FetchError::Unsupported(format!("could not start Node.js ({node}): {e}")))?;
    let output = tokio::time::timeout(FETCH_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| FetchError::Timeout("claude usage helper timed out".to_string()))?
        .map_err(|e| FetchError::Other(format!("helper failed: {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or("");
    if line.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail: String = stderr.chars().take(300).collect();
        return Err(classify_error_message(&format!(
            "helper produced no JSON output: {}",
            detail.trim()
        )));
    }
    parse_helper_output(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_FULL: &str = include_str!("../../tests/fixtures/claude_usage_full.json");
    const FIXTURE_LIMITS_ONLY: &str =
        include_str!("../../tests/fixtures/claude_usage_limits_only.json");
    const FIXTURE_UNAVAILABLE: &str =
        include_str!("../../tests/fixtures/claude_usage_unavailable.json");
    const FIXTURE_SIGNIN: &str = include_str!("../../tests/fixtures/claude_usage_signin.json");

    #[test]
    fn parses_typed_windows_including_model_scoped() {
        let out = parse_helper_output(FIXTURE_FULL).unwrap();
        assert_eq!(out.account_key.as_deref(), Some("max"));
        let ids: Vec<&str> = out.windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["claude:overall", "claude:5h", "claude:model:fable"]
        );
        let overall = &out.windows[0];
        assert_eq!(overall.used_percent, Some(63.0));
        assert_eq!(overall.remaining_percent(), Some(37.0));
        assert!(overall.resets_at.is_some());
        let fable = &out.windows[2];
        assert_eq!(fable.label, "Claude Fable");
        assert_eq!(fable.used_percent, Some(100.0));
    }

    #[test]
    fn falls_back_to_limits_array_when_model_scoped_absent() {
        // Mirrors the live response observed on 2026-11-16: five_hour null,
        // model windows only inside limits[], plus unknown fields everywhere.
        let out = parse_helper_output(FIXTURE_LIMITS_ONLY).unwrap();
        let ids: Vec<&str> = out.windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, vec!["claude:overall", "claude:model:fable"]);
        assert_eq!(out.windows[0].used_percent, Some(63.0));
        assert_eq!(out.windows[1].used_percent, Some(100.0));
        assert!(out.windows[1].resets_at.is_some());
    }

    #[test]
    fn absent_windows_are_not_invented() {
        let out = parse_helper_output(FIXTURE_LIMITS_ONLY).unwrap();
        assert!(!out.windows.iter().any(|w| w.id == "claude:5h"));
    }

    #[test]
    fn unavailable_rate_limits_map_to_unsupported() {
        let err = parse_helper_output(FIXTURE_UNAVAILABLE).unwrap_err();
        assert!(matches!(err, FetchError::Unsupported(_)));
    }

    #[test]
    fn helper_sign_in_error_maps_to_sign_in_required() {
        let err = parse_helper_output(FIXTURE_SIGNIN).unwrap_err();
        assert!(matches!(err, FetchError::SignInRequired(_)));
    }

    #[test]
    fn failed_endpoint_reads_are_transient_and_rate_limits_keep_their_hold() {
        let err = parse_helper_output(
            r#"{"ok":false,"error":{"kind":"rate_limited","message":"Claude usage endpoint answered HTTP 429; retry in 300s","retry_after_secs":300}}"#,
        )
        .unwrap_err();
        assert_eq!(
            err,
            FetchError::RateLimited {
                retry_after_secs: Some(300),
                message: "Claude usage endpoint answered HTTP 429; retry in 300s".into(),
            }
        );
        let err = parse_helper_output(
            r#"{"ok":false,"error":{"kind":"error","message":"Claude usage endpoint read failed twice (Failed to load usage data: timeout of 5000ms exceeded)"}}"#,
        )
        .unwrap_err();
        assert!(matches!(err, FetchError::Other(_)), "{err:?}");
        // Older helpers pass the null through; it is still a transient failure.
        let err = parse_helper_output(
            r#"{"ok":true,"data":{"subscription_type":"max","rate_limits_available":true,"rate_limits":null}}"#,
        )
        .unwrap_err();
        assert!(matches!(err, FetchError::Other(_)), "{err:?}");
    }

    #[test]
    fn null_utilization_is_unavailable_not_zero() {
        let json = r#"{"ok":true,"data":{"subscription_type":"max","rate_limits_available":true,
            "rate_limits":{"seven_day":{"utilization":null,"resets_at":"2026-11-20T00:00:00+00:00"}}}}"#;
        let out = parse_helper_output(json).unwrap();
        assert_eq!(out.windows[0].used_percent, None);
        assert_eq!(out.windows[0].remaining_percent(), None);
        assert!(out.windows[0].resets_at.is_some());
    }
}

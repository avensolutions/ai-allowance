//! Work/Codex adapter.
//!
//! Talks to the official local `codex app-server` over newline-delimited
//! JSON-RPC with the user's existing ChatGPT sign-in: `initialize` request,
//! `initialized` notification, then `account/rateLimits/read`. Requesting
//! quota never starts a model turn and never consumes earned reset credits.
//!
//! Validated on 2026-11-16 against codex-cli 0.169.0-alpha.11.1:
//! `usedPercent` is 0-100 within the window, `windowDurationMins` is the
//! window length and `resetsAt` is a Unix timestamp in seconds.
//! `rateLimitsByLimitId` is the multi-bucket view; the legacy `rateLimits`
//! value is a fallback only, never an additional bucket.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use chrono::DateTime;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::CodexConfig;
use crate::providers::hide_console;
use crate::state::{FetchError, FetchOutcome, QuotaWindow};

const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct RateLimitsResult {
    #[serde(default, rename = "rateLimits")]
    rate_limits: Option<Bucket>,
    #[serde(default, rename = "rateLimitsByLimitId")]
    rate_limits_by_limit_id: Option<BTreeMap<String, Bucket>>,
    #[serde(default, rename = "accountId")]
    account_id: Option<String>,
}

#[derive(Deserialize)]
struct Bucket {
    #[serde(default, rename = "limitId")]
    limit_id: Option<String>,
    #[serde(default, rename = "limitName")]
    limit_name: Option<String>,
    #[serde(default)]
    primary: Option<WindowJson>,
    #[serde(default)]
    secondary: Option<WindowJson>,
}

#[derive(Deserialize)]
struct WindowJson {
    #[serde(default, rename = "usedPercent")]
    used_percent: Option<f64>,
    #[serde(default, rename = "windowDurationMins")]
    window_duration_mins: Option<i64>,
    #[serde(default, rename = "resetsAt")]
    resets_at: Option<i64>,
}

fn duration_label(mins: Option<i64>) -> Option<String> {
    let mins = mins?;
    if mins <= 0 {
        return None;
    }
    Some(if mins % (60 * 24) == 0 {
        format!("{}d", mins / (60 * 24))
    } else if mins % 60 == 0 {
        format!("{}h", mins / 60)
    } else {
        format!("{mins}m")
    })
}

fn push_window(
    windows: &mut Vec<QuotaWindow>,
    bucket_key: &str,
    bucket: &Bucket,
    which: &str,
    w: &WindowJson,
    single_bucket: bool,
) {
    let mut label = "Work/Codex".to_string();
    if !single_bucket || bucket_key != "codex" {
        let name = bucket
            .limit_name
            .clone()
            .or_else(|| bucket.limit_id.clone())
            .unwrap_or_else(|| bucket_key.to_string());
        label.push(' ');
        label.push_str(&name);
    }
    if which == "secondary" {
        if let Some(d) = duration_label(w.window_duration_mins) {
            label.push(' ');
            label.push_str(&d);
        } else {
            label.push_str(" secondary");
        }
    }
    let mut q = QuotaWindow::new(format!("codex:{bucket_key}:{which}"), label);
    q.used_percent = w.used_percent.filter(|u| u.is_finite());
    q.resets_at = w.resets_at.and_then(|s| DateTime::from_timestamp(s, 0));
    q.window_secs = w.window_duration_mins.filter(|m| *m > 0).map(|m| m * 60);
    windows.push(q);
}

/// Parse the `account/rateLimits/read` result into normalized windows.
/// Prefers `rateLimitsByLimitId`; the legacy value is a fallback, not an
/// additional bucket to count twice.
pub fn parse_rate_limits(result: &Value) -> Result<FetchOutcome, FetchError> {
    let parsed: RateLimitsResult = serde_json::from_value(result.clone())
        .map_err(|e| FetchError::Other(format!("unexpected rateLimits shape: {e}")))?;
    let mut windows = Vec::new();
    if let Some(by_id) = parsed.rate_limits_by_limit_id.filter(|m| !m.is_empty()) {
        let single = by_id.len() == 1;
        for (key, bucket) in &by_id {
            if let Some(p) = bucket.primary.as_ref() {
                push_window(&mut windows, key, bucket, "primary", p, single);
            }
            if let Some(s) = bucket.secondary.as_ref() {
                push_window(&mut windows, key, bucket, "secondary", s, single);
            }
        }
    } else if let Some(bucket) = parsed.rate_limits.as_ref() {
        let key = bucket
            .limit_id
            .clone()
            .unwrap_or_else(|| "codex".to_string());
        if let Some(p) = bucket.primary.as_ref() {
            push_window(&mut windows, &key, bucket, "primary", p, true);
        }
        if let Some(s) = bucket.secondary.as_ref() {
            push_window(&mut windows, &key, bucket, "secondary", s, true);
        }
    }
    if windows.is_empty() {
        return Err(FetchError::Other(
            "rate limit response contained no windows".to_string(),
        ));
    }
    Ok(FetchOutcome {
        account_key: parsed.account_id,
        windows,
    })
}

fn classify_rpc_error(message: &str) -> FetchError {
    let lower = message.to_ascii_lowercase();
    if lower.contains("auth") || lower.contains("login") || lower.contains("sign") {
        FetchError::SignInRequired(message.to_string())
    } else {
        FetchError::Other(message.to_string())
    }
}

/// Find codex.exe: explicit config, PATH, then the newest ChatGPT VS Code
/// extension bundle.
pub fn find_codex(cfg: &CodexConfig) -> Option<PathBuf> {
    if let Some(cmd) = &cfg.command {
        return cmd.exists().then(|| cmd.clone());
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            for name in ["codex.exe", "codex.cmd", "codex"] {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    let home = dirs::home_dir()?;
    let ext_dir = home.join(".vscode").join("extensions");
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(&ext_dir)
        .ok()?
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("openai.chatgpt-")
        })
        .map(|e| {
            e.path()
                .join("bin")
                .join("windows-x86_64")
                .join("codex.exe")
        })
        .filter(|p| p.is_file())
        .collect();
    candidates.sort();
    candidates.pop()
}

/// Supervised app-server round trip: initialize, initialized,
/// account/rateLimits/read, then terminate the process.
pub async fn fetch(cfg: &CodexConfig) -> Result<FetchOutcome, FetchError> {
    let Some(exe) = find_codex(cfg) else {
        return Err(FetchError::Unsupported(
            "codex not found; install the Codex CLI or set providers.codex.command".to_string(),
        ));
    };
    tokio::time::timeout(FETCH_TIMEOUT, fetch_inner(&exe))
        .await
        .map_err(|_| FetchError::Timeout("codex app-server timed out".to_string()))?
}

async fn fetch_inner(exe: &PathBuf) -> Result<FetchOutcome, FetchError> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("app-server")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    hide_console(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| FetchError::Other(format!("could not start codex app-server: {e}")))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let mut lines = BufReader::new(stdout).lines();

    let write = |m: Value| {
        let mut s = m.to_string();
        s.push('\n');
        s
    };
    stdin
        .write_all(
            write(json!({
                "method": "initialize",
                "id": 0,
                "params": {
                    "clientInfo": {
                        "name": "aiallowance",
                        "title": "AI Allowance",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }
            }))
            .as_bytes(),
        )
        .await
        .map_err(|e| FetchError::Other(format!("app-server write failed: {e}")))?;

    let mut initialized = false;
    let result = loop {
        let Some(line) = lines
            .next_line()
            .await
            .map_err(|e| FetchError::Other(format!("app-server read failed: {e}")))?
        else {
            break Err(FetchError::Other(
                "codex app-server closed the connection".to_string(),
            ));
        };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = msg.get("id").and_then(Value::as_i64);
        if id == Some(0) && !initialized {
            if let Some(err) = msg.get("error") {
                break Err(classify_rpc_error(&err.to_string()));
            }
            initialized = true;
            let batch = format!(
                "{}{}",
                write(json!({"method": "initialized", "params": {}})),
                write(json!({"method": "account/rateLimits/read", "id": 1, "params": {}}))
            );
            stdin
                .write_all(batch.as_bytes())
                .await
                .map_err(|e| FetchError::Other(format!("app-server write failed: {e}")))?;
            continue;
        }
        if id == Some(1) {
            if let Some(err) = msg.get("error") {
                let message = err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("rate limit read failed");
                break Err(classify_rpc_error(message));
            }
            let Some(result) = msg.get("result") else {
                break Err(FetchError::Other(
                    "rate limit response had no result".to_string(),
                ));
            };
            break parse_rate_limits(result);
        }
    };
    let _ = child.kill().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn parses_single_bucket() {
        let out = parse_rate_limits(&fixture("codex_ratelimits_single.json")).unwrap();
        assert_eq!(out.account_key.as_deref(), Some("acct-1"));
        assert_eq!(out.windows.len(), 1);
        let w = &out.windows[0];
        assert_eq!(w.id, "codex:codex:primary");
        assert_eq!(w.label, "Work/Codex");
        assert_eq!(w.used_percent, Some(80.0));
        assert_eq!(w.remaining_percent(), Some(20.0));
        assert_eq!(
            w.resets_at.unwrap(),
            DateTime::from_timestamp(1_794_866_591, 0).unwrap()
        );
    }

    #[test]
    fn parses_multiple_buckets_and_secondary_windows() {
        let out = parse_rate_limits(&fixture("codex_ratelimits_multi.json")).unwrap();
        let ids: Vec<&str> = out.windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "codex:codex:primary",
                "codex:codex:secondary",
                "codex:codex_other:primary"
            ]
        );
        // Buckets are never summed; each keeps its own percent.
        assert_eq!(out.windows[0].used_percent, Some(25.0));
        assert_eq!(out.windows[1].used_percent, Some(42.0));
        assert_eq!(out.windows[2].used_percent, Some(10.0));
        assert_eq!(out.windows[1].label, "Work/Codex codex 7d");
        assert_eq!(out.windows[2].label, "Work/Codex codex_other");
    }

    #[test]
    fn legacy_value_is_fallback_not_extra_bucket() {
        // Fixture carries both views; only the by-id view must be used.
        let out = parse_rate_limits(&fixture("codex_ratelimits_multi.json")).unwrap();
        assert_eq!(out.windows.len(), 3);

        let legacy_only = serde_json::json!({
            "rateLimits": {
                "limitId": "codex",
                "primary": {"usedPercent": 50, "windowDurationMins": 300, "resetsAt": 1794866591}
            }
        });
        let out = parse_rate_limits(&legacy_only).unwrap();
        assert_eq!(out.windows.len(), 1);
        assert_eq!(out.windows[0].used_percent, Some(50.0));
    }

    #[test]
    fn missing_percent_stays_unavailable() {
        let v = serde_json::json!({
            "rateLimitsByLimitId": {
                "codex": {"limitId": "codex", "primary": {"resetsAt": 1794866591}}
            }
        });
        let out = parse_rate_limits(&v).unwrap();
        assert_eq!(out.windows[0].used_percent, None);
        assert_eq!(out.windows[0].remaining_percent(), None);
    }

    #[test]
    fn empty_response_is_an_error() {
        assert!(parse_rate_limits(&serde_json::json!({})).is_err());
    }
}

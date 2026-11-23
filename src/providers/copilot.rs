//! Experimental Copilot quota snapshots, collected read-only through GitHub CLI.

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;

use crate::config::CopilotConfig;
use crate::providers::hide_console;
use crate::state::{FetchError, FetchOutcome, QuotaWindow};

const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_OUTPUT: u64 = 256 * 1024;
const QUERY: &str = "{plan: .copilot_plan, reset: .quota_reset_date, quota: .quota_snapshots}";

#[derive(Deserialize)]
struct Report {
    plan: Option<String>,
    reset: Option<String>,
    quota: Option<BTreeMap<String, Snapshot>>,
}

#[derive(Deserialize)]
struct Snapshot {
    has_quota: Option<bool>,
    unlimited: Option<bool>,
    percent_remaining: Option<f64>,
    credits_used: Option<f64>,
    entitlement: Option<f64>,
    overage_count: Option<f64>,
    quota_reset_at: Option<i64>,
    timestamp_utc: Option<DateTime<Utc>>,
}

pub fn parse_quota(json: &str, account_key: String) -> Result<FetchOutcome, FetchError> {
    let report: Report = serde_json::from_str(json)
        .map_err(|_| FetchError::Other("Unexpected Copilot quota response.".into()))?;
    if report.plan.as_deref().is_none_or(|p| p.is_empty()) {
        return Err(FetchError::Unsupported(
            "No Copilot plan was returned.".into(),
        ));
    }
    let quota = report
        .quota
        .ok_or_else(|| FetchError::Other("Copilot quota snapshots are missing.".into()))?;
    let monthly_reset = report
        .reset
        .as_deref()
        .map(|reset| {
            NaiveDate::parse_from_str(reset, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|d| d.and_utc())
                .ok_or_else(|| FetchError::Other("Copilot returned an invalid reset date.".into()))
        })
        .transpose()?;
    let mut windows = Vec::new();
    for (id, quota) in quota {
        if quota.unlimited == Some(true) {
            continue;
        }
        // Observed 2026-11-23: an exhausted finite snapshot reports
        // `has_quota: false` alongside its positive entitlement, so the flag
        // means "allowance remaining", not "snapshot has an allowance". Only
        // skip it when there is no entitlement to show.
        if quota.has_quota == Some(false) && !quota.entitlement.is_some_and(|n| n > 0.0) {
            continue;
        }
        let mut w = QuotaWindow::new(
            format!("copilot:{id}"),
            match id.as_str() {
                "premium_interactions" => "Copilot".to_string(),
                "chat" => "Copilot Chat".to_string(),
                "completions" => "Copilot completions".to_string(),
                _ => format!("Copilot {}", id.replace('_', " ")),
            },
        );
        if let Some(left) = quota.percent_remaining {
            if !left.is_finite() || !(0.0..=100.0).contains(&left) {
                return Err(FetchError::Other(
                    "Copilot returned an invalid remaining percentage.".into(),
                ));
            }
            w.used_percent = Some(100.0 - left);
        }
        let quantity = |value: Option<f64>| -> Result<Option<f64>, FetchError> {
            if value.is_some_and(|n| !n.is_finite() || n < 0.0) {
                Err(FetchError::Other(
                    "Copilot returned an invalid quota quantity.".into(),
                ))
            } else {
                Ok(value)
            }
        };
        w.used_units = quantity(quota.credits_used)?;
        w.allowance_units = quantity(quota.entitlement)?.filter(|n| *n > 0.0);
        w.overage_units = quantity(quota.overage_count)?;
        w.provider_data_at = quota.timestamp_utc;
        w.resets_at = match quota.quota_reset_at.filter(|n| *n != 0) {
            Some(timestamp) => Some(
                DateTime::from_timestamp(timestamp, 0)
                    .filter(|_| timestamp > 0)
                    .ok_or_else(|| {
                        FetchError::Other("Copilot returned an invalid reset timestamp.".into())
                    })?,
            ),
            None => monthly_reset,
        };
        // Copilot allowances are monthly: the window runs from the same day
        // of the previous month to the reset.
        w.window_secs = w.resets_at.and_then(|reset| {
            reset
                .checked_sub_months(chrono::Months::new(1))
                .map(|start| (reset - start).num_seconds())
        });
        if w.used_percent.is_none() && w.used_units.is_none() {
            return Err(FetchError::Other(
                "Copilot returned a quota without usage values.".into(),
            ));
        }
        windows.push(w);
    }
    windows.sort_by_key(|w| (w.id != "copilot:premium_interactions", w.id.clone()));
    if windows.is_empty() {
        return Err(FetchError::Unsupported(
            "No finite Copilot allowance was reported.".into(),
        ));
    }
    Ok(FetchOutcome {
        account_key: Some(account_key),
        windows,
    })
}

async fn gh(cfg: &CopilotConfig, endpoint: &str, query: &str) -> Result<String, FetchError> {
    use tokio::io::AsyncReadExt;
    let mut cmd = tokio::process::Command::new(
        cfg.command
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("gh")),
    );
    cmd.args(["api", endpoint, "--hostname", "github.com", "--jq", query])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    hide_console(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            FetchError::Unsupported(
                "GitHub CLI was not found. Install gh or set its executable in Settings.".into(),
            )
        } else {
            FetchError::Other("Could not start GitHub CLI.".into())
        }
    })?;
    let mut output = Vec::new();
    child
        .stdout
        .take()
        .expect("piped stdout")
        .take(MAX_OUTPUT + 1)
        .read_to_end(&mut output)
        .await
        .map_err(|_| FetchError::Other("Could not read the GitHub CLI response.".into()))?;
    if output.len() as u64 > MAX_OUTPUT {
        return Err(FetchError::Other(
            "GitHub CLI response exceeded the size limit.".into(),
        ));
    }
    let status = child
        .wait()
        .await
        .map_err(|_| FetchError::Other("Could not wait for GitHub CLI.".into()))?;
    if !status.success() {
        return Err(FetchError::SignInRequired(
            "GitHub CLI could not read Copilot quotas. Check gh auth status and sign in with gh auth login.".into(),
        ));
    }
    String::from_utf8(output)
        .map_err(|_| FetchError::Other("GitHub CLI returned invalid text.".into()))
}

pub async fn fetch(cfg: &CopilotConfig) -> Result<FetchOutcome, FetchError> {
    if !cfg.enabled {
        return Err(FetchError::NotConfigured("disabled in Settings".into()));
    }
    tokio::time::timeout(FETCH_TIMEOUT, async {
        let account = gh(cfg, "/user", ".id").await?;
        let account = account.trim();
        if account.is_empty() || !account.bytes().all(|b| b.is_ascii_digit()) {
            return Err(FetchError::Other(
                "GitHub CLI did not identify the signed-in account.".into(),
            ));
        }
        let json = gh(cfg, "/copilot_internal/user", QUERY).await?;
        parse_quota(&json, format!("github:{account}"))
    })
    .await
    .map_err(|_| FetchError::Timeout("GitHub CLI quota request timed out.".into()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_and_missing_cli_are_explicit_without_network_access() {
        let mut cfg = CopilotConfig {
            enabled: false,
            command: Some(std::env::temp_dir().join("ai-allowance-nonexistent-gh.exe")),
        };
        assert!(matches!(
            fetch(&cfg).await,
            Err(FetchError::NotConfigured(_))
        ));
        cfg.enabled = true;
        assert!(matches!(fetch(&cfg).await, Err(FetchError::Unsupported(_))));
    }

    #[test]
    fn finite_premium_quota_uses_reported_percentage_and_utc_reset() {
        let out = parse_quota(
            include_str!("../../tests/fixtures/copilot_quota.json"),
            "github:123".into(),
        )
        .unwrap();
        assert_eq!(out.windows.len(), 1);
        let w = &out.windows[0];
        assert_eq!(w.id, "copilot:premium_interactions");
        assert!((w.remaining_percent().unwrap() - 63.9).abs() < 0.001);
        assert_eq!(w.allowance_units, Some(20000.0));
        assert_eq!(
            w.resets_at.unwrap().to_rfc3339(),
            "2026-12-01T00:00:00+00:00"
        );
        assert!(w.provider_data_at.is_some());
        assert!(!w.allowance_is_configured);
    }

    #[test]
    fn absent_and_invalid_snapshots_do_not_invent_full_allowances() {
        for json in [
            "{}",
            r#"{"plan":"max","quota":{}}"#,
            r#"{"plan":"max","quota":{"premium_interactions":{}}}"#,
            r#"{"plan":"max","quota":{"premium_interactions":{"percent_remaining":150}}}"#,
        ] {
            assert!(parse_quota(json, "github:123".into()).is_err());
        }
        let out = parse_quota(
            r#"{"plan":"max","quota":{"premium_interactions":{"credits_used":20}}}"#,
            "github:123".into(),
        )
        .unwrap();
        assert_eq!(out.windows[0].remaining_percent(), None);
    }

    #[test]
    fn exhausted_quota_with_has_quota_false_is_shown_as_empty() {
        let out = parse_quota(
            include_str!("../../tests/fixtures/copilot_quota_exhausted.json"),
            "github:123".into(),
        )
        .unwrap();
        assert_eq!(out.windows.len(), 1);
        let w = &out.windows[0];
        assert_eq!(w.id, "copilot:premium_interactions");
        assert_eq!(w.remaining_percent(), Some(0.0));
        assert_eq!(w.used_units, Some(20009.0));
        assert_eq!(w.allowance_units, Some(20000.0));

        // Without an entitlement, has_quota false still means no allowance.
        assert!(parse_quota(
            r#"{"plan":"free","quota":{"premium_interactions":{"has_quota":false,"unlimited":false,"entitlement":0,"percent_remaining":0}}}"#,
            "github:123".into(),
        )
        .is_err());
    }

    #[test]
    fn separate_finite_windows_and_window_specific_reset_are_retained() {
        let out = parse_quota(r#"{"plan":"max","reset":"2026-12-01","quota":{
            "chat":{"has_quota":true,"unlimited":false,"percent_remaining":80,"quota_reset_at":1795000000},
            "premium_interactions":{"percent_remaining":50}
        }}"#, "github:123".into()).unwrap();
        assert_eq!(out.windows.len(), 2);
        assert_eq!(out.windows[0].id, "copilot:premium_interactions");
        assert_eq!(out.windows[1].resets_at.unwrap().timestamp(), 1795000000);
    }
}

//! GitHub Copilot adapter.
//!
//! Uses the documented personal-account billing REST API:
//! `GET /users/{username}/settings/billing/ai_credit/usage`
//! with a user-supplied fine-grained PAT (Plan read permission) stored in
//! Windows Credential Manager. The usage report does not expose the total
//! monthly allowance; a percentage is only shown when the user configures
//! `monthly_included_credits`, and it is marked as configured. Included
//! consumption (discounted quantity) is tracked separately from billed
//! overage (net quantity). Included credits reset on the first day of each
//! calendar month at 00:00 UTC per GitHub's documentation; billing data can
//! lag, so collection time is not a provider data timestamp.

use std::time::Duration;

use chrono::{DateTime, Datelike, TimeZone, Utc};
use serde::Deserialize;

use crate::config::CopilotConfig;
use crate::state::{FetchError, FetchOutcome, QuotaWindow};

const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
const API_VERSION: &str = "2022-11-28";

#[derive(Deserialize)]
struct UsageReport {
    #[serde(default, rename = "usageItems")]
    usage_items: Vec<UsageItem>,
}

#[derive(Deserialize)]
struct UsageItem {
    #[serde(default)]
    product: Option<String>,
    #[serde(default, rename = "discountQuantity")]
    discount_quantity: Option<f64>,
    #[serde(default, rename = "netQuantity")]
    net_quantity: Option<f64>,
}

/// First day of the next calendar month at 00:00 UTC.
pub fn next_month_start(now: DateTime<Utc>) -> DateTime<Utc> {
    let (y, m) = (now.year(), now.month());
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    Utc.with_ymd_and_hms(ny, nm, 1, 0, 0, 0).unwrap()
}

/// Parse the monthly usage report into one credits window.
pub fn parse_usage_report(
    json: &str,
    cfg: &CopilotConfig,
    now: DateTime<Utc>,
) -> Result<FetchOutcome, FetchError> {
    let report: UsageReport = serde_json::from_str(json)
        .map_err(|e| FetchError::Other(format!("unexpected usage report shape: {e}")))?;

    let filters: Vec<String> = cfg
        .products
        .iter()
        .map(|p| p.to_ascii_lowercase())
        .collect();
    let mut discounted = 0.0f64;
    let mut net = 0.0f64;
    let mut matched = false;
    for item in &report.usage_items {
        let product = item
            .product
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !filters.is_empty() && !filters.iter().any(|f| product.contains(f.as_str())) {
            continue;
        }
        matched = true;
        discounted += item.discount_quantity.unwrap_or(0.0);
        net += item.net_quantity.unwrap_or(0.0);
    }

    let mut w = QuotaWindow::new("copilot:credits", "Copilot");
    if matched {
        // Included-credit consumption is the discounted quantity; a billed
        // amount of zero does not mean no included allowance was consumed.
        w.used_units = Some(discounted);
        w.overage_units = Some(net);
    } else {
        // An empty month means zero consumption so far, which is a real
        // observation, not missing data.
        w.used_units = Some(0.0);
        w.overage_units = Some(0.0);
    }
    if let Some(total) = cfg.monthly_included_credits {
        if total > 0.0 {
            w.allowance_units = Some(total);
            w.allowance_is_configured = true;
        }
    }
    w.resets_at = Some(next_month_start(now));
    Ok(FetchOutcome {
        account_key: cfg.username.clone(),
        windows: vec![w],
    })
}

#[cfg(windows)]
pub fn stored_token() -> Result<Option<String>, String> {
    use crate::config::{KEYRING_COPILOT_USER, KEYRING_SERVICE};
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_COPILOT_USER)
        .map_err(|e| format!("credential manager unavailable: {e}"))?;
    match entry.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("could not read stored token: {e}")),
    }
}

#[cfg(windows)]
pub fn store_token(token: &str) -> Result<(), String> {
    use crate::config::{KEYRING_COPILOT_USER, KEYRING_SERVICE};
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_COPILOT_USER)
        .map_err(|e| format!("credential manager unavailable: {e}"))?;
    entry
        .set_password(token)
        .map_err(|e| format!("could not store token: {e}"))
}

#[cfg(windows)]
pub async fn fetch(cfg: &CopilotConfig) -> Result<FetchOutcome, FetchError> {
    let Some(username) = cfg.username.as_deref().filter(|u| !u.is_empty()) else {
        return Err(FetchError::NotConfigured(
            "set providers.copilot.username in config.toml".to_string(),
        ));
    };
    let token = match stored_token() {
        Ok(Some(t)) => t,
        Ok(None) => {
            return Err(FetchError::SignInRequired(
                "store a fine-grained PAT with Plan read permission via \
                 aiallowance --set-copilot-token"
                    .to_string(),
            ))
        }
        Err(e) => return Err(FetchError::Other(e)),
    };
    let now = Utc::now();
    let url = format!(
        "https://api.github.com/users/{username}/settings/billing/ai_credit/usage?year={}&month={}",
        now.year(),
        now.month()
    );
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent(concat!("AIAllowance/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| FetchError::Other(format!("http client: {e}")))?;
    let resp = client
        .get(&url)
        .bearer_auth(&token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", API_VERSION)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                FetchError::Timeout("GitHub billing request timed out".to_string())
            } else {
                FetchError::Other(format!("GitHub billing request failed: {e}"))
            }
        })?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::SignInRequired(format!(
            "GitHub rejected the token ({status}); it needs the Plan read permission"
        )));
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        return Err(FetchError::RateLimited {
            retry_after_secs: retry_after,
            message: "GitHub API rate limited".to_string(),
        });
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(FetchError::SignInRequired(format!(
            "GitHub returned 404 for {username}'s billing usage; the token likely lacks the \
             Plan read permission or the username is wrong"
        )));
    }
    if !status.is_success() {
        return Err(FetchError::Other(format!(
            "GitHub billing request failed with {status}"
        )));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| FetchError::Other(format!("GitHub response read failed: {e}")))?;
    parse_usage_report(&body, cfg, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(allowance: Option<f64>) -> CopilotConfig {
        CopilotConfig {
            enabled: true,
            username: Some("someone".to_string()),
            monthly_included_credits: allowance,
            products: vec!["copilot".to_string()],
        }
    }

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    const FIXTURE: &str = include_str!("../../tests/fixtures/copilot_ai_credit_usage.json");

    #[test]
    fn included_usage_comes_from_discounted_quantity() {
        let now = t("2026-11-16T03:00:00Z");
        let out = parse_usage_report(FIXTURE, &cfg(None), now).unwrap();
        let w = &out.windows[0];
        // 120.5 + 29.5 discounted across the two copilot items; the
        // non-copilot item is filtered out; billed overage kept separately.
        assert_eq!(w.used_units, Some(150.0));
        assert_eq!(w.overage_units, Some(12.0));
        // No allowance configured: no invented percentage.
        assert_eq!(w.remaining_percent(), None);
        assert!(!w.allowance_is_configured);
    }

    #[test]
    fn configured_allowance_yields_marked_percentage() {
        let now = t("2026-11-16T03:00:00Z");
        let out = parse_usage_report(FIXTURE, &cfg(Some(300.0)), now).unwrap();
        let w = &out.windows[0];
        assert!(w.allowance_is_configured);
        assert_eq!(w.remaining_percent(), Some(50.0));
    }

    #[test]
    fn reset_is_first_of_next_month_utc() {
        let now = t("2026-11-16T03:00:00Z");
        let out = parse_usage_report(FIXTURE, &cfg(None), now).unwrap();
        assert_eq!(out.windows[0].resets_at.unwrap(), t("2026-12-01T00:00:00Z"));
        // December rolls into January of the next year.
        assert_eq!(
            next_month_start(t("2026-12-31T23:00:00Z")),
            t("2027-01-01T00:00:00Z")
        );
    }

    #[test]
    fn empty_month_is_zero_usage_not_missing_data() {
        let now = t("2026-11-16T03:00:00Z");
        let out = parse_usage_report(r#"{"usageItems":[]}"#, &cfg(None), now).unwrap();
        let w = &out.windows[0];
        assert_eq!(w.used_units, Some(0.0));
        assert_eq!(w.remaining_percent(), None);
    }

    #[test]
    fn malformed_body_is_an_error_not_zero() {
        let now = t("2026-11-16T03:00:00Z");
        assert!(parse_usage_report("not json", &cfg(None), now).is_err());
    }
}

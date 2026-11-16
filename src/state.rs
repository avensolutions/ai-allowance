//! Account snapshots, quota windows and display state.
//!
//! Raw provider values stay in `QuotaWindow`; display conversion happens in
//! the formatting helpers so tests can exercise them without any UI.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

/// Windows tray tooltips have 127 usable UTF-16 code units.
pub const TOOLTIP_MAX_UTF16: usize = 127;

/// Data older than this is refreshed when the user hovers the tray icon.
pub const HOVER_MAX_AGE_SECS: i64 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProviderKind {
    Claude,
    Codex,
    Copilot,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 3] = [
        ProviderKind::Claude,
        ProviderKind::Codex,
        ProviderKind::Copilot,
    ];

    pub fn key(self) -> &'static str {
        match self {
            ProviderKind::Claude => "claude",
            ProviderKind::Codex => "codex",
            ProviderKind::Copilot => "copilot",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ProviderKind::Claude => "Claude",
            ProviderKind::Codex => "Work/Codex",
            ProviderKind::Copilot => "Copilot",
        }
    }

    pub fn usage_url(self) -> &'static str {
        match self {
            ProviderKind::Claude => "https://claude.ai/settings/usage",
            ProviderKind::Codex => "https://chatgpt.com/codex/settings/usage",
            ProviderKind::Copilot => "https://github.com/settings/billing",
        }
    }
}

/// One allowance window for one provider account.
#[derive(Clone, Debug, PartialEq)]
pub struct QuotaWindow {
    /// Stable identity, e.g. `claude:overall` or `codex:codex:primary`.
    pub id: String,
    /// Display label, e.g. `Claude Fable`.
    pub label: String,
    /// Raw used percentage as reported by the provider, 0-100.
    pub used_percent: Option<f64>,
    /// Raw used units (e.g. Copilot credits consumed from the included pool).
    pub used_units: Option<f64>,
    /// Total allowance in the same units, when known.
    pub allowance_units: Option<f64>,
    /// True when `allowance_units` came from user configuration, not the provider.
    pub allowance_is_configured: bool,
    /// Units consumed beyond the included allowance (billed overage).
    pub overage_units: Option<f64>,
    /// UTC time the window resets, when known.
    pub resets_at: Option<DateTime<Utc>>,
    /// Provider-side data timestamp, when the provider reports one.
    pub provider_data_at: Option<DateTime<Utc>>,
}

impl QuotaWindow {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        QuotaWindow {
            id: id.into(),
            label: label.into(),
            used_percent: None,
            used_units: None,
            allowance_units: None,
            allowance_is_configured: false,
            overage_units: None,
            resets_at: None,
            provider_data_at: None,
        }
    }

    /// Remaining percentage bounded to 0-100, or None when no valid basis exists.
    /// Missing or malformed data must never become 0% used or 100% left.
    pub fn remaining_percent(&self) -> Option<f64> {
        if let Some(used) = self.used_percent {
            if used.is_finite() {
                return Some((100.0 - used).clamp(0.0, 100.0));
            }
            return None;
        }
        match (self.used_units, self.allowance_units) {
            (Some(used), Some(total)) if total > 0.0 && used.is_finite() && total.is_finite() => {
                Some(((1.0 - used / total) * 100.0).clamp(0.0, 100.0))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProviderStatus {
    /// Provider disabled or missing required configuration.
    NotConfigured(String),
    /// First fetch is running and no data exists yet.
    Loading,
    /// Last fetch succeeded.
    Ready,
    /// Last fetch failed; retained windows are from an earlier success.
    Stale(String),
    /// Authentication is missing or rejected.
    SignInRequired(String),
    /// The provider asked us to back off.
    RateLimited(String),
    /// The interface is unavailable (e.g. CLI not installed, plan without limits).
    Unsupported(String),
    /// Any other failure with no retained data.
    Error(String),
}

#[derive(Clone, Debug)]
pub struct ProviderSnapshot {
    pub status: ProviderStatus,
    pub windows: Vec<QuotaWindow>,
    /// Local collection time of the retained windows.
    pub collected_at: Option<DateTime<Utc>>,
    /// Identity of the account the retained windows belong to.
    pub account_key: Option<String>,
}

impl ProviderSnapshot {
    pub fn not_configured(reason: impl Into<String>) -> Self {
        ProviderSnapshot {
            status: ProviderStatus::NotConfigured(reason.into()),
            windows: Vec::new(),
            collected_at: None,
            account_key: None,
        }
    }
}

/// Successful fetch result produced by a provider adapter.
#[derive(Clone, Debug)]
pub struct FetchOutcome {
    pub account_key: Option<String>,
    pub windows: Vec<QuotaWindow>,
}

/// Classified fetch failure.
#[derive(Clone, Debug, PartialEq)]
pub enum FetchError {
    NotConfigured(String),
    SignInRequired(String),
    RateLimited {
        retry_after_secs: Option<u64>,
        message: String,
    },
    Unsupported(String),
    Timeout(String),
    Other(String),
}

impl FetchError {
    pub fn message(&self) -> &str {
        match self {
            FetchError::NotConfigured(m)
            | FetchError::SignInRequired(m)
            | FetchError::Unsupported(m)
            | FetchError::Timeout(m)
            | FetchError::Other(m) => m,
            FetchError::RateLimited { message, .. } => message,
        }
    }
}

#[derive(Debug, Default)]
pub struct AppState {
    providers: BTreeMap<ProviderKind, ProviderSnapshot>,
}

impl AppState {
    pub fn snapshot(&self, kind: ProviderKind) -> Option<&ProviderSnapshot> {
        self.providers.get(&kind)
    }

    pub fn set_not_configured(&mut self, kind: ProviderKind, reason: impl Into<String>) {
        self.providers
            .insert(kind, ProviderSnapshot::not_configured(reason));
    }

    pub fn mark_loading(&mut self, kind: ProviderKind) {
        let entry = self
            .providers
            .entry(kind)
            .or_insert_with(|| ProviderSnapshot {
                status: ProviderStatus::Loading,
                windows: Vec::new(),
                collected_at: None,
                account_key: None,
            });
        if entry.windows.is_empty() {
            entry.status = ProviderStatus::Loading;
        }
    }

    /// Apply a successful fetch. Returns true when the account identity changed,
    /// so callers can reset per-account bookkeeping such as notification state.
    pub fn apply_success(
        &mut self,
        kind: ProviderKind,
        outcome: FetchOutcome,
        now: DateTime<Utc>,
    ) -> bool {
        let account_changed = self
            .providers
            .get(&kind)
            .and_then(|s| s.account_key.as_ref())
            .is_some_and(|old| outcome.account_key.as_ref() != Some(old));
        self.providers.insert(
            kind,
            ProviderSnapshot {
                status: ProviderStatus::Ready,
                windows: outcome.windows,
                collected_at: Some(now),
                account_key: outcome.account_key,
            },
        );
        account_changed
    }

    /// Apply a failed fetch. A failure in one adapter never erases another
    /// adapter's data, and transient failures keep this adapter's last good
    /// windows, labelled stale.
    pub fn apply_failure(&mut self, kind: ProviderKind, err: &FetchError) {
        let entry = self
            .providers
            .entry(kind)
            .or_insert_with(|| ProviderSnapshot {
                status: ProviderStatus::Loading,
                windows: Vec::new(),
                collected_at: None,
                account_key: None,
            });
        let msg = err.message().to_string();
        entry.status = match err {
            FetchError::NotConfigured(_) => ProviderStatus::NotConfigured(msg),
            FetchError::SignInRequired(_) => ProviderStatus::SignInRequired(msg),
            FetchError::RateLimited { .. } => ProviderStatus::RateLimited(msg),
            FetchError::Unsupported(_) => ProviderStatus::Unsupported(msg),
            FetchError::Timeout(_) | FetchError::Other(_) => {
                if entry.windows.is_empty() {
                    ProviderStatus::Error(msg)
                } else {
                    ProviderStatus::Stale(msg)
                }
            }
        };
        // Sign-in and configuration problems invalidate retained account data.
        if matches!(
            entry.status,
            ProviderStatus::SignInRequired(_) | ProviderStatus::NotConfigured(_)
        ) {
            entry.windows.clear();
            entry.collected_at = None;
            entry.account_key = None;
        }
    }
}

/// Short local countdown such as `7h`, `18h`, `7d` or `12m`.
/// Reaching a reset timestamp does not prove the allowance is restored,
/// so an elapsed timestamp renders as `checking reset`.
pub fn format_countdown(resets_at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (resets_at - now).num_seconds();
    if secs <= 0 {
        return "checking reset".to_string();
    }
    let mins = (secs + 59) / 60;
    if mins < 60 {
        return format!("{mins}m");
    }
    let hours = mins / 60;
    if hours < 48 {
        return format!("{hours}h");
    }
    format!("{}d", hours / 24)
}

/// Compact value such as `75% left` or `412 used`, with "left" used
/// consistently and no invented percentage without a valid denominator.
pub fn format_window_value(window: &QuotaWindow) -> Option<String> {
    if let Some(left) = window.remaining_percent() {
        return Some(format!("{}% left", left.round() as i64));
    }
    window.used_units.map(|used| {
        if used.fract() == 0.0 {
            format!("{} used", used as i64)
        } else {
            format!("{used:.1} used")
        }
    })
}

/// One tooltip row for a window, e.g. `Claude: 75% left | 7h`.
pub fn format_window_row(window: &QuotaWindow, now: DateTime<Utc>, stale: bool) -> Option<String> {
    let value = format_window_value(window)?;
    let mut row = format!("{}: {}", window.label, value);
    if let Some(resets_at) = window.resets_at {
        row.push_str(&format!(" | {}", format_countdown(resets_at, now)));
    }
    if stale {
        row.push_str(" (stale)");
    }
    Some(row)
}

fn provider_status_row(kind: ProviderKind, status: &ProviderStatus) -> Option<String> {
    let text = match status {
        ProviderStatus::Loading => "loading",
        ProviderStatus::SignInRequired(_) => "sign-in required",
        ProviderStatus::Unsupported(_) => "unavailable",
        ProviderStatus::RateLimited(_) => "rate limited",
        ProviderStatus::Error(_) => "error",
        _ => return None,
    };
    Some(format!("{}: {}", kind.display_name(), text))
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Build the compact tooltip within the Windows limit. `selection` lists the
/// window ids the user wants in the tooltip; an empty selection shows the
/// first window of each provider that has data.
pub fn format_tooltip(state: &AppState, selection: &[String], now: DateTime<Utc>) -> String {
    let mut rows: Vec<String> = Vec::new();

    let selected_ids: Vec<String> = if selection.is_empty() {
        let mut ids = Vec::new();
        for kind in ProviderKind::ALL {
            if let Some(snap) = state.snapshot(kind) {
                if let Some(w) = snap.windows.first() {
                    ids.push(w.id.clone());
                }
            }
        }
        ids
    } else {
        selection.to_vec()
    };

    for id in &selected_ids {
        for kind in ProviderKind::ALL {
            let Some(snap) = state.snapshot(kind) else {
                continue;
            };
            if let Some(w) = snap.windows.iter().find(|w| &w.id == id) {
                let stale = matches!(snap.status, ProviderStatus::Stale(_));
                if let Some(row) = format_window_row(w, now, stale) {
                    rows.push(row);
                }
            }
        }
    }

    // Providers without data still get an explicit status row.
    for kind in ProviderKind::ALL {
        if let Some(snap) = state.snapshot(kind) {
            if snap.windows.is_empty() {
                if let Some(row) = provider_status_row(kind, &snap.status) {
                    rows.push(row);
                }
            }
        }
    }

    if rows.is_empty() {
        return "AIAllowance".to_string();
    }

    let mut out = String::new();
    let mut used = 0usize;
    for row in rows {
        let sep = usize::from(!out.is_empty());
        let len = utf16_len(&row) + sep;
        if used + len > TOOLTIP_MAX_UTF16 {
            break;
        }
        if sep == 1 {
            out.push('\n');
        }
        out.push_str(&row);
        used += len;
    }
    if out.is_empty() {
        out = "AIAllowance".to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn remaining_percent_from_used_percent_is_bounded() {
        let mut w = QuotaWindow::new("x", "X");
        w.used_percent = Some(63.0);
        assert_eq!(w.remaining_percent(), Some(37.0));
        w.used_percent = Some(130.0);
        assert_eq!(w.remaining_percent(), Some(0.0));
        w.used_percent = Some(-5.0);
        assert_eq!(w.remaining_percent(), Some(100.0));
    }

    #[test]
    fn missing_values_never_become_full_or_empty() {
        let mut w = QuotaWindow::new("x", "X");
        assert_eq!(w.remaining_percent(), None);
        w.used_percent = Some(f64::NAN);
        assert_eq!(w.remaining_percent(), None);
        w.used_percent = None;
        w.used_units = Some(10.0);
        // No denominator: no percentage.
        assert_eq!(w.remaining_percent(), None);
        w.allowance_units = Some(0.0);
        assert_eq!(w.remaining_percent(), None);
    }

    #[test]
    fn remaining_percent_from_units() {
        let mut w = QuotaWindow::new("x", "X");
        w.used_units = Some(75.0);
        w.allowance_units = Some(300.0);
        assert_eq!(w.remaining_percent(), Some(75.0));
        w.used_units = Some(400.0);
        assert_eq!(w.remaining_percent(), Some(0.0));
    }

    #[test]
    fn countdown_formatting() {
        let now = t("2026-11-16T00:00:00Z");
        assert_eq!(format_countdown(t("2026-11-23T00:00:00Z"), now), "7d");
        assert_eq!(format_countdown(t("2026-11-16T18:00:00Z"), now), "18h");
        assert_eq!(format_countdown(t("2026-11-17T23:00:00Z"), now), "47h");
        assert_eq!(format_countdown(t("2026-11-18T00:00:00Z"), now), "2d");
        assert_eq!(format_countdown(t("2026-11-16T00:12:30Z"), now), "13m");
        assert_eq!(
            format_countdown(t("2026-11-15T00:00:00Z"), now),
            "checking reset"
        );
    }

    #[test]
    fn tooltip_stays_within_limit_and_formats_rows() {
        let now = t("2026-11-16T00:00:00Z");
        let mut state = AppState::default();
        let mut claude = QuotaWindow::new("claude:overall", "Claude");
        claude.used_percent = Some(25.0);
        claude.resets_at = Some(t("2026-11-16T07:00:00Z"));
        let mut codex = QuotaWindow::new("codex:codex:primary", "Work/Codex");
        codex.used_percent = Some(50.0);
        codex.resets_at = Some(t("2026-11-16T18:00:00Z"));
        state.apply_success(
            ProviderKind::Claude,
            FetchOutcome {
                account_key: None,
                windows: vec![claude],
            },
            now,
        );
        state.apply_success(
            ProviderKind::Codex,
            FetchOutcome {
                account_key: None,
                windows: vec![codex],
            },
            now,
        );
        let tip = format_tooltip(&state, &[], now);
        assert_eq!(tip, "Claude: 75% left | 7h\nWork/Codex: 50% left | 18h");
        assert!(tip.encode_utf16().count() <= TOOLTIP_MAX_UTF16);
    }

    #[test]
    fn tooltip_truncates_rather_than_overflowing() {
        let now = Utc.with_ymd_and_hms(2026, 11, 16, 0, 0, 0).unwrap();
        let mut state = AppState::default();
        let mut windows = Vec::new();
        for i in 0..12 {
            let mut w = QuotaWindow::new(
                format!("codex:bucket{i}:primary"),
                format!("Work/Codex extra bucket number {i}"),
            );
            w.used_percent = Some(50.0);
            w.resets_at = Some(now + chrono::Duration::hours(5));
            windows.push(w);
        }
        let ids: Vec<String> = windows.iter().map(|w| w.id.clone()).collect();
        state.apply_success(
            ProviderKind::Codex,
            FetchOutcome {
                account_key: None,
                windows,
            },
            now,
        );
        let tip = format_tooltip(&state, &ids, now);
        assert!(tip.encode_utf16().count() <= TOOLTIP_MAX_UTF16);
        assert!(tip.starts_with("Work/Codex extra bucket number 0"));
    }

    #[test]
    fn stale_failure_keeps_last_good_values() {
        let now = t("2026-11-16T00:00:00Z");
        let mut state = AppState::default();
        let mut w = QuotaWindow::new("claude:overall", "Claude");
        w.used_percent = Some(10.0);
        state.apply_success(
            ProviderKind::Claude,
            FetchOutcome {
                account_key: Some("a".into()),
                windows: vec![w],
            },
            now,
        );
        state.apply_failure(ProviderKind::Claude, &FetchError::Timeout("slow".into()));
        let snap = state.snapshot(ProviderKind::Claude).unwrap();
        assert!(matches!(snap.status, ProviderStatus::Stale(_)));
        assert_eq!(snap.windows.len(), 1);

        // A failure in one adapter must not erase another adapter's data.
        let mut c = QuotaWindow::new("copilot:credits", "Copilot");
        c.used_units = Some(5.0);
        state.apply_success(
            ProviderKind::Copilot,
            FetchOutcome {
                account_key: None,
                windows: vec![c],
            },
            now,
        );
        state.apply_failure(ProviderKind::Claude, &FetchError::Other("boom".into()));
        assert_eq!(
            state.snapshot(ProviderKind::Copilot).unwrap().windows.len(),
            1
        );
    }

    #[test]
    fn sign_in_required_clears_cached_account_data() {
        let now = t("2026-11-16T00:00:00Z");
        let mut state = AppState::default();
        let mut w = QuotaWindow::new("copilot:credits", "Copilot");
        w.used_units = Some(5.0);
        state.apply_success(
            ProviderKind::Copilot,
            FetchOutcome {
                account_key: Some("user1".into()),
                windows: vec![w],
            },
            now,
        );
        state.apply_failure(
            ProviderKind::Copilot,
            &FetchError::SignInRequired("token rejected".into()),
        );
        let snap = state.snapshot(ProviderKind::Copilot).unwrap();
        assert!(snap.windows.is_empty());
        assert!(snap.account_key.is_none());
    }

    #[test]
    fn account_change_is_reported() {
        let now = t("2026-11-16T00:00:00Z");
        let mut state = AppState::default();
        let outcome = |key: &str| FetchOutcome {
            account_key: Some(key.to_string()),
            windows: vec![QuotaWindow::new("codex:codex:primary", "Work/Codex")],
        };
        assert!(!state.apply_success(ProviderKind::Codex, outcome("a"), now));
        assert!(!state.apply_success(ProviderKind::Codex, outcome("a"), now));
        assert!(state.apply_success(ProviderKind::Codex, outcome("b"), now));
    }
}

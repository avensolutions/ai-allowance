//! Threshold and reset notifications.
//!
//! Alert decisions are pure so they can be tested; toast delivery lives in
//! the platform layer. Alerts fire on crossings, never on every refresh, are
//! deduplicated per window and reset cycle, and are only computed from fresh
//! (non-stale) data.

use std::collections::{HashMap, HashSet};

use crate::state::QuotaWindow;

/// Remaining-percentage thresholds, highest first.
pub const THRESHOLDS: [u8; 2] = [20, 10];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Alert {
    /// Remaining allowance crossed at or below a threshold.
    Low { label: String, threshold: u8 },
    /// A new reset cycle was confirmed by a fresh fetch.
    Reset { label: String },
}

#[derive(Debug, Default)]
pub struct AlertEngine {
    /// (window id, cycle key, threshold) combinations already notified.
    notified: HashSet<(String, String, u8)>,
    /// Last observed cycle key per window id.
    last_cycle: HashMap<String, String>,
    /// Last observed remaining percentage per window id.
    last_remaining: HashMap<String, f64>,
}

fn cycle_key(window: &QuotaWindow) -> String {
    window
        .resets_at
        .map(|t| t.to_rfc3339())
        .unwrap_or_else(|| "none".to_string())
}

impl AlertEngine {
    /// Observe a freshly fetched window and return alerts to deliver.
    /// Callers must not pass stale or unavailable data.
    pub fn observe_fresh(&mut self, window: &QuotaWindow) -> Vec<Alert> {
        let mut alerts = Vec::new();
        let id = window.id.clone();
        let cycle = cycle_key(window);
        let remaining = window.remaining_percent();

        if let (Some(prev_cycle), Some(rem)) = (self.last_cycle.get(&id), remaining) {
            if prev_cycle != &cycle && prev_cycle != "none" {
                let grew = self
                    .last_remaining
                    .get(&id)
                    .is_none_or(|prev_rem| rem > *prev_rem);
                if grew {
                    alerts.push(Alert::Reset {
                        label: window.label.clone(),
                    });
                }
            }
        }

        if let Some(rem) = remaining {
            // All thresholds the current value is at or below; alert only the
            // tightest new one, and mark the looser ones notified so a later
            // bounce back up cannot re-announce an old crossing.
            let crossed: Vec<u8> = THRESHOLDS
                .into_iter()
                .filter(|t| rem <= f64::from(*t))
                .collect();
            if let Some(&tightest) = crossed.iter().min() {
                let mut tightest_is_new = false;
                for t in &crossed {
                    let newly = self.notified.insert((id.clone(), cycle.clone(), *t));
                    if newly && *t == tightest {
                        tightest_is_new = true;
                    }
                }
                if tightest_is_new {
                    alerts.push(Alert::Low {
                        label: window.label.clone(),
                        threshold: tightest,
                    });
                }
            }
        }

        self.last_cycle.insert(id.clone(), cycle);
        if let Some(rem) = remaining {
            self.last_remaining.insert(id, rem);
        }
        alerts
    }

    /// Forget everything for an account change.
    pub fn reset_windows_with_prefix(&mut self, id_prefix: &str) {
        self.notified
            .retain(|(id, _, _)| !id.starts_with(id_prefix));
        self.last_cycle.retain(|id, _| !id.starts_with(id_prefix));
        self.last_remaining
            .retain(|id, _| !id.starts_with(id_prefix));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};

    fn window(used: f64, resets: &str) -> QuotaWindow {
        let mut w = QuotaWindow::new("claude:overall", "Claude");
        w.used_percent = Some(used);
        w.resets_at = Some(
            DateTime::parse_from_rfc3339(resets)
                .unwrap()
                .with_timezone(&Utc),
        );
        w
    }

    #[test]
    fn threshold_alerts_fire_once_per_cycle() {
        let mut e = AlertEngine::default();
        let r = "2026-11-20T00:00:00Z";
        assert_eq!(e.observe_fresh(&window(50.0, r)), vec![]);
        assert_eq!(
            e.observe_fresh(&window(81.0, r)),
            vec![Alert::Low {
                label: "Claude".into(),
                threshold: 20
            }]
        );
        // Repeated refreshes below the threshold stay silent.
        assert_eq!(e.observe_fresh(&window(82.0, r)), vec![]);
        assert_eq!(
            e.observe_fresh(&window(91.0, r)),
            vec![Alert::Low {
                label: "Claude".into(),
                threshold: 10
            }]
        );
        assert_eq!(e.observe_fresh(&window(95.0, r)), vec![]);
    }

    #[test]
    fn jumping_straight_past_both_thresholds_alerts_tightest_only() {
        let mut e = AlertEngine::default();
        let r = "2026-11-20T00:00:00Z";
        assert_eq!(e.observe_fresh(&window(10.0, r)), vec![]);
        assert_eq!(
            e.observe_fresh(&window(95.0, r)),
            vec![Alert::Low {
                label: "Claude".into(),
                threshold: 10
            }]
        );
    }

    #[test]
    fn new_cycle_rearms_thresholds_and_confirms_reset() {
        let mut e = AlertEngine::default();
        let r1 = "2026-11-20T00:00:00Z";
        let r2 = "2026-11-27T00:00:00Z";
        assert_eq!(
            e.observe_fresh(&window(85.0, r1)),
            vec![Alert::Low {
                label: "Claude".into(),
                threshold: 20
            }]
        );
        // Fresh fetch shows a new cycle with restored allowance.
        assert_eq!(
            e.observe_fresh(&window(1.0, r2)),
            vec![Alert::Reset {
                label: "Claude".into()
            }]
        );
        // The reset alert does not repeat.
        assert_eq!(e.observe_fresh(&window(2.0, r2)), vec![]);
        // Thresholds are re-armed for the new cycle.
        assert_eq!(
            e.observe_fresh(&window(85.0, r2)),
            vec![Alert::Low {
                label: "Claude".into(),
                threshold: 20
            }]
        );
    }

    #[test]
    fn missing_percentages_never_alert() {
        let mut e = AlertEngine::default();
        let mut w = QuotaWindow::new("copilot:credits", "Copilot");
        w.used_units = Some(1000.0);
        assert_eq!(e.observe_fresh(&w), vec![]);
    }

    #[test]
    fn account_change_resets_dedup_state() {
        let mut e = AlertEngine::default();
        let r = "2026-11-20T00:00:00Z";
        assert_eq!(e.observe_fresh(&window(85.0, r)).len(), 1);
        e.reset_windows_with_prefix("claude:");
        assert_eq!(e.observe_fresh(&window(85.0, r)).len(), 1);
    }
}

//! Refresh scheduling, timeouts and duplicate suppression.
//!
//! Pure bookkeeping: the event loop asks `try_begin` before spawning a fetch,
//! and reports `complete` afterwards. Repeated hover events cannot create
//! duplicate fetches because at most one request per provider is in flight,
//! and failures back off exponentially (bounded), honouring Retry-After.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};

use crate::state::ProviderKind;

const BASE_BACKOFF_SECS: i64 = 30;
const MAX_BACKOFF_SECS: i64 = 900;

#[derive(Debug, Default)]
pub struct RefreshCoordinator {
    in_flight: HashSet<ProviderKind>,
    backoff_until: HashMap<ProviderKind, DateTime<Utc>>,
    consecutive_failures: HashMap<ProviderKind, u32>,
}

impl RefreshCoordinator {
    /// Begin a fetch when data is older than `max_age_secs`, the provider is
    /// not already in flight and no backoff applies. Returns true when the
    /// caller should spawn the fetch.
    pub fn try_begin(
        &mut self,
        provider: ProviderKind,
        last_collected: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
        max_age_secs: i64,
    ) -> bool {
        if self.in_flight.contains(&provider) {
            return false;
        }
        if let Some(until) = self.backoff_until.get(&provider) {
            if now < *until {
                return false;
            }
        }
        if let Some(collected) = last_collected {
            if (now - collected).num_seconds() < max_age_secs {
                return false;
            }
        }
        self.in_flight.insert(provider);
        true
    }

    /// Begin a user-requested fetch, ignoring data age and backoff but still
    /// suppressing duplicates.
    pub fn force_begin(&mut self, provider: ProviderKind) -> bool {
        if self.in_flight.contains(&provider) {
            return false;
        }
        self.in_flight.insert(provider);
        true
    }

    pub fn complete(
        &mut self,
        provider: ProviderKind,
        success: bool,
        retry_after_secs: Option<u64>,
        now: DateTime<Utc>,
    ) {
        self.in_flight.remove(&provider);
        if success {
            self.consecutive_failures.remove(&provider);
            self.backoff_until.remove(&provider);
            return;
        }
        let failures = self
            .consecutive_failures
            .entry(provider)
            .and_modify(|f| *f = f.saturating_add(1))
            .or_insert(1);
        let backoff_secs = match retry_after_secs {
            // Respect Retry-After exactly when the provider supplies one.
            Some(s) => (s as i64).max(1),
            None => (BASE_BACKOFF_SECS << (*failures - 1).min(10)).min(MAX_BACKOFF_SECS),
        };
        self.backoff_until
            .insert(provider, now + Duration::seconds(backoff_secs));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-16T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn duplicate_requests_are_suppressed() {
        let mut c = RefreshCoordinator::default();
        let t0 = now();
        assert!(c.try_begin(ProviderKind::Claude, None, t0, 60));
        // Repeated hover events while a fetch is running must not duplicate it.
        assert!(!c.try_begin(ProviderKind::Claude, None, t0, 60));
        assert!(!c.force_begin(ProviderKind::Claude));
        // Another provider is independent.
        assert!(c.try_begin(ProviderKind::Codex, None, t0, 60));
        c.complete(ProviderKind::Claude, true, None, t0);
        assert!(c.try_begin(ProviderKind::Claude, None, t0, 60));
    }

    #[test]
    fn fresh_data_is_not_refetched() {
        let mut c = RefreshCoordinator::default();
        let t0 = now();
        let collected = t0 - Duration::seconds(30);
        assert!(!c.try_begin(ProviderKind::Claude, Some(collected), t0, 60));
        let old = t0 - Duration::seconds(61);
        assert!(c.try_begin(ProviderKind::Claude, Some(old), t0, 60));
    }

    #[test]
    fn failures_back_off_exponentially_and_recover() {
        let mut c = RefreshCoordinator::default();
        let t0 = now();
        assert!(c.try_begin(ProviderKind::Copilot, None, t0, 60));
        c.complete(ProviderKind::Copilot, false, None, t0);
        // Backoff blocks an immediate retry.
        assert!(!c.try_begin(ProviderKind::Copilot, None, t0, 60));
        let t1 = t0 + Duration::seconds(31);
        assert!(c.try_begin(ProviderKind::Copilot, None, t1, 60));
        c.complete(ProviderKind::Copilot, false, None, t1);
        // Second failure doubles the backoff.
        assert!(!c.try_begin(ProviderKind::Copilot, None, t1 + Duration::seconds(31), 60));
        assert!(c.try_begin(ProviderKind::Copilot, None, t1 + Duration::seconds(61), 60));
        c.complete(
            ProviderKind::Copilot,
            true,
            None,
            t1 + Duration::seconds(61),
        );
        c.complete(
            ProviderKind::Copilot,
            false,
            None,
            t1 + Duration::seconds(61),
        );
        // Success reset the failure count, so backoff is back to the base.
        assert!(c.try_begin(ProviderKind::Copilot, None, t1 + Duration::seconds(92), 60));
    }

    #[test]
    fn retry_after_is_respected() {
        let mut c = RefreshCoordinator::default();
        let t0 = now();
        assert!(c.force_begin(ProviderKind::Copilot));
        c.complete(ProviderKind::Copilot, false, Some(120), t0);
        assert!(!c.try_begin(ProviderKind::Copilot, None, t0 + Duration::seconds(119), 60));
        assert!(c.try_begin(ProviderKind::Copilot, None, t0 + Duration::seconds(120), 60));
    }

    #[test]
    fn force_begin_ignores_backoff() {
        let mut c = RefreshCoordinator::default();
        let t0 = now();
        assert!(c.force_begin(ProviderKind::Claude));
        c.complete(ProviderKind::Claude, false, None, t0);
        assert!(!c.try_begin(ProviderKind::Claude, None, t0, 60));
        // Explicit "Refresh now" still works during backoff.
        assert!(c.force_begin(ProviderKind::Claude));
    }
}

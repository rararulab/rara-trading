//! Broker connection health tracking with automatic degradation detection.
//!
//! Tracks consecutive failures and maps them to health states using
//! configurable thresholds. Supports manual disable/enable and exponential
//! backoff for recovery.

use bon::Builder;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

/// Number of consecutive failures before a broker is considered degraded.
pub const DEGRADED_THRESHOLD: u32 = 3;

/// Number of consecutive failures before a broker is considered offline.
pub const OFFLINE_THRESHOLD: u32 = 6;

/// Base delay in milliseconds for recovery backoff.
pub const RECOVERY_BASE_MS: u64 = 5_000;

/// Maximum delay in milliseconds for recovery backoff.
pub const RECOVERY_MAX_MS: u64 = 60_000;

/// Health status of a broker connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum BrokerHealth {
    /// Connection is operating normally.
    Healthy,
    /// Connection is experiencing intermittent failures.
    Degraded,
    /// Connection is down or has been manually disabled.
    Offline,
}

/// Snapshot of a broker's health state for serialization and display.
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct BrokerHealthInfo {
    /// Current health status.
    pub status:               BrokerHealth,
    /// Number of consecutive failures without a success.
    pub consecutive_failures: u32,
    /// Most recent error message, if any.
    pub last_error:           Option<String>,
    /// Timestamp of the last successful operation.
    pub last_success_at:      Option<jiff::Timestamp>,
    /// Timestamp of the last failed operation.
    pub last_failure_at:      Option<jiff::Timestamp>,
    /// Whether a recovery attempt is in progress.
    pub recovering:           bool,
    /// Whether the broker has been manually disabled.
    pub disabled:             bool,
}

/// Tracks broker connection health and computes status from failure history.
///
/// Health degrades automatically based on consecutive failure counts:
/// - `>= DEGRADED_THRESHOLD` failures: `Degraded`
/// - `>= OFFLINE_THRESHOLD` failures: `Offline`
/// - Manually disabled brokers always report `Offline`.
pub struct HealthTracker {
    consecutive_failures: u32,
    last_error:           Option<String>,
    last_success_at:      Option<jiff::Timestamp>,
    last_failure_at:      Option<jiff::Timestamp>,
    recovering:           bool,
    disabled:             bool,
}

impl HealthTracker {
    /// Creates a new tracker in the healthy state.
    pub const fn new() -> Self {
        Self {
            consecutive_failures: 0,
            last_error:           None,
            last_success_at:      None,
            last_failure_at:      None,
            recovering:           false,
            disabled:             false,
        }
    }

    /// Returns the current health status based on failure count and disabled
    /// flag.
    pub const fn status(&self) -> BrokerHealth {
        if self.disabled {
            return BrokerHealth::Offline;
        }
        if self.consecutive_failures >= OFFLINE_THRESHOLD {
            BrokerHealth::Offline
        } else if self.consecutive_failures >= DEGRADED_THRESHOLD {
            BrokerHealth::Degraded
        } else {
            BrokerHealth::Healthy
        }
    }

    /// Returns a serializable snapshot of the current health state.
    pub fn info(&self) -> BrokerHealthInfo {
        BrokerHealthInfo {
            status:               self.status(),
            consecutive_failures: self.consecutive_failures,
            last_error:           self.last_error.clone(),
            last_success_at:      self.last_success_at,
            last_failure_at:      self.last_failure_at,
            recovering:           self.recovering,
            disabled:             self.disabled,
        }
    }

    /// Records a successful operation, resetting the failure counter.
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.last_error = None;
        self.last_success_at = Some(jiff::Timestamp::now());
        self.recovering = false;
    }

    /// Records a failed operation, incrementing the failure counter.
    pub fn record_failure(&mut self, error: &str) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.last_error = Some(error.to_owned());
        self.last_failure_at = Some(jiff::Timestamp::now());
    }

    /// Permanently disables the broker with a reason logged as the last error.
    pub fn disable(&mut self, reason: &str) {
        self.disabled = true;
        self.last_error = Some(reason.to_owned());
    }

    /// Re-enables a previously disabled broker.
    pub const fn enable(&mut self) { self.disabled = false; }

    /// Returns whether the broker has been manually disabled.
    pub const fn is_disabled(&self) -> bool { self.disabled }

    /// Sets the recovering flag to indicate a recovery attempt is in progress.
    pub const fn set_recovering(&mut self, recovering: bool) { self.recovering = recovering; }

    /// Computes the recovery delay for a given attempt using exponential
    /// backoff.
    ///
    /// Delay doubles each attempt starting from [`RECOVERY_BASE_MS`],
    /// capped at [`RECOVERY_MAX_MS`].
    pub fn recovery_delay_ms(attempt: u32) -> u64 {
        let delay = RECOVERY_BASE_MS.saturating_mul(1u64 << attempt.min(31));
        delay.min(RECOVERY_MAX_MS)
    }
}

impl Default for HealthTracker {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_healthy() {
        let tracker = HealthTracker::new();
        assert_eq!(tracker.status(), BrokerHealth::Healthy);
        assert!(!tracker.is_disabled());
    }

    #[test]
    fn degrades_after_threshold() {
        let mut tracker = HealthTracker::new();
        for i in 0..DEGRADED_THRESHOLD {
            // Should still be healthy before reaching the threshold
            if i < DEGRADED_THRESHOLD - 1 {
                assert_eq!(tracker.status(), BrokerHealth::Healthy);
            }
            tracker.record_failure("timeout");
        }
        assert_eq!(tracker.status(), BrokerHealth::Degraded);
    }

    #[test]
    fn goes_offline_after_threshold() {
        let mut tracker = HealthTracker::new();
        for _ in 0..OFFLINE_THRESHOLD {
            tracker.record_failure("connection refused");
        }
        assert_eq!(tracker.status(), BrokerHealth::Offline);
    }

    #[test]
    fn resets_on_success() {
        let mut tracker = HealthTracker::new();
        // Drive to offline
        for _ in 0..OFFLINE_THRESHOLD {
            tracker.record_failure("error");
        }
        assert_eq!(tracker.status(), BrokerHealth::Offline);

        tracker.record_success();
        assert_eq!(tracker.status(), BrokerHealth::Healthy);
        assert_eq!(tracker.info().consecutive_failures, 0);
    }

    #[test]
    fn permanent_error_disables() {
        let mut tracker = HealthTracker::new();
        tracker.disable("invalid API key");
        assert!(tracker.is_disabled());
        assert_eq!(tracker.status(), BrokerHealth::Offline);

        // Even with zero failures, disabled means offline
        assert_eq!(tracker.info().consecutive_failures, 0);
    }

    #[test]
    fn recovery_delay_exponential_backoff() {
        assert_eq!(HealthTracker::recovery_delay_ms(0), 5_000);
        assert_eq!(HealthTracker::recovery_delay_ms(1), 10_000);
        assert_eq!(HealthTracker::recovery_delay_ms(2), 20_000);
        assert_eq!(HealthTracker::recovery_delay_ms(3), 40_000);
        // Capped at RECOVERY_MAX_MS
        assert_eq!(HealthTracker::recovery_delay_ms(4), 60_000);
        assert_eq!(HealthTracker::recovery_delay_ms(10), 60_000);
    }
}

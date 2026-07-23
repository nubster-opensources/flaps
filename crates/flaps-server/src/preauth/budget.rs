//! Layered budget guarding every unauthenticated entry point.
//!
//! The per-identity layer already existed, keyed on the submitted username. It
//! caps brute force against one account and is blind to rotation: every fresh
//! identifier starts with a full bucket. The global and per-address layers
//! added here are what an attacker rotating identifiers runs into.

use crate::preauth::client_address::ClientAddress;
use crate::rate_limit::{RateLimitConfig, RateLimiter};

/// Configuration of the three budget layers.
#[derive(Debug, Clone, Copy)]
pub struct PreAuthBudgetConfig {
    /// Process-wide budget for unauthenticated attempts.
    pub global: RateLimitConfig,
    /// Per-connection-address budget.
    pub per_client: RateLimitConfig,
    /// Per-identity budget (username, or presented key material).
    pub per_identity: RateLimitConfig,
}

/// Why a pre-authentication attempt was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreAuthRejection {
    /// The process-wide budget for unauthenticated attempts is exhausted.
    GlobalBudgetExhausted,
    /// The budget for this client address is exhausted.
    ClientBudgetExhausted,
    /// The budget for this identity is exhausted.
    IdentityBudgetExhausted,
}

impl PreAuthRejection {
    /// Returns the delay, in seconds, to advertise in `Retry-After`.
    #[must_use]
    pub fn retry_after_seconds(&self) -> u64 {
        match self {
            Self::GlobalBudgetExhausted => GLOBAL_RETRY_AFTER_SECS,
            Self::ClientBudgetExhausted | Self::IdentityBudgetExhausted => KEYED_RETRY_AFTER_SECS,
        }
    }
}

/// Retry guidance for a saturated global budget.
const GLOBAL_RETRY_AFTER_SECS: u64 = 5;

/// Retry guidance for a saturated per-address or per-identity budget.
const KEYED_RETRY_AFTER_SECS: u64 = 30;

/// The key the global layer is bucketed under.
///
/// The global layer is a single bucket; it still goes through the ordinary
/// limiter so that all three layers share one refill implementation.
const GLOBAL_BUCKET_KEY: &str = "global";

/// Layered budget guarding every unauthenticated entry point.
pub struct PreAuthBudget {
    global: RateLimiter,
    per_client: RateLimiter,
    per_identity: RateLimiter,
}

impl PreAuthBudget {
    /// Builds a budget from the three layer configurations.
    #[must_use]
    pub fn new(config: PreAuthBudgetConfig) -> Self {
        Self {
            global: RateLimiter::new(config.global),
            per_client: RateLimiter::new(config.per_client),
            per_identity: RateLimiter::new(config.per_identity),
        }
    }

    /// Consumes one attempt across all layers, in a constant order.
    ///
    /// The layers are consulted from widest to narrowest, always in the same
    /// order, so the cheap global refusal happens before any per-key work.
    ///
    /// # Errors
    /// Returns the first layer that refused. Saturation is answered
    /// immediately: nothing is ever queued, since a waiting request holds
    /// exactly the resource an attacker is trying to exhaust.
    pub fn consume(&self, client: ClientAddress, identity: &str) -> Result<(), PreAuthRejection> {
        self.global
            .check(GLOBAL_BUCKET_KEY)
            .map_err(|_| PreAuthRejection::GlobalBudgetExhausted)?;
        self.per_client
            .check(&client.bucket_key())
            .map_err(|_| PreAuthRejection::ClientBudgetExhausted)?;
        self.per_identity
            .check(identity)
            .map_err(|_| PreAuthRejection::IdentityBudgetExhausted)?;
        Ok(())
    }

    /// Checks, without consuming, whether the wide layers (global and
    /// per-client) still admit an attempt from `client`. The per-identity
    /// layer is NOT consulted: on the SDK path the identity is the presented
    /// key, which an attacker rotates, so only the wide layers meaningfully
    /// bound a flood.
    ///
    /// # Errors
    /// Returns the widest exhausted layer.
    pub fn sdk_admits(&self, client: ClientAddress) -> Result<(), PreAuthRejection> {
        if !self.global.has_capacity(GLOBAL_BUCKET_KEY) {
            return Err(PreAuthRejection::GlobalBudgetExhausted);
        }
        if !self.per_client.has_capacity(&client.bucket_key()) {
            return Err(PreAuthRejection::ClientBudgetExhausted);
        }
        Ok(())
    }

    /// Consumes one attempt from the wide layers (global then per-client)
    /// after a FAILED SDK key lookup. Valid keys never reach this call, so
    /// legitimate SDK traffic never touches the budget.
    ///
    /// # Errors
    /// Returns the widest layer that was already exhausted at consume time.
    pub fn consume_sdk_failure(&self, client: ClientAddress) -> Result<(), PreAuthRejection> {
        self.global
            .check(GLOBAL_BUCKET_KEY)
            .map_err(|_| PreAuthRejection::GlobalBudgetExhausted)?;
        self.per_client
            .check(&client.bucket_key())
            .map_err(|_| PreAuthRejection::ClientBudgetExhausted)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn address(last_octet: u8) -> ClientAddress {
        ClientAddress::Known(IpAddr::V4(Ipv4Addr::new(203, 0, 113, last_octet)))
    }

    fn config(global: u32, per_client: u32, per_identity: u32) -> PreAuthBudgetConfig {
        let layer = |capacity| RateLimitConfig {
            enabled: true,
            capacity,
            refill_per_second: 0.000_001,
        };
        PreAuthBudgetConfig {
            global: layer(global),
            per_client: layer(per_client),
            per_identity: layer(per_identity),
        }
    }

    #[test]
    fn an_attempt_within_every_layer_is_allowed() {
        let budget = PreAuthBudget::new(config(10, 10, 10));
        assert!(budget.consume(address(1), "alice").is_ok());
    }

    #[test]
    fn rotating_identities_are_stopped_by_the_client_layer() {
        // This is the case that traverses the current code without any
        // resistance: every fresh username starts with a full bucket.
        let budget = PreAuthBudget::new(config(1_000, 3, 100));

        for attempt in 0..3 {
            assert!(
                budget
                    .consume(address(1), &format!("user-{attempt}"))
                    .is_ok(),
                "attempt {attempt} is within the per-client budget"
            );
        }

        assert_eq!(
            budget.consume(address(1), "user-3"),
            Err(PreAuthRejection::ClientBudgetExhausted),
            "a fresh identity from the same address must not buy a fresh budget"
        );
    }

    #[test]
    fn rotating_addresses_are_stopped_by_the_global_layer() {
        let budget = PreAuthBudget::new(config(3, 1_000, 1_000));

        for attempt in 0..3 {
            assert!(budget.consume(address(attempt), "alice").is_ok());
        }

        assert_eq!(
            budget.consume(address(9), "alice"),
            Err(PreAuthRejection::GlobalBudgetExhausted)
        );
    }

    #[test]
    fn a_single_identity_is_stopped_by_the_identity_layer() {
        let budget = PreAuthBudget::new(config(1_000, 1_000, 2));

        assert!(budget.consume(address(1), "alice").is_ok());
        assert!(budget.consume(address(2), "alice").is_ok());
        assert_eq!(
            budget.consume(address(3), "alice"),
            Err(PreAuthRejection::IdentityBudgetExhausted)
        );
    }

    #[test]
    fn layers_are_consulted_from_widest_to_narrowest() {
        // When several layers are exhausted at once, the widest one answers.
        // A constant order is what keeps the cheap refusal ahead of the
        // per-key work, and what makes the outcome reproducible.
        let budget = PreAuthBudget::new(config(1, 1, 1));

        assert!(budget.consume(address(1), "alice").is_ok());
        assert_eq!(
            budget.consume(address(1), "alice"),
            Err(PreAuthRejection::GlobalBudgetExhausted)
        );
    }

    #[test]
    fn unknown_addresses_share_one_budget_rather_than_escaping_it() {
        let budget = PreAuthBudget::new(config(1_000, 2, 1_000));

        assert!(budget.consume(ClientAddress::Unknown, "alice").is_ok());
        assert!(budget.consume(ClientAddress::Unknown, "bob").is_ok());
        assert_eq!(
            budget.consume(ClientAddress::Unknown, "carol"),
            Err(PreAuthRejection::ClientBudgetExhausted),
            "an absent address degrades the layer, it never disables it"
        );
    }

    #[test]
    fn sdk_admits_is_ok_within_budget_and_err_once_the_client_layer_is_drained() {
        let budget = PreAuthBudget::new(config(1_000, 3, 1_000));

        assert!(budget.sdk_admits(address(1)).is_ok());

        for attempt in 0..3 {
            assert!(
                budget.consume_sdk_failure(address(1)).is_ok(),
                "attempt {attempt} is within the per-client budget"
            );
        }

        assert_eq!(
            budget.sdk_admits(address(1)),
            Err(PreAuthRejection::ClientBudgetExhausted),
            "the per-client layer must be reported exhausted by the peek, without a further consume"
        );
    }

    #[test]
    fn consume_sdk_failure_never_touches_the_per_identity_layer() {
        let budget = PreAuthBudget::new(config(1_000, 2, 1));

        // Drain the per-client layer to zero via the SDK-failure path, on the
        // same address the login-shaped consume below will use.
        for attempt in 0..2 {
            assert!(
                budget.consume_sdk_failure(address(1)).is_ok(),
                "attempt {attempt} is within the per-client budget"
            );
        }
        assert_eq!(
            budget.sdk_admits(address(1)),
            Err(PreAuthRejection::ClientBudgetExhausted),
            "the per-client layer must now be exhausted"
        );

        // A login-shaped consume on a fresh client address, but the same
        // identity, still has its own untouched per-identity capacity: the
        // SDK-failure path above never consulted the per-identity layer.
        assert!(budget.consume(address(2), "alice").is_ok());
    }

    #[test]
    fn every_rejection_carries_a_retry_delay() {
        for rejection in [
            PreAuthRejection::GlobalBudgetExhausted,
            PreAuthRejection::ClientBudgetExhausted,
            PreAuthRejection::IdentityBudgetExhausted,
        ] {
            assert!(rejection.retry_after_seconds() >= 1);
        }
    }
}

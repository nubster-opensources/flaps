//! Layered budget guarding every unauthenticated entry point.
//!
//! Two layers, consulted widest first: a process-wide global budget and a
//! per-connection-address budget. Together they are what an attacker rotating
//! identifiers or addresses runs into, before any per-key work.
//!
//! The per-identity throttle (one account, keyed on the submitted username) is
//! not a layer here. A single component, the login rate limiter, owns that
//! policy: this budget once carried a redundant per-identity layer configured
//! identically to the login limiter and keyed on the same username, so the two
//! enforced the same rule on the same key. The layer was collapsed into the
//! login limiter (see issue #158); a login throttle still surfaces as
//! [`PreAuthRejection::IdentityBudgetExhausted`], indistinguishable from the
//! other refusals.

use crate::preauth::client_address::ClientAddress;
use crate::rate_limit::{RateLimitConfig, RateLimiter};

/// Configuration of the two budget layers.
#[derive(Debug, Clone, Copy)]
pub struct PreAuthBudgetConfig {
    /// Process-wide budget for unauthenticated attempts.
    pub global: RateLimitConfig,
    /// Per-connection-address budget.
    pub per_client: RateLimitConfig,
}

/// Why a pre-authentication attempt was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreAuthRejection {
    /// The process-wide budget for unauthenticated attempts is exhausted.
    GlobalBudgetExhausted,
    /// The budget for this client address is exhausted.
    ClientBudgetExhausted,
    /// The budget for this identity is exhausted.
    ///
    /// Raised by the login rate limiter on the login path, the single
    /// component that owns the per-account throttle. Kept as a budget
    /// rejection so a login throttle is indistinguishable from a global or
    /// per-address refusal, and carries the same `Retry-After`.
    IdentityBudgetExhausted,
}

impl PreAuthRejection {
    /// Returns the delay, in seconds, to advertise in `Retry-After`.
    ///
    /// The value is the same constant across all three variants, deliberately.
    /// If one variant advertised a different delay than the others, the header
    /// itself would disclose which budget refused the request, defeating the
    /// point of collapsing every rejection into one indistinguishable 429.
    // Kept as a method taking `self` (even though the body no longer reads
    // it) so call sites stay `rejection.retry_after_seconds()`; the uniform
    // value is exactly the point of this fix, not an oversight.
    #[must_use]
    #[allow(clippy::unused_self)]
    pub fn retry_after_seconds(&self) -> u64 {
        PREAUTH_RETRY_AFTER_SECS
    }
}

/// Retry guidance advertised for every pre-authentication rejection,
/// regardless of which budget (global, per-client or per-identity) refused.
/// Kept as one constant so the `Retry-After` header never becomes a
/// side-channel for which budget is under pressure.
const PREAUTH_RETRY_AFTER_SECS: u64 = 30;

/// The key the global layer is bucketed under.
///
/// The global layer is a single bucket; it still goes through the ordinary
/// limiter so that both layers share one refill implementation.
const GLOBAL_BUCKET_KEY: &str = "global";

/// Layered budget guarding every unauthenticated entry point.
pub struct PreAuthBudget {
    global: RateLimiter,
    per_client: RateLimiter,
}

impl PreAuthBudget {
    /// Builds a budget from the two layer configurations.
    #[must_use]
    pub fn new(config: PreAuthBudgetConfig) -> Self {
        Self {
            global: RateLimiter::new(config.global),
            per_client: RateLimiter::new(config.per_client),
        }
    }

    /// Consumes one attempt across the global and per-client layers, widest
    /// first.
    ///
    /// The layers are consulted from widest to narrowest, always in the same
    /// order, so the cheap global refusal happens before any per-key work.
    ///
    /// # Errors
    /// Returns the first layer that refused. Saturation is answered
    /// immediately: nothing is ever queued, since a waiting request holds
    /// exactly the resource an attacker is trying to exhaust.
    pub fn consume(&self, client: ClientAddress) -> Result<(), PreAuthRejection> {
        self.global
            .check(GLOBAL_BUCKET_KEY)
            .map_err(|_| PreAuthRejection::GlobalBudgetExhausted)?;
        self.per_client
            .check(&client.bucket_key())
            .map_err(|_| PreAuthRejection::ClientBudgetExhausted)?;
        Ok(())
    }

    /// Checks, without consuming, whether the per-client layer still admits
    /// an attempt from `client`.
    ///
    /// Only the per-client layer applies on the SDK path. The global layer is
    /// deliberately NOT consulted here: it is reserved for the login path, so
    /// that a flood of absent keys hitting other addresses can never throttle
    /// a valid key on an address the flood never touched.
    ///
    /// # Errors
    /// Returns `ClientBudgetExhausted` if the per-client layer is drained.
    pub fn sdk_admits(&self, client: ClientAddress) -> Result<(), PreAuthRejection> {
        if !self.per_client.has_capacity(&client.bucket_key()) {
            return Err(PreAuthRejection::ClientBudgetExhausted);
        }
        Ok(())
    }

    /// Consumes one attempt from the per-client layer after a FAILED SDK key
    /// lookup. Valid keys never reach this call, so legitimate SDK traffic
    /// never touches the budget.
    ///
    /// Only the per-client layer is consumed. The global layer is
    /// deliberately NOT consulted here: it stays reserved for the login path,
    /// so a flood of absent keys from other addresses can never throttle a
    /// valid SDK re-auth, nor `/login`, on an unaffected address.
    ///
    /// # Errors
    /// Returns `ClientBudgetExhausted` if the per-client layer was already
    /// exhausted at consume time.
    pub fn consume_sdk_failure(&self, client: ClientAddress) -> Result<(), PreAuthRejection> {
        self.per_client
            .check(&client.bucket_key())
            .map_err(|_| PreAuthRejection::ClientBudgetExhausted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn address(last_octet: u8) -> ClientAddress {
        ClientAddress::Known(IpAddr::V4(Ipv4Addr::new(203, 0, 113, last_octet)))
    }

    fn config(global: u32, per_client: u32) -> PreAuthBudgetConfig {
        let layer = |capacity| RateLimitConfig {
            enabled: true,
            capacity,
            refill_per_second: 0.000_001,
        };
        PreAuthBudgetConfig {
            global: layer(global),
            per_client: layer(per_client),
        }
    }

    #[test]
    fn an_attempt_within_every_layer_is_allowed() {
        let budget = PreAuthBudget::new(config(10, 10));
        assert!(budget.consume(address(1)).is_ok());
    }

    #[test]
    fn repeated_attempts_from_one_address_are_stopped_by_the_client_layer() {
        // The per-client layer is what a flood from a single address runs
        // into, whatever identity each attempt carries: the budget no longer
        // keys anything on the identity, so rotating usernames buys nothing.
        let budget = PreAuthBudget::new(config(1_000, 3));

        for attempt in 0..3 {
            assert!(
                budget.consume(address(1)).is_ok(),
                "attempt {attempt} is within the per-client budget"
            );
        }

        assert_eq!(
            budget.consume(address(1)),
            Err(PreAuthRejection::ClientBudgetExhausted),
            "a fourth attempt from the same address must not be admitted"
        );
    }

    #[test]
    fn rotating_addresses_are_stopped_by_the_global_layer() {
        let budget = PreAuthBudget::new(config(3, 1_000));

        for attempt in 0..3 {
            assert!(budget.consume(address(attempt)).is_ok());
        }

        assert_eq!(
            budget.consume(address(9)),
            Err(PreAuthRejection::GlobalBudgetExhausted)
        );
    }

    #[test]
    fn layers_are_consulted_from_widest_to_narrowest() {
        // When both layers are exhausted at once, the widest one answers. A
        // constant order is what keeps the cheap refusal ahead of the per-key
        // work, and what makes the outcome reproducible.
        let budget = PreAuthBudget::new(config(1, 1));

        assert!(budget.consume(address(1)).is_ok());
        assert_eq!(
            budget.consume(address(1)),
            Err(PreAuthRejection::GlobalBudgetExhausted)
        );
    }

    #[test]
    fn unknown_addresses_share_one_budget_rather_than_escaping_it() {
        let budget = PreAuthBudget::new(config(1_000, 2));

        assert!(budget.consume(ClientAddress::Unknown).is_ok());
        assert!(budget.consume(ClientAddress::Unknown).is_ok());
        assert_eq!(
            budget.consume(ClientAddress::Unknown),
            Err(PreAuthRejection::ClientBudgetExhausted),
            "an absent address degrades the layer, it never disables it"
        );
    }

    #[test]
    fn sdk_admits_is_ok_within_budget_and_err_once_the_client_layer_is_drained() {
        let budget = PreAuthBudget::new(config(1_000, 3));

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
    fn consume_sdk_failure_never_touches_the_global_layer() {
        let budget = PreAuthBudget::new(config(2, 2));

        // Drain the SDK-failure path (per-client only) from one address.
        for attempt in 0..2 {
            assert!(
                budget.consume_sdk_failure(address(1)).is_ok(),
                "attempt {attempt} is within the per-client budget"
            );
        }
        assert_eq!(
            budget.sdk_admits(address(1)),
            Err(PreAuthRejection::ClientBudgetExhausted),
            "the per-client layer must now be exhausted for address(1)"
        );

        // A login-shaped consume from a DIFFERENT address still succeeds: the
        // global layer, which `consume` also checks, was never touched by the
        // SDK-failure path above, no matter how many times it ran.
        assert!(
            budget.consume(address(2)).is_ok(),
            "the global layer must still have its full capacity: the SDK \
             path never consumes from it"
        );
    }

    #[test]
    fn every_rejection_carries_a_retry_delay() {
        let delays: Vec<u64> = [
            PreAuthRejection::GlobalBudgetExhausted,
            PreAuthRejection::ClientBudgetExhausted,
            PreAuthRejection::IdentityBudgetExhausted,
        ]
        .into_iter()
        .map(|rejection| rejection.retry_after_seconds())
        .collect();

        for delay in &delays {
            assert!(*delay >= 1);
        }
        assert_eq!(
            delays[0], delays[1],
            "the global and per-client delays must be identical"
        );
        assert_eq!(
            delays[1], delays[2],
            "the per-client and per-identity delays must be identical"
        );
    }
}

//! Cache implementations for Flaps.
//!
//! This module provides caching layers for high-performance flag evaluation.

mod redis;

pub use redis::{RedisCacheConfig, RedisFlagCache};

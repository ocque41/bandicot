//! Account/provider fallback chain for multi-subscription routing.
//!
//! Transport retries stay in `xai-grok-sampler`. This module runs **above**
//! that loop: after same-account retries exhaust on a quota/rate-limit signal,
//! Bandicot advances to the next configured hop (credential + catalog model).

mod chain;
mod classify;
mod router;
mod state;

pub use chain::{
    FallbackChainHop, FallbackConfig, FallbackMapEntry, FallbackProvider, parse_fallback_config,
};
pub use classify::{FailoverReason, classify_error_info, classify_sampling_error};
pub use router::{FallbackPlan, HopAttempt, plan_hops, resolve_hop_catalog_id};
pub use state::FallbackState;

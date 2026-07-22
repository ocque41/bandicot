use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::types::{GraphBudgets, NodeDefaults, NodeSpec, UsageAccounting};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BudgetReservationId {
    pub run_id: String,
    pub node_id: String,
    pub attempt: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetSnapshot {
    pub charged: UsageAccounting,
    pub reserved: UsageAccounting,
    pub active_reservations: usize,
    pub stopped: bool,
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct BudgetPersistentState {
    pub charged: UsageAccounting,
    pub reservations: Vec<(BudgetReservationId, UsageAccounting)>,
    pub stopped: Option<String>,
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum BudgetError {
    #[error("budget is already stopped: {0}")]
    Stopped(String),
    #[error(
        "budget limit `{dimension}` would be exceeded: limit={limit}, used={used}, requested={requested}"
    )]
    Exceeded {
        dimension: &'static str,
        limit: String,
        used: String,
        requested: String,
    },
    #[error("budget reservation does not exist")]
    ReservationMissing,
}

/// Transaction-friendly run budget accounting.
///
/// The store persists the same charged/reserved values. This in-memory form is
/// deliberately deterministic and has no timers, random choices, or provider
/// assumptions, making it safe to reconstruct after restart.
#[derive(Debug, Clone)]
pub struct BudgetLedger {
    limits: GraphBudgets,
    charged: UsageAccounting,
    reservations: BTreeMap<BudgetReservationId, UsageAccounting>,
    stopped: Option<String>,
}

impl BudgetLedger {
    pub fn new(limits: GraphBudgets) -> Self {
        Self {
            limits,
            charged: UsageAccounting::default(),
            reservations: BTreeMap::new(),
            stopped: None,
        }
    }

    pub fn from_persistent_state(limits: GraphBudgets, state: BudgetPersistentState) -> Self {
        Self {
            limits,
            charged: state.charged,
            reservations: state.reservations.into_iter().collect(),
            stopped: state.stopped,
        }
    }

    pub fn persistent_state(&self) -> BudgetPersistentState {
        BudgetPersistentState {
            charged: self.charged.clone(),
            reservations: self
                .reservations
                .iter()
                .map(|(id, usage)| (id.clone(), usage.clone()))
                .collect(),
            stopped: self.stopped.clone(),
        }
    }

    pub fn estimate_node(node: &NodeSpec, defaults: &NodeDefaults) -> UsageAccounting {
        if !node.is_model_worker() {
            return UsageAccounting {
                node_attempts: 1,
                ..UsageAccounting::default()
            };
        }
        UsageAccounting {
            input_tokens: node
                .max_input_tokens
                .or(defaults.max_input_tokens)
                .unwrap_or(1),
            output_tokens: node
                .max_output_tokens
                .or(defaults.max_output_tokens)
                .unwrap_or(1),
            model_calls: 1,
            tool_calls: node.max_tool_calls.or(defaults.max_tool_calls).unwrap_or(0) as u64,
            node_attempts: 1,
            ..UsageAccounting::default()
        }
    }

    pub fn reserve(
        &mut self,
        id: BudgetReservationId,
        requested: UsageAccounting,
    ) -> Result<(), BudgetError> {
        if let Some(reason) = &self.stopped {
            return Err(BudgetError::Stopped(reason.clone()));
        }
        let total = add_usage(&add_usage(&self.charged, &self.reserved()), &requested);
        check_limits(&self.limits, &total, &self.charged, &requested)?;
        self.reservations.insert(id, requested);
        Ok(())
    }

    /// Reconcile a reservation with provider/worker usage. When usage is not
    /// available, the conservative policy charges the full reservation.
    pub fn reconcile(
        &mut self,
        id: &BudgetReservationId,
        actual: Option<UsageAccounting>,
    ) -> Result<UsageAccounting, BudgetError> {
        let reserved = self
            .reservations
            .remove(id)
            .ok_or(BudgetError::ReservationMissing)?;
        let charged = actual.unwrap_or(reserved);
        self.charged = add_usage(&self.charged, &charged);
        if let Err(err) = check_limits(
            &self.limits,
            &self.charged,
            &UsageAccounting::default(),
            &charged,
        ) {
            self.stopped = Some(err.to_string());
            return Err(err);
        }
        Ok(charged)
    }

    pub fn release(&mut self, id: &BudgetReservationId) -> bool {
        self.reservations.remove(id).is_some()
    }

    pub fn stop(&mut self, reason: impl Into<String>) {
        self.stopped = Some(reason.into());
    }

    pub fn snapshot(&self) -> BudgetSnapshot {
        BudgetSnapshot {
            charged: self.charged.clone(),
            reserved: self.reserved(),
            active_reservations: self.reservations.len(),
            stopped: self.stopped.is_some(),
            stop_reason: self.stopped.clone(),
        }
    }

    fn reserved(&self) -> UsageAccounting {
        self.reservations
            .values()
            .fold(UsageAccounting::default(), |sum, item| {
                add_usage(&sum, item)
            })
    }
}

pub fn add_usage(left: &UsageAccounting, right: &UsageAccounting) -> UsageAccounting {
    UsageAccounting {
        input_tokens: left.input_tokens.saturating_add(right.input_tokens),
        cached_input_tokens: left
            .cached_input_tokens
            .saturating_add(right.cached_input_tokens),
        cache_write_tokens: left
            .cache_write_tokens
            .saturating_add(right.cache_write_tokens),
        output_tokens: left.output_tokens.saturating_add(right.output_tokens),
        reasoning_tokens: left.reasoning_tokens.saturating_add(right.reasoning_tokens),
        model_calls: left.model_calls.saturating_add(right.model_calls),
        tool_calls: left.tool_calls.saturating_add(right.tool_calls),
        node_attempts: left.node_attempts.saturating_add(right.node_attempts),
        generated_dynamic_nodes: left
            .generated_dynamic_nodes
            .saturating_add(right.generated_dynamic_nodes),
        loop_iterations: left.loop_iterations.saturating_add(right.loop_iterations),
        estimated_cost_usd: left.estimated_cost_usd + right.estimated_cost_usd,
        provider_reported_cost_usd: match (
            left.provider_reported_cost_usd,
            right.provider_reported_cost_usd,
        ) {
            (Some(a), Some(b)) => Some(a + b),
            (Some(value), None) | (None, Some(value)) => Some(value),
            (None, None) => None,
        },
        failures: left.failures.saturating_add(right.failures),
        rate_limited: left.rate_limited.saturating_add(right.rate_limited),
    }
}

fn check_limits(
    limits: &GraphBudgets,
    total: &UsageAccounting,
    used: &UsageAccounting,
    requested: &UsageAccounting,
) -> Result<(), BudgetError> {
    macro_rules! check_u64 {
        ($limit:expr, $field:ident, $name:literal) => {
            if let Some(limit) = $limit {
                if total.$field > limit {
                    return Err(BudgetError::Exceeded {
                        dimension: $name,
                        limit: limit.to_string(),
                        used: used.$field.to_string(),
                        requested: requested.$field.to_string(),
                    });
                }
            }
        };
    }
    check_u64!(limits.max_input_tokens, input_tokens, "input_tokens");
    check_u64!(
        limits.max_cached_input_tokens,
        cached_input_tokens,
        "cached_input_tokens"
    );
    check_u64!(
        limits.max_cache_write_tokens,
        cache_write_tokens,
        "cache_write_tokens"
    );
    check_u64!(limits.max_output_tokens, output_tokens, "output_tokens");
    check_u64!(
        limits.max_reasoning_tokens,
        reasoning_tokens,
        "reasoning_tokens"
    );
    check_u64!(limits.max_model_calls, model_calls, "model_calls");
    check_u64!(limits.max_tool_calls, tool_calls, "tool_calls");
    check_u64!(limits.max_node_attempts, node_attempts, "node_attempts");
    check_u64!(
        limits.max_generated_dynamic_nodes,
        generated_dynamic_nodes,
        "generated_dynamic_nodes"
    );
    check_u64!(
        limits.max_loop_iterations,
        loop_iterations,
        "loop_iterations"
    );
    check_u64!(limits.max_failures, failures, "failures");
    check_u64!(limits.max_rate_limited, rate_limited, "rate_limited");
    if let Some(limit) = limits.max_estimated_cost_usd {
        if total.estimated_cost_usd > limit {
            return Err(BudgetError::Exceeded {
                dimension: "estimated_cost_usd",
                limit: limit.to_string(),
                used: used.estimated_cost_usd.to_string(),
                requested: requested.estimated_cost_usd.to_string(),
            });
        }
    }
    if let (Some(limit), Some(total_cost)) = (
        limits.max_provider_reported_cost_usd,
        total.provider_reported_cost_usd,
    ) {
        if total_cost > limit {
            return Err(BudgetError::Exceeded {
                dimension: "provider_reported_cost_usd",
                limit: limit.to_string(),
                used: used.provider_reported_cost_usd.unwrap_or(0.0).to_string(),
                requested: requested
                    .provider_reported_cost_usd
                    .unwrap_or(0.0)
                    .to_string(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(index: u32) -> BudgetReservationId {
        BudgetReservationId {
            run_id: "run".into(),
            node_id: format!("node-{index}"),
            attempt: 1,
        }
    }

    #[test]
    fn reservations_prevent_concurrent_oversubscription() {
        let mut ledger = BudgetLedger::new(GraphBudgets {
            max_model_calls: Some(3),
            ..GraphBudgets::default()
        });
        for index in 0..3 {
            ledger
                .reserve(
                    id(index),
                    UsageAccounting {
                        model_calls: 1,
                        ..UsageAccounting::default()
                    },
                )
                .unwrap();
        }
        let error = ledger
            .reserve(
                id(4),
                UsageAccounting {
                    model_calls: 1,
                    ..UsageAccounting::default()
                },
            )
            .unwrap_err();
        assert!(matches!(
            error,
            BudgetError::Exceeded {
                dimension: "model_calls",
                ..
            }
        ));
        assert_eq!(ledger.snapshot().active_reservations, 3);
    }

    #[test]
    fn reconcile_releases_unused_reservation_and_charges_actual() {
        let mut ledger = BudgetLedger::new(GraphBudgets {
            max_input_tokens: Some(100),
            ..GraphBudgets::default()
        });
        ledger
            .reserve(
                id(1),
                UsageAccounting {
                    input_tokens: 80,
                    ..UsageAccounting::default()
                },
            )
            .unwrap();
        ledger
            .reconcile(
                &id(1),
                Some(UsageAccounting {
                    input_tokens: 20,
                    ..UsageAccounting::default()
                }),
            )
            .unwrap();
        let snapshot = ledger.snapshot();
        assert_eq!(snapshot.charged.input_tokens, 20);
        assert_eq!(snapshot.reserved.input_tokens, 0);
    }

    #[test]
    fn missing_usage_charges_full_reservation() {
        let mut ledger = BudgetLedger::new(GraphBudgets::default());
        ledger
            .reserve(
                id(1),
                UsageAccounting {
                    output_tokens: 50,
                    ..UsageAccounting::default()
                },
            )
            .unwrap();
        ledger.reconcile(&id(1), None).unwrap();
        assert_eq!(ledger.snapshot().charged.output_tokens, 50);
    }
}

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::normalization::canonical_graph_hash;
use super::types::{CapabilityMode, GraphSpec};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalBinding {
    pub schema_version: u32,
    pub normalized_graph_hash: String,
    pub graph_revision: u32,
    pub budget_hash: String,
    pub side_effect_summary_hash: String,
    pub permission_summary_hash: String,
    pub repository_commit: String,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionApproval {
    pub binding: ApprovalBinding,
    pub approved_at_ms: i64,
    pub acknowledgment: String,
}

#[derive(Debug, Error)]
pub enum ApprovalError {
    #[error("graph normalization failed: {0}")]
    Normalization(#[from] super::normalization::NormalizationError),
    #[error("approval is expired")]
    Expired,
    #[error("approval binding changed: {0}")]
    BindingChanged(&'static str),
    #[error("repository commit could not be resolved")]
    RepositoryCommitUnavailable,
}

impl ApprovalBinding {
    pub fn for_spec(
        spec: &GraphSpec,
        repository_commit: impl Into<String>,
        expires_at_ms: i64,
    ) -> Result<Self, ApprovalError> {
        let repository_commit = repository_commit.into();
        if repository_commit.trim().is_empty() {
            return Err(ApprovalError::RepositoryCommitUnavailable);
        }
        let side_effects = spec
            .spec
            .nodes
            .iter()
            .map(|node| (&node.id, &node.write_set, &node.external_effects))
            .collect::<Vec<_>>();
        let permissions = spec
            .spec
            .nodes
            .iter()
            .map(|node| {
                (
                    &node.id,
                    node.effective_capability(&spec.spec.defaults),
                    &node.tool_allowlist,
                    &node.tool_denylist,
                    &node.network_policy,
                )
            })
            .collect::<Vec<_>>();
        Ok(Self {
            schema_version: 1,
            normalized_graph_hash: canonical_graph_hash(spec)?,
            graph_revision: spec.metadata.graph_version,
            budget_hash: hash_json(&spec.spec.budgets),
            side_effect_summary_hash: hash_json(&side_effects),
            permission_summary_hash: hash_json(&permissions),
            repository_commit,
            expires_at_ms,
        })
    }

    pub fn verify(
        &self,
        spec: &GraphSpec,
        repository_commit: &str,
        now_ms: i64,
    ) -> Result<(), ApprovalError> {
        if now_ms >= self.expires_at_ms {
            return Err(ApprovalError::Expired);
        }
        let current = Self::for_spec(spec, repository_commit, self.expires_at_ms)?;
        macro_rules! same {
            ($field:ident, $name:literal) => {
                if self.$field != current.$field {
                    return Err(ApprovalError::BindingChanged($name));
                }
            };
        }
        same!(normalized_graph_hash, "graph");
        same!(graph_revision, "revision");
        same!(budget_hash, "budget");
        same!(side_effect_summary_hash, "side effects");
        same!(permission_summary_hash, "permissions");
        same!(repository_commit, "repository commit");
        Ok(())
    }
}

pub fn graph_requires_execution_approval(spec: &GraphSpec) -> bool {
    spec.spec.execution.orchestration_policy == super::types::OrchestrationMode::Swarm
        || spec.spec.nodes.len() >= 10
        || spec.spec.nodes.iter().any(|node| {
            node.effective_capability(&spec.spec.defaults) > CapabilityMode::ReadOnly
                || !node.external_effects.is_empty()
        })
}

pub fn resolve_repository_commit(repo: &Path) -> Result<String, ApprovalError> {
    let git_dir = repo.join(".git");
    let head = std::fs::read_to_string(git_dir.join("HEAD"))
        .map_err(|_| ApprovalError::RepositoryCommitUnavailable)?;
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref: ") {
        std::fs::read_to_string(git_dir.join(reference))
            .map(|value| value.trim().to_string())
            .map_err(|_| ApprovalError::RepositoryCommitUnavailable)
    } else if head.len() >= 7 {
        Ok(head.to_string())
    } else {
        Err(ApprovalError::RepositoryCommitUnavailable)
    }
}

fn hash_json(value: &impl Serialize) -> String {
    let encoded = serde_json::to_vec(value).unwrap_or_default();
    let digest = Sha256::digest(encoded);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::agent_graph::build_exact_100_worker_graph;

    #[test]
    fn approval_rejects_budget_mutation_and_expiry() {
        let spec = build_exact_100_worker_graph("approval");
        let binding = ApprovalBinding::for_spec(&spec, "abc123", 200).unwrap();
        binding.verify(&spec, "abc123", 199).unwrap();
        assert!(matches!(
            binding.verify(&spec, "abc123", 200),
            Err(ApprovalError::Expired)
        ));

        let mut changed = spec;
        changed.spec.budgets.max_model_calls = Some(1);
        assert!(matches!(
            binding.verify(&changed, "abc123", 100),
            Err(ApprovalError::BindingChanged("graph"))
                | Err(ApprovalError::BindingChanged("budget"))
        ));
    }

    #[test]
    fn swarm_always_requires_bound_approval() {
        assert!(graph_requires_execution_approval(
            &build_exact_100_worker_graph("approval")
        ));
    }
}

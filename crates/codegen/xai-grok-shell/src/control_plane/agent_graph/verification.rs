use std::collections::BTreeMap;
use std::path::{Component, Path};

use thiserror::Error;

use super::resources::path_patterns_overlap;
use super::types::{FindingSeverity, NodeOutput, NodeStatus};
use super::validation::{ValidationError, validate_node_output};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimState {
    Verified,
    Refuted,
    Unverified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputVerificationReport {
    pub errors: Vec<VerificationError>,
    pub claim_states: BTreeMap<String, ClaimState>,
}

impl OutputVerificationReport {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum VerificationError {
    #[error("node output failed schema validation: {0}")]
    Schema(String),
    #[error("node output status `{status:?}` is not terminal")]
    NonTerminalStatus { status: NodeStatus },
    #[error("node output confidence `{confidence}` is outside 0.0..=1.0")]
    InvalidConfidence { confidence: String },
    #[error("path `{path}` is outside the repository root")]
    PathOutsideRoot { path: String },
    #[error("changed path `{path}` is outside declared write scope")]
    ChangedPathOutsideWriteSet { path: String },
}

pub fn verify_node_output(
    output: &NodeOutput,
    repo_root: &Path,
    declared_write_set: &[String],
) -> OutputVerificationReport {
    let mut errors = validate_node_output(output)
        .into_iter()
        .map(validation_error_to_verification_error)
        .collect::<Vec<_>>();

    if !output.status.is_terminal() {
        errors.push(VerificationError::NonTerminalStatus {
            status: output.status,
        });
    }
    if !(0.0..=1.0).contains(&output.confidence) || output.confidence.is_nan() {
        errors.push(VerificationError::InvalidConfidence {
            confidence: output.confidence.to_string(),
        });
    }

    for path in output
        .files_read
        .iter()
        .chain(output.files_changed.iter())
        .chain(output.artifacts.iter().map(|artifact| &artifact.path))
        .chain(
            output
                .findings
                .iter()
                .flat_map(|finding| finding.evidence.iter().map(|evidence| &evidence.path)),
        )
    {
        if !is_repo_relative_path(path) || repo_root.join(path).starts_with(repo_root) == false {
            errors.push(VerificationError::PathOutsideRoot { path: path.clone() });
        }
    }

    if !declared_write_set.is_empty() {
        for path in &output.files_changed {
            if !declared_write_set
                .iter()
                .any(|pattern| path_patterns_overlap(pattern, path))
            {
                errors.push(VerificationError::ChangedPathOutsideWriteSet { path: path.clone() });
            }
        }
    }

    OutputVerificationReport {
        errors,
        claim_states: classify_claims(output),
    }
}

pub fn classify_claims(output: &NodeOutput) -> BTreeMap<String, ClaimState> {
    let mut states = BTreeMap::new();
    for finding in &output.findings {
        let state = if matches!(
            finding.severity,
            FindingSeverity::High | FindingSeverity::Critical
        ) {
            ClaimState::Refuted
        } else if finding.evidence.is_empty() {
            ClaimState::Unverified
        } else {
            ClaimState::Verified
        };
        states.insert(finding.claim.clone(), state);
    }
    states
}

fn validation_error_to_verification_error(error: ValidationError) -> VerificationError {
    VerificationError::Schema(error.to_string())
}

fn is_repo_relative_path(path: &str) -> bool {
    let candidate = Path::new(path);
    !candidate.is_absolute()
        && candidate.components().all(|component| {
            !matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

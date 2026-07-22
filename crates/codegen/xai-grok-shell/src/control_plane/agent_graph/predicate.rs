use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value as JsonValue;
use thiserror::Error;

use super::types::{JsonNumber, NodeId, NodeStatus, Predicate};

#[derive(Debug, Clone)]
pub struct PredicateContext<'a> {
    pub document: &'a JsonValue,
    pub statuses: &'a BTreeMap<NodeId, NodeStatus>,
    pub deadline_reached: bool,
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum PredicateError {
    #[error("invalid json path `{path}`: {reason}")]
    InvalidJsonPath { path: String, reason: String },
    #[error("compound predicate `{operator}` must contain at least one child")]
    EmptyCompound { operator: &'static str },
    #[error("predicate references unknown node `{node_id}`")]
    UnknownNode { node_id: NodeId },
    #[error("success count predicate must require at least one success")]
    InvalidSuccessCount,
    #[error("success count predicate must include at least one node")]
    EmptyNodeSet,
    #[error("in predicate must include at least one value")]
    EmptyInSet,
}

pub fn evaluate_predicate(
    predicate: &Predicate,
    context: &PredicateContext<'_>,
) -> Result<bool, PredicateError> {
    match predicate {
        Predicate::Equals { path, value } => {
            Ok(get_json_path(context.document, path)?.is_some_and(|actual| actual == value))
        }
        Predicate::NotEquals { path, value } => {
            Ok(get_json_path(context.document, path)?.is_some_and(|actual| actual != value))
        }
        Predicate::Exists { path } => Ok(get_json_path(context.document, path)?.is_some()),
        Predicate::GreaterThan { path, value } => Ok(get_json_path(context.document, path)?
            .and_then(JsonValue::as_f64)
            .is_some_and(|actual| actual > value.0)),
        Predicate::LessThan { path, value } => Ok(get_json_path(context.document, path)?
            .and_then(JsonValue::as_f64)
            .is_some_and(|actual| actual < value.0)),
        Predicate::In { path, values } => Ok(get_json_path(context.document, path)?
            .is_some_and(|actual| values.iter().any(|value| value == actual))),
        Predicate::And { predicates } => {
            for child in predicates {
                if !evaluate_predicate(child, context)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Predicate::Or { predicates } => {
            for child in predicates {
                if evaluate_predicate(child, context)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Predicate::Not { predicate } => Ok(!evaluate_predicate(predicate, context)?),
        Predicate::StatusIs { node_id, status } => Ok(context
            .statuses
            .get(node_id)
            .is_some_and(|actual| actual == status)),
        Predicate::SuccessCountAtLeast { node_set, count } => {
            let successes = node_set
                .iter()
                .filter(|node_id| {
                    context
                        .statuses
                        .get(*node_id)
                        .is_some_and(|status| *status == NodeStatus::Succeeded)
                })
                .count() as u32;
            Ok(successes >= *count)
        }
        Predicate::DeadlineReached => Ok(context.deadline_reached),
    }
}

pub fn validate_predicate(
    predicate: &Predicate,
    known_nodes: &BTreeSet<NodeId>,
) -> Result<(), PredicateError> {
    match predicate {
        Predicate::Equals { path, .. }
        | Predicate::NotEquals { path, .. }
        | Predicate::Exists { path }
        | Predicate::GreaterThan { path, .. }
        | Predicate::LessThan { path, .. } => {
            parse_json_path(path)?;
        }
        Predicate::In { path, values } => {
            parse_json_path(path)?;
            if values.is_empty() {
                return Err(PredicateError::EmptyInSet);
            }
        }
        Predicate::And { predicates } => {
            if predicates.is_empty() {
                return Err(PredicateError::EmptyCompound { operator: "and" });
            }
            for child in predicates {
                validate_predicate(child, known_nodes)?;
            }
        }
        Predicate::Or { predicates } => {
            if predicates.is_empty() {
                return Err(PredicateError::EmptyCompound { operator: "or" });
            }
            for child in predicates {
                validate_predicate(child, known_nodes)?;
            }
        }
        Predicate::Not { predicate } => validate_predicate(predicate, known_nodes)?,
        Predicate::StatusIs { node_id, .. } => {
            if !known_nodes.contains(node_id) {
                return Err(PredicateError::UnknownNode {
                    node_id: node_id.clone(),
                });
            }
        }
        Predicate::SuccessCountAtLeast { node_set, count } => {
            if *count == 0 {
                return Err(PredicateError::InvalidSuccessCount);
            }
            if node_set.is_empty() {
                return Err(PredicateError::EmptyNodeSet);
            }
            for node_id in node_set {
                if !known_nodes.contains(node_id) {
                    return Err(PredicateError::UnknownNode {
                        node_id: node_id.clone(),
                    });
                }
            }
        }
        Predicate::DeadlineReached => {}
    }

    Ok(())
}

pub fn get_json_path<'a>(
    value: &'a JsonValue,
    path: &str,
) -> Result<Option<&'a JsonValue>, PredicateError> {
    let tokens = parse_json_path(path)?;
    let mut current = value;
    for token in tokens {
        match token {
            PathToken::Key(key) => {
                let Some(object) = current.as_object() else {
                    return Ok(None);
                };
                let Some(next) = object.get(&key) else {
                    return Ok(None);
                };
                current = next;
            }
            PathToken::Index(index) => {
                let Some(array) = current.as_array() else {
                    return Ok(None);
                };
                let Some(next) = array.get(index) else {
                    return Ok(None);
                };
                current = next;
            }
        }
    }
    Ok(Some(current))
}

pub fn validate_json_path(path: &str) -> Result<(), PredicateError> {
    parse_json_path(path).map(|_| ())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PathToken {
    Key(String),
    Index(usize),
}

fn parse_json_path(path: &str) -> Result<Vec<PathToken>, PredicateError> {
    if path == "$" {
        return Ok(Vec::new());
    }
    let Some(rest) = path.strip_prefix("$.") else {
        return Err(PredicateError::InvalidJsonPath {
            path: path.to_string(),
            reason: "path must start with `$` or `$.`".to_string(),
        });
    };
    if rest.is_empty() {
        return Err(PredicateError::InvalidJsonPath {
            path: path.to_string(),
            reason: "path cannot end after `$.`".to_string(),
        });
    }

    let mut tokens = Vec::new();
    for segment in rest.split('.') {
        if segment.is_empty() {
            return Err(PredicateError::InvalidJsonPath {
                path: path.to_string(),
                reason: "path contains an empty segment".to_string(),
            });
        }
        parse_segment(path, segment, &mut tokens)?;
    }
    Ok(tokens)
}

fn parse_segment(
    original_path: &str,
    segment: &str,
    tokens: &mut Vec<PathToken>,
) -> Result<(), PredicateError> {
    let mut key = String::new();
    let mut chars = segment.chars().peekable();
    while let Some(ch) = chars.peek().copied() {
        if ch == '[' {
            break;
        }
        if !is_json_path_key_char(ch) {
            return Err(PredicateError::InvalidJsonPath {
                path: original_path.to_string(),
                reason: format!("invalid key character `{ch}`"),
            });
        }
        key.push(ch);
        chars.next();
    }
    if key.is_empty() {
        return Err(PredicateError::InvalidJsonPath {
            path: original_path.to_string(),
            reason: "segment must start with a key".to_string(),
        });
    }
    tokens.push(PathToken::Key(key));

    while chars.peek().is_some() {
        if chars.next() != Some('[') {
            return Err(PredicateError::InvalidJsonPath {
                path: original_path.to_string(),
                reason: "array indexes must use `[number]`".to_string(),
            });
        }
        let mut digits = String::new();
        while let Some(ch) = chars.peek().copied() {
            if ch == ']' {
                break;
            }
            if !ch.is_ascii_digit() {
                return Err(PredicateError::InvalidJsonPath {
                    path: original_path.to_string(),
                    reason: "array index must be a non-negative integer".to_string(),
                });
            }
            digits.push(ch);
            chars.next();
        }
        if chars.next() != Some(']') || digits.is_empty() {
            return Err(PredicateError::InvalidJsonPath {
                path: original_path.to_string(),
                reason: "array index is not closed".to_string(),
            });
        }
        let index = digits
            .parse::<usize>()
            .map_err(|_| PredicateError::InvalidJsonPath {
                path: original_path.to_string(),
                reason: "array index is too large".to_string(),
            })?;
        tokens.push(PathToken::Index(index));
    }
    Ok(())
}

fn is_json_path_key_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')
}

impl From<f64> for JsonNumber {
    fn from(value: f64) -> Self {
        Self(value)
    }
}

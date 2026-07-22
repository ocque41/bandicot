use std::collections::BTreeMap;

use serde_json::{Map, Value as JsonValue};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::types::{EdgeSpec, GraphSpec, NodeSpec, ResourceClaim};

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedGraphSpec {
    pub spec: GraphSpec,
    pub hash: String,
}

#[derive(Debug, Error)]
pub enum NormalizationError {
    #[error("failed to serialize normalized graph: {0}")]
    Serialize(#[from] serde_json::Error),
}

pub fn normalize_graph_spec(spec: &GraphSpec) -> Result<NormalizedGraphSpec, NormalizationError> {
    let mut normalized = spec.clone();
    normalized.spec.schemas = normalized
        .spec
        .schemas
        .into_iter()
        .map(|(key, value)| (key, normalize_json_value(value)))
        .collect();

    normalized
        .spec
        .nodes
        .sort_by(|left, right| left.id.cmp(&right.id));
    for node in &mut normalized.spec.nodes {
        normalize_node(node);
    }

    normalized.spec.edges.sort_by_key(edge_sort_key);
    for edge in &mut normalized.spec.edges {
        edge.bindings.sort_by(|left, right| {
            left.input
                .cmp(&right.input)
                .then(left.path.cmp(&right.path))
                .then(left.schema_ref.cmp(&right.schema_ref))
        });
    }

    let hash = hash_graph_spec(&normalized)?;
    Ok(NormalizedGraphSpec {
        spec: normalized,
        hash,
    })
}

pub fn canonical_graph_hash(spec: &GraphSpec) -> Result<String, NormalizationError> {
    normalize_graph_spec(spec).map(|normalized| normalized.hash)
}

fn hash_graph_spec(spec: &GraphSpec) -> Result<String, NormalizationError> {
    let bytes = serde_json::to_vec(spec)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn normalize_node(node: &mut NodeSpec) {
    node.read_set = normalize_string_list(std::mem::take(&mut node.read_set));
    node.write_set = normalize_string_list(std::mem::take(&mut node.write_set));
    node.tool_allowlist = normalize_string_list(std::mem::take(&mut node.tool_allowlist));
    node.tool_denylist = normalize_string_list(std::mem::take(&mut node.tool_denylist));
    node.credential_refs = normalize_string_list(std::mem::take(&mut node.credential_refs));
    node.definition_of_done = normalize_string_list(std::mem::take(&mut node.definition_of_done));
    node.evidence_requirements.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then(left.required.cmp(&right.required))
    });
    node.verification_requirements.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then(left.required.cmp(&right.required))
    });
    node.resource_claims.sort_by_key(resource_claim_sort_key);
    node.external_effects
        .sort_by(|left, right| format!("{left:?}").cmp(&format!("{right:?}")));
    node.tags = normalize_string_list(std::mem::take(&mut node.tags));
    node.routes.sort_by(|left, right| {
        left.to
            .cmp(&right.to)
            .then(left.fallback.cmp(&right.fallback))
            .then(
                predicate_sort_value(&left.predicate).cmp(&predicate_sort_value(&right.predicate)),
            )
    });
    if let Some(value) = node.output_schema.take() {
        node.output_schema = Some(normalize_json_value(value));
    }
}

fn edge_sort_key(edge: &EdgeSpec) -> (String, String, String, String) {
    (
        edge.id.clone().unwrap_or_default(),
        edge.from.clone(),
        edge.to.clone(),
        format!("{:?}", edge.kind),
    )
}

fn resource_claim_sort_key(claim: &ResourceClaim) -> (String, u32) {
    (claim.resource.clone(), claim.amount)
}

fn normalize_string_list(mut values: Vec<String>) -> Vec<String> {
    for value in &mut values {
        *value = normalize_pattern(value);
    }
    values.sort();
    values.dedup();
    values
}

fn normalize_pattern(value: &str) -> String {
    let mut normalized = value.trim().replace('\\', "/");
    while let Some(rest) = normalized.strip_prefix("./") {
        normalized = rest.to_string();
    }
    while normalized.contains("//") {
        normalized = normalized.replace("//", "/");
    }
    normalized
}

fn predicate_sort_value(predicate: &super::types::Predicate) -> String {
    serde_json::to_string(predicate).unwrap_or_else(|_| format!("{predicate:?}"))
}

pub fn normalize_json_value(value: JsonValue) -> JsonValue {
    match value {
        JsonValue::Object(map) => {
            let sorted: BTreeMap<_, _> = map
                .into_iter()
                .map(|(key, value)| (key, normalize_json_value(value)))
                .collect();
            let mut canonical = Map::new();
            for (key, value) in sorted {
                canonical.insert(key, value);
            }
            JsonValue::Object(canonical)
        }
        JsonValue::Array(values) => {
            JsonValue::Array(values.into_iter().map(normalize_json_value).collect())
        }
        other => other,
    }
}

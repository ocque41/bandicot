use std::collections::{BTreeMap, BTreeSet, VecDeque};

use thiserror::Error;

use super::types::{GraphSpec, NodeId, NodeSpec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyReport {
    pub node_count: usize,
    pub edge_count: usize,
    pub initial_ready_width: u32,
    pub initial_ready_agent_width: u32,
    pub maximum_theoretical_width: u32,
    pub critical_path_length: u32,
    pub topological_order: Vec<NodeId>,
    pub disconnected_independent_nodes: Vec<NodeId>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TopologyError {
    #[error("duplicate node id `{node_id}`")]
    DuplicateNode { node_id: NodeId },
    #[error("dependency references unknown node `{node_id}`")]
    UnknownNode { node_id: NodeId },
    #[error("graph contains a cycle involving: {nodes:?}")]
    Cycle { nodes: Vec<NodeId> },
}

pub fn analyze_topology(spec: &GraphSpec) -> Result<TopologyReport, TopologyError> {
    let node_map = build_node_map(spec)?;
    let dependencies = dependency_map(spec, &node_map)?;
    let topological_order = topological_sort(&dependencies)?;
    let roots = dependencies
        .iter()
        .filter_map(|(node_id, deps)| deps.is_empty().then_some(node_id.clone()))
        .collect::<Vec<_>>();

    let mut levels: BTreeMap<NodeId, u32> = BTreeMap::new();
    for node_id in &topological_order {
        let level = dependencies
            .get(node_id)
            .into_iter()
            .flat_map(|deps| deps.iter())
            .filter_map(|dep| levels.get(dep).copied())
            .map(|level| level + 1)
            .max()
            .unwrap_or(0);
        levels.insert(node_id.clone(), level);
    }

    let mut width_by_level: BTreeMap<u32, u32> = BTreeMap::new();
    for (node_id, level) in &levels {
        let node = node_map
            .get(node_id)
            .expect("node id from level map exists");
        *width_by_level.entry(*level).or_default() += node.expected_instance_count();
    }

    let initial_ready_width = roots
        .iter()
        .filter_map(|node_id| node_map.get(node_id))
        .map(|node| node.expected_instance_count())
        .sum();
    let initial_ready_agent_width = roots
        .iter()
        .filter_map(|node_id| node_map.get(node_id))
        .filter(|node| node.is_model_worker())
        .map(|node| node.expected_instance_count())
        .sum();

    Ok(TopologyReport {
        node_count: spec.spec.nodes.len(),
        edge_count: spec.spec.edges.len(),
        initial_ready_width,
        initial_ready_agent_width,
        maximum_theoretical_width: width_by_level.values().copied().max().unwrap_or(0),
        critical_path_length: levels.values().copied().max().map(|v| v + 1).unwrap_or(0),
        topological_order,
        disconnected_independent_nodes: roots,
    })
}

fn build_node_map<'a>(
    spec: &'a GraphSpec,
) -> Result<BTreeMap<NodeId, &'a NodeSpec>, TopologyError> {
    let mut node_map = BTreeMap::new();
    for node in &spec.spec.nodes {
        if node_map.insert(node.id.clone(), node).is_some() {
            return Err(TopologyError::DuplicateNode {
                node_id: node.id.clone(),
            });
        }
    }
    Ok(node_map)
}

fn dependency_map(
    spec: &GraphSpec,
    node_map: &BTreeMap<NodeId, &NodeSpec>,
) -> Result<BTreeMap<NodeId, BTreeSet<NodeId>>, TopologyError> {
    let mut dependencies = node_map
        .keys()
        .map(|node_id| (node_id.clone(), BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();

    for edge in &spec.spec.edges {
        ensure_known(&edge.from, node_map)?;
        ensure_known(&edge.to, node_map)?;
        dependencies
            .get_mut(&edge.to)
            .expect("target node checked")
            .insert(edge.from.clone());
    }

    for node in &spec.spec.nodes {
        for binding in node.inputs.values() {
            ensure_known(&binding.from_node, node_map)?;
            dependencies
                .get_mut(&node.id)
                .expect("node id checked")
                .insert(binding.from_node.clone());
        }
        if let Some(map) = &node.map {
            ensure_known(&map.from_node, node_map)?;
            dependencies
                .get_mut(&node.id)
                .expect("node id checked")
                .insert(map.from_node.clone());
        }
        if let Some(reduce) = &node.reduce {
            ensure_known(&reduce.from_node, node_map)?;
            dependencies
                .get_mut(&node.id)
                .expect("node id checked")
                .insert(reduce.from_node.clone());
        }
        for route in &node.routes {
            ensure_known(&route.to, node_map)?;
            dependencies
                .get_mut(&route.to)
                .expect("route target checked")
                .insert(node.id.clone());
        }
    }

    Ok(dependencies)
}

fn topological_sort(
    dependencies: &BTreeMap<NodeId, BTreeSet<NodeId>>,
) -> Result<Vec<NodeId>, TopologyError> {
    let mut remaining = dependencies.clone();
    let mut ready = remaining
        .iter()
        .filter_map(|(node_id, deps)| deps.is_empty().then_some(node_id.clone()))
        .collect::<VecDeque<_>>();
    let mut order = Vec::with_capacity(remaining.len());

    while let Some(node_id) = ready.pop_front() {
        if !remaining.contains_key(&node_id) {
            continue;
        }
        remaining.remove(&node_id);
        order.push(node_id.clone());

        let affected = remaining
            .iter_mut()
            .filter_map(|(candidate, deps)| {
                if deps.remove(&node_id) && deps.is_empty() {
                    Some(candidate.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        for candidate in affected {
            ready.push_back(candidate);
        }
    }

    if remaining.is_empty() {
        Ok(order)
    } else {
        Err(TopologyError::Cycle {
            nodes: remaining.keys().cloned().collect(),
        })
    }
}

fn ensure_known(
    node_id: &NodeId,
    node_map: &BTreeMap<NodeId, &NodeSpec>,
) -> Result<(), TopologyError> {
    if node_map.contains_key(node_id) {
        Ok(())
    } else {
        Err(TopologyError::UnknownNode {
            node_id: node_id.clone(),
        })
    }
}

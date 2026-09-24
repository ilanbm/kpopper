//! Bounded, pure storage for exact observation `saw` sets.
//!
//! This module deliberately stores no semantic object payload and makes no claim about
//! acceptance, heads, timestamps, or authority.  A node is only a sparse transition from
//! its explicitly named observation base.

use crate::{Error, Result, identity::sha256, require};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const FORMAT: &str = "node-observation/v1";
pub const MAX_NODE_BYTES: usize = 256 * 1024;
pub const MAX_CHAIN_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_NODES: usize = 100_000;
pub const MAX_IDS_PER_NODE: usize = 16_384;
pub const MAX_CARDINALITY: usize = 1_000_000;
pub const MAX_REPLAY_DEPTH: usize = MAX_NODES;

fn canonical_id(id: &str) -> bool {
    crate::history_paths::object_id(id)
}

fn sorted_unique(ids: &[String]) -> bool {
    ids.windows(2).all(|pair| pair[0] < pair[1]) && ids.iter().all(|id| canonical_id(id))
}

fn saw_digest(saw: &BTreeSet<String>) -> String {
    let mut bytes = b"node-observation-saw/v1\0".to_vec();
    for id in saw {
        bytes.extend_from_slice(id.as_bytes());
        bytes.push(0);
    }
    sha256(&bytes)
}

/// One exact sparse transition. `base` is the sole encoding base reference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationNode {
    pub format: String,
    /// Exact v2 observation ID of this node. It is supplied by the caller and preserved.
    pub id: String,
    /// The exact observation whose reconstructed saw set is the encoding base.
    pub base: Option<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Cardinality after applying this node's transition.
    pub cardinality: usize,
    pub saw_digest: String,
}

impl ObservationNode {
    pub fn root(id: impl Into<String>, saw: &BTreeSet<String>) -> Result<Self> {
        let id = id.into();
        let node = Self {
            format: FORMAT.into(),
            id,
            base: None,
            added: saw.iter().cloned().collect(),
            removed: Vec::new(),
            cardinality: saw.len(),
            saw_digest: saw_digest(saw),
        };
        node.validate()?;
        Ok(node)
    }

    pub fn delta(
        id: impl Into<String>,
        base: impl Into<String>,
        added: Vec<String>,
        removed: Vec<String>,
        cardinality: usize,
        saw_digest: impl Into<String>,
    ) -> Result<Self> {
        let node = Self {
            format: FORMAT.into(),
            id: id.into(),
            base: Some(base.into()),
            added,
            removed,
            cardinality,
            saw_digest: saw_digest.into(),
        };
        node.validate()?;
        Ok(node)
    }

    pub fn validate(&self) -> Result<()> {
        require(self.format == FORMAT, "node_observation_format")?;
        require(canonical_id(&self.id), "node_observation_id")?;
        require(
            self.base.as_ref().is_none_or(|id| canonical_id(id)),
            "node_observation_base",
        )?;
        require(
            self.added.len() <= MAX_IDS_PER_NODE && self.removed.len() <= MAX_IDS_PER_NODE,
            "node_observation_delta_limit",
        )?;
        require(
            self.cardinality <= MAX_CARDINALITY,
            "node_observation_cardinality_limit",
        )?;
        require(
            sorted_unique(&self.added) && sorted_unique(&self.removed),
            "node_observation_delta",
        )?;
        require(
            self.added
                .iter()
                .all(|id| self.removed.binary_search(id).is_err()),
            "node_observation_delta",
        )?;
        require(
            self.saw_digest.len() == 64 && canonical_id(&self.saw_digest),
            "node_observation_digest",
        )?;
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        require(bytes.len() <= MAX_NODE_BYTES, "node_observation_node_limit")?;
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        require(
            bytes.len() <= MAX_NODE_BYTES && bytes.last() == Some(&b'\n'),
            "node_observation_frame",
        )?;
        let node: Self = serde_json::from_slice(&bytes[..bytes.len() - 1])?;
        require(node.encode()? == bytes, "node_observation_noncanonical")?;
        Ok(node)
    }
}

/// A validated observation DAG. Nodes are retained in compact form; expanded sets are transient.
#[derive(Clone, Debug, Default)]
pub struct ObservationChain {
    nodes: BTreeMap<String, ObservationNode>,
    encoded_bytes: usize,
}

impl ObservationChain {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_nodes(nodes: impl IntoIterator<Item = ObservationNode>) -> Result<Self> {
        let mut chain = Self::new();
        for node in nodes {
            chain.insert_unchecked_base(node)?;
        }
        chain.validate_graph()?;
        Ok(chain)
    }

    pub fn insert(&mut self, node: ObservationNode) -> Result<()> {
        if let Some(base) = &node.base {
            require(
                self.nodes.contains_key(base),
                "node_observation_missing_base",
            )?;
        }
        self.insert_unchecked_base(node)
    }
    fn insert_unchecked_base(&mut self, node: ObservationNode) -> Result<()> {
        node.validate()?;
        require(self.nodes.len() < MAX_NODES, "node_observation_node_limit")?;
        require(
            !self.nodes.contains_key(&node.id),
            "node_observation_duplicate",
        )?;
        let bytes = node.encode()?.len();
        require(
            self.encoded_bytes.saturating_add(bytes) <= MAX_CHAIN_BYTES,
            "node_observation_chain_limit",
        )?;
        self.encoded_bytes += bytes;
        self.nodes.insert(node.id.clone(), node);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&ObservationNode> {
        self.nodes.get(id)
    }
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
    pub fn encoded_bytes(&self) -> usize {
        self.encoded_bytes
    }

    fn validate_graph(&self) -> Result<()> {
        let mut children = BTreeMap::<&str, Vec<&str>>::new();
        let mut ready = VecDeque::new();
        for node in self.nodes.values() {
            if let Some(base) = &node.base {
                require(
                    self.nodes.contains_key(base),
                    "node_observation_missing_base",
                )?;
                children.entry(base).or_default().push(&node.id);
            } else {
                ready.push_back(node.id.as_str());
            }
        }
        let mut seen = 0;
        while let Some(id) = ready.pop_front() {
            seen += 1;
            for child in children.get(id).into_iter().flatten() {
                ready.push_back(child);
            }
        }
        require(seen == self.nodes.len(), "node_observation_cycle")
    }

    fn path_ids(&self, id: &str) -> Result<Vec<String>> {
        let mut path = Vec::new();
        require(self.nodes.contains_key(id), "node_observation_missing_base")?;
        let mut seen = BTreeSet::new();
        let mut current = Some(id);
        for _ in 0..=MAX_REPLAY_DEPTH {
            let Some(current_id) = current else {
                return Ok(path);
            };
            require(seen.insert(current_id), "node_observation_cycle")?;
            path.push(current_id.to_owned());
            current = self.nodes.get(current_id).and_then(|n| n.base.as_deref());
        }
        Err(Error("node_observation_depth".into()))
    }

    /// Reconstruct one exact set by replaying its explicit base chain.
    pub fn replay(&self, id: &str) -> Result<BTreeSet<String>> {
        let ids = self.path_ids(id)?;
        let mut saw = BTreeSet::new();
        for node_id in ids.iter().rev() {
            let node = self
                .nodes
                .get(node_id)
                .ok_or_else(|| Error("node_observation_missing_base".into()))?;
            for value in &node.removed {
                require(saw.remove(value), "node_observation_remove")?;
            }
            for value in &node.added {
                require(saw.insert(value.clone()), "node_observation_add")?;
            }
            require(
                saw.len() == node.cardinality && saw_digest(&saw) == node.saw_digest,
                "node_observation_state",
            )?;
        }
        Ok(saw)
    }

    /// Visit each state once, retaining one exact expanded set across forks.
    pub fn visit<F>(&self, mut visitor: F) -> Result<()>
    where
        F: FnMut(&ObservationNode, &BTreeSet<String>) -> Result<()>,
    {
        self.visit_states(|node, saw, enter| {
            if enter {
                visitor(node, saw)?;
            }
            Ok(())
        })
    }

    /// Sparse transition notifications after the corresponding exact state is validated.
    pub fn visit_sparse<F>(&self, mut visitor: F) -> Result<()>
    where
        F: FnMut(&ObservationNode, bool) -> Result<()>,
    {
        self.visit_states(|node, _, enter| visitor(node, enter))
    }

    pub(crate) fn visit_states<F>(&self, mut visitor: F) -> Result<()>
    where
        F: FnMut(&ObservationNode, &BTreeSet<String>, bool) -> Result<()>,
    {
        let mut children = BTreeMap::<Option<&str>, Vec<&str>>::new();
        for node in self.nodes.values() {
            children
                .entry(node.base.as_deref())
                .or_default()
                .push(&node.id);
        }
        let mut pending = children
            .get(&None)
            .into_iter()
            .flatten()
            .rev()
            .map(|id| (*id, false))
            .collect::<Vec<_>>();
        let mut saw = BTreeSet::new();
        let mut visited = 0;
        while let Some((id, exit)) = pending.pop() {
            let node = &self.nodes[id];
            if exit {
                visitor(node, &saw, false)?;
                for added in &node.added {
                    require(saw.remove(added), "node_observation_undo")?;
                }
                for removed in &node.removed {
                    require(saw.insert(removed.clone()), "node_observation_undo")?;
                }
            } else {
                for removed in &node.removed {
                    require(saw.remove(removed), "node_observation_remove")?;
                }
                for added in &node.added {
                    require(saw.insert(added.clone()), "node_observation_add")?;
                }
                require(
                    saw.len() == node.cardinality && saw_digest(&saw) == node.saw_digest,
                    "node_observation_state",
                )?;
                visitor(node, &saw, true)?;
                visited += 1;
                pending.push((id, true));
                for child in children.get(&Some(id)).into_iter().flatten().rev() {
                    pending.push((child, false));
                }
            }
        }
        require(visited == self.nodes.len(), "node_observation_cycle")
    }
}

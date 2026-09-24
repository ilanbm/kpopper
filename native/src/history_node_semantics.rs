//! Semantic adapter for compact node-local history observations.
//!
//! Objects retain their original typed payload with `saw` omitted. Observation nodes
//! supply the exact direct membership relation needed by the legacy reducer.

use crate::{
    Error, Result,
    history_contract::{self, map, references, string_is, text, validate_object},
    history_node_observation::{ObservationChain, ObservationNode},
    history_reduce::{self, SawProvider},
    require,
    value::TypedValue as V,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
struct MembershipIndex {
    times: BTreeMap<String, usize>,
    cardinalities: BTreeMap<String, usize>,
    intervals: BTreeMap<String, Vec<(usize, usize)>>,
}

impl MembershipIndex {
    fn build(
        chain: &ObservationChain,
        mut verify: impl FnMut(&ObservationNode, &BTreeSet<String>) -> Result<()>,
    ) -> Result<Self> {
        let mut times = BTreeMap::new();
        let mut intervals: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
        let mut cardinalities = BTreeMap::new();
        let mut active = BTreeMap::<String, usize>::new();
        let mut clock = 0usize;
        chain.visit_states(|node, saw, enter| {
            if enter {
                verify(node, saw)?;
                for id in &node.removed {
                    if let Some(start) = active.remove(id) {
                        intervals
                            .entry(id.clone())
                            .or_default()
                            .push((start, clock));
                    }
                }
                for id in &node.added {
                    require(
                        active.insert(id.clone(), clock).is_none(),
                        "node_observation_index",
                    )?;
                }
                times.insert(node.id.clone(), clock);
                cardinalities.insert(node.id.clone(), node.cardinality);
                clock += 1;
            } else {
                let end = clock;
                for id in &node.added {
                    let start = active
                        .remove(id)
                        .ok_or_else(|| Error("node_observation_index".into()))?;
                    intervals.entry(id.clone()).or_default().push((start, end));
                }
                for id in &node.removed {
                    require(
                        active.insert(id.clone(), end).is_none(),
                        "node_observation_index",
                    )?;
                }
            }
            Ok(())
        })?;
        for (id, start) in active {
            intervals.entry(id).or_default().push((start, clock));
        }
        Ok(Self {
            times,
            cardinalities,
            intervals,
        })
    }

    fn contains(&self, observer: &str, target: &str) -> Result<bool> {
        let time = *self
            .times
            .get(observer)
            .ok_or_else(|| Error("node_observation_missing_base".into()))?;
        Ok(self.intervals.get(target).is_some_and(|ranges| {
            ranges
                .binary_search_by(|(start, end)| {
                    if time < *start {
                        std::cmp::Ordering::Greater
                    } else if time >= *end {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
                .is_ok()
        }))
    }

    fn is_empty(&self, observer: &str) -> Result<bool> {
        self.cardinalities
            .get(observer)
            .map(|cardinality| *cardinality == 0)
            .ok_or_else(|| Error("node_observation_missing_base".into()))
    }
}

impl SawProvider for MembershipIndex {
    fn saw_is_empty(&self, id: &str) -> Result<bool> {
        self.is_empty(id)
    }
    fn saw_contains(&self, observer: &str, target: &str) -> Result<bool> {
        self.contains(observer, target)
    }
}

#[derive(Clone, Debug)]
pub struct History {
    objects: BTreeMap<String, V>,
    observations: ObservationChain,
    index: MembershipIndex,
}

impl History {
    pub fn from_objects(objects: impl IntoIterator<Item = (V, ObservationNode)>) -> Result<Self> {
        let mut compact = BTreeMap::new();
        let mut nodes = Vec::new();
        for (value, node) in objects {
            let m = map(&value)?;
            require(!m.contains_key("saw"), "compact_saw_present")?;
            let id = text(m.get("id").ok_or_else(|| Error("invalid_schema".into()))?)?.to_owned();
            require(
                compact.insert(id.clone(), value).is_none(),
                "duplicate_object",
            )?;
            require(node.id == id, "identity_mismatch")?;
            nodes.push(node);
        }
        require(
            compact.len() <= history_contract::MAX_OBJECTS,
            "history_limit",
        )?;
        let observations = ObservationChain::from_nodes(nodes)?;
        let index = MembershipIndex::build(&observations, |node, saw| {
            let compact_value = compact
                .get(&node.id)
                .ok_or_else(|| Error("incomplete_closure".into()))?;
            let mut full = map(compact_value)?.clone();
            full.insert(
                "saw".into(),
                V::List(saw.iter().cloned().map(V::Text).collect()),
            );
            let full = V::Map(full);
            require(
                string_is(map(&full)?.get("id").unwrap(), &node.id),
                "identity_mismatch",
            )?;
            for reference in references(&full)? {
                let target = compact
                    .get(&reference.id)
                    .ok_or_else(|| Error("incomplete_closure".into()))?;
                let target_m = map(target)?;
                require(
                    string_is(
                        target_m
                            .get("subject")
                            .ok_or_else(|| Error("invalid_schema".into()))?,
                        &reference.subject,
                    ) && (!reference.claim
                        || !string_is(
                            target_m
                                .get("kind")
                                .ok_or_else(|| Error("invalid_schema".into()))?,
                            "act",
                        )),
                    "reference_mismatch",
                )?;
            }
            Ok(())
        })?;
        require(index.times.len() == compact.len(), "incomplete_closure")?;
        Ok(Self {
            objects: compact,
            observations,
            index,
        })
    }

    pub fn objects(&self) -> &BTreeMap<String, V> {
        &self.objects
    }
    pub fn object(&self, id: &str) -> Result<V> {
        let compact = self
            .objects
            .get(id)
            .ok_or_else(|| Error("missing_object".into()))?;
        let saw = self.observations.replay(id)?;
        let mut full = map(compact)?.clone();
        full.insert(
            "saw".into(),
            V::List(saw.into_iter().map(V::Text).collect()),
        );
        let full = V::Map(full);
        validate_object(&full)?;
        Ok(full)
    }
    pub fn observations(&self) -> &ObservationChain {
        &self.observations
    }
    pub(crate) fn reduce_subset(
        &self,
        objects: &BTreeMap<String, V>,
        rules: Option<&BTreeMap<String, V>>,
        ancestry: Option<&crate::source_clock::Ancestry<'_>>,
    ) -> Result<V> {
        history_reduce::reduce_compact(objects, &self.index, rules, ancestry)
    }
    pub fn reduce(
        &self,
        rules: Option<&BTreeMap<String, V>>,
        ancestry: Option<&crate::source_clock::Ancestry<'_>>,
    ) -> Result<V> {
        history_reduce::reduce_compact(&self.objects, &self.index, rules, ancestry)
    }
}

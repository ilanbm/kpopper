//! Temporal observations over committed semantic objects. Publication edges never order source clocks.
use crate::{
    Result, history_authority as A,
    history_contract::*,
    history_node_capture::Capture,
    history_node_writer as W,
    history_temporal_capture::{self as T, Source},
    value::TypedValue as V,
};
use std::collections::{BTreeMap, BTreeSet};

struct NodeSource<'a> {
    capture: &'a Capture,
    introduced: BTreeMap<String, BTreeSet<String>>,
}
impl Source for NodeSource<'_> {
    fn operations(&self) -> BTreeSet<String> {
        self.capture.snapshot.transactions.keys().cloned().collect()
    }
    fn parents(&self, operation: &str) -> Result<BTreeSet<String>> {
        Ok(self
            .capture
            .snapshot
            .transactions
            .get(operation)
            .ok_or_else(|| error("incomplete_commit"))?
            .parents
            .iter()
            .cloned()
            .collect())
    }
    fn introduced(&self, operation: &str) -> Result<BTreeSet<String>> {
        Ok(self.introduced.get(operation).cloned().unwrap_or_default())
    }
    fn objects(&self) -> &Map {
        self.capture.history.objects()
    }
    fn receipt(&self, operation: &str) -> Result<V> {
        W::receipt(&self.capture.snapshot, operation)
    }
    fn active(&self, operation: &str) -> Result<bool> {
        for id in self.introduced(operation)? {
            if A::has_temporal_metadata(&self.objects()[&id])? {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn reduce(&self, objects: &Map) -> Result<V> {
        // Source revision/commit clocks need separately authenticated source ancestry.
        // Neither a JSONL encoding predecessor nor a publication parent supplies it.
        self.capture.history.reduce_subset(
            objects,
            Some(map(&map(self.capture.state())?["rules"])?),
            None,
        )
    }
    fn template(&self, operations: &BTreeSet<String>) -> Result<V> {
        let parents = operations
            .iter()
            .map(|op| self.parents(op))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect::<BTreeSet<_>>();
        let mut template = None;
        for operation in operations.difference(&parents) {
            let receipt = self.receipt(operation)?;
            let after = map(&map(&receipt)?["after"])?;
            let document = after
                .get("document")
                .ok_or_else(|| error("missing_document_template"))?;
            let next = A::document_template(document)?;
            crate::require(
                template.as_ref().is_none_or(|old| old == &next),
                "divergent_templates",
            )?;
            template = Some(next);
        }
        Ok(template.unwrap_or_else(|| V::Map(Map::new())))
    }
}

pub(crate) fn capture(capture: &Capture) -> Result<Option<V>> {
    let mut active = false;
    let mut introduced = BTreeMap::<String, BTreeSet<String>>::new();
    for (id, object) in capture.history.objects() {
        active |= A::has_temporal_metadata(object)?;
        let event = capture.storage_event(id)?;
        let operation = capture
            .snapshot
            .operations
            .get(event)
            .ok_or_else(|| error("incomplete_commit"))?;
        introduced
            .entry(operation.clone())
            .or_default()
            .insert(id.clone());
    }
    if !active {
        return Ok(None);
    }
    T::capture_source(&NodeSource {
        capture,
        introduced,
    })
}

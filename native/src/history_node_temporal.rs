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
    components: crate::history_node_receipt_components::Components<'a>,
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
        if self
            .capture
            .snapshot
            .transactions
            .get(operation)
            .and_then(|tx| tx.context.as_ref())
            .is_some_and(|c| {
                crate::history_node_legacy::kind(c, crate::history_node_legacy::FORMAT)
            })
        {
            return crate::history_node_legacy::inventory(&self.capture.snapshot, operation);
        }
        Ok(self.introduced.get(operation).cloned().unwrap_or_default())
    }
    fn objects(&self) -> &Map {
        self.capture.history.objects()
    }
    fn receipt(&self, operation: &str) -> Result<V> {
        W::receipt_index(&self.capture.snapshot, operation, &self.components)
    }
    fn temporal_recipe(&self, operation: &str, phase: &str) -> Result<Option<V>> {
        let Some(context) = self
            .capture
            .snapshot
            .transactions
            .get(operation)
            .and_then(|tx| tx.context.as_ref())
        else {
            return Ok(None);
        };
        if !crate::history_node_transaction::is_context(context) {
            return Ok(None);
        }
        let result = map(field(map(context)?, "result")?)?;
        result
            .get("temporal")
            .map(|recipe| crate::history_node_temporal_recipe::phase(recipe, phase))
            .transpose()
            .map(Option::flatten)
    }
    fn requires_temporal_recipe(&self, operation: &str) -> bool {
        self.capture
            .snapshot
            .transactions
            .get(operation)
            .and_then(|tx| tx.context.as_ref())
            .is_some_and(|context| {
                if !crate::history_node_transaction::is_context(context) {
                    return false;
                }
                let kind = map(context)
                    .ok()
                    .and_then(|c| c.get("action"))
                    .and_then(|action| map(action).ok())
                    .and_then(|action| action.get("kind"));
                !kind.is_some_and(|kind| {
                    string_is(kind, "branch-union") || string_is(kind, "source-clocks")
                })
            })
    }
    fn active(&self, operation: &str) -> Result<bool> {
        for id in self.introduced(operation)? {
            if A::has_temporal_metadata(&self.objects()[&id])? {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn reduce(&self, objects: &Map, operations: &BTreeSet<String>) -> Result<V> {
        // A later proof cannot change a previous observation's source-clock world.
        let graph = self.capture.snapshot.source_clocks.select(
            operations
                .iter()
                .flat_map(|op| self.capture.snapshot.transactions[op].evidence.keys()),
        );
        graph.with_ancestry(|ancestry| {
            self.capture.history.reduce_subset(
                objects,
                Some(map(&map(self.capture.state())?["rules"])?),
                Some(ancestry),
            )
        })
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
            let next =
                crate::history_node_transaction::after_template(&self.capture.snapshot, operation)?;
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
        components: crate::history_node_receipt_components::Components::new(&capture.snapshot)?,
        introduced,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        history_node_codec as C, history_node_observation::ObservationNode,
        history_node_publication as P,
    };
    use serde_json::json;
    #[test]
    fn source_clock_proofs_are_limited_to_each_temporal_operation_closure() {
        let (graph, a, b, proof_path) = crate::history_source_ancestry::tests::graph();
        let v = |j: serde_json::Value| V::from_json(&j).unwrap();
        let mut versions = BTreeMap::new();
        let mut operations = BTreeMap::new();
        for (op, clock, n) in [("left", a, 1), ("right", b, 2)] {
            let mut object = v(
                json!({"schema_version":2,"id_scheme":"typed-history/v2","kind":"reading","subject":"p.clock",
                "op":op,"on":"2026-09-24","by":"fixture","body":{"v":n,"from":"source.repository"},"at":{"commit":clock},"saw":[],"pins":{},
                "authored":{"collection":"known","profile":"ordinary-reader/v1","fields":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"}}}),
            );
            let id = crate::identity::typed_object_identity(&object).unwrap();
            crate::history_view::map_mut(&mut object)
                .unwrap()
                .insert("id".into(), V::Text(id.clone()));
            let obs = ObservationNode::root(&id, &BTreeSet::new()).unwrap();
            let event = C::Event::create(
                "p.clock",
                op,
                vec![],
                None,
                Some(crate::history_node_capture::payload(&object, &obs).unwrap()),
            )
            .unwrap();
            operations.insert(event.id().into(), op.into());
            versions.insert(event.id().into(), event.reconstruct(None).unwrap());
        }
        let doc = v(
            json!({"meta":{},"schema":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"},"known":{}}),
        );
        let mut snapshot = P::Snapshot {
            authority: V::Null,
            revision: String::new(),
            current: Some(crate::history_yaml::encode_document(&doc).unwrap()),
            versions: BTreeMap::from([("p.clock".into(), versions)]),
            operations,
            transactions: BTreeMap::new(),
            source_clocks: Default::default(),
            legacy: Default::default(),
            raw_evidence: Default::default(),
        };
        let mut capture = Capture::from_snapshot(snapshot.clone()).unwrap();
        snapshot.source_clocks = graph;
        snapshot.transactions.insert(
            "late-proof".into(),
            P::Transaction {
                digest: String::new(),
                parents: vec![],
                context: None,
                evidence: BTreeMap::from([(proof_path, "verified elsewhere".into())]),
            },
        );
        for op in ["left", "right"] {
            snapshot.transactions.insert(
                op.into(),
                P::Transaction {
                    digest: String::new(),
                    parents: vec![],
                    context: None,
                    evidence: BTreeMap::new(),
                },
            );
        }
        capture.snapshot = snapshot;
        let source = NodeSource {
            capture: &capture,
            components: crate::history_node_receipt_components::Components::new(&capture.snapshot)
                .unwrap(),
            introduced: BTreeMap::new(),
        };
        let old = source
            .reduce(
                capture.history.objects(),
                &BTreeSet::from(["left".into(), "right".into()]),
            )
            .unwrap();
        let new = source
            .reduce(
                capture.history.objects(),
                &BTreeSet::from(["left".into(), "right".into(), "late-proof".into()]),
            )
            .unwrap();
        assert!(string_is(
            &map(&map(&map(&old).unwrap()["subjects"]).unwrap()["p.clock"]).unwrap()["acceptance"],
            "contested"
        ));
        assert!(string_is(
            &map(&map(&map(&new).unwrap()["subjects"]).unwrap()["p.clock"]).unwrap()["acceptance"],
            "accepted"
        ));
    }
}

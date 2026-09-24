//! Reusable, borrowed version topology for reconstructing causal receipt components.
//! The index retains only each frame's phase and receipt piece, not semantic state.
use crate::{
    Result, history_contract::Map, history_node_codec as C, history_node_frame as Frame,
    history_node_publication as P, history_node_receipt::Nodes, require, value::TypedValue as V,
};
use std::collections::BTreeSet;

struct Entry<'a> {
    id: &'a str,
    version: &'a C::Version,
    before: bool,
    receipt: Option<V>,
}

struct Subject<'a> {
    name: &'a str,
    versions: Vec<Entry<'a>>,
}

pub(crate) struct Components<'a> {
    subjects: Vec<Subject<'a>>,
}

impl<'a> Components<'a> {
    pub(crate) fn new(snapshot: &'a P::Snapshot) -> Result<Self> {
        let mut subjects = Vec::with_capacity(snapshot.versions.len());
        for (name, versions) in &snapshot.versions {
            let mut entries = Vec::with_capacity(versions.len());
            for (id, version) in versions {
                let state = version.state().ok_or_else(|| {
                    crate::history_contract::error("node_semantic_absent_unsupported")
                })?;
                // Decode once, retaining only the fields needed by receipt selection.
                // This preserves Frame::decode's format and receipt validation.
                if crate::history_node_ledger::is_ledger(state) {
                    crate::history_node_ledger::unpack(state)?;
                    entries.push(Entry {
                        id,
                        version,
                        before: false,
                        receipt: None,
                    });
                    continue;
                }
                let payload = Frame::decode(state)?;
                entries.push(Entry {
                    id,
                    version,
                    before: payload.phase == "before",
                    receipt: payload.receipt,
                });
            }
            subjects.push(Subject {
                name,
                versions: entries,
            });
        }
        Ok(Self { subjects })
    }

    pub(crate) fn at(&self, allowed: &BTreeSet<String>, before: Option<&str>) -> Result<Nodes> {
        let mut values = Map::new();
        for subject in &self.subjects {
            let keep = |entry: &Entry<'_>| {
                allowed.contains(entry.version.operation())
                    && (before != Some(entry.version.operation()) || entry.before)
            };
            // Frame::tip excludes every selected parent ID, including parents that are
            // not themselves in the selected set. Keep that exact frontier rule.
            let parents: BTreeSet<&str> = subject
                .versions
                .iter()
                .filter(|entry| keep(entry))
                .flat_map(|entry| entry.version.parents().iter().map(String::as_str))
                .collect();
            let mut tip = None;
            for entry in subject.versions.iter().filter(|entry| keep(entry)) {
                if !parents.contains(entry.id) {
                    require(tip.is_none(), "node_semantic_merge_required")?;
                    tip = Some(entry);
                }
            }
            if let Some(receipt) = tip.and_then(|entry| entry.receipt.as_ref()) {
                values.insert(subject.name.to_owned(), receipt.clone());
            }
        }
        Nodes::from_values(values)
    }
}

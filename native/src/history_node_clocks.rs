//! Atomic admission of source ancestry evidence without adding a semantic claim.
use crate::{
    Result,
    history_contract::*,
    history_node_capture::Capture,
    history_node_publication as P, history_node_writer as W,
    history_source_ancestry::{self as A, Proof},
    require,
    value::TypedValue as V,
};
use std::{collections::BTreeMap, path::Path};
pub(crate) const FORMAT: &str = "node-source-clocks/v1";
pub(crate) fn is_clocks(context: &V) -> Result<bool> {
    Ok(map(context)?
        .get("format")
        .is_some_and(|v| string_is(v, FORMAT)))
}
fn plan(
    capture: &Capture,
    operation: &str,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<W::Materialized> {
    token(&V::Text(operation.into()))?;
    require(
        !capture.snapshot.transactions.contains_key(operation),
        "operation_already_prepared",
    )?;
    require(
        !files.is_empty() && files.len() <= 256,
        "source_ancestry_limit",
    )?;
    require(
        files.values().map(Vec::len).sum::<usize>() <= 4 * 1024 * 1024,
        "source_ancestry_limit",
    )?;
    let mut graph = capture.clocks.clone();
    for (path, raw) in files {
        graph.insert(path, raw)?;
    }
    let after = capture.with_clocks(graph)?;
    W::clock_transition(capture, &after, operation, files)
}
/// The caller explicitly selects the local repository and exact source-clock endpoints.
/// Publication and recovery use only retained verified bytes, never the repository.
pub fn prepare(root: &Path, operation: &str, proof: &Proof) -> Result<P::Prepared> {
    let capture = Capture::read(root)?;
    let built = plan(&capture, operation, proof.files())?;
    let prepared = P::prepare_with_evidence(
        root,
        operation,
        built.after,
        built.frames,
        Some(&built.context),
        proof.files().clone(),
    )?;
    verify(root, &prepared)?;
    Ok(prepared)
}
pub(crate) fn verify(root: &Path, prepared: &P::Prepared) -> Result<()> {
    require(
        prepared.imports()?.is_empty() && prepared.canonical_before()?.is_none(),
        "source_ancestry_publication",
    )?;
    let (before, after) = prepared.snapshots(root)?;
    let before = Capture::from_snapshot(before)?;
    Capture::from_snapshot(after)?;
    let files = prepared.evidence()?;
    require(
        files.keys().all(|p| p.starts_with(A::PREFIX)),
        "source_ancestry_path",
    )?;
    let expected = plan(&before, prepared.operation(), &files)?;
    require(
        expected.after == prepared.after_view()?
            && expected.frames == prepared.frames()?
            && Some(expected.context) == prepared.context()?,
        "source_ancestry_replay",
    )
}

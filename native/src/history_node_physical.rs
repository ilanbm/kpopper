//! Capture physical hypothesis originals as immutable proposals in a new copy.
//! Raw originals remain hash-bound evidence; no proposal is accepted by import.
//! Later folds follow the record profile's normal authoring rules.
use crate::{
    Result, history_authoring as A,
    history_contract::*,
    history_hypothesis_authoring as H, history_hypothesis_import_prepare as I,
    history_node_capture::Capture,
    history_node_publication as P, history_node_writer as W,
    history_view::{list, map_mut},
    history_yaml as Y, require,
    value::TypedValue as V,
};
use std::{collections::BTreeMap, path::Path};
pub(crate) const FORMAT: &str = "node-physical-import/v1";
pub(crate) fn plan(
    capture: &Capture,
    options: &A::Options,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<(Vec<V>, V, V)> {
    require(!files.is_empty(), "node_physical_empty")?;
    let mut sources = Vec::new();
    for (path, raw) in files {
        let name = path
            .strip_prefix(".kpopper/hypotheses/")
            .and_then(|p| p.strip_suffix(".yaml").or_else(|| p.strip_suffix(".yml")))
            .ok_or_else(|| error("invalid_hypothesis_import_path"))?;
        hypothesis_name(&A::s(name))?;
        let mut document = Y::decode_document(raw)?;
        let head = map_mut(&mut document)?
            .remove("hypothesis")
            .filter(|v| *v != V::Null)
            .unwrap_or_else(A::empty);
        sources.push(I::Source {
            name: name.into(),
            document,
            head,
            path: path.clone(),
            bytes: raw.clone(),
            profile: None,
            fields: None,
            error: None,
        });
    }
    let existing = crate::history_node_hypothesis::context(capture)?;
    require(
        sources
            .iter()
            .all(|s| !map(&existing.groups).unwrap().contains_key(&s.name)),
        "hypothesis_authority_collision",
    )?;
    let base_objects = capture
        .history
        .objects()
        .keys()
        .map(|id| Ok((id.clone(), capture.history.object(id)?)))
        .collect::<Result<Map>>()?;
    let imported = I::prepare_with_reduce(
        &base_objects,
        &sources,
        capture.document(),
        "GROUNDING.yaml",
        &options.operation,
        &options.recorded_at,
        &|combined| {
            let added = combined
                .iter()
                .filter(|(id, _)| !base_objects.contains_key(*id))
                .map(|(_, o)| o.clone())
                .collect::<Vec<_>>();
            Ok(capture.candidate(&added)?.state().clone())
        },
    )?;
    let imported = map(&imported)?;
    let mut objects = map(&imported["objects"])?
        .values()
        .cloned()
        .collect::<Vec<_>>();
    objects.sort_by_key(|v| {
        let m = map(v).unwrap();
        (
            text(&m["subject"]).unwrap().to_owned(),
            list(&m["saw"]).unwrap().len(),
            text(&m["id"]).unwrap().to_owned(),
        )
    });
    let mut template = capture.document().clone();
    for object in &objects {
        let m = map(object)?;
        if !string_is(&m["kind"], "act") {
            map_mut(&mut template)?
                .entry(text(&map(&m["authored"])?["collection"])?.into())
                .or_insert_with(A::empty);
        }
    }
    let meta = map_mut(
        map_mut(&mut template)?
            .get_mut("meta")
            .ok_or_else(|| error("invalid_document"))?,
    )?;
    let mut mappings = meta
        .get("history_hypothesis_import")
        .map(map)
        .transpose()?
        .and_then(|m| m.get("physical"))
        .map(list)
        .transpose()?
        .unwrap_or(&[])
        .to_vec();
    mappings.extend(
        list(&map(&imported["physical"])?["physical"])?
            .iter()
            .cloned(),
    );
    mappings.sort_by(|a, b| {
        text(&map(a).unwrap()["name"])
            .unwrap()
            .cmp(text(&map(b).unwrap()["name"]).unwrap())
    });
    let mapping = A::obj([("version", A::n("1")), ("physical", V::List(mappings))]);
    crate::history_hypothesis_import::validate_mapping(&mapping, "GROUNDING.yaml")?;
    meta.insert("history_hypothesis_import".into(), mapping);
    let candidate = capture.candidate_with_template(&objects, &template)?;
    let before = A::destination(capture.document())?;
    let after = A::destination(candidate.document())?;
    let cap = crate::reasoning_fields::capabilities(&before, None)?;
    let receipt = crate::history_transaction::semantic_receipt(
        text(&map(&cap)?["profile"])?,
        &cap,
        &A::obj([("document", before)]),
        &A::obj([("document", after.clone())]),
    )?;
    Ok((objects, after, receipt))
}
pub(crate) fn verify(root: &Path, prepared: &P::Prepared) -> Result<()> {
    require(
        prepared.imports()?.is_empty() && prepared.canonical_before()?.is_none(),
        "node_physical_publication",
    )?;
    let (before, after) = prepared.snapshots(root)?;
    let before = Capture::from_snapshot(before)?;
    Capture::from_snapshot(after)?;
    let c = prepared
        .context()?
        .ok_or_else(|| error("node_transaction_missing_context"))?;
    let c = W::context(&c)?;
    let options = schema(&c["options"], &["authoring", "evidence"], &[])?;
    let options = W::decode_options(prepared.operation(), &options["authoring"])?;
    let expected = W::materialize(
        &before,
        &A::obj([("kind", A::s("physical-import"))]),
        &options,
        None,
        None,
        &A::empty(),
        &prepared.evidence()?,
    )?;
    require(
        expected.after == prepared.after_view()?
            && expected.frames == prepared.frames()?
            && Some(expected.context) == prepared.context()?,
        "node_physical_replay_mismatch",
    )
}
/// Used only while a copy is staged; the original source directory is never written.
pub(crate) fn finish_copy(root: &Path) -> Result<()> {
    let store = crate::history_store::Store::new(&root.join("GROUNDING.yaml"))?;
    let capture = Capture::read(root)?;
    let active = H::active_physical(&store, capture.document())?;
    if active.is_empty() {
        return Ok(());
    }
    let all = H::physical_files(&store)?;
    let files = active
        .values()
        .map(|path| (path.clone(), all[path].clone()))
        .collect::<BTreeMap<_, _>>();
    let options = A::Options {
        operation: format!("physical-{}", uuid::Uuid::new_v4().simple()),
        recorded_at: chrono::Utc::now().to_rfc3339(),
        recording_day: crate::source_clock::latest_day(),
        by: V::Null,
        strict: true,
        paths: crate::history_paths::Scheme::Hashed,
        receipt_version: None,
    };
    let built = W::materialize(
        &capture,
        &A::obj([("kind", A::s("physical-import"))]),
        &options,
        None,
        None,
        &A::empty(),
        &files,
    )?;
    let prepared = P::prepare_with_evidence(
        root,
        &options.operation,
        built.after,
        built.frames,
        Some(&built.context),
        files,
    )?;
    W::publish(root, &prepared, None, |_| Ok(()))
}

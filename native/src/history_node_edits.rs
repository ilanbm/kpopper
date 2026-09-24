//! Explicit edited-view proposals retain raw evidence and an independently verified baseline.
use crate::{
    Result,
    history_authoring::{self as A, Options, Proposal},
    history_authoring_audit::ReplayAudit,
    history_authority as Authority,
    history_contract::*,
    history_node_capture::Capture,
    history_node_publication as P, history_transaction as T, history_transaction_fs as F,
    history_view::{list, map_mut},
    history_yaml as Y,
    identity::sha256,
    reasoning_runtime::Runtime,
    reasoning_snapshot::entries,
    require,
    value::TypedValue as V,
};
use A::{obj, s, strings};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(crate) fn capture(root: &Path, canonical: &[u8]) -> Result<Capture> {
    Capture::from_snapshot(P::capture_edited_snapshot(root, canonical)?)
}
pub(crate) fn edited(root: &Path) -> Result<Vec<u8>> {
    F::read(&F::target(root, "GROUNDING.yaml")?)?.ok_or_else(|| error("node_semantic_missing_view"))
}
pub(crate) fn step(operation: &str, index: usize) -> Result<String> {
    Ok(format!(
        "edit-step-{}",
        obj([
            ("operation", s(operation)),
            ("index", A::n(&index.to_string()))
        ])
        .digest()?
    ))
}
pub(crate) fn selection(
    capture: &Capture,
    raw: &[u8],
    subjects: Option<&[String]>,
) -> Result<Vec<String>> {
    require(raw.len() <= 16 * 1024 * 1024, "history_limit")?;
    let before = Y::decode_document(capture.entry_bytes())?;
    let after = Y::decode_document(raw)?;
    require(
        Authority::document_template(&before)? == Authority::document_template(&after)?,
        "template_disposition_required",
    )?;
    let before = entries(&before)?;
    let after = entries(&after)?;
    require(
        before.keys().all(|id| after.contains_key(id)),
        "deletion_disposition_required",
    )?;
    let mut changed = BTreeSet::new();
    for (id, (collection, body)) in &after {
        if let Some((old_collection, old_body)) = before.get(id) {
            require(
                old_collection == collection,
                "collection_disposition_required",
            )?;
            if old_body == body {
                continue;
            }
        }
        changed.insert(id.clone());
    }
    require(!changed.is_empty(), "no_body_proposals")?;
    let selected = subjects
        .map(|v| v.to_vec())
        .unwrap_or_else(|| changed.iter().cloned().collect());
    let set = selected.iter().cloned().collect::<BTreeSet<_>>();
    require(set.len() == selected.len(), "invalid_edit_subjects")?;
    require(set == changed, "unhandled_view_edits")?;
    require(set.len() <= 64, "history_limit")?;
    Ok(set.into_iter().collect())
}
pub(crate) fn action(intent: &Map) -> Result<V> {
    Ok(obj([
        ("kind", s("view-edit-proposals")),
        ("because", field(intent, "because")?.clone()),
        ("subjects", field(intent, "subjects")?.clone()),
    ]))
}
pub(crate) fn prepare(
    capture: &Capture,
    action: &V,
    options: &Options,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
    archive: &V,
    evidence: &BTreeMap<String, Vec<u8>>,
) -> Result<(Vec<V>, V, V)> {
    let a = schema(action, &["kind", "because", "subjects"], &[])?;
    let because = text(&a["because"])?;
    require(!because.trim().is_empty(), "act_reason_required")?;
    require(options.strict, "explicit_root_disposition_required")?;
    let requested = list(&a["subjects"])?
        .iter()
        .map(|v| text(v).map(str::to_owned))
        .collect::<Result<Vec<_>>>()?;
    let path = format!("evidence/view-edits/{}.yaml", options.operation);
    require(evidence.len() == 1, "node_edit_evidence_mismatch")?;
    let raw = evidence
        .get(&path)
        .ok_or_else(|| error("node_edit_evidence_mismatch"))?;
    let subjects = selection(capture, raw, Some(&requested))?;
    let authored = entries(&Y::decode_document(raw)?)?;
    let mut objects = Vec::new();
    let mut steps = Vec::new();
    let mut first = None;
    for (index, subject) in subjects.iter().enumerate() {
        let (collection, body) = &authored[subject];
        let mut child = options.clone();
        child.operation = step(&options.operation, index)?;
        child.receipt_version = Some(5); // Preserve explicitly authored historical snapshots.
        let proposal = Proposal {
            subject: subject.clone(),
            collection: collection.clone(),
            body: body.clone(),
            because: because.into(),
            hypothesis: None,
        };
        let plan = crate::history_authoring_proposal::prepare(capture, &proposal, &child, runtime)?;
        let receipt = crate::history_authoring_proposal::receipt(
            capture, &proposal, &plan, &child, runtime, audit, archive,
        )?;
        steps.push(obj([
            ("operation", s(&child.operation)),
            ("receipt_digest", map(&receipt)?["digest"].clone()),
        ]));
        if first.is_none() {
            first = Some(receipt);
        }
        objects.extend(plan.objects);
    }
    let first = first.unwrap();
    let first = map(&first)?;
    let mut before = first["before"].clone();
    map_mut(&mut before)?.insert(
        "authoring".into(),
        obj([
            ("version", A::n("1")),
            ("kind", s("view-edit-proposals")),
            ("because", s(because)),
            ("by", options.by.clone()),
            ("recorded_at", s(&options.recorded_at)),
            ("subjects", strings(subjects)),
            ("archive", archive.clone()),
            (
                "baseline",
                crate::history_authoring_core::Input::baseline(capture).clone(),
            ),
            ("original_view_sha256", s(&sha256(capture.entry_bytes()))),
            ("edited_view_sha256", s(&sha256(raw))),
            ("evidence", V::Map(Map::from([(path, s(&sha256(raw)))]))),
        ]),
    );
    let mut after = first["before"].clone();
    map_mut(&mut after)?.insert(
        "authoring".into(),
        obj([
            ("proposal_steps", V::List(steps)),
            (
                "objects",
                strings(
                    objects
                        .iter()
                        .map(|v| text(&map(v).unwrap()["id"]).unwrap().to_owned())
                        .collect::<BTreeSet<_>>(),
                ),
            ),
        ]),
    );
    let receipt = T::semantic_receipt(
        text(&first["profile"])?,
        &first["capabilities"],
        &before,
        &after,
    )?;
    let projected = capture.candidate(&objects)?;
    Ok((objects, projected.document().clone(), receipt))
}

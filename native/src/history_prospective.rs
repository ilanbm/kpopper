//! Pure proposed history worlds; detached mutations never become accepted authority.
use crate::{
    Result, history_adapter as D,
    history_authoring::{empty, n, obj, s, strings},
    history_authority as A,
    history_capture::{self as H, Capture},
    history_contract::*,
    history_reduce,
    history_transaction::PreparedMutation,
    history_view::{self as View, list, map_mut},
    history_yaml as Y,
    identity::sha256,
    reasoning_context::CapturedAssessment,
    reasoning_operations::findings,
    reasoning_runtime::{OperationalBounds, Runtime},
    reasoning_snapshot::{CaptureOptions, Snapshot},
    require,
    value::TypedValue as V,
};
use std::collections::BTreeSet;

pub fn capture_after(captured: &Capture, mutation: &PreparedMutation) -> Result<Capture> {
    let data = mutation.to_data();
    let data = map(&data)?;
    require(
        data["authority"].digest()? == captured.marker.digest()?,
        "authority_mismatch",
    )?;
    require(
        data["baseline"].digest()? == captured.baseline.digest()?,
        "baseline_mismatch",
    )?;
    let mut record = None;
    let mut commit = None;
    let mut objects = vec![];
    for file in mutation.files() {
        match file.role.as_str() {
            "record" => {
                require(record.replace(file).is_none(), "duplicate_mutation_role")?;
            }
            "history_commit" => {
                require(commit.replace(file).is_none(), "duplicate_mutation_role")?;
            }
            "history_object" => objects.push(file),
            // Branch evidence requires the separate retained adoption audit.
            // Refuse until that audit is available; a hash is not its substitute.
            "history_evidence" => return Err(error("branch_adoption_audit_unsupported")),
            _ => return Err(error("unsupported_mutation_role")),
        }
        require(
            if file.role == "record" {
                file.before.as_ref() == Some(&captured.entry_bytes)
            } else {
                file.before.is_none()
            },
            "before_image_mismatch",
        )?;
        require(file.after.is_some(), "invalid_mutation")?;
    }
    require(!objects.is_empty(), "unsupported_mutation_shape")?;
    let record = record.ok_or_else(|| error("unsupported_mutation_shape"))?;
    let commit = commit.ok_or_else(|| error("unsupported_mutation_shape"))?;
    require(record.path == text(&data["entry"])?, "role_path_mismatch")?;
    let mut virtual_capture = captured.clone();
    virtual_capture.entry_bytes = record.after.clone().unwrap();
    virtual_capture.document = Y::decode_document(&virtual_capture.entry_bytes)?;
    for file in objects {
        let raw = file.after.as_ref().unwrap();
        let object = Y::decode_document(raw)?;
        validate_object(&object)?;
        let object = map(&object)?;
        let key = (
            text(&object["subject"])?.to_owned(),
            text(&object["id"])?.to_owned(),
        );
        require(
            virtual_capture
                .object_bytes
                .get(&key)
                .is_none_or(|old| old == raw),
            "object_rewrite",
        )?;
        virtual_capture.object_bytes.insert(key, raw.clone());
    }
    let manifest = Y::decode_document(commit.after.as_ref().unwrap())?;
    A::validate_commit(&manifest)?;
    let manifest = map(&manifest)?;
    require(
        manifest["operation"] == data["operation"],
        "operation_mismatch",
    )?;
    require(
        manifest["record_id"] == map(&captured.marker)?["record_id"]
            && manifest["authority_generation"] == map(&captured.marker)?["generation"],
        "authority_mismatch",
    )?;
    require(
        field(manifest, "baseline_digest")? == &s(&captured.baseline.digest()?),
        "baseline_mismatch",
    )?;
    require(
        manifest["parents"] == V::Map(A::commit_frontier(&captured.commits)?),
        "parent_baseline_mismatch",
    )?;
    let operation = text(&data["operation"])?;
    require(
        !virtual_capture.commits.contains_key(operation),
        "operation_already_prepared",
    )?;
    for generation in captured.inactive_generations.values() {
        require(
            !generation.commits.contains_key(operation),
            "operation_collision",
        )?;
    }
    virtual_capture
        .commits
        .insert(operation.into(), commit.after.clone().unwrap());
    virtual_capture.objects = A::committed_objects(
        &captured.marker,
        &virtual_capture.commits,
        &virtual_capture.object_bytes,
    )?;
    let total: usize = virtual_capture
        .commits
        .values()
        .map(Vec::len)
        .sum::<usize>()
        .saturating_add(virtual_capture.object_bytes.values().map(Vec::len).sum());
    require(total <= 64 * 1024 * 1024, "history_limit")?;
    let selected = virtual_capture
        .object_bytes
        .iter()
        .filter(|((_, id), _)| virtual_capture.objects.contains_key(id))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    virtual_capture.state =
        history_reduce::reduce_bytes(&selected, Some(map(&map(&captured.state)?["rules"])?), None)?;
    virtual_capture.baseline = H::baseline(
        &captured.marker,
        &virtual_capture.commits,
        &virtual_capture.state,
    )?;
    require(
        field(
            map(field(map(&virtual_capture.document)?, "meta")?)?,
            "history",
        )?
        .digest()?
            == virtual_capture.baseline.digest()?,
        "baseline_mismatch",
    )?;
    require(
        field(manifest, "view_sha256")? == &s(&sha256(&virtual_capture.entry_bytes)),
        "view_mismatch",
    )?;
    require(
        View::render(
            &virtual_capture,
            &virtual_capture.objects,
            &virtual_capture.object_bytes,
            &virtual_capture.commits,
        )? == virtual_capture.entry_bytes,
        "view_mismatch",
    )?;
    Ok(virtual_capture)
}

pub fn snapshot_after(
    captured: &Capture,
    mutation: &PreparedMutation,
    mut options: CaptureOptions,
) -> Result<Snapshot> {
    let mut context = options
        .context
        .take()
        .filter(|v| *v != V::Null)
        .unwrap_or_else(empty);
    require(
        !["history", "history_hypotheses", "history_view", "operation"]
            .iter()
            .any(|k| map(&context).is_ok_and(|m| m.contains_key(*k))),
        "duplicate_prospective_context",
    )?;
    let virtual_capture = capture_after(captured, mutation)?;
    map_mut(&mut context)?.insert("read_mode".into(), s("supplied"));
    map_mut(&mut context)?.insert(
        "operation".into(),
        obj([
            ("version", n("1")),
            ("phase", s("prospective")),
            ("kind", s("prepared_history")),
            ("original_baseline", captured.baseline.clone()),
            (
                "mutation_operation",
                map(&mutation.to_data())?["operation"].clone(),
            ),
        ]),
    );
    options.context = Some(context);
    D::from_store_capture(&virtual_capture)?.snapshot(options)
}

pub struct Assessment {
    pub before: CapturedAssessment,
    pub after: CapturedAssessment,
    pub before_findings: V,
    pub after_findings: V,
    pub introduced: V,
}
pub fn assess(
    captured: &Capture,
    mutation: &PreparedMutation,
    hypotheses: Option<&V>,
    as_of: Option<&V>,
    runtime: Option<&Runtime>,
) -> Result<Assessment> {
    let context = obj([
        ("read_mode", s("supplied")),
        ("operation_scope", s("committed_history")),
    ]);
    let before = D::from_store_capture(captured)?.snapshot(CaptureOptions {
        context: Some(context.clone()),
        hypotheses: hypotheses.cloned(),
        as_of: as_of.cloned(),
        ..Default::default()
    })?;
    let after = snapshot_after(
        captured,
        mutation,
        CaptureOptions {
            context: Some(context),
            hypotheses: hypotheses.cloned(),
            as_of: Some(map(before.data())?["as_of"].clone()),
            ..Default::default()
        },
    )?;
    let before = CapturedAssessment::from_snapshot(
        before,
        None,
        "focused-review/v1",
        runtime,
        OperationalBounds::default(),
        None,
    )?;
    let after = CapturedAssessment::from_snapshot(
        after,
        None,
        "focused-review/v1",
        runtime,
        OperationalBounds::default(),
        None,
    )?;
    let before_findings = findings(&before)?;
    let after_findings = findings(&after)?;
    let mut introduced = Map::new();
    for key in ["falsified", "holes", "moved", "notes"] {
        let old = list(&map(&before_findings)?[key])?
            .iter()
            .map(|v| text(v).map(str::to_owned))
            .collect::<Result<BTreeSet<_>>>()?;
        let new = list(&map(&after_findings)?[key])?
            .iter()
            .map(|v| text(v).map(str::to_owned))
            .collect::<Result<BTreeSet<_>>>()?;
        introduced.insert(key.into(), strings(new.difference(&old).cloned()));
    }
    Ok(Assessment {
        before,
        after,
        before_findings,
        after_findings,
        introduced: V::Map(introduced),
    })
}

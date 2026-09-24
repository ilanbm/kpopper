//! Storage-independent explicit act validation, objects and exact receipt evidence.
use crate::{
    Result,
    history_authoring::{Options, empty, make_object, n, obj, s, strings},
    history_authoring_audit::ReplayAudit,
    history_authoring_core::Input,
    history_contract::*,
    history_paths as P, history_transaction as T,
    history_view::{list, map_mut},
    reasoning_fields as F,
    reasoning_runtime::Runtime,
    require,
    value::TypedValue as V,
};
use std::collections::BTreeSet;
pub(crate) struct Plan {
    pub objects: Vec<V>,
    pub action: V,
    pub profile: String,
}
pub(crate) fn prepare(input: &dyn Input, action: &V, options: &Options) -> Result<Plan> {
    let a = schema(action, &["kind", "id", "of", "over", "because"], &[])?;
    let kind = text(&a["kind"])?;
    let subject = text(&a["id"])?;
    let target_id = text(&a["of"])?;
    require(
        ["accept", "refute", "correct", "propose", "retire"].contains(&kind),
        "invalid_history_act",
    )?;
    require(P::object_id(target_id), "invalid_identifier")?;
    let over = list(&a["over"])?;
    let mut versions = BTreeSet::new();
    for version in over {
        let v = text(version)?;
        require(P::object_id(v), "invalid_identifier")?;
        versions.insert(v.to_owned());
    }
    require(
        versions.len() == over.len() && !versions.contains(target_id),
        "invalid_act_over",
    )?;
    require(
        !["refute", "propose", "retire"].contains(&kind) || over.is_empty(),
        if kind == "refute" {
            "invalid_refute_over"
        } else {
            "invalid_act_over"
        },
    )?;
    require(
        options.strict || !["propose", "retire"].contains(&kind),
        "strict_history_capability_required",
    )?;
    require(
        !text(&a["because"])?.trim().is_empty(),
        "act_reason_required",
    )?;
    let mut action = action.clone();
    map_mut(&mut action)?.insert("over".into(), strings(versions.clone()));
    for version in std::iter::once(target_id).chain(versions.iter().map(String::as_str)) {
        let target = input
            .object(version)
            .map_err(|_| error("missing_act_target"))?;
        let target = map(target)?;
        require(
            !string_is(&target["kind"], "act"),
            "act_target_must_be_claim",
        )?;
        require(
            string_is(&target["subject"], subject),
            "act_subject_mismatch",
        )?;
    }
    let before_doc = input.document()?;
    let target = input.object(target_id)?;
    if ["accept", "correct"].contains(&kind) {
        require_interpretable_claim(target)?;
    }
    let profile = text(field(map(field(map(target)?, "authored")?)?, "profile")?)?;
    F::capabilities(&before_doc, Some(profile))?;

    let body = obj([
        ("act", s(kind)),
        ("of", s(target_id)),
        ("over", strings(versions)),
        ("because", a["because"].clone()),
    ]);
    let new = vec![make_object(
        subject,
        "act",
        body,
        strings(input.saw(subject)?),
        None,
        empty(),
        empty(),
        options,
    )?];
    Ok(Plan {
        objects: new,
        action,
        profile: profile.to_owned(),
    })
}

pub(crate) fn receipt(
    input: &dyn Input,
    projected: &dyn Input,
    plan: &Plan,
    options: &Options,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
    frozen_archive: &V,
) -> Result<V> {
    let before_doc = input.document()?;
    let after_doc = projected.document()?;
    let new = &plan.objects;
    let profile = plan.profile.as_str();
    let subject = text(&map(&plan.action)?["id"])?;
    let cap = F::capabilities(&after_doc, Some(profile))?;
    let mut before = crate::history_authoring_reader::AuthoringReader::document_evidence(
        &before_doc,
        runtime,
        audit,
        &input.accepted_versions()?,
    )?;
    map_mut(&mut before)?.insert(
        "authoring".into(),
        obj([
            ("version", n("4")),
            ("kind", s("act")),
            ("action", plan.action.clone()),
            ("by", options.by.clone()),
            ("recorded_at", s(&options.recorded_at)),
            ("archive", frozen_archive.clone()),
            ("baseline", input.baseline().clone()),
        ]),
    );
    let mut after = crate::history_authoring_reader::AuthoringReader::document_evidence(
        &after_doc,
        runtime,
        audit,
        &projected.accepted_versions()?,
    )?;
    map_mut(&mut after)?.insert(
        "authoring".into(),
        obj([
            ("objects", V::List(vec![map(&new[0])?["id"].clone()])),
            ("subject", s(subject)),
            (
                "acceptance",
                map(&map(&map(projected.state())?["subjects"])?[subject])?["acceptance"].clone(),
            ),
        ]),
    );
    let receipt = T::semantic_receipt(profile, &cap, &before, &after)?;
    Ok(receipt)
}

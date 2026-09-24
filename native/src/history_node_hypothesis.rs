//! Named hypothesis planning on a fully verified node capture.
use crate::{
    Result,
    history_authoring::{self as A, Options},
    history_authoring_audit::ReplayAudit,
    history_contract::*,
    history_hypothesis_authoring as HA, history_hypothesis_core as Core,
    history_node_capture::Capture,
    history_node_projection as Projection,
    history_view::{list, map_mut, truth},
    reasoning_runtime::Runtime,
    reasoning_snapshot::CaptureOptions,
    require,
    value::TypedValue as V,
};
use std::path::Path;

pub(crate) fn sources(root: &Path) -> Result<()> {
    let store = crate::history_store::Store::new(&root.join("GROUNDING.yaml"))?;
    if HA::physical_files(&store)?.is_empty() {
        return Ok(());
    }
    let raw = crate::history_transaction_fs::read(&root.join("GROUNDING.yaml"))?
        .ok_or_else(|| error("node_semantic_missing_view"))?;
    let document = crate::history_yaml::decode_document(&raw)?;
    require(
        HA::active_physical(&store, &document)?.is_empty(),
        "node_hypothesis_physical_unsupported",
    )
}

pub(crate) fn context(capture: &Capture) -> Result<HA::Context> {
    let projected = Projection::capture(capture)?;
    let (groups, index) =
        crate::history_hypotheses::layers(projected.projection(), capture.document())?;
    Ok(HA::Context {
        base: capture.document().clone(),
        groups,
        index,
        physical: Default::default(),
    })
}

pub(crate) fn action(intent: &Map) -> Result<V> {
    let inner = match text(field(intent, "kind")?)? {
        "edit" => {
            return Ok(A::obj([
                ("kind", A::s("hypothesis")),
                ("name", field(intent, "name")?.clone()),
                ("head", field(intent, "head")?.clone()),
                ("action", field(intent, "action")?.clone()),
            ]));
        }
        "fold" | "refute" => V::Map(
            intent
                .iter()
                .filter(|(k, _)| {
                    [
                        "kind",
                        "names",
                        "because",
                        "take",
                        "drops",
                        "assessment_version",
                    ]
                    .contains(&k.as_str())
                })
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
        _ => return Err(error("invalid_hypothesis_receipt")),
    };
    Ok(A::obj([("kind", A::s("hypothesis")), ("action", inner)]))
}

pub(crate) fn assess(
    before: &Capture,
    after: &Capture,
    stamp: &str,
    runtime: Option<&Runtime>,
) -> Result<crate::history_prospective::Assessment> {
    let options = || CaptureOptions {
        context: Some(A::obj([
            ("read_mode", A::s("supplied")),
            ("operation_scope", A::s("committed_history")),
        ])),
        as_of: Some(A::s(stamp)),
        ..Default::default()
    };
    let before = Projection::capture(before)?.snapshot(options())?;
    let after = Projection::capture(after)?.snapshot(options())?;
    crate::history_prospective::assess_snapshots(before, after, runtime)
}

pub(crate) fn prepare(
    capture: &Capture,
    request: &V,
    options: &Options,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
    archive: &V,
) -> Result<(Vec<V>, V, V)> {
    let request = schema(request, &["kind", "action"], &["name", "head"])?;
    let action = &request["action"];
    capture.check_expected(action)?;
    let inner = map(action)?;
    let kind = text(field(inner, "kind")?)?;
    let context = context(capture)?;
    let plan = match kind {
        "add" | "set" | "review" => Core::prepare(
            capture,
            context,
            text(field(request, "name")?)?,
            action,
            request.get("head"),
            options,
            runtime,
        )?,
        "fold" | "refute" => {
            require(
                !request.contains_key("name") && !request.contains_key("head"),
                "invalid_hypothesis_action",
            )?;
            let inner = schema(
                action,
                &["kind", "names", "because"],
                &["take", "drops", "assessment_version"],
            )?;
            let names = list(&inner["names"])?
                .iter()
                .map(|v| text(v).map(str::to_owned))
                .collect::<Result<Vec<_>>>()?;
            let because = text(&inner["because"])?;
            if kind == "refute" {
                require(
                    inner.get("take").is_none_or(|v| !truth(v))
                        && inner.get("drops").is_none_or(|v| !truth(v))
                        && !inner.contains_key("assessment_version"),
                    "invalid_hypothesis_action",
                )?;
                Core::refute(capture, &context, &names, because, options)?
            } else {
                require(
                    inner
                        .get("assessment_version")
                        .is_none_or(|v| is_int(v, "1")),
                    "new_fold_requires_prospective_assessment",
                )?;
                let take = inner
                    .get("take")
                    .map(list)
                    .transpose()?
                    .unwrap_or(&[])
                    .iter()
                    .map(|v| text(v).map(str::to_owned))
                    .collect::<Result<Vec<_>>>()?;
                let fold = HA::Fold {
                    names,
                    because: because.into(),
                    take,
                    drops: inner.get("drops").cloned().unwrap_or_else(A::empty),
                    assessment_version: 1,
                };
                Core::fold(capture, &context, &fold, options, runtime)?
            }
        }
        _ => return Err(error("invalid_hypothesis_action")),
    };
    let mut template = capture.document().clone();
    for object in &plan.objects {
        let object = map(object)?;
        if !string_is(&object["kind"], "act") {
            map_mut(&mut template)?
                .entry(text(&map(&object["authored"])?["collection"])?.into())
                .or_insert_with(A::empty);
        }
    }
    let candidate = capture.candidate_with_template(&plan.objects, &template)?;
    if let Some(stamp) = &plan.stamp {
        let assessment = assess(capture, &candidate, stamp, runtime)?;
        let introduced = map(&assessment.introduced)?;
        require(
            !truth(&introduced["falsified"]) && !truth(&introduced["holes"]),
            "hypothesis_candidate_not_clean",
        )?;
    }
    let receipt = Core::receipt(capture, &plan, archive, &A::empty(), runtime, audit)?;
    Ok((plan.objects, candidate.document().clone(), receipt))
}

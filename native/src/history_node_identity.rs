//! Identity planning for native node-authored claims. Legacy imports need archived source order.
use crate::{
    Result,
    history_authoring::{self as A, Options, empty, n, obj, s, strings},
    history_authoring_audit::ReplayAudit,
    history_authoring_core::Input as _,
    history_contract::*,
    history_hypothesis_authoring as HA, history_identity as I, history_identity_core as Core,
    history_node_capture::Capture,
    history_view::map_mut,
    history_yaml::SourceValue,
    reasoning_fields as F,
    reasoning_runtime::Runtime,
    require,
    value::TypedValue as V,
};
use std::{collections::BTreeSet, path::Path};

impl Core::Input for Capture {
    fn source_body(&self, object: &V) -> Result<SourceValue> {
        // Native objects are authored from TypedValue and have canonical field order.
        // A future legacy import must supply its original ordered source, not use this path.
        require(
            string_is(field(map(object)?, "id_scheme")?, "typed-history/v2"),
            "node_identity_source_order_unavailable",
        )?;
        let event = self.storage_event(text(field(map(object)?, "id")?)?)?;
        let operation = self
            .snapshot
            .operations
            .get(event)
            .ok_or_else(|| error("incomplete_closure"))?;
        let context = self
            .snapshot
            .transactions
            .get(operation)
            .and_then(|tx| tx.context.as_ref())
            .ok_or_else(|| error("node_identity_source_order_unavailable"))?;
        require(
            string_is(
                field(map(context)?, "format")?,
                "node-authoring-transaction/v1",
            ),
            "node_identity_source_order_unavailable",
        )?;
        Ok(SourceValue::from_typed(field(map(object)?, "body")?))
    }
    fn template(&self) -> Result<V> {
        let mut doc = self.document().clone();
        for collection in F::collections(self.document())?.keys() {
            map_mut(&mut doc)?.insert(collection.clone(), empty());
        }
        Ok(doc)
    }
}

pub(crate) fn sources(root: &Path) -> Result<()> {
    let store = crate::history_store::Store::new(&root.join("GROUNDING.yaml"))?;
    require(
        map(&HA::physical_evidence(&store)?)?.is_empty(),
        "node_identity_physical_hypotheses_unsupported",
    )?;
    require(
        crate::history_transaction_fs::read(&crate::history_transaction_fs::target(
            root,
            &store.layout.view,
        )?)?
        .is_none(),
        "node_identity_brief_unsupported",
    )
}

pub(crate) fn prepare(
    capture: &Capture,
    action: &V,
    options: &Options,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
    archive: &V,
) -> Result<(Vec<V>, V, V)> {
    // Computational time is bounded by the independent source clock, including replay.
    let mut current_options = options.clone();
    current_options.recording_day = crate::source_clock::latest_day();
    let options = &current_options;
    let a = schema(action, &["kind", "a", "b"], &["keep", "because", "as_of"])?;
    let kind = text(&a["kind"])?;
    require(
        ["same", "distinct"].contains(&kind),
        "invalid_identity_action",
    )?;
    let intent = I::Intent {
        a: text(&a["a"])?.into(),
        b: text(&a["b"])?.into(),
        action: if kind == "same" {
            I::Action::Same {
                keep: a
                    .get("keep")
                    .filter(|v| **v != V::Null)
                    .map(text)
                    .transpose()?
                    .map(str::to_owned),
            }
        } else {
            I::Action::Distinct {
                because: text(field(a, "because")?)?.into(),
            }
        },
        as_of: a
            .get("as_of")
            .filter(|v| **v != V::Null)
            .map(text)
            .transpose()?
            .map(str::to_owned),
    };
    let projected = crate::history_node_projection::capture(capture)?;
    let (groups, index) =
        crate::history_hypotheses::layers(projected.projection(), capture.document())?;
    require(
        map(&groups)?.is_empty(),
        "node_identity_named_hypotheses_unsupported",
    )?;
    let context = HA::Context {
        base: capture.document().clone(),
        groups,
        index,
        physical: Default::default(),
    };
    let plan = Core::plan(capture, &context, &intent, options, runtime)?;
    let candidate = capture.candidate_with_template(&plan.objects, &plan.template)?;
    require(
        candidate.document().digest()? == plan.after_document.digest()?,
        "identity_projection_mismatch",
    )?;
    let mut data = obj([
        ("version", n("1")),
        ("kind", s(kind)),
        ("a", s(&intent.a)),
        ("b", s(&intent.b)),
        ("by", options.by.clone()),
        ("operation", s(&options.operation)),
        ("recorded_at", s(&options.recorded_at)),
        ("baseline", capture.baseline().clone()),
        ("archive", archive.clone()),
        ("physical", empty()),
        ("view_sha256", V::Null),
    ]);
    match &intent.action {
        I::Action::Same { keep } => {
            map_mut(&mut data)?.insert("keep".into(), keep.as_deref().map(s).unwrap_or(V::Null));
        }
        I::Action::Distinct { because } => {
            require(!because.contains('\n'), "identity_reason_one_line")?;
            map_mut(&mut data)?.insert("because".into(), s(because));
        }
    }
    if let Some(day) = &intent.as_of {
        map_mut(&mut data)?.insert("as_of".into(), s(day));
    }
    let mut before = A::evidence(
        &context.base,
        &mut HA::world(&context.base, None, runtime)?,
        audit,
    )?;
    map_mut(&mut before)?.insert("identity_authoring".into(), data);
    let mut after = A::evidence(
        &plan.after_document,
        &mut HA::world(&plan.after_document, None, runtime)?,
        audit,
    )?;
    let ids = plan
        .objects
        .iter()
        .map(|o| text(&map(o)?["id"]).map(str::to_owned))
        .collect::<Result<BTreeSet<_>>>()?;
    map_mut(&mut after)?.insert(
        "identity_authoring".into(),
        obj([("objects", strings(ids))]),
    );
    let cap = F::capabilities(&context.base, None)?;
    let receipt = crate::history_transaction::semantic_receipt(
        text(&map(&cap)?["profile"])?,
        &cap,
        &before,
        &after,
    )?;
    Ok((plan.objects, candidate.document().clone(), receipt))
}

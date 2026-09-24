//! Receipt-backed node publication and semantic recovery on explicitly marked records.
//! Public activation and conversion are separate from this storage adapter.
use crate::{
    Result, history_authoring as A,
    history_authoring_audit::ReplayAudit,
    history_authoring_core as Core,
    history_contract::*,
    history_node_capture::{self as N, Capture},
    history_node_codec as C,
    history_node_current::Original,
    history_node_frame as Frame,
    history_node_observation::ObservationNode,
    history_node_publication as P,
    history_node_receipt::{Nodes, Receipt},
    history_transaction_fs as FS,
    history_view::{list, map_mut},
    history_yaml as Y,
    reasoning_runtime::Runtime,
    require,
    value::TypedValue as V,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
const FORMAT: &str = "node-authoring-transaction/v1";
/// Batch children have original semantic operation IDs distinct from their atomic publication.
/// Membership is derived only from the explicit bounded batch intent, never event ancestry.
pub(crate) fn operation_member(
    snapshot: &P::Snapshot,
    transaction: &str,
    semantic: &str,
) -> Result<bool> {
    if transaction == semantic {
        return Ok(true);
    }
    let Some(tx) = snapshot.transactions.get(transaction) else {
        return Ok(false);
    };
    let Some(value) = &tx.context else {
        return Ok(false);
    };
    if crate::history_node_legacy::kind(value, crate::history_node_legacy::FORMAT) {
        return crate::history_node_legacy::member(snapshot, transaction, semantic);
    }
    let intent = if crate::history_node_transaction::is_context(value) {
        map(field(
            crate::history_node_transaction::validate(value)?,
            "action",
        )?)?
    } else {
        let c = context(value)?;
        let side = map(&c["before"])?;
        let literal = map(field(side, "literal")?)?;
        let Some(intent) = literal.get("authoring") else {
            return Ok(false);
        };
        map(intent)?
    };
    if intent
        .get("kind")
        .is_some_and(|v| string_is(v, "view-edit-proposals"))
    {
        let subjects = list(field(intent, "subjects")?)?;
        require(
            !subjects.is_empty() && subjects.len() <= 64,
            "history_limit",
        )?;
        for index in 0..subjects.len() {
            if crate::history_node_edits::step(transaction, index)? == semantic {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    if !intent.get("kind").is_some_and(|v| string_is(v, "batch")) {
        return Ok(false);
    }
    let actions = list(field(intent, "actions")?)?;
    require(!actions.is_empty() && actions.len() <= 64, "invalid_batch")?;
    for index in 0..actions.len() {
        let id = format!(
            "batch-step-{}",
            A::obj([
                ("operation", s(transaction)),
                ("index", A::n(&index.to_string()))
            ])
            .digest()?
        );
        if id == semantic {
            return Ok(true);
        }
    }
    Ok(false)
}
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn node_value(version: &C::Version) -> Result<&V> {
    version
        .state()
        .ok_or_else(|| error("node_semantic_absent_unsupported"))
}
pub(crate) fn context(value: &V) -> Result<&Map> {
    let c = schema(
        value,
        &["format", "options", "header", "before", "after"],
        &[],
    )?;
    require(
        string_is(&c["format"], FORMAT)
            || string_is(&c["format"], crate::history_node_branch::FORMAT)
            || string_is(&c["format"], crate::history_node_clocks::FORMAT)
            || string_is(&c["format"], crate::history_node_legacy::FORMAT)
            || string_is(&c["format"], crate::history_node_legacy::CHECKPOINT)
            || string_is(&c["format"], crate::history_node_bootstrap::FORMAT)
            || string_is(&c["format"], crate::history_node_physical::FORMAT),
        "node_transaction_format",
    )?;
    Ok(c)
}
fn ancestors(snapshot: &P::Snapshot, operation: &str) -> Result<BTreeSet<String>> {
    let mut found = BTreeSet::new();
    let mut todo = vec![operation.to_owned()];
    while let Some(op) = todo.pop() {
        if !found.insert(op.clone()) {
            continue;
        }
        let tx = snapshot
            .transactions
            .get(&op)
            .ok_or_else(|| error("node_transaction_missing_parent"))?;
        todo.extend(tx.parents.iter().cloned());
        require(found.len() <= C::MAX_EVENTS, "node_transaction_limit")?;
    }
    Ok(found)
}
fn components(
    snapshot: &P::Snapshot,
    allowed: &BTreeSet<String>,
    before: Option<&str>,
) -> Result<Nodes> {
    crate::history_node_receipt_components::Components::new(snapshot)?.at(allowed, before)
}
/// Restore the exact original receipt using only its causal node components and compact context.
pub fn receipt(snapshot: &P::Snapshot, operation: &str) -> Result<V> {
    if snapshot
        .transactions
        .get(operation)
        .and_then(|t| t.context.as_ref())
        .is_some_and(crate::history_node_transaction::is_context)
    {
        return crate::history_node_transaction::derive_receipt(snapshot, operation);
    }

    if snapshot
        .transactions
        .get(operation)
        .and_then(|t| t.context.as_ref())
        .is_some_and(|c| crate::history_node_legacy::kind(c, crate::history_node_legacy::FORMAT))
    {
        return crate::history_node_legacy::receipt(snapshot, operation);
    }
    let index = crate::history_node_receipt_components::Components::new(snapshot)?;
    receipt_index(snapshot, operation, &index)
}
pub(crate) fn receipt_index(
    snapshot: &P::Snapshot,
    operation: &str,
    index: &crate::history_node_receipt_components::Components<'_>,
) -> Result<V> {
    if snapshot
        .transactions
        .get(operation)
        .and_then(|t| t.context.as_ref())
        .is_some_and(crate::history_node_transaction::is_context)
    {
        return crate::history_node_transaction::derive_receipt(snapshot, operation);
    }

    if snapshot
        .transactions
        .get(operation)
        .and_then(|t| t.context.as_ref())
        .is_some_and(|c| crate::history_node_legacy::kind(c, crate::history_node_legacy::FORMAT))
    {
        return crate::history_node_legacy::receipt(snapshot, operation);
    }
    let tx = snapshot
        .transactions
        .get(operation)
        .ok_or_else(|| error("node_transaction_missing_parent"))?;
    let c = context(
        tx.context
            .as_ref()
            .ok_or_else(|| error("node_transaction_missing_context"))?,
    )?;
    let allowed = ancestors(snapshot, operation)?;
    let before = index.at(&allowed, Some(operation))?.side(&c["before"])?;
    let after = index.at(&allowed, None)?.side(&c["after"])?;
    Receipt::from_parts(c["header"].clone(), before, after)?.restore()
}

pub(crate) fn verify_receipts(snapshot: &P::Snapshot) -> Result<BTreeMap<String, V>> {
    let templates = crate::history_node_transaction::templates(snapshot)?;
    let index = crate::history_node_receipt_components::Components::new(snapshot)?;
    for (op, tx) in &snapshot.transactions {
        if let Some(value) = tx
            .context
            .as_ref()
            .filter(|c| crate::history_node_transaction::is_context(c))
        {
            let context = crate::history_node_transaction::validate(value)?;
            let expected = V::Map(tx.evidence.iter().map(|(p, h)| (p.clone(), s(h))).collect());
            require(
                context["evidence"] == expected,
                "node_receipt_evidence_mismatch",
            )?;
            require(templates.contains_key(op), "node_template_missing")?;
            if tx
                .evidence
                .keys()
                .any(|p| p.starts_with(crate::history_source_ancestry::PREFIX))
            {
                require(
                    crate::history_node_clocks::is_clocks(value)?
                        || crate::history_node_branch::is_union(value)?,
                    "source_ancestry_admission",
                )?;
            }
            continue;
        }
        if tx.context.as_ref().is_some_and(|c| {
            crate::history_node_legacy::kind(c, crate::history_node_legacy::FORMAT)
        }) {
            crate::history_node_legacy::receipt(snapshot, op)?;
            continue;
        }
        if tx
            .evidence
            .keys()
            .any(|path| path.starts_with(crate::history_source_ancestry::PREFIX))
        {
            let context = tx
                .context
                .as_ref()
                .ok_or_else(|| error("source_ancestry_admission"))?;
            require(
                crate::history_node_clocks::is_clocks(context)?
                    || crate::history_node_branch::is_union(context)?,
                "source_ancestry_admission",
            )?;
        }
        if tx.context.is_some() {
            let restored = receipt_index(snapshot, op, &index)?;
            let expected = if [
                crate::history_node_bootstrap::FORMAT,
                crate::history_node_physical::FORMAT,
            ]
            .iter()
            .any(|format| crate::history_node_legacy::kind(tx.context.as_ref().unwrap(), format))
            {
                let context = context(tx.context.as_ref().unwrap())?;
                field(map(&context["options"])?, "evidence")?.clone()
            } else if crate::history_node_branch::is_union(tx.context.as_ref().unwrap())?
                || crate::history_node_clocks::is_clocks(tx.context.as_ref().unwrap())?
                || crate::history_node_legacy::kind(
                    tx.context.as_ref().unwrap(),
                    crate::history_node_legacy::CHECKPOINT,
                )
            {
                crate::history_node_branch::evidence(tx.context.as_ref().unwrap())?
            } else {
                intent(&restored)?
                    .get("evidence")
                    .cloned()
                    .unwrap_or_else(A::empty)
            };
            let actual = V::Map(tx.evidence.iter().map(|(p, h)| (p.clone(), s(h))).collect());
            require(actual == expected, "node_receipt_evidence_mismatch")?;
        }
    }
    Ok(templates)
}

fn archive(root: &Path) -> Result<V> {
    let layout = crate::history_transaction::Layout::for_entry("GROUNDING.yaml")?;
    let raw = FS::read(&FS::target(root, &layout.replaced)?)?;
    require(
        raw.as_ref().is_none_or(|v| v.len() <= 16 * 1024 * 1024),
        "history_limit",
    )?;
    Ok(V::Map(Map::from([
        ("path".into(), s(&layout.replaced)),
        (
            "sha256".into(),
            raw.map(|v| s(&crate::identity::sha256(&v)))
                .unwrap_or(V::Null),
        ),
    ])))
}
fn encode_options(o: &A::Options) -> Result<V> {
    require(
        !o.recorded_at.is_empty() && !o.recording_day.is_empty(),
        "missing_recording_time",
    )?;
    Ok(V::Map(Map::from([
        ("recorded_at".into(), s(&o.recorded_at)),
        ("recording_day".into(), s(&o.recording_day)),
        ("by".into(), o.by.clone()),
        ("strict".into(), V::Bool(o.strict)),
        (
            "receipt_version".into(),
            o.receipt_version
                .map(|n| V::from_json(&serde_json::json!(n)).unwrap())
                .unwrap_or(V::Null),
        ),
    ])))
}
pub(crate) fn decode_options(operation: &str, value: &V) -> Result<A::Options> {
    let o = schema(
        value,
        &[
            "recorded_at",
            "recording_day",
            "by",
            "strict",
            "receipt_version",
        ],
        &[],
    )?;
    let V::Bool(strict) = o["strict"] else {
        return Err(error("node_transaction_options"));
    };
    let version = if o["receipt_version"] == V::Null {
        None
    } else if is_int(&o["receipt_version"], "1") {
        Some(1)
    } else if is_int(&o["receipt_version"], "2") {
        Some(2)
    } else if is_int(&o["receipt_version"], "7") {
        Some(7)
    } else if is_int(&o["receipt_version"], "5") {
        Some(5)
    } else if is_int(&o["receipt_version"], "9") {
        Some(9)
    } else if is_int(&o["receipt_version"], "6") {
        Some(6)
    } else if is_int(&o["receipt_version"], "8") {
        Some(8)
    } else {
        return Err(error("invalid_authoring_receipt"));
    };
    let result = A::Options {
        operation: operation.into(),
        recorded_at: text(&o["recorded_at"])?.into(),
        recording_day: text(&o["recording_day"])?.into(),
        by: o["by"].clone(),
        strict,
        paths: crate::history_paths::Scheme::Hashed,
        receipt_version: version,
    };
    require(
        encode_options(&result)? == *value,
        "node_transaction_options",
    )?;
    Ok(result)
}

struct Build<'a> {
    operation: &'a str,
    original_document: V,
    originals: Map,
    tails: Map,
    bases: BTreeMap<String, C::Version>,
    frames: BTreeMap<String, Vec<u8>>,
    fresh: BTreeSet<String>,
}
impl<'a> Build<'a> {
    fn new(capture: &Capture, operation: &'a str) -> Result<Self> {
        let doc = Y::decode_document(
            capture
                .snapshot
                .current
                .as_deref()
                .ok_or_else(|| error("node_semantic_missing_view"))?,
        )?;
        let originals = map(&doc)?
            .get("meta")
            .map(map)
            .transpose()?
            .and_then(|m| m.get("node_history"))
            .map(map)
            .transpose()?
            .and_then(|m| m.get("originals"))
            .map(map)
            .transpose()?
            .cloned()
            .unwrap_or_default();
        let tails = map(&doc)?
            .get("meta")
            .map(map)
            .transpose()?
            .and_then(|m| m.get("node_history"))
            .map(map)
            .transpose()?
            .and_then(|m| m.get("tails"))
            .map(map)
            .transpose()?
            .cloned()
            .unwrap_or_default();
        let mut bases = BTreeMap::new();
        for (subject, versions) in &capture.snapshot.versions {
            if let Some(tip) = Frame::tip(versions)? {
                bases.insert(subject.clone(), tip.clone());
            }
        }
        Ok(Self {
            operation,
            original_document: doc,
            originals,
            tails,
            bases,
            frames: BTreeMap::new(),
            fresh: BTreeSet::new(),
        })
    }
    fn lazy(&mut self, subject: &str, payload: &V) -> Result<()> {
        let p = map(payload)?;
        let original = Original::create(
            subject,
            text(&p["collection"])?,
            self.operation,
            p["body"].clone(),
            map(&p["context"])?.clone(),
        )?;
        let event = original.restore(p["body"].clone())?;
        self.bases.insert(subject.into(), event.reconstruct(None)?);
        self.originals.insert(subject.into(), original.encode()?);
        self.fresh.insert(subject.into());
        Ok(())
    }
    fn append(&mut self, subject: &str, payload: V) -> Result<()> {
        let fresh = self.fresh.contains(subject);
        if !fresh && let Some(binding) = self.originals.remove(subject) {
            let original = Original::decode(&binding)?;
            let event = original.from_document(&self.original_document)?;
            let frames = self.frames.entry(subject.into()).or_default();
            frames.extend(event.encode()?);
            if let Some(tail) = self.tails.remove(subject) {
                frames.extend(
                    STANDARD
                        .decode(text(&tail)?)
                        .map_err(|_| error("node_publication_base64"))?,
                );
            }
        }
        let base = self.bases.get(subject);
        let event = C::Event::create(
            subject,
            self.operation,
            base.map(|b| vec![b.id().into()]).unwrap_or_default(),
            base,
            Some(payload),
        )?;
        let next = event.reconstruct(base)?;
        if fresh {
            let mut tail = self
                .tails
                .get(subject)
                .map(|v| {
                    STANDARD
                        .decode(text(v)?)
                        .map_err(|_| error("node_publication_base64"))
                })
                .transpose()?
                .unwrap_or_default();
            tail.extend(event.encode()?);
            self.tails.insert(subject.into(), s(&STANDARD.encode(tail)));
        } else {
            self.frames
                .entry(subject.into())
                .or_default()
                .extend(event.encode()?);
        }
        self.bases.insert(subject.into(), next);
        Ok(())
    }
    fn evidence(
        &mut self,
        nodes: &mut Nodes,
        side: &crate::history_node_receipt::Side,
        phase: &str,
    ) -> Result<()> {
        let changes = nodes.prepare(side)?;
        for change in &changes {
            let base = self
                .bases
                .get(&change.subject)
                .ok_or_else(|| error("node_evidence_missing_subject"))?;
            let payload = Frame::decode(node_value(base)?)?;
            let fresh =
                self.fresh.contains(&change.subject) && !self.tails.contains_key(&change.subject);
            let value = Frame::encode(
                &payload.semantic,
                if fresh { "semantic" } else { "evidence" },
                phase,
                change.after.as_ref(),
            )?;
            if fresh {
                self.lazy(&change.subject, &value)?;
            } else {
                self.append(&change.subject, value)?;
            }
        }
        nodes.apply(&changes)
    }
}
pub(crate) struct Materialized {
    pub(crate) after: Vec<u8>,
    pub(crate) frames: BTreeMap<String, Vec<u8>>,
    pub(crate) context: V,
}
/// Recover the operation request without changing the original receipt's hash domain.
pub(crate) fn action(receipt: &V) -> Result<V> {
    if let Some(intent) = map(&map(receipt)?["before"])?.get("hypothesis_authoring") {
        return crate::history_node_hypothesis::action(map(intent)?);
    }
    let intent = intent(receipt)?;
    if intent
        .get("kind")
        .is_some_and(|v| ["same", "distinct"].iter().any(|k| string_is(v, k)))
    {
        let fields = ["kind", "a", "b", "keep", "because", "as_of"];
        return Ok(V::Map(
            intent
                .iter()
                .filter(|(k, _)| fields.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ));
    }
    if intent
        .get("kind")
        .is_some_and(|v| string_is(v, "view-edit-proposals"))
    {
        return crate::history_node_edits::action(intent);
    }
    if intent.get("kind").is_some_and(|v| string_is(v, "proposal")) {
        require(
            field(intent, "hypothesis")? == &V::Null,
            "node_history_hypotheses_unsupported",
        )?;
        Ok(A::obj([
            ("kind", s("proposal")),
            ("id", field(intent, "subject")?.clone()),
            ("body", field(intent, "body")?.clone()),
            ("into", field(intent, "collection")?.clone()),
            ("because", field(intent, "because")?.clone()),
        ]))
    } else if intent.get("kind").is_some_and(|v| string_is(v, "batch")) {
        let mut action = A::obj([
            ("kind", s("batch")),
            ("actions", field(intent, "actions")?.clone()),
        ]);
        if let Some(context) = intent.get("context").filter(|v| **v != A::empty()) {
            map_mut(&mut action)?.insert("context".into(), context.clone());
        }
        Ok(action)
    } else {
        Ok(field(intent, "action")?.clone())
    }
}
pub(crate) fn intent(receipt: &V) -> Result<&Map> {
    let before = map(&map(receipt)?["before"])?;
    let keys = ["authoring", "identity_authoring", "hypothesis_authoring"]
        .into_iter()
        .filter(|k| before.contains_key(*k))
        .collect::<Vec<_>>();
    require(keys.len() == 1, "node_authoring_intent")?;
    map(&before[keys[0]])
}
pub(crate) fn materialize(
    capture: &Capture,
    action: &V,
    options: &A::Options,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
    archive: &V,
    evidence: &BTreeMap<String, Vec<u8>>,
) -> Result<Materialized> {
    materialize_mode(
        capture, action, options, runtime, audit, archive, evidence, false,
    )
}
fn materialize_ledger(
    capture: &Capture,
    action: &V,
    options: &A::Options,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
    archive: &V,
    evidence: &BTreeMap<String, Vec<u8>>,
) -> Result<Materialized> {
    materialize_mode(
        capture, action, options, runtime, audit, archive, evidence, true,
    )
}
fn materialize_mode(
    capture: &Capture,
    action: &V,
    options: &A::Options,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
    archive: &V,
    evidence: &BTreeMap<String, Vec<u8>>,
    compact: bool,
) -> Result<Materialized> {
    // Preserve the optimistic snapshot guard on the same exact before bytes used by replay.
    capture.check_expected(action)?;
    let kind = text(field(map(action)?, "kind")?)?;
    require(
        ["batch", "view-edit-proposals", "physical-import"].contains(&kind) || evidence.is_empty(),
        "node_evidence_action_unsupported",
    )?;
    let mut effective_options = options.clone();
    if ["batch", "hypothesis"].contains(&kind) {
        effective_options.strict = true;
    }
    let options = &effective_options;
    let encoded_options = if kind == "physical-import" {
        A::obj([
            ("authoring", encode_options(options)?),
            (
                "evidence",
                V::Map(
                    evidence
                        .iter()
                        .map(|(p, raw)| (p.clone(), s(&crate::identity::sha256(raw))))
                        .collect(),
                ),
            ),
        ])
    } else {
        encode_options(options)?
    };
    let (objects, document, receipt) = if ["accept", "refute", "correct", "propose", "retire"]
        .contains(&kind)
    {
        let plan = crate::history_authoring_act::prepare(capture, action, options)?;
        let projected = capture.candidate(&plan.objects)?;
        let receipt = crate::history_authoring_act::receipt(
            capture, &projected, &plan, options, runtime, audit, archive,
        )?;
        (plan.objects, projected.document().clone(), receipt)
    } else if kind == "physical-import" {
        crate::history_node_physical::plan(capture, options, evidence)?
    } else if kind == "hypothesis" {
        crate::history_node_hypothesis::prepare(capture, action, options, runtime, audit, archive)?
    } else if kind == "view-edit-proposals" {
        crate::history_node_edits::prepare(
            capture, action, options, runtime, audit, archive, evidence,
        )?
    } else if kind == "proposal" {
        let a = schema(action, &["kind", "id", "body", "into", "because"], &[])?;
        let proposal = A::Proposal {
            subject: text(&a["id"])?.into(),
            body: a["body"].clone(),
            collection: text(&a["into"])?.into(),
            because: text(&a["because"])?.into(),
            hypothesis: None,
        };
        let plan =
            crate::history_authoring_proposal::prepare(capture, &proposal, options, runtime)?;
        let projected = capture.candidate(&plan.objects)?;
        let receipt = crate::history_authoring_proposal::receipt(
            capture, &proposal, &plan, options, runtime, audit, archive,
        )?;
        (plan.objects, projected.document().clone(), receipt)
    } else if ["same", "distinct"].contains(&kind) {
        crate::history_node_identity::prepare(capture, action, options, runtime, audit, archive)?
    } else if kind == "batch" {
        let a = schema(action, &["kind", "actions"], &["context"])?;
        let mut actions = list(&a["actions"])?.to_vec();
        require(!actions.is_empty() && actions.len() <= 64, "invalid_batch")?;
        for action in &mut actions {
            capture.check_expected(action)?;
            let a = map_mut(action)?;
            if !a.get("as_of").is_some_and(crate::history_view::truth) {
                a.insert("as_of".into(), s(&options.recording_day));
            }
        }
        let version = options.receipt_version.unwrap_or(8);
        require([6, 8].contains(&version), "invalid_authoring_receipt")?;
        let batch = crate::history_authoring_batch::BatchOptions {
            authoring: options.clone(),
            receipt_version: version,
            context: a.get("context").cloned().unwrap_or_else(A::empty),
            evidence: evidence.clone(),
        };
        crate::history_node_receipt::report_context(&batch.context)?;
        if !map(&batch.context)?.is_empty() {
            let c = map(&batch.context)?;
            let path = format!("evidence/reports/{}.txt", text(&c["event_id"])?);
            require(
                evidence.len() == 1
                    && evidence.get(&path).is_some_and(|raw| {
                        string_is(&c["source_sha256"], &crate::identity::sha256(raw))
                    }),
                "node_report_evidence_mismatch",
            )?;
        }
        let plan = crate::history_authoring_batch_core::prepare(
            capture, &actions, &batch, runtime, audit, archive,
        )?;
        let projected = capture.candidate(&plan.objects)?;
        require(
            projected.document().digest()? == plan.document.digest()?,
            "batch_final_projection_mismatch",
        )?;
        let mut ordered = plan
            .objects
            .into_iter()
            .map(|object| {
                let o = map(&object)?;
                Ok((
                    (
                        text(&o["subject"])?.to_owned(),
                        list(&o["saw"])?.len(),
                        text(&o["id"])?.to_owned(),
                    ),
                    object,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        ordered.sort_by(|a, b| a.0.cmp(&b.0));
        (
            ordered.into_iter().map(|(_, o)| o).collect(),
            plan.document,
            plan.receipt,
        )
    } else {
        let plan = Core::prepare(capture, action, options, runtime)?;
        let receipt = Core::receipt(capture, &plan, options, runtime, audit, archive)?;
        (plan.objects, plan.document, receipt)
    };
    if compact {
        return ledger_materialized(
            capture,
            &objects,
            document,
            action,
            options,
            encoded_options,
            archive,
            evidence,
            &receipt,
        );
    }
    let receipt = Receipt::pack(&receipt)?;
    let mut build = Build::new(capture, &options.operation)?;
    let mut nodes = components(
        &capture.snapshot,
        &capture.snapshot.transactions.keys().cloned().collect(),
        None,
    )?;
    build.evidence(&mut nodes, receipt.before(), "before")?;
    let mut observations = capture.history.observations().clone();
    let mut claims = BTreeMap::<String, usize>::new();
    for object in &objects {
        let object = map(object)?;
        if !string_is(&object["kind"], "act") {
            *claims.entry(text(&object["subject"])?.into()).or_default() += 1;
        }
    }
    for object in &objects {
        let o = map(object)?;
        let subject = text(&o["subject"])?;
        let id = text(&o["id"])?;
        let saw = list(&o["saw"])?
            .iter()
            .map(|v| text(v).map(str::to_owned))
            .collect::<Result<BTreeSet<_>>>()?;
        let observation = if let Some(base) = build.bases.get(subject) {
            let old = Frame::decode(node_value(base)?)?;
            let old_context = map(&map(&old.semantic)?["context"])?;
            let old_id = text(&map(&old_context["header"])?["id"])?;
            ObservationNode::between(id, old_id, &observations.replay(old_id)?, &saw)?
        } else {
            ObservationNode::root(id, &saw)?
        };
        observations.insert(observation.clone())?;
        let value = Frame::encode(
            &N::payload(object, &observation)?,
            "semantic",
            "semantic",
            nodes.values().get(subject),
        )?;
        let visible = map(&document)?
            .get(text(&map(&value)?["collection"])?)
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get(subject))
            == Some(&o["body"]);
        if !build.bases.contains_key(subject)
            && !string_is(&o["kind"], "act")
            && visible
            && claims.get(subject) == Some(&1)
        {
            build.lazy(subject, &value)?;
        } else {
            build.append(subject, value)?;
        }
    }
    build.evidence(&mut nodes, receipt.after(), "after")?;
    finish(
        build,
        receipt,
        document,
        &options.operation,
        if kind == "physical-import" {
            crate::history_node_physical::FORMAT
        } else {
            FORMAT
        },
        encoded_options,
    )
}
/// One physical node transition contains all semantic objects authored for that subject.
fn ledger_materialized(
    capture: &Capture,
    objects: &[V],
    document: V,
    action: &V,
    options: &A::Options,
    encoded_options: V,
    archive: &V,
    evidence: &BTreeMap<String, Vec<u8>>,
    receipt: &V,
) -> Result<Materialized> {
    let mut build = Build::new(capture, &options.operation)?;
    let mut observations = capture.history.observations().clone();
    let mut grouped = BTreeMap::<String, Vec<(V, ObservationNode)>>::new();
    let mut last = BTreeMap::<String, String>::new();
    for (subject, base) in &build.bases {
        let state = node_value(base)?;
        let id = if crate::history_node_ledger::is_ledger(state) {
            crate::history_node_ledger::observation_base(state)?
        } else {
            let payload = Frame::decode(state)?;
            Some(text(&map(&map(&map(&payload.semantic)?["context"])?["header"])?["id"])?.into())
        };
        if let Some(id) = id {
            last.insert(subject.clone(), id);
        }
    }
    for object in objects {
        let o = map(object)?;
        let subject = text(&o["subject"])?;
        let id = text(&o["id"])?;
        let saw = list(&o["saw"])?
            .iter()
            .map(|v| text(v).map(str::to_owned))
            .collect::<Result<BTreeSet<_>>>()?;
        let observation = if let Some(previous) = last.get(subject) {
            ObservationNode::between(id, previous, &observations.replay(previous)?, &saw)?
        } else {
            ObservationNode::root(id, &saw)?
        };
        observations.insert(observation.clone())?;
        last.insert(subject.into(), id.into());
        grouped
            .entry(subject.into())
            .or_default()
            .push((object.clone(), observation));
    }
    for (subject, objects) in grouped {
        let previous = build.bases.get(&subject).map(node_value).transpose()?;
        let value = crate::history_node_ledger::pack(previous, &objects)?;
        let p = map(&value)?;
        let visible = map(&document)?
            .get(text(&p["collection"])?)
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get(&subject))
            == Some(&p["body"]);
        if previous.is_none() && visible {
            build.lazy(&subject, &value)?;
        } else {
            build.append(&subject, value)?;
        }
    }
    let context = crate::history_node_transaction::create(
        &capture.snapshot,
        action,
        encoded_options,
        archive,
        evidence,
        &document,
        receipt,
    )?;
    finish_context(build, document, &options.operation, context)
}
fn finish_context(
    build: Build<'_>,
    mut document: V,
    operation: &str,
    context: V,
) -> Result<Materialized> {
    let meta = map_mut(
        map_mut(&mut document)?
            .get_mut("meta")
            .ok_or_else(|| error("invalid_document"))?,
    )?;
    meta.insert(
        "node_history".into(),
        A::obj([
            ("version", A::n("2")),
            ("originals", V::Map(build.originals)),
            ("tails", V::Map(build.tails)),
        ]),
    );
    Ok(Materialized {
        after: P::bind_view(&Y::encode_document(&document)?, operation)?,
        frames: build.frames,
        context,
    })
}
fn finish(
    build: Build<'_>,
    receipt: Receipt,
    document: V,
    operation: &str,
    format: &str,
    encoded_options: V,
) -> Result<Materialized> {
    let mut doc = document;
    let meta = map_mut(
        map_mut(&mut doc)?
            .get_mut("meta")
            .ok_or_else(|| error("invalid_document"))?,
    )?;
    meta.insert(
        "node_history".into(),
        V::Map(Map::from([
            ("version".into(), V::from_json(&serde_json::json!(2))?),
            ("originals".into(), V::Map(build.originals)),
            ("tails".into(), V::Map(build.tails)),
        ])),
    );
    let after = P::bind_view(&Y::encode_document(&doc)?, operation)?;
    let context = V::Map(Map::from([
        ("format".into(), s(format)),
        ("options".into(), encoded_options),
        ("header".into(), receipt.header().clone()),
        ("before".into(), receipt.before().context().clone()),
        ("after".into(), receipt.after().context().clone()),
    ]));
    Ok(Materialized {
        after,
        frames: build.frames,
        context,
    })
}

/// Evidence-only transition, with exact before/after components and no new claim.
pub(crate) fn clock_transition(
    capture: &Capture,
    candidate: &Capture,
    operation: &str,
    evidence: &BTreeMap<String, Vec<u8>>,
) -> Result<Materialized> {
    let before_doc = A::destination(capture.document())?;
    let after_doc = A::destination(candidate.document())?;
    let capabilities = crate::reasoning_fields::capabilities(&before_doc, None)?;
    let profile = text(field(map(&capabilities)?, "profile")?)?;
    let receipt = crate::history_transaction::semantic_receipt(
        profile,
        &capabilities,
        &A::obj([("document", before_doc)]),
        &A::obj([("document", after_doc.clone())]),
    )?;
    let context = crate::history_node_transaction::create(
        &capture.snapshot,
        &A::obj([("kind", s("source-clocks"))]),
        A::empty(),
        &A::empty(),
        evidence,
        &after_doc,
        &receipt,
    )?;
    finish_context(
        Build::new(capture, operation)?,
        after_doc,
        operation,
        context,
    )
}

pub fn prepare(
    root: &Path,
    action: &V,
    options: &A::Options,
    runtime: Option<&Runtime>,
) -> Result<P::Prepared> {
    prepare_evidence(root, action, options, runtime, BTreeMap::new())
}

/// Record all edited bodies as proposals, using a caller-retained exact accepted view.
pub fn prepare_edits(
    root: &Path,
    canonical: &[u8],
    because: &str,
    subjects: Option<&[String]>,
    options: &A::Options,
    runtime: Option<&Runtime>,
) -> Result<P::Prepared> {
    let _lock = FS::DirectoryGuard::acquire(root, false)?;
    let capture = crate::history_node_edits::capture(root, canonical)?;
    let raw = crate::history_node_edits::edited(root)?;
    let selected = crate::history_node_edits::selection(&capture, &raw, subjects)?;
    let action = A::obj([
        ("kind", s("view-edit-proposals")),
        ("because", s(because)),
        ("subjects", A::strings(selected)),
    ]);
    let evidence = BTreeMap::from([(
        format!("evidence/view-edits/{}.yaml", options.operation),
        raw,
    )]);
    let result = materialize_ledger(
        &capture,
        &action,
        options,
        runtime,
        None,
        &archive(root)?,
        &evidence,
    )?;
    let prepared = P::prepare_edited(
        root,
        &options.operation,
        result.after,
        result.frames,
        &result.context,
        evidence,
        canonical,
    )?;
    verify(root, &prepared, runtime)?;
    Ok(prepared)
}

pub fn prepare_batch(
    root: &Path,
    actions: &[V],
    batch: &crate::history_authoring_batch::BatchOptions,
    runtime: Option<&Runtime>,
) -> Result<P::Prepared> {
    let mut action = A::obj([("kind", s("batch")), ("actions", V::List(actions.to_vec()))]);
    if batch.context != A::empty() {
        map_mut(&mut action)?.insert("context".into(), batch.context.clone());
    }
    let mut options = batch.authoring.clone();
    options.receipt_version = Some(batch.receipt_version);
    prepare_evidence(root, &action, &options, runtime, batch.evidence.clone())
}
fn prepare_evidence(
    root: &Path,
    action: &V,
    options: &A::Options,
    runtime: Option<&Runtime>,
    evidence: BTreeMap<String, Vec<u8>>,
) -> Result<P::Prepared> {
    let _lock = FS::DirectoryGuard::acquire(root, false)?;
    if map(action)?
        .get("kind")
        .is_some_and(|v| ["same", "distinct"].iter().any(|k| string_is(v, k)))
    {
        crate::history_node_identity::sources(root)?;
    }
    if map(action)?
        .get("kind")
        .is_some_and(|v| string_is(v, "hypothesis"))
    {
        crate::history_node_hypothesis::sources(root)?;
    }
    let capture = Capture::read(root)?;
    let result = materialize_ledger(
        &capture,
        action,
        options,
        runtime,
        None,
        &archive(root)?,
        &evidence,
    )?;
    let prepared = P::prepare_with_evidence(
        root,
        &options.operation,
        result.after,
        result.frames,
        Some(&result.context),
        evidence,
    )?;
    verify(root, &prepared, runtime)?;
    Ok(prepared)
}

pub fn verify(root: &Path, prepared: &P::Prepared, runtime: Option<&Runtime>) -> Result<()> {
    if prepared
        .context()?
        .as_ref()
        .is_some_and(|c| crate::history_node_legacy::kind(c, crate::history_node_physical::FORMAT))
    {
        return crate::history_node_physical::verify(root, prepared);
    }

    if prepared
        .context()?
        .as_ref()
        .is_some_and(|c| crate::history_node_clocks::is_clocks(c).unwrap_or(false))
    {
        return crate::history_node_clocks::verify(root, prepared);
    }
    if prepared
        .context()?
        .as_ref()
        .is_some_and(|c| crate::history_node_branch::is_union(c).unwrap_or(false))
    {
        return crate::history_node_branch::verify(root, prepared);
    }
    if prepared
        .context()?
        .as_ref()
        .is_some_and(crate::history_node_transaction::is_context)
    {
        return verify_ledger(root, prepared, runtime);
    }
    require(
        prepared.imports()?.is_empty(),
        "node_import_action_unsupported",
    )?;
    let (before, after) = prepared.snapshots(root)?;
    let capture = Capture::from_snapshot(before)?;
    // Capturing after verifies semantic IDs, exact observations, receipt digests and current bodies.
    let candidate = Capture::from_snapshot(after)?;
    let recorded = receipt(&candidate.snapshot, prepared.operation())?;
    let c = prepared
        .context()?
        .ok_or_else(|| error("node_transaction_missing_context"))?;
    let c = context(&c)?;
    let options = decode_options(prepared.operation(), &c["options"])?;
    let intent = intent(&recorded)?;
    let is_edit = intent
        .get("kind")
        .is_some_and(|v| string_is(v, "view-edit-proposals"));
    require(
        is_edit == prepared.canonical_before()?.is_some(),
        "node_edit_baseline_required",
    )?;
    if is_edit {
        let raw = prepared
            .before_view()?
            .ok_or_else(|| error("node_edit_evidence_mismatch"))?;
        let evidence = prepared.evidence()?;
        require(
            evidence.get(&format!(
                "evidence/view-edits/{}.yaml",
                prepared.operation()
            )) == Some(&raw),
            "node_edit_evidence_mismatch",
        )?;
    }
    if intent
        .get("kind")
        .is_some_and(|v| ["same", "distinct"].iter().any(|k| string_is(v, k)))
    {
        crate::history_node_identity::sources(root)?;
    }
    if map(&map(&recorded)?["before"])?.contains_key("hypothesis_authoring") {
        crate::history_node_hypothesis::sources(root)?;
    }
    let current_archive = archive(root)?;
    require(
        current_archive == *field(intent, "archive")?,
        "concurrent_archive_edit",
    )?;
    let audit = ReplayAudit::from_receipts_lazy(&recorded, || {
        let receipt_components =
            crate::history_node_receipt_components::Components::new(&capture.snapshot)?;
        capture
            .snapshot
            .transactions
            .iter()
            .filter(|(_, tx)| {
                tx.context.as_ref().is_some_and(|c| {
                    !crate::history_node_branch::is_union(c).unwrap_or(false)
                        && !crate::history_node_clocks::is_clocks(c).unwrap_or(false)
                        && !crate::history_node_legacy::kind(
                            c,
                            crate::history_node_legacy::CHECKPOINT,
                        )
                        && !crate::history_node_legacy::kind(
                            c,
                            crate::history_node_bootstrap::FORMAT,
                        )
                        && !crate::history_node_legacy::kind(
                            c,
                            crate::history_node_physical::FORMAT,
                        )
                })
            })
            .map(|(op, _)| receipt_index(&capture.snapshot, op, &receipt_components))
            .collect::<Result<Vec<_>>>()
    })?;
    let replay = materialize(
        &capture,
        &action(&recorded)?,
        &options,
        runtime,
        Some(&audit),
        &current_archive,
        &prepared.evidence()?,
    )?;
    require(
        replay.after == prepared.after_view()?
            && replay.frames == prepared.frames()?
            && replay.context == V::Map(c.clone()),
        "node_authoring_replay_mismatch",
    )
}

fn verify_ledger(root: &Path, prepared: &P::Prepared, runtime: Option<&Runtime>) -> Result<()> {
    require(
        prepared.imports()?.is_empty(),
        "node_import_action_unsupported",
    )?;
    let (before, after) = prepared.snapshots(root)?;
    let capture = Capture::from_snapshot(before)?;
    Capture::from_snapshot(after)?;
    let context = prepared
        .context()?
        .ok_or_else(|| error("node_transaction_missing_context"))?;
    let c = crate::history_node_transaction::validate(&context)?;
    let action = &c["action"];
    let kind = text(field(map(action)?, "kind")?)?;
    let is_edit = kind == "view-edit-proposals";
    require(
        is_edit == prepared.canonical_before()?.is_some(),
        "node_edit_baseline_required",
    )?;
    let evidence = prepared.evidence()?;
    if is_edit {
        require(
            evidence.get(&format!(
                "evidence/view-edits/{}.yaml",
                prepared.operation()
            )) == prepared.before_view()?.as_ref(),
            "node_edit_evidence_mismatch",
        )?;
    }
    if ["same", "distinct"].contains(&kind) {
        crate::history_node_identity::sources(root)?;
    }
    if kind == "hypothesis" {
        crate::history_node_hypothesis::sources(root)?;
    }
    let current_archive = archive(root)?;
    require(current_archive == c["archive"], "concurrent_archive_edit")?;
    let audit =
        ReplayAudit::from_compact(crate::history_node_transaction::audits(&context)?, || {
            capture
                .snapshot
                .transactions
                .iter()
                .filter_map(|(op, tx)| tx.context.as_ref().map(|c| (op, c)))
                .map(|(op, c)| {
                    if crate::history_node_transaction::is_context(c) {
                        Ok(crate::history_node_transaction::validate(c)?["audits"].clone())
                    } else {
                        ReplayAudit::compact(&receipt(&capture.snapshot, op)?)
                    }
                })
                .collect::<Result<Vec<_>>>()
        })?;
    let options = decode_options(prepared.operation(), &c["options"])?;
    let replay = materialize_ledger(
        &capture,
        action,
        &options,
        runtime,
        Some(&audit),
        &current_archive,
        &evidence,
    )?;
    require(
        replay.after == prepared.after_view()?
            && replay.frames == prepared.frames()?
            && replay.context == context,
        "node_authoring_replay_mismatch",
    )
}

pub fn publish(
    root: &Path,
    prepared: &P::Prepared,
    runtime: Option<&Runtime>,
    mut boundary: impl FnMut(P::Phase) -> Result<()>,
) -> Result<()> {
    P::publish(
        root,
        prepared,
        |p| verify(root, p, runtime),
        |phase| {
            verify_sources(root, prepared)?;
            boundary(phase)?;
            verify_sources(root, prepared)
        },
    )
}
pub(crate) fn verify_sources(root: &Path, prepared: &P::Prepared) -> Result<()> {
    if let Some(context) = prepared
        .context()?
        .as_ref()
        .filter(|c| crate::history_node_transaction::is_context(c))
    {
        let action = &crate::history_node_transaction::validate(context)?["action"];
        let kind = text(field(map(action)?, "kind")?)?;
        if kind == "hypothesis" {
            crate::history_node_hypothesis::sources(root)?;
        }
        if ["same", "distinct"].contains(&kind) {
            crate::history_node_identity::sources(root)?;
        }
        return Ok(());
    }
    let c = prepared
        .context()?
        .ok_or_else(|| error("node_transaction_missing_context"))?;
    let c = context(&c)?;
    let before = map(field(map(&c["before"])?, "literal")?)?;
    if before.contains_key("hypothesis_authoring") {
        crate::history_node_hypothesis::sources(root)?;
    }
    if before.contains_key("identity_authoring") {
        crate::history_node_identity::sources(root)?;
    }
    Ok(())
}
pub fn recover(root: &Path, runtime: Option<&Runtime>) -> Result<&'static str> {
    P::recover(root, |p| verify(root, p, runtime))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn v(j: serde_json::Value) -> V {
        V::from_json(&j).unwrap()
    }
    #[test]
    fn core_add_set_review_replays_receipts_and_preserves_original_pins() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".kpopper")).unwrap();
        std::fs::write(root.path().join(".kpopper/history.yaml"), "version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: fixture\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
        let doc = v(
            json!({"meta":{"purpose":"Fixture","reasoning":{"version":2,"profile":"core/v1","requires":["arithmetic/v1"]}},"schema":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"},"readings":{},"judgments":{}}),
        );
        std::fs::write(
            root.path().join("GROUNDING.yaml"),
            Y::encode_document(&doc).unwrap(),
        )
        .unwrap();
        let runtime = A::tests::runtime(&root.path().join("runtime"));
        let actions = [
            json!({"kind":"add","id":"p.a","body":{"v":1},"into":"readings"}),
            json!({"kind":"add","id":"d.b","body":{"claim":"Input is small","rests_on":["p.a"],"wrong_if":"p.a > 3"},"into":"judgments"}),
            json!({"kind":"set","id":"p.a","value":2}),
            json!({"kind":"review","id":"d.b","why":"checked new input"}),
        ];
        let mut receipts = Vec::new();
        for (i, action) in actions.into_iter().enumerate() {
            let options = A::Options {
                operation: format!("op-{i}"),
                recorded_at: "2026-09-24T12:00:00+00:00".into(),
                recording_day: "2026-09-24".into(),
                by: s("writer"),
                strict: true,
                paths: crate::history_paths::Scheme::Hashed,
                receipt_version: None,
            };
            let prepared = prepare(root.path(), &v(action), &options, Some(&runtime))
                .unwrap_or_else(|e| panic!("prepare {i}: {e}"));
            publish(root.path(), &prepared, Some(&runtime), |_| Ok(()))
                .unwrap_or_else(|e| panic!("publish {i}: {e}"));
            receipts.push(
                receipt(
                    &P::capture_snapshot(root.path()).unwrap(),
                    &options.operation,
                )
                .unwrap(),
            );
        }
        let capture = Capture::read(root.path()).unwrap();
        let projected = crate::history_node_projection::capture(&capture).unwrap();
        let source = crate::source_capture::capture_source_with_runtime(
            &[root.path().join("GROUNDING.yaml")],
            root.path(),
            crate::source_capture::ReadMode::Frozen,
            Some(s("2026-09-24")),
            Some(&runtime),
        )
        .unwrap();
        source.verify().unwrap();
        let projected_doc = projected.document();
        assert_eq!(
            map(&map(projected_doc).unwrap()["readings"]).unwrap()["p.a"],
            map(&map(capture.document()).unwrap()["readings"]).unwrap()["p.a"]
        );
        assert_eq!(capture.object_count(), 7);
        let state = map(&map(capture.state()).unwrap()["subjects"]).unwrap();
        let head = text(&map(&state["d.b"]).unwrap()["head"]).unwrap();
        let judgment = capture.object("d.b", head).unwrap();
        let pin = text(&map(&map(&judgment).unwrap()["pins"]).unwrap()["p.a"]).unwrap();
        assert_eq!(
            map(&map(&capture.object("p.a", pin).unwrap()).unwrap()["body"]).unwrap()["v"],
            v(json!(1))
        );
        let bundle = P::export(root.path()).unwrap();
        let copy = bundle.reconstruct().unwrap();
        drop(capture);
        drop(root);
        let restored = Capture::read(copy.path()).unwrap();
        for (i, expected) in receipts.iter().enumerate() {
            assert_eq!(
                receipt(&restored.snapshot, &format!("op-{i}")).unwrap(),
                *expected
            );
        }
    }
}

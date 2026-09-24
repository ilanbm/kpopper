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
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn node_value(version: &C::Version) -> Result<&V> {
    version
        .state()
        .ok_or_else(|| error("node_semantic_absent_unsupported"))
}
fn context(value: &V) -> Result<&Map> {
    let c = schema(
        value,
        &["format", "options", "header", "before", "after"],
        &[],
    )?;
    require(string_is(&c["format"], FORMAT), "node_transaction_format")?;
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
    let mut values = Map::new();
    for (subject, versions) in &snapshot.versions {
        let selected = versions
            .iter()
            .filter_map(|(id, version)| {
                if !allowed.contains(version.operation()) {
                    return None;
                }
                Some((|| {
                    let payload = Frame::decode(node_value(version)?)?;
                    Ok((
                        id,
                        version,
                        before != Some(version.operation()) || payload.phase == "before",
                    ))
                })())
            })
            .collect::<Result<Vec<_>>>()?;
        let selected = selected
            .into_iter()
            .filter(|(_, _, keep)| *keep)
            .map(|(id, v, _)| (id.clone(), v.clone()))
            .collect::<BTreeMap<_, _>>();
        if let Some(tip) = Frame::tip(&selected)? {
            if let Some(value) = Frame::decode(node_value(tip)?)?.receipt {
                values.insert(subject.clone(), value);
            }
        }
    }
    Nodes::from_values(values)
}
/// Restore the exact original receipt using only its causal node components and compact context.
pub fn receipt(snapshot: &P::Snapshot, operation: &str) -> Result<V> {
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
    let before = components(snapshot, &allowed, Some(operation))?.side(&c["before"])?;
    let after = components(snapshot, &allowed, None)?.side(&c["after"])?;
    Receipt::from_parts(c["header"].clone(), before, after)?.restore()
}

pub(crate) fn verify_receipts(snapshot: &P::Snapshot) -> Result<()> {
    for (op, tx) in &snapshot.transactions {
        if tx.context.is_some() {
            receipt(snapshot, op)?;
        }
    }
    Ok(())
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
fn decode_options(operation: &str, value: &V) -> Result<A::Options> {
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
    } else if is_int(&o["receipt_version"], "7") {
        Some(7)
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
struct Materialized {
    after: Vec<u8>,
    frames: BTreeMap<String, Vec<u8>>,
    context: V,
}
fn materialize(
    capture: &Capture,
    action: &V,
    options: &A::Options,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
    archive: &V,
) -> Result<Materialized> {
    let encoded_options = encode_options(options)?;
    // Preserve the optimistic snapshot guard on the same exact before bytes used by replay.
    capture.check_expected(action)?;
    let plan = Core::prepare(capture, action, options, runtime)?;
    let receipt = Receipt::pack(&Core::receipt(
        capture, &plan, options, runtime, audit, archive,
    )?)?;
    let mut build = Build::new(capture, &options.operation)?;
    let mut nodes = components(
        &capture.snapshot,
        &capture.snapshot.transactions.keys().cloned().collect(),
        None,
    )?;
    build.evidence(&mut nodes, receipt.before(), "before")?;
    let mut observations = capture.history.observations().clone();
    for object in &plan.objects {
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
        if !build.bases.contains_key(subject) && !string_is(&o["kind"], "act") {
            build.lazy(subject, &value)?;
        } else {
            build.append(subject, value)?;
        }
    }
    build.evidence(&mut nodes, receipt.after(), "after")?;
    let mut doc = plan.document;
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
    let after = P::bind_view(&Y::encode_document(&doc)?, &options.operation)?;
    let context = V::Map(Map::from([
        ("format".into(), s(FORMAT)),
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

pub fn prepare(
    root: &Path,
    action: &V,
    options: &A::Options,
    runtime: Option<&Runtime>,
) -> Result<P::Prepared> {
    let _lock = FS::DirectoryGuard::acquire(root, false)?;
    let capture = Capture::read(root)?;
    let result = materialize(&capture, action, options, runtime, None, &archive(root)?)?;
    let prepared = P::prepare_with_context(
        root,
        &options.operation,
        result.after,
        result.frames,
        Some(&result.context),
    )?;
    verify(root, &prepared, runtime)?;
    Ok(prepared)
}

pub fn verify(root: &Path, prepared: &P::Prepared, runtime: Option<&Runtime>) -> Result<()> {
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
    let intent = map(field(map(&map(&recorded)?["before"])?, "authoring")?)?;
    let current_archive = archive(root)?;
    require(
        current_archive == *field(intent, "archive")?,
        "concurrent_archive_edit",
    )?;
    let parents = capture
        .snapshot
        .transactions
        .iter()
        .filter(|(_, tx)| tx.context.is_some())
        .map(|(op, _)| receipt(&capture.snapshot, op))
        .collect::<Result<Vec<_>>>()?;
    let audit = ReplayAudit::from_receipts(&recorded, &parents)?;
    let replay = materialize(
        &capture,
        field(intent, "action")?,
        &options,
        runtime,
        Some(&audit),
        &current_archive,
    )?;
    require(
        replay.after == prepared.after_view()?
            && replay.frames == prepared.frames()?
            && replay.context == V::Map(c.clone()),
        "node_authoring_replay_mismatch",
    )
}

pub fn publish(
    root: &Path,
    prepared: &P::Prepared,
    runtime: Option<&Runtime>,
    boundary: impl FnMut(P::Phase) -> Result<()>,
) -> Result<()> {
    P::publish(root, prepared, |p| verify(root, p, runtime), boundary)
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

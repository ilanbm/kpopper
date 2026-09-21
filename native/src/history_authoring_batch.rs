//! Final-world admission and immutable final pins; legacy sequential receipts replay exactly.
use crate::{
    Result,
    history_authoring::{self as A, Options},
    history_authoring_audit::ReplayAudit,
    history_capture::Capture,
    history_contract::*,
    history_preparation as P,
    history_store::Store,
    history_transaction::{self as T, FileImage, PreparedMutation},
    history_view::{list, map_mut, truth},
    identity::sha256,
    reasoning_authoring::{self as W, World},
    reasoning_fields as F,
    reasoning_runtime::{OperationalBounds, Runtime},
    reasoning_snapshot::entries,
    require,
    value::TypedValue as V,
};
use A::{empty, n, obj, s, strings};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub struct BatchOptions {
    pub authoring: Options,
    pub receipt_version: u8,
    pub context: V,
    pub evidence: crate::history_authority::Files,
}
#[derive(Clone)]
struct Step {
    index: usize,
    subject: String,
    action: V,
    prior: Option<(String, V)>,
    collection: String,
    body: V,
}
fn stage(document: &V, actions: &[V]) -> Result<(V, Vec<Step>)> {
    let mut document = document.clone();
    let mut steps = vec![];
    for (index, action) in actions.iter().enumerate() {
        let a = map(action).map_err(|_| error("invalid_batch_action"))?;
        let kind = text(field(a, "kind")?)?;
        require(
            ["add", "set", "review"].contains(&kind),
            "invalid_batch_action",
        )?;
        require(
            !a.get("hypothesis").is_some_and(truth) && !a.get("section").is_some_and(truth),
            "history_hypothesis_write_unsupported",
        )?;
        let subject = text(field(a, "id")?)?;
        crate::history_paths::subject(subject)?;
        let prior = entries(&document)?.get(subject).cloned();
        let fields = F::snapshot_fields(&document)?;
        let (collection, body) = if kind == "add" {
            let body = field(a, "body")?.clone();
            let collection = if let Some((c, _)) = &prior {
                c.clone()
            } else {
                crate::reasoning_authoring_guards::collection_for_document(
                    &document,
                    &fields,
                    subject,
                    &body,
                    a.get("into").and_then(|v| text(v).ok()),
                )?
            };
            map_mut(
                map_mut(&mut document)?
                    .entry(collection.clone())
                    .or_insert_with(empty),
            )?
            .insert(subject.into(), body.clone());
            (collection, body)
        } else {
            let (collection, mut body) = prior
                .clone()
                .ok_or_else(|| error("unknown_batch_subject"))?;
            if kind == "set" {
                let b = map_mut(&mut body).map_err(|_| error("history_body_mapping_required"))?;
                let field_name = if b.contains_key("v") {
                    "v"
                } else if b.contains_key("quoted") {
                    "quoted"
                } else {
                    return Err(error("set needs an entry with v or quoted"));
                };
                b.insert(field_name.into(), field(a, "value")?.clone());
                b.insert("of".into(), field(a, "as_of")?.clone());
                if let Some(source) = a.get("source").filter(|v| **v != V::Null) {
                    let at = a
                        .get("at")
                        .filter(|v| **v != V::Null)
                        .ok_or_else(|| error("a changed source needs its exact at location"))?;
                    b.insert("from".into(), source.clone());
                    b.insert("at".into(), at.clone());
                }
                if let Some(scope) = a.get("_record_scope") {
                    b.insert("scope".into(), scope.clone());
                }
                map_mut(map_mut(&mut document)?.get_mut(&collection).unwrap())?
                    .insert(subject.into(), body.clone());
            } else if let Some(scope) = a.get("_record_scope") {
                require(
                    map(&body)?.get("scope").unwrap_or(&V::Null).digest()? == scope.digest()?,
                    "history_review_scope_change_requires_claim",
                )?;
            }
            (collection, body)
        };
        steps.push(Step {
            index,
            subject: subject.into(),
            action: action.clone(),
            prior,
            collection,
            body,
        });
    }
    if let Some(meta) = map_mut(&mut document)?.get_mut("meta")
        && map(meta)?.contains_key("updated")
    {
        map_mut(meta)?.insert(
            "updated".into(),
            field(map(actions.last().unwrap())?, "as_of")?.clone(),
        );
    }
    Ok((document, steps))
}
fn admission_document(final_document: &V, step: &Step) -> Result<V> {
    let mut doc = final_document.clone();
    for name in F::collections(final_document)?.keys() {
        map_mut(map_mut(&mut doc)?.get_mut(name).unwrap())?.remove(&step.subject);
    }
    if let Some((collection, body)) = &step.prior {
        map_mut(
            map_mut(&mut doc)?
                .entry(collection.clone())
                .or_insert_with(empty),
        )?
        .insert(step.subject.clone(), body.clone());
    }
    Ok(doc)
}
fn evidence_files(options: &BatchOptions) -> Result<Vec<FileImage>> {
    options
        .evidence
        .iter()
        .map(|(path, raw)| {
            crate::history_authority::relative_path(path)?;
            Ok(FileImage {
                path: path.clone(),
                role: "history_evidence".into(),
                before: None,
                after: Some(raw.clone()),
            })
        })
        .collect()
}
pub fn prepare_batch(
    store: &Store,
    capture: &Capture,
    actions: &[V],
    options: &BatchOptions,
    runtime: Option<&Runtime>,
) -> Result<PreparedMutation> {
    prepare_inner(store, capture, actions, options, runtime, None)
}
pub(crate) fn prepare_inner(
    store: &Store,
    capture: &Capture,
    actions: &[V],
    options: &BatchOptions,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
) -> Result<PreparedMutation> {
    require(!actions.is_empty() && actions.len() <= 64, "invalid_batch")?;
    require(
        [2, 3, 6, 8].contains(&options.receipt_version),
        "invalid_authoring_receipt",
    )?;
    let mut actions = actions.to_vec();
    for action in &mut actions {
        let a = map_mut(action).map_err(|_| error("invalid_batch_action"))?;
        if !a.get("as_of").is_some_and(truth) {
            require(
                !options.authoring.recording_day.is_empty(),
                "missing_recording_time",
            )?;
            a.insert("as_of".into(), s(&options.authoring.recording_day));
        }
    }
    if options.receipt_version < 6 {
        sequential(store, capture, &actions, options, runtime, audit)
    } else {
        final_world(store, capture, &actions, options, runtime, audit)
    }
}
fn final_world(
    store: &Store,
    original: &Capture,
    actions: &[V],
    options: &BatchOptions,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
) -> Result<PreparedMutation> {
    let mut op = options.authoring.clone();
    op.strict = true;
    A::guards(store, original, &op)?;
    crate::history_sources::capture(&store.root, &store.layout.entry, &original.document)?;
    let frozen = A::archive(store)?;
    let before_doc = A::document(original)?;
    W::validate_declared(&before_doc)?;
    let (staged, raw_steps) = stage(&before_doc, actions)?;
    let staged = A::destination(&staged)?;
    let normalization = if W::selected(&staged, None)? {
        Some(World::new(
            &staged,
            None,
            runtime,
            OperationalBounds::default(),
        )?)
    } else {
        None
    };
    let mut normalized = vec![];
    let mut notes = vec![];
    for step in &raw_steps {
        let doc = admission_document(&staged, step)?;
        let previous = entries(&doc)?
            .into_iter()
            .map(|(id, (_, body))| (id, body))
            .collect();
        let (action, observed) = if let Some(normalization) = &normalization {
            normalization.normalize(&step.action, Some(&previous))?
        } else {
            let mut reader = crate::history_authoring_reader::AuthoringReader::new(&doc, runtime)?;
            reader.normalize(&step.action)?
        };
        normalized.push(action);
        notes.push(observed);
    }
    let (doc, mut steps) = stage(&before_doc, &normalized)?;
    let mut doc = A::destination(&doc)?;
    let mut profiles = BTreeSet::new();
    for state in map(&map(&original.state)?["subjects"])?.values() {
        if let Some(h) = map(state)?.get("head") {
            let o = original
                .objects
                .get(text(h)?)
                .ok_or_else(|| error("incomplete_closure"))?;
            profiles.insert(text(field(map(field(map(o)?, "authored")?)?, "profile")?)?);
        }
    }
    require(profiles.len() <= 1, "incompatible_authored_profiles")?;
    let cap = F::capabilities(&doc, profiles.first().copied())?;
    let mut final_world = crate::history_authoring_reader::AuthoringReader::new(&doc, runtime)?;
    let mut effective_options = options.clone();
    if !final_world.core() {
        effective_options.receipt_version = 6;
    }
    let options = &effective_options;
    let mut witnesses = vec![];
    for step in &steps {
        let a = map(&step.action)?;
        require(
            a.get("profile")
                .is_none_or(|v| *v == V::Null || *v == map(&cap).unwrap()["profile"]),
            "history_profile_migration_required",
        )?;
        if map(&map(&original.state)?["subjects"])?.contains_key(&step.subject) {
            A::head(original, &step.subject)?;
        }
        let prior = admission_document(&doc, step)?;
        let mut admission = crate::history_authoring_reader::AuthoringReader::batch_admission(
            &doc,
            &prior,
            &step.subject,
            runtime,
        )?;
        admission.for_action(&step.action)?;
        let refusals = admission.validate(&step.action)?;
        require(
            refusals.is_empty(),
            &format!("refused - {}", refusals.join("\n          ")),
        )?;
        witnesses.push(obj([
            ("index", n(&step.index.to_string())),
            ("subject", s(&step.subject)),
            ("action_digest", s(&step.action.digest()?)),
            (
                "prior_body_digest",
                step.prior
                    .as_ref()
                    .map(|(_, b)| b.digest().map(|h| s(&h)))
                    .transpose()?
                    .unwrap_or(V::Null),
            ),
            ("notes", strings(notes[step.index].clone())),
        ]));
    }
    let fields = F::snapshot_fields(&doc)?;
    let deps_field = text(&fields["deps"])?;
    let snapshot_field = text(&fields["snapshot"])?;
    if options.receipt_version == 8 {
        for step in &mut steps {
            if string_is(&map(&step.action)?["kind"], "add")
                && map(&step.body).is_ok_and(|b| b.contains_key(deps_field))
            {
                let mut seen = Map::new();
                for dep in list(&map(&step.body)?[deps_field])? {
                    let name = text(dep)?;
                    if final_world.raw().contains_key(name) {
                        seen.insert(name.into(), final_world.history(name)?);
                    }
                }
                map_mut(&mut step.body)?.insert(snapshot_field.into(), V::Map(seen));
                map_mut(&mut step.action)?.insert("body".into(), step.body.clone());
                map_mut(map_mut(&mut doc)?.get_mut(&step.collection).unwrap())?
                    .insert(step.subject.clone(), step.body.clone());
            }
        }
        final_world = crate::history_authoring_reader::AuthoringReader::new(&doc, runtime)?;
    }
    let mut after = final_world.evidence(&doc, audit, &Map::new())?;
    let mut by_subject = BTreeMap::<String, Vec<usize>>::new();
    let mut producing = BTreeSet::new();
    for step in &steps {
        by_subject
            .entry(step.subject.clone())
            .or_default()
            .push(step.index);
        if !string_is(&map(&step.action)?["kind"], "review") {
            producing.insert(step.subject.clone());
        }
    }
    let original_states = map(&map(&original.state)?["subjects"])?;
    let mut versions = Map::new();
    for (name, state) in original_states {
        let state = map(state)?;
        if string_is(&state["acceptance"], "accepted")
            && !producing.contains(name)
            && let Some(h) = state.get("head")
        {
            versions.insert(name.clone(), h.clone());
        }
    }
    let mut objects = Map::new();
    let mut produced = BTreeMap::new();
    let mut remaining = by_subject.keys().cloned().collect::<BTreeSet<_>>();
    while !remaining.is_empty() {
        let mut progressed = false;
        for subject in remaining.clone() {
            let sequence = &by_subject[&subject];
            let mut dependencies = BTreeSet::new();
            for index in sequence {
                let step = &steps[*index];
                let blocked = !W::blocked_text(&step.body).is_empty();
                for dep in map(&step.body)
                    .ok()
                    .and_then(|b| b.get(deps_field))
                    .map(list)
                    .transpose()?
                    .unwrap_or(&[])
                {
                    let name = text(dep)?;
                    if !((options.receipt_version == 8 || !final_world.core())
                        && blocked
                        && !original_states.contains_key(name)
                        && !producing.contains(name))
                    {
                        dependencies.insert(name.to_owned());
                    }
                }
            }
            if dependencies
                .iter()
                .any(|d| producing.contains(d) && !versions.contains_key(d))
            {
                continue;
            }
            for dep in &dependencies {
                require(versions.contains_key(dep), "unresolved_history_subject")?;
            }
            let mut saw = A::saw(original, &subject)?;
            let mut current = if original_states.contains_key(&subject) {
                Some(A::head(original, &subject)?.clone())
            } else {
                None
            };
            let mut heads = if current.is_some() {
                map(&original_states[&subject])?["heads"].clone()
            } else {
                V::List(vec![])
            };
            for index in sequence {
                let step = &steps[*index];
                let a = map(&step.action)?;
                let kind = text(&a["kind"])?;
                let mut child = op.clone();
                child.operation = format!(
                    "batch-step-{}",
                    obj([
                        ("operation", s(&op.operation)),
                        ("index", n(&index.to_string()))
                    ])
                    .digest()?
                );
                let mut pins = Map::new();
                let mut gaps = Map::new();
                for dep in map(&step.body)
                    .ok()
                    .and_then(|b| b.get(deps_field))
                    .map(list)
                    .transpose()?
                    .unwrap_or(&[])
                {
                    let name = text(dep)?;
                    if let Some(version) = versions.get(name) {
                        pins.insert(name.into(), version.clone());
                    } else {
                        gaps.insert(name.into(), s("unavailable"));
                    }
                }
                let mut generated = vec![];
                let because = a
                    .get("why")
                    .filter(|v| truth(v))
                    .map(|v| s(&W::py(v)))
                    .unwrap_or_else(|| s(&format!("explicit {kind}")));
                if kind == "review" {
                    let c = current.as_ref().ok_or_else(|| error("invalid_review"))?;
                    require(string_is(&map(c)?["kind"], "judgment"), "invalid_review")?;
                    generated.push(A::make_object(
                        &subject,
                        "act",
                        obj([
                            ("act", s("review")),
                            ("of", map(c)?["id"].clone()),
                            ("over", V::List(vec![])),
                            ("because", because),
                            ("read", V::Map(pins)),
                        ]),
                        strings(saw.clone()),
                        None,
                        empty(),
                        empty(),
                        &child,
                    )?);
                } else {
                    let authored = if kind == "set" {
                        field(
                            map(current
                                .as_ref()
                                .ok_or_else(|| error("unresolved_history_subject"))?)?,
                            "authored",
                        )?
                        .clone()
                    } else {
                        obj([
                            ("collection", s(&step.collection)),
                            ("fields", V::Map(fields.clone())),
                            ("profile", map(&cap)?["profile"].clone()),
                        ])
                    };
                    let claim = A::make_object(
                        &subject,
                        if map(&step.body).is_ok_and(|b| b.contains_key(deps_field)) {
                            "judgment"
                        } else {
                            "reading"
                        },
                        step.body.clone(),
                        strings(saw.clone()),
                        Some(authored),
                        V::Map(pins),
                        V::Map(gaps),
                        &child,
                    )?;
                    let id = map(&claim)?["id"].clone();
                    let mut seen = saw.clone();
                    seen.push(text(&id)?.into());
                    seen.sort();
                    let act = A::make_object(
                        &subject,
                        "act",
                        obj([
                            ("act", s("accept")),
                            ("of", id.clone()),
                            ("over", heads),
                            ("because", because),
                        ]),
                        strings(seen),
                        None,
                        empty(),
                        empty(),
                        &child,
                    )?;
                    current = Some(claim.clone());
                    heads = V::List(vec![id]);
                    generated.extend([claim, act]);
                }
                let mut ids = BTreeSet::new();
                for object in generated {
                    let id = text(&map(&object)?["id"])?.to_owned();
                    ids.insert(id.clone());
                    saw.push(id.clone());
                    objects.insert(id, object);
                }
                saw.sort();
                produced.insert(*index, strings(ids));
            }
            versions.insert(
                subject.clone(),
                map(current.as_ref().ok_or_else(|| error("invalid_review"))?)?["id"].clone(),
            );
            remaining.remove(&subject);
            progressed = true;
        }
        require(progressed, "cyclic_batch_pin_dependencies")?;
    }
    let mut selected = original.objects.clone();
    selected.extend(objects.clone());
    validate_closure(&selected)?;
    let files = evidence_files(options)?;
    final_world.attach_temporal(&mut after, &doc, &versions)?;
    let mut before_world =
        crate::history_authoring_reader::AuthoringReader::new(&before_doc, runtime)?;
    let mut before = before_world.evidence(&before_doc, audit, &A::accepted_versions(original)?)?;
    map_mut(&mut before)?.insert(
        "authoring".into(),
        intent(original, actions, options, &frozen, true),
    );
    for (index, witness) in witnesses.iter_mut().enumerate() {
        map_mut(witness)?.insert("objects".into(), produced[&index].clone());
    }
    map_mut(&mut after)?.insert(
        "authoring".into(),
        obj([
            ("steps", V::List(witnesses)),
            ("objects", strings(objects.keys().cloned())),
            ("validation_world", s("final")),
            ("final_dependency_pins", V::Bool(true)),
        ]),
    );
    let receipt = T::semantic_receipt(text(&map(&cap)?["profile"])?, &cap, &before, &after)?;
    let mutation = P::prepare_commit_with_files(
        original,
        &op.operation,
        &objects.values().cloned().collect::<Vec<_>>(),
        &A::template(original, &doc)?,
        &receipt,
        op.requires_for(&doc)?.as_ref(),
        &files,
    )?;
    let adapted = A::document(&A::candidate(original, &mutation)?)?;
    require(
        adapted.digest()? == doc.digest()?,
        "batch_final_projection_mismatch",
    )?;
    require(A::archive(store)? == frozen, "concurrent_archive_edit")?;
    crate::history_sources::capture(&store.root, &store.layout.entry, &original.document)?;
    Ok(mutation)
}
fn intent(
    original: &Capture,
    actions: &[V],
    options: &BatchOptions,
    archive: &V,
    evidence: bool,
) -> V {
    let mut value = obj([
        ("version", n(&options.receipt_version.to_string())),
        ("kind", s("batch")),
        ("actions", V::List(actions.to_vec())),
        ("by", options.authoring.by.clone()),
        ("recorded_at", s(&options.authoring.recorded_at)),
        ("archive", archive.clone()),
        ("baseline", original.baseline.clone()),
        (
            "context",
            if truth(&options.context) {
                options.context.clone()
            } else {
                empty()
            },
        ),
    ]);
    if evidence {
        map_mut(&mut value).unwrap().insert(
            "evidence".into(),
            V::Map(
                options
                    .evidence
                    .iter()
                    .map(|(p, raw)| (p.clone(), s(&sha256(raw))))
                    .collect(),
            ),
        );
    }
    value
}
fn sequential(
    store: &Store,
    original: &Capture,
    actions: &[V],
    options: &BatchOptions,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
) -> Result<PreparedMutation> {
    require(
        options.receipt_version == 3 || options.evidence.is_empty(),
        "invalid_authoring_receipt",
    )?;
    require(
        !original.commits.contains_key(&options.authoring.operation),
        "operation_already_prepared",
    )?;
    let frozen = A::archive(store)?;
    let mut virtual_capture = original.clone();
    let mut steps = vec![];
    let mut first = None;
    let mut last = None;
    for (index, action) in actions.iter().enumerate() {
        let mut child = options.authoring.clone();
        child.operation = format!(
            "batch-step-{}",
            obj([
                ("operation", s(&options.authoring.operation)),
                ("index", n(&index.to_string()))
            ])
            .digest()?
        );
        // Fresh sequential batches use the current direct receipt version. A
        // retained old batch can explicitly replay its v1 child semantics.
        child.receipt_version = options.authoring.receipt_version;
        let mutation = A::prepare_inner(store, &virtual_capture, action, &child, runtime, audit)?;
        let data = mutation.to_data();
        let receipt = map(&data)?["receipt"].clone();
        if first.is_none() {
            first = Some(receipt.clone());
        }
        last = Some(receipt.clone());
        steps.push(obj([
            ("operation", s(&child.operation)),
            (
                if options.receipt_version == 2 {
                    "receipt"
                } else {
                    "receipt_digest"
                },
                if options.receipt_version == 2 {
                    receipt.clone()
                } else {
                    map(&receipt)?["digest"].clone()
                },
            ),
        ]));
        virtual_capture = A::candidate(&virtual_capture, &mutation)?;
    }
    let first = first.unwrap();
    let last = last.unwrap();
    let first = map(&first)?;
    let last = map(&last)?;
    let mut before = first["before"].clone();
    map_mut(&mut before)?.insert(
        "authoring".into(),
        intent(
            original,
            actions,
            options,
            &frozen,
            options.receipt_version == 3,
        ),
    );
    let new = virtual_capture
        .objects
        .iter()
        .filter(|(id, _)| !original.objects.contains_key(*id))
        .map(|(_, o)| o.clone())
        .collect::<Vec<_>>();
    let mut after = last["after"].clone();
    map_mut(&mut after)?.insert(
        "authoring".into(),
        obj([
            ("steps", V::List(steps)),
            (
                "objects",
                strings(
                    new.iter()
                        .map(|o| text(&map(o).unwrap()["id"]).unwrap().to_owned()),
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
    let mutation = P::prepare_commit_with_files(
        original,
        &options.authoring.operation,
        &new,
        &crate::history_view::template(&virtual_capture.commits)?,
        &receipt,
        options
            .authoring
            .requires_for(field(map(&last["after"])?, "document")?)?
            .as_ref(),
        &evidence_files(options)?,
    )?;
    require(A::archive(store)? == frozen, "concurrent_archive_edit")?;
    Ok(mutation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as J;
    fn open_runtime(cache: &std::path::Path) -> Runtime {
        let archive = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!(
                "{}.kpopper-runtime",
                crate::reasoning_runtime::target_name().unwrap()
            ));
        Runtime::open(&archive, cache, OperationalBounds::default()).unwrap()
    }
    #[test]
    fn admission_compares_the_prior_formula_using_final_inputs() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = open_runtime(cache.path());
        let final_doc=V::from_json(&serde_json::json!({"meta":{"reasoning":{"version":2,"profile":"core/v1","requires":["arithmetic/v1"]}},"known":{"p.input":{"v":6},"p.ratio":{"rule":{"expr":"p.input / 2"}}}})).unwrap();
        let mut prior = final_doc.clone();
        map_mut(
            map_mut(map_mut(&mut prior).unwrap().get_mut("known").unwrap())
                .unwrap()
                .get_mut("p.ratio")
                .unwrap(),
        )
        .unwrap()
        .insert("rule".into(), obj([("expr", s("p.input / 3"))]));
        let mut world = World::batch_admission(
            &final_doc,
            &prior,
            "p.ratio",
            Some(&runtime),
            OperationalBounds::default(),
        )
        .unwrap();
        assert_eq!(
            world.result("p.ratio").unwrap()["value"],
            serde_json::json!({"type":"number","numerator":"2","denominator":"1"})
        );
        assert!(!world.same_value("p.ratio", &n("3")).unwrap());
    }
    #[test]
    fn native_batch_replay_preserves_final_pins_evidence_and_retry() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = open_runtime(cache.path());
        for (version, name, raw) in [
            (
                2,
                "2-set",
                include_str!("../tests/fixtures/history-authoring-batch.json"),
            ),
            (
                3,
                "3-set",
                include_str!("../tests/fixtures/history-authoring-batch.json"),
            ),
            (
                6,
                "6-forward-dependency",
                include_str!("../tests/fixtures/history-authoring-batch.json"),
            ),
            (
                8,
                "8-forward-dependency",
                include_str!("../tests/fixtures/history-authoring-batch-candidate.json"),
            ),
        ] {
            let data: J = serde_json::from_str(raw).unwrap();
            let case = data["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["name"] == name)
                .unwrap();
            let temp = tempfile::tempdir().unwrap();
            for (name, raw) in case["files"].as_object().unwrap() {
                let p = temp.path().join(name);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, raw.as_str().unwrap()).unwrap();
            }
            let store = Store::new(&temp.path().join("GROUNDING.yaml")).unwrap();
            let capture = store.capture().unwrap();
            let actions = V::from_tagged(&case["actions"]).unwrap();
            let mut options = BatchOptions {
                authoring: Options {
                    operation: "batch-1".into(),
                    recorded_at: "2026-09-19T09:30:00+00:00".into(),
                    recording_day: "2026-09-19".into(),
                    by: s("writer"),
                    strict: version >= 3,
                    paths: crate::history_paths::Scheme::Hashed,
                    receipt_version: None,
                },
                receipt_version: version,
                context: empty(),
                evidence: Default::default(),
            };
            if version >= 3 {
                options.evidence.insert(
                    ".kpopper/evidence/reports/batch-source.txt".into(),
                    b"source report".to_vec(),
                );
            }
            let mutation = prepare_batch(
                &store,
                &capture,
                list(&actions).unwrap(),
                &options,
                Some(&runtime),
            )
            .unwrap();
            A::verify_prepared(&store, &mutation, Some(&runtime)).unwrap();
            assert!(
                A::verify_prepared(&store, &mutation, None).is_err(),
                "replay skipped a fresh evaluator for {version}"
            );
            A::commit(&store, &mutation, Some(&runtime), &mut |_| Ok(())).unwrap();
            A::commit(&store, &mutation, Some(&runtime), &mut |_| Ok(())).unwrap();
            let after = store.capture().unwrap();
            assert_eq!(after.commits.len(), 2);
            for (key, raw) in &capture.object_bytes {
                assert_eq!(&after.object_bytes[key], raw);
            }
            if version >= 6 {
                let judgment = map(A::head(&after, "d.ready").unwrap()).unwrap();
                let input = map(A::head(&after, "p.new").unwrap()).unwrap();
                assert_eq!(map(&judgment["pins"]).unwrap()["p.new"], input["id"]);
            }
            if version >= 3 {
                assert_eq!(
                    std::fs::read(
                        store
                            .root
                            .join(".kpopper/evidence/reports/batch-source.txt")
                    )
                    .unwrap(),
                    b"source report"
                );
            }
        }
    }
    #[test]
    fn final_and_sequential_batches_match_exact_python_envelopes() {
        let archive = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!(
                "{}.kpopper-runtime",
                crate::reasoning_runtime::target_name().unwrap()
            ));
        let cache = tempfile::tempdir().unwrap();
        let runtime = Runtime::open(&archive, cache.path(), OperationalBounds::default()).unwrap();
        let mut failures = vec![];
        for raw in [
            include_str!("../tests/fixtures/history-authoring-batch.json"),
            include_str!("../tests/fixtures/history-authoring-batch-candidate.json"),
        ] {
            let data: J = serde_json::from_str(raw).unwrap();
            for case in data["cases"].as_array().unwrap() {
                let temp = tempfile::tempdir().unwrap();
                for (name, raw) in case["files"].as_object().unwrap() {
                    let path = temp.path().join(name);
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    std::fs::write(path, raw.as_str().unwrap()).unwrap();
                }
                let store = Store::new(&temp.path().join("GROUNDING.yaml")).unwrap();
                let capture = store.capture().unwrap();
                let actions = V::from_tagged(&case["actions"]).unwrap();
                let version = case["version"].as_u64().unwrap() as u8;
                let options = BatchOptions {
                    authoring: Options {
                        operation: "batch-1".into(),
                        recorded_at: "2026-09-19T09:30:00+00:00".into(),
                        recording_day: "2026-09-19".into(),
                        by: s("writer"),
                        strict: version >= 3,
                        paths: crate::history_paths::Scheme::Hashed,
                        receipt_version: Some(1),
                    },
                    receipt_version: version,
                    context: V::from_tagged(&case["context"]).unwrap(),
                    evidence: Default::default(),
                };
                let audit = case
                    .get("receipt")
                    .map(|v| ReplayAudit::oracle(&V::from_tagged(v).unwrap()).unwrap());
                let got = prepare_inner(
                    &store,
                    &capture,
                    list(&actions).unwrap(),
                    &options,
                    Some(&runtime),
                    audit.as_ref(),
                );
                if let Some(expected) = case.get("output") {
                    match got {
                        Ok(m) => {
                            if m.to_bytes().unwrap() != expected.as_str().unwrap().as_bytes() {
                                std::fs::write(
                                    temp.path().join("actual.json"),
                                    m.to_bytes().unwrap(),
                                )
                                .unwrap();
                                let path = temp.keep();
                                failures.push(format!(
                                    "{} mismatch {}",
                                    case["name"],
                                    path.display()
                                ));
                            }
                        }
                        Err(e) => failures.push(format!("{} refused {e}", case["name"])),
                    }
                } else if got.is_ok() {
                    failures.push(format!(
                        "{} accepted refusal: {}",
                        case["name"], case["refused"]
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}

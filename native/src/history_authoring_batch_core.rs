//! Provider-neutral final-world authoring for batches.
use crate::{
    Result, history_authoring as A,
    history_authoring_audit::ReplayAudit,
    history_authoring_core::Input,
    history_contract::*,
    history_transaction as T,
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

pub(crate) struct Plan {
    pub objects: Vec<V>,
    pub document: V,
    pub receipt: V,
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
pub(crate) fn prepare(
    input: &dyn Input,
    actions: &[V],
    options: &crate::history_authoring_batch::BatchOptions,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
    frozen: &V,
) -> Result<Plan> {
    require(!actions.is_empty() && actions.len() <= 64, "invalid_batch")?;
    require(
        [6, 8].contains(&options.receipt_version),
        "invalid_authoring_receipt",
    )?;
    let mut op = options.authoring.clone();
    op.strict = true;
    let before_doc = input.document()?;
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
    for state in map(&map(&input.state())?["subjects"])?.values() {
        if let Some(h) = map(state)?.get("head") {
            let o = input.object(text(h)?)?;
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
        if map(&map(&input.state())?["subjects"])?.contains_key(&step.subject) {
            input.head(&step.subject)?;
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
    let original_states = map(&map(&input.state())?["subjects"])?;
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
            let mut saw = input.saw(&subject)?;
            let mut current = if original_states.contains_key(&subject) {
                Some(input.head(&subject)?.clone())
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
    final_world.attach_temporal(&mut after, &doc, &versions)?;
    let mut before_world =
        crate::history_authoring_reader::AuthoringReader::new(&before_doc, runtime)?;
    let mut before = before_world.evidence(&before_doc, audit, &input.accepted_versions()?)?;
    map_mut(&mut before)?.insert(
        "authoring".into(),
        intent(input, actions, options, frozen, true),
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
    Ok(Plan {
        objects: objects.values().cloned().collect(),
        document: doc,
        receipt,
    })
}
fn intent(
    original: &dyn Input,
    actions: &[V],
    options: &crate::history_authoring_batch::BatchOptions,
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
        ("baseline", original.baseline().clone()),
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

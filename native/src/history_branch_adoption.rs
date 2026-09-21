//! Actual branch adoption preparation and deterministic retained replay.
use crate::{
    Result, history_adapter as D,
    history_authoring::{self as W, empty, n, obj, s, strings},
    history_authority as A,
    history_authority::{Files, ObjectBytes},
    history_branch::{self as B, Observation},
    history_branch_audit as Audit,
    history_capture::{self as H, Capture},
    history_contract::*,
    history_emit as E, history_hypotheses as HH, history_hypothesis_import_prepare as HI,
    history_preparation as P, history_reduce as R,
    history_transaction::{self as T, FileImage, PreparedMutation},
    history_view::{self as View, list, map_mut, truth},
    history_yaml as Y,
    identity::sha256,
    reasoning_capabilities as Cap,
    reasoning_snapshot::entries,
    require,
    value::TypedValue as V,
};
use std::collections::{BTreeMap, BTreeSet};

pub struct Options {
    pub operation: String,
    pub recorded_at: String,
    pub by: String,
}
fn data(observation: &Observation) -> Result<(&Map, &Map)> {
    let e = map(&observation.envelope)?;
    Ok((e, map(&e["manifest"])?))
}
fn current_raw(objects: &Map, raw: &ObjectBytes) -> ObjectBytes {
    raw.iter()
        .filter(|((_, id), _)| objects.contains_key(id))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}
fn merged(
    target: &Capture,
    sources: &[&Capture],
    observations: &Map,
) -> Result<(Map, ObjectBytes, Map)> {
    let actual = A::committed_objects(&target.marker, &target.commits, &target.object_bytes)?;
    require(
        actual == target.objects
            && R::reduce_bytes(
                &current_raw(&actual, &target.object_bytes),
                Some(map(&map(&target.state)?["rules"])?),
                None,
            )? == target.state,
        "invalid_target_capture",
    )?;
    let mut incoming = Map::new();
    let mut raw = target.object_bytes.clone();
    for source in sources {
        require(
            map(&source.state)?["rules"] == map(&target.state)?["rules"],
            "rules_mismatch",
        )?;
        for (id, value) in &source.objects {
            require(
                incoming.get(id).is_none_or(|old| old == value),
                "identity_mismatch",
            )?;
            incoming.insert(id.clone(), value.clone());
        }
        for (key, bytes) in &source.object_bytes {
            require(
                raw.get(key).is_none_or(|old| old == bytes),
                "object_bytes_mismatch",
            )?;
            raw.insert(key.clone(), bytes.clone());
        }
    }
    for (id, value) in observations {
        require(
            incoming.get(id).is_none_or(|old| old == value),
            "identity_mismatch",
        )?;
        incoming.insert(id.clone(), value.clone());
        let m = map(value)?;
        raw.insert(
            (text(&m["subject"])?.into(), id.clone()),
            E::encode_document(value)?,
        );
    }
    validate_closure(&incoming)?;
    let mut combined = target.objects.clone();
    for (id, value) in &incoming {
        require(
            combined.get(id).is_none_or(|old| old == value),
            "identity_mismatch",
        )?;
        combined.insert(id.clone(), value.clone());
    }
    Ok((combined, raw, incoming))
}
fn physical(observation: &Observation, source: &Capture) -> Result<Vec<HI::Source>> {
    let adapted = D::from_store_capture(source)?;
    let (immutable, _) = HH::layers(adapted.projection(), adapted.document())?;
    let (_, m) = data(observation)?;
    let entry = text(&map(&m["source"])?["entry"])?;
    let directory = B::hypothesis_directory(entry)?;
    let mut result = vec![];
    for (name, hypothesis) in map(&map(&m["snapshot"])?["hypotheses"])? {
        if map(&immutable)?.contains_key(name) {
            continue;
        }
        let matches = observation
            .files
            .iter()
            .filter(|(path, _)| B::hypothesis_file(path, &directory) == Some(name.as_str()))
            .collect::<Vec<_>>();
        require(matches.len() == 1, "ambiguous_branch_hypothesis")?;
        let (path, raw) = matches[0];
        let h = map(hypothesis)?;
        let parent = B::parts(entry)?.0;
        let path = if parent.is_empty() {
            path.clone()
        } else {
            path.strip_prefix(&format!("{parent}/"))
                .ok_or_else(|| error("invalid_path"))?
                .into()
        };
        result.push(HI::Source {
            name: name.clone(),
            document: field(h, "document")?.clone(),
            head: field(h, "head")?.clone(),
            path,
            bytes: raw.clone(),
            error: h.get("error").cloned(),
            profile: None,
            fields: None,
        });
    }
    Ok(result)
}
pub(crate) fn require_evidence(required: &V, evidence: &Files) -> Result<()> {
    let mut total = 0usize;
    for (relative, item) in map(required)? {
        let raw = evidence
            .get(relative)
            .ok_or_else(|| error("branch_target_evidence_missing"))?;
        total = total.saturating_add(raw.len());
        require(
            raw.len() <= crate::reasoning_snapshot::MAX_REQUEST_BYTES && total <= B::MAX_BYTES,
            "branch_target_evidence_limit",
        )?;
        require(
            string_is(field(map(item)?, "sha256")?, &sha256(raw)),
            "branch_target_evidence_conflicting",
        )?;
    }
    Ok(())
}
fn subject_claims(combined: &Map, subject: &str) -> Result<Vec<String>> {
    combined
        .iter()
        .filter_map(|(id, value)| match map(value) {
            Ok(m) if string_is(&m["subject"], subject) && !string_is(&m["kind"], "act") => {
                Some(Ok(id.clone()))
            }
            Ok(_) => None,
            Err(e) => Some(Err(e)),
        })
        .collect()
}
/// Inputs are complete frozen source observations and explicit target evidence.
/// No filesystem access or implicit choice of an overlapping claim occurs here.
pub fn prepare(
    target: &Capture,
    observations: &[Observation],
    choices: &V,
    options: &Options,
    evidence: &Files,
) -> Result<PreparedMutation> {
    let requires = strings([crate::history_paths::CAPABILITY.into()]);
    prepare_with_requires(
        target,
        observations,
        choices,
        options,
        evidence,
        Some(&requires),
    )
}
fn prepare_with_requires(
    target: &Capture,
    observations: &[Observation],
    choices: &V,
    options: &Options,
    evidence: &Files,
    requires: Option<&V>,
) -> Result<PreparedMutation> {
    require(!observations.is_empty(), "invalid_branch_source_set")?;
    let (ordered, revision) = if observations.len() == 1 {
        let observation = &observations[0];
        let source = B::validate(&observation.envelope, &observation.files)?;
        (
            vec![(observation, source)],
            text(&map(&observation.envelope)?["revision"])?.to_owned(),
        )
    } else {
        Audit::source_set(observations)?
    };
    require(
        View::render(
            target,
            &target.objects,
            &target.object_bytes,
            &target.commits,
        )? == target.entry_bytes,
        "unresolved_view_edit",
    )?;
    token(&s(&options.operation))?;
    require(
        !options.by.is_empty() && !options.recorded_at.is_empty(),
        "invalid_adopter",
    )?;
    require(
        !target.commits.contains_key(&options.operation),
        "operation_already_prepared",
    )?;
    let choices = map(choices).map_err(|_| error("invalid_adoption_choices"))?;
    let sources = ordered.iter().map(|(_, c)| c).collect::<Vec<_>>();
    let (original_combined, _, _) = merged(target, &sources, &Map::new())?;
    let target_subjects = map(&map(&target.state)?["subjects"])?;
    let mut count = BTreeMap::<String, usize>::new();
    let mut required_hashes = Map::new();
    for (observation, source) in &ordered {
        let required = Audit::evidence_requirements(observation, source)?;
        for (path, item) in map(&required)? {
            let digest = field(map(item)?, "sha256")?;
            require(
                required_hashes.get(path).is_none_or(|v| v == digest),
                "branch_evidence_sources_conflict",
            )?;
            required_hashes.insert(path.clone(), digest.clone());
        }
    }
    let mut imports = Map::new();
    let mut bindings = vec![];
    let mut audits = vec![];
    let mut audit_total = 0usize;
    for (observation, source) in &ordered {
        let physical = physical(observation, source)?;
        let mut subjects = map(&map(&source.state)?["subjects"])?
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        for p in &physical {
            subjects.extend(entries(&p.document)?.keys().cloned());
        }
        for subject in subjects {
            *count.entry(subject).or_default() += 1;
        }
        let required = Audit::evidence_requirements(observation, source)?;
        require_evidence(&required, evidence)?;
        let (_, manifest) = data(observation)?;
        let source_entry = text(&map(&manifest["source"])?["entry"])?;
        let imported = if physical.is_empty() {
            obj([
                ("objects", empty()),
                (
                    "physical",
                    obj([("version", n("1")), ("physical", V::List(vec![]))]),
                ),
            ])
        } else {
            HI::prepare(
                &source.objects,
                &physical,
                D::from_store_capture(source)?.document(),
                B::parts(source_entry)?.1,
                &options.operation,
                &options.recorded_at,
            )?
        };
        for (id, value) in map(&map(&imported)?["objects"])? {
            require(
                imports.get(id).is_none_or(|old| old == value),
                "identity_mismatch",
            )?;
            imports.insert(id.clone(), value.clone());
        }
        let raw = observation.to_bytes()?;
        audit_total = audit_total.saturating_add(raw.len());
        require(audit_total <= B::MAX_BYTES, "branch_capture_limit")?;
        let e = map(&observation.envelope)?;
        let path = Audit::audit_path(&target.layout.entry, text(&e["revision"])?)?;
        let mut binding = Audit::binding(observation, source)?;
        map_mut(&mut binding)?.extend([
            (
                "audit".into(),
                obj([
                    ("path", s(&path)),
                    ("sha256", s(&sha256(&raw))),
                    ("revision", e["revision"].clone()),
                ]),
            ),
            ("required_evidence".into(), required),
            (
                "physical_observation".into(),
                obj([
                    ("operation", s(&options.operation)),
                    ("recorded_at", s(&options.recorded_at)),
                    ("original_writer", V::Null),
                    ("original_operation", V::Null),
                    ("original_recorded_at", V::Null),
                    ("files", map(&imported)?["physical"].clone()),
                ]),
            ),
        ]);
        bindings.push(binding);
        audits.push(FileImage {
            path,
            role: "history_evidence".into(),
            before: None,
            after: Some(raw),
        });
    }
    for (subject, chosen) in choices {
        require(
            count.contains_key(subject)
                && subject_claims(&original_combined, subject)?
                    .iter()
                    .any(|id| string_is(chosen, id)),
            "invalid_adoption_choice",
        )?;
    }
    for (subject, n) in &count {
        require(
            (*n == 1 && !target_subjects.contains_key(subject)) || choices.contains_key(subject),
            "adoption_choice_required",
        )?;
    }
    let (mut combined, mut raw, incoming) = merged(target, &sources, &imports)?;
    let rules = map(&map(&target.state)?["rules"])?;
    let before = R::reduce_bytes(&current_raw(&combined, &raw), Some(rules), None)?;
    let mut resolutions = Map::new();
    let act_options = W::Options {
        operation: options.operation.clone(),
        recorded_at: options.recorded_at.clone(),
        recording_day: options.recorded_at.clone(),
        by: s(&options.by),
        strict: false,
        paths: crate::history_paths::Scheme::Hashed,
        receipt_version: None,
    };
    for (subject, chosen) in choices {
        let st = map(&map(&map(&before)?["subjects"])?[subject])?;
        let competing = list(&st["heads"])?
            .iter()
            .chain(list(&st["disputed_acts"])?)
            .filter(|v| *v != chosen)
            .map(|v| text(v).map(str::to_owned))
            .collect::<Result<BTreeSet<_>>>()?;
        let saw = combined
            .iter()
            .filter(|(_, v)| map(v).is_ok_and(|m| string_is(&m["subject"], subject)))
            .map(|(id, _)| id.clone());
        let act = W::make_object(
            subject,
            "act",
            obj([
                ("act", s("accept")),
                ("of", chosen.clone()),
                ("over", strings(competing)),
                (
                    "because",
                    s(&format!("explicit branch adoption of {revision}")),
                ),
            ]),
            strings(saw),
            None,
            empty(),
            empty(),
            &act_options,
        )?;
        let id = text(&map(&act)?["id"])?.to_owned();
        raw.insert((subject.clone(), id.clone()), E::encode_document(&act)?);
        combined.insert(id.clone(), act.clone());
        resolutions.insert(id, act);
    }
    let state = R::reduce_bytes(&current_raw(&combined, &raw), Some(rules), None)?;
    for (subject, chosen) in choices {
        let actual = map(&map(&map(&state)?["subjects"])?[subject])?;
        let heads = list(&actual["heads"])?;
        let meaning = R::claim_meaning(map(&combined[text(chosen)?])?)?;
        require(
            string_is(&actual["acceptance"], "accepted") && heads.contains(chosen),
            "unresolved_adoption_choice",
        )?;
        for head in heads {
            require(
                R::claim_meaning(map(&combined[text(head)?])?)? == meaning,
                "unresolved_adoption_choice",
            )?;
        }
    }
    let mut template = View::template(&target.commits)?;
    require(
        !map(&template)?
            .get("meta")
            .is_some_and(|v| map(v).is_ok_and(|m| m.contains_key("history_subset"))),
        "cannot_adopt_into_transport_authority",
    )?;
    let meta_reasoning = |v: &V| {
        map(v)
            .ok()
            .and_then(|m| m.get("meta"))
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get("reasoning"))
            .cloned()
            .unwrap_or(V::Null)
    };
    for source in &sources {
        let t = View::template(&source.commits)?;
        require(
            map(&template)?.get("schema").cloned().unwrap_or_else(empty)
                == map(&t)?.get("schema").cloned().unwrap_or_else(empty)
                && meta_reasoning(&template) == meta_reasoning(&t),
            "adoption_profile_mismatch",
        )?;
    }
    let mut inventory = Map::new();
    for (id, value) in &incoming {
        let m = map(value)?;
        let subject = text(&m["subject"])?;
        if !string_is(&m["kind"], "act") {
            map_mut(&mut template)?
                .entry(text(&map(&m["authored"])?["collection"])?.into())
                .or_insert_with(empty);
        }
        inventory.insert(
            id.clone(),
            obj([
                ("subject", s(subject)),
                ("sha256", s(&sha256(&raw[&(subject.into(), id.clone())]))),
            ]),
        );
    }
    let mut adoption = obj([
        (
            "version",
            n(if observations.len() == 1 { "1" } else { "2" }),
        ),
        ("objects", V::Map(inventory)),
        ("choices", V::Map(choices.clone())),
        ("by", s(&options.by)),
        ("recorded_at", s(&options.recorded_at)),
        ("resolutions", strings(resolutions.keys().cloned())),
        ("observed_proposals", strings(imports.keys().cloned())),
    ]);
    if observations.len() == 1 {
        map_mut(&mut adoption)?.extend([
            ("source_revision".into(), s(&revision)),
            ("source".into(), bindings.remove(0)),
        ]);
    } else {
        map_mut(&mut adoption)?.extend([
            ("source_set_revision".into(), s(&revision)),
            (
                "source_revisions".into(),
                V::List(
                    ordered
                        .iter()
                        .map(|(o, _)| map(&o.envelope).unwrap()["revision"].clone())
                        .collect(),
                ),
            ),
            ("sources".into(), V::List(bindings)),
        ]);
    }
    let mut cap_doc = template.clone();
    if let Some(meta) = map_mut(&mut cap_doc)?.get_mut("meta") {
        map_mut(meta)?.remove("history");
    }
    let cap = Cap::document_capabilities(&cap_doc)?;
    let receipt = T::semantic_receipt(
        text(&map(&cap)?["profile"])?,
        &cap,
        &obj([
            ("baseline", target.baseline.clone()),
            (
                "authoring",
                obj([(
                    "evidence",
                    V::Map(
                        audits
                            .iter()
                            .map(|f| (f.path.clone(), s(&sha256(f.after.as_ref().unwrap()))))
                            .collect(),
                    ),
                )]),
            ),
        ]),
        &obj([("history_branch_adoption", adoption)]),
    )?;
    let mut adopted = incoming;
    adopted.extend(resolutions);
    let pairs = adopted
        .into_iter()
        .map(|(id, v)| {
            let key = (text(&map(&v)?["subject"])?.to_owned(), id);
            Ok((v, raw[&key].clone()))
        })
        .collect::<Result<Vec<_>>>()?;
    let mutation = P::prepare_commit_pairs(
        target,
        &options.operation,
        pairs,
        &template,
        &receipt,
        requires,
        &audits,
    )?;
    // Prospective rendering is verified independently without accepting proposals.
    let after = crate::history_prospective::capture_after(target, &mutation)?;
    let adapted = D::from_store_capture(&after)?;
    let (groups, _) = HH::layers(adapted.projection(), adapted.document())?;
    require(
        map(&groups)?
            .values()
            .all(|g| map(g).is_ok_and(|m| !m.get("error").is_some_and(truth))),
        "branch_hypothesis_conflict",
    )?;
    Ok(mutation)
}

/// Reconstruct the exact causal parent capture. Incoming commits and siblings
/// cannot supply an authoring witness or silently change the original baseline.
pub fn verify(captured: &Capture, mutation: &PreparedMutation, evidence: &Files) -> Result<()> {
    let data = mutation.to_data();
    let data = map(&data)?;
    let observations = Audit::audit_evidences(&mutation.to_data())?;
    let manifest = mutation
        .files()
        .iter()
        .find(|f| f.role == "history_commit")
        .and_then(|f| f.after.as_deref())
        .ok_or_else(|| error("invalid_mutation"))?;
    let manifest = Y::decode_document(manifest)?;
    A::validate_commit(&manifest)?;
    let manifest = map(&manifest)?;
    require(data["authority"] == captured.marker, "authority_mismatch")?;
    let mut commits = Files::new();
    let mut todo = map(&manifest["parents"])?
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let mut visits = 0usize;
    while let Some(operation) = todo.pop() {
        visits += 1;
        require(visits <= 100_000, "history_limit")?;
        if commits.contains_key(&operation) {
            continue;
        }
        let raw = captured
            .commits
            .get(&operation)
            .ok_or_else(|| error("incomplete_commit"))?;
        let parent = Y::decode_document(raw)?;
        A::validate_commit(&parent)?;
        todo.extend(map(&map(&parent)?["parents"])?.keys().cloned());
        commits.insert(operation, raw.clone());
    }
    let objects = A::committed_objects(&captured.marker, &commits, &captured.object_bytes)?;
    let mut before = captured.clone();
    before.commits = commits;
    before.objects = objects;
    before.object_bytes = current_raw(&before.objects, &captured.object_bytes);
    before.state = R::reduce_bytes(
        &before.object_bytes,
        Some(map(&map(&captured.state)?["rules"])?),
        None,
    )?;
    before.baseline = H::baseline(&before.marker, &before.commits, &before.state)?;
    before.entry_bytes = mutation
        .files()
        .iter()
        .find(|f| f.role == "record")
        .and_then(|f| f.before.clone())
        .ok_or_else(|| error("invalid_mutation"))?;
    before.document = Y::decode_document(&before.entry_bytes)?;
    require(before.baseline == data["baseline"], "baseline_mismatch")?;
    let adoption = map(field(
        map(&map(&data["receipt"])?["after"])?,
        "history_branch_adoption",
    )?)?;
    let options = Options {
        operation: text(&data["operation"])?.into(),
        by: text(field(adoption, "by")?)?.into(),
        recorded_at: text(field(adoption, "recorded_at")?)?.into(),
    };
    let expected = prepare_with_requires(
        &before,
        &observations,
        field(adoption, "choices")?,
        &options,
        evidence,
        manifest.get("requires"),
    )?;
    require(
        expected.to_bytes()? == mutation.to_bytes()?,
        "branch_adoption_mutation_mismatch",
    )
}

pub(crate) fn live_evidence(
    store: &crate::history_store::Store,
    mutation: &PreparedMutation,
) -> Result<Files> {
    let mut files = Files::new();
    let mut total = 0usize;
    for observation in Audit::audit_evidences(&mutation.to_data())? {
        let source = B::validate(&observation.envelope, &observation.files)?;
        let required = Audit::evidence_requirements(&observation, &source)?;
        for relative in map(&required)?.keys() {
            if files.contains_key(relative) {
                continue;
            }
            let path = crate::history_transaction_fs::target(&store.root, relative)?;
            let metadata = std::fs::metadata(&path)?;
            require(
                metadata.is_file()
                    && metadata.len() <= crate::reasoning_snapshot::MAX_REQUEST_BYTES as u64,
                "branch_target_evidence_limit",
            )?;
            let raw = crate::history_transaction_fs::read(&path)?
                .ok_or_else(|| error("branch_target_evidence_missing"))?;
            total = total.saturating_add(raw.len());
            require(
                raw.len() <= crate::reasoning_snapshot::MAX_REQUEST_BYTES && total <= B::MAX_BYTES,
                "branch_target_evidence_limit",
            )?;
            files.insert(relative.clone(), raw);
        }
        require_evidence(&required, &files)?;
    }
    Ok(files)
}
pub fn commit(
    store: &crate::history_store::Store,
    mutation: &PreparedMutation,
    verify_callback: crate::history_transaction_fs::Verify<'_>,
) -> Result<V> {
    store.commit_branch(mutation, verify_callback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde_json::Value as J;
    fn files(value: &J) -> Files {
        value
            .as_object()
            .unwrap()
            .iter()
            .map(|(p, r)| (p.clone(), STANDARD.decode(r.as_str().unwrap()).unwrap()))
            .collect()
    }
    fn inputs(case: &J) -> (Capture, Vec<Observation>, V, Options, Files) {
        let mut target = crate::history_bundle::capture(&files(&case["target"]), None).unwrap();
        target.layout = H::Layout::for_entry(case["entry"].as_str().unwrap()).unwrap();
        let sources = case["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| Observation {
                envelope: V::from_tagged(&o["envelope"]).unwrap(),
                files: files(&o["files"]),
            })
            .collect();
        let options = V::from_tagged(&case["options"]).unwrap();
        let o = map(&options).unwrap();
        let settings = Options {
            operation: text(&o["operation"]).unwrap().into(),
            by: text(&o["by"]).unwrap().into(),
            recorded_at: text(&o["recorded_at"]).unwrap().into(),
        };
        (
            target,
            sources,
            o["choices"].clone(),
            settings,
            files(&case["evidence"]),
        )
    }
    fn cases() -> J {
        serde_json::from_str(include_str!(
            "../tests/fixtures/history-branch-adoption.json"
        ))
        .unwrap()
    }
    fn same(path: &str, a: &V, b: &V) {
        match (a, b) {
            (V::Map(a), V::Map(b)) => {
                assert_eq!(
                    a.keys().collect::<Vec<_>>(),
                    b.keys().collect::<Vec<_>>(),
                    "{path}"
                );
                for (k, v) in a {
                    same(&format!("{path}/{k}"), v, &b[k]);
                }
            }
            (V::List(a), V::List(b)) => {
                assert_eq!(a.len(), b.len(), "{path}");
                for (i, (a, b)) in a.iter().zip(b).enumerate() {
                    same(&format!("{path}/{i}"), a, b);
                }
            }
            _ => assert_eq!(a, b, "{path}"),
        }
    }
    #[test]
    fn single_and_atomic_set_adoption_match_exact_python_envelopes_and_replay() {
        for case in cases().as_array().unwrap() {
            let (target, sources, choices, options, evidence) = inputs(case);
            let result = prepare(&target, &sources, &choices, &options, &evidence);
            if let Some(expected) = case.get("output") {
                let result = result.unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
                same(
                    case["name"].as_str().unwrap(),
                    &result.to_data(),
                    &V::from_tagged(expected).unwrap(),
                );
                verify(&target, &result, &evidence)
                    .unwrap_or_else(|e| panic!("replay {}: {e}", case["name"]));
                let after = crate::history_prospective::capture_after(&target, &result).unwrap();
                verify(&after, &result, &evidence)
                    .unwrap_or_else(|e| panic!("committed replay {}: {e}", case["name"]));
            } else {
                assert!(
                    result.is_err(),
                    "accepted {} expected {}",
                    case["name"],
                    case["error"]
                );
            }
        }
    }
    fn install(
        target: &Capture,
        evidence: &Files,
        root: &std::path::Path,
    ) -> crate::history_store::Store {
        let l = &target.layout;
        let mut all = Files::from([
            (l.entry.clone(), target.entry_bytes.clone()),
            (l.authority.clone(), target.authority_bytes.clone()),
        ]);
        for (op, raw) in target.commits.iter().chain(
            target
                .inactive_generations
                .values()
                .flat_map(|g| g.commits.iter()),
        ) {
            all.insert(format!("{}/{op}.yaml", l.commits), raw.clone());
        }
        for (key, raw) in &target.object_bytes {
            all.insert(
                format!("{}/{}", l.objects, target.object_paths[key]),
                raw.clone(),
            );
        }
        for (name, raw) in &target.cancellation_bytes {
            all.insert(format!("{}/{name}", l.cancellations), raw.clone());
        }
        all.extend(evidence.clone());
        for (path, raw) in all {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, raw).unwrap();
        }
        crate::history_store::Store::new(&root.join(&l.entry)).unwrap()
    }
    #[test]
    fn store_enforces_replay_and_rechecks_actual_evidence_after_callback() {
        let cases = cases();
        let case = cases
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c.get("output").is_some() && !c["evidence"].as_object().unwrap().is_empty())
            .unwrap();
        let (target, sources, choices, options, evidence) = inputs(case);
        let mutation = prepare(&target, &sources, &choices, &options, &evidence).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = install(&target, &evidence, dir.path());
        assert!(store.commit(&mutation, &mut |_| Ok(())).is_err());
        let relative = evidence.keys().next().unwrap();
        let path = dir.path().join(relative);
        let result = commit(&store, &mutation, &mut |_| {
            std::fs::write(&path, b"late change")?;
            Ok(())
        });
        assert!(result.is_err());
        assert!(
            !store
                .capture()
                .unwrap()
                .commits
                .contains_key(&options.operation)
        );
        std::fs::write(&path, &evidence[relative]).unwrap();
        commit(&store, &mutation, &mut |_| Ok(())).unwrap();
        commit(&store, &mutation, &mut |_| Ok(())).unwrap();
        assert!(
            store
                .capture()
                .unwrap()
                .commits
                .contains_key(&options.operation)
        );
    }

    #[test]
    fn rehashed_intent_forgery_cannot_pass_a_noop_store_verifier() {
        let all = cases();
        let case = all
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c.get("output").is_some() && !c["multiple"].as_bool().unwrap())
            .unwrap();
        let (target, sources, choices, options, evidence) = inputs(case);
        let mutation = prepare(&target, &sources, &choices, &options, &evidence).unwrap();
        let data = mutation.to_data();
        let receipt = map(&map(&data).unwrap()["receipt"]).unwrap();
        let mut after = receipt["after"].clone();
        map_mut(
            map_mut(&mut after)
                .unwrap()
                .get_mut("history_branch_adoption")
                .unwrap(),
        )
        .unwrap()
        .insert("by".into(), s("forged adopter"));
        let forged_receipt = T::semantic_receipt(
            text(&receipt["profile"]).unwrap(),
            &receipt["capabilities"],
            &receipt["before"],
            &after,
        )
        .unwrap();
        let manifest = Y::decode_document(
            mutation
                .files()
                .iter()
                .find(|f| f.role == "history_commit")
                .unwrap()
                .after
                .as_ref()
                .unwrap(),
        )
        .unwrap();
        let manifest = map(&manifest).unwrap();
        let pairs = mutation
            .files()
            .iter()
            .filter(|f| f.role == "history_object")
            .map(|f| {
                let raw = f.after.clone().unwrap();
                (Y::decode_document(&raw).unwrap(), raw)
            })
            .collect();
        let audits = mutation
            .files()
            .iter()
            .filter(|f| f.role == "history_evidence")
            .cloned()
            .collect::<Vec<_>>();
        let forged = P::prepare_commit_pairs(
            &target,
            &options.operation,
            pairs,
            &manifest["view_template"],
            &forged_receipt,
            manifest.get("requires"),
            &audits,
        )
        .unwrap();
        Audit::audit_evidences(&forged.to_data()).unwrap();
        assert!(verify(&target, &forged, &evidence).is_err());
        let dir = tempfile::tempdir().unwrap();
        let store = install(&target, &evidence, dir.path());
        assert!(commit(&store, &forged, &mut |_| Ok(())).is_err());
        assert!(
            !store
                .capture()
                .unwrap()
                .commits
                .contains_key(&options.operation)
        );
    }
    #[test]
    fn partial_publication_retries_retain_original_bytes_and_finish_one_commit() {
        let all = cases();
        let case = all
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c.get("output").is_some() && c["multiple"].as_bool().unwrap())
            .unwrap();
        let (target, sources, choices, options, evidence) = inputs(case);
        let mutation = prepare(&target, &sources, &choices, &options, &evidence).unwrap();
        for phase in ["objects", "capsules", "manifest"] {
            let dir = tempfile::tempdir().unwrap();
            let store = install(&target, &evidence, dir.path());
            for f in mutation.files() {
                if f.role == "history_object"
                    || phase != "objects" && f.role == "history_evidence"
                    || phase == "manifest" && f.role == "history_commit"
                {
                    let path = dir.path().join(&f.path);
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    std::fs::write(path, f.after.as_ref().unwrap()).unwrap();
                }
            }
            commit(&store, &mutation, &mut |_| Ok(())).unwrap_or_else(|e| panic!("{phase}: {e}"));
            let captured = store.capture().unwrap();
            assert!(captured.commits.contains_key(&options.operation));
            for (key, raw) in &target.object_bytes {
                assert_eq!(captured.object_bytes.get(key), Some(raw));
            }
            assert_eq!(
                captured.entry_bytes,
                mutation
                    .files()
                    .iter()
                    .find(|f| f.role == "record")
                    .unwrap()
                    .after
                    .clone()
                    .unwrap()
            );
            commit(&store, &mutation, &mut |_| Ok(())).unwrap();
        }
    }
}

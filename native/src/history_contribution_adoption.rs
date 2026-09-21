//! Scoped contribution adoption preserves the exact imported claims and records
//! only the adopter's explicit choices. Artifact replay never reads a source.
use crate::{
    Result, history_adapter,
    history_authoring::{self as W, empty, n, obj, s, strings},
    history_authority::{self as A, Files, ObjectBytes},
    history_bundle as B,
    history_capture::{self as H, Capture},
    history_contract::*,
    history_emit as E, history_preparation as P, history_reduce as R,
    history_store::Store,
    history_transaction::{self as T, PreparedMutation},
    history_transaction_fs as F,
    history_view::{self as View, list, map_mut},
    history_yaml as Y,
    identity::sha256,
    reasoning_capabilities as Cap, require,
    value::TypedValue as V,
};
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub struct Artifact {
    pub manifest: V,
    pub revision: V,
    pub files: Files,
}
pub struct Options {
    pub operation: String,
    pub recorded_at: String,
    pub by: String,
}

impl Artifact {
    pub fn from_contribution(bundle: &crate::pending_state::Bundle) -> Result<Self> {
        crate::pending_bundle::validate(&bundle.value, &bundle.files)?;
        let manifest = map(field(map(&bundle.value)?, "manifest")?)?;
        require(
            is_int(field(manifest, "version")?, "3"),
            "invalid_history_contribution",
        )?;
        let binding = map(field(manifest, "history")?)?;
        let artifact = Self {
            manifest: field(binding, "manifest")?.clone(),
            revision: field(binding, "revision")?.clone(),
            files: map(field(map(field(binding, "manifest")?)?, "files")?)?
                .keys()
                .filter_map(|p| {
                    bundle
                        .files
                        .get(&format!("history-closure/{p}"))
                        .map(|b| (p.clone(), b.clone()))
                })
                .collect(),
        };
        artifact.capture()?;
        Ok(artifact)
    }
    pub fn capture(&self) -> Result<Capture> {
        B::validate_artifact(&self.manifest, &self.revision, &self.files)
    }
    pub(crate) fn retained(&self) -> V {
        obj([
            ("manifest", self.manifest.clone()),
            ("revision", self.revision.clone()),
            (
                "files",
                V::Map(
                    self.files
                        .iter()
                        .map(|(p, b)| (p.clone(), T::blob(Some(b))))
                        .collect(),
                ),
            ),
        ])
    }
    pub(crate) fn decode(value: &V) -> Result<Self> {
        let m = schema(value, &["manifest", "revision", "files"], &[])?;
        let result = Self {
            manifest: m["manifest"].clone(),
            revision: m["revision"].clone(),
            files: map(&m["files"])?
                .iter()
                .map(|(p, v)| {
                    Ok((
                        p.clone(),
                        T::unblob(v)?.ok_or_else(|| error("invalid_history_journal"))?,
                    ))
                })
                .collect::<Result<_>>()?,
        };
        result.capture()?;
        Ok(result)
    }
}
fn raw_for(objects: &Map, raw: &ObjectBytes) -> ObjectBytes {
    raw.iter()
        .filter(|((_, id), _)| objects.contains_key(id))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}
fn combine(target: &Capture, source: &Capture) -> Result<(Map, ObjectBytes)> {
    let actual = A::committed_objects(&target.marker, &target.commits, &target.object_bytes)?;
    require(
        actual == target.objects
            && R::reduce_bytes(
                &raw_for(&actual, &target.object_bytes),
                Some(map(&map(&target.state)?["rules"])?),
                None,
            )? == target.state,
        "invalid_target_capture",
    )?;
    validate_closure(&source.objects)?;
    let mut combined = target.objects.clone();
    let mut raw = target.object_bytes.clone();
    for (id, value) in &source.objects {
        require(
            combined.get(id).is_none_or(|old| old == value),
            "identity_mismatch",
        )?;
        combined.insert(id.clone(), value.clone());
    }
    for (key, value) in &source.object_bytes {
        require(
            raw.get(key).is_none_or(|old| old == value),
            "object_bytes_mismatch",
        )?;
        raw.insert(key.clone(), value.clone());
    }
    Ok((combined, raw))
}
fn source(target: &Capture, artifact: &Artifact) -> Result<Capture> {
    let source = artifact.capture()?;
    require(
        is_int(field(map(&artifact.manifest)?, "version")?, "2"),
        "subset_adoption_required",
    )?;
    require(
        map(&target.marker)?["record_id"] != map(&source.marker)?["record_id"],
        "independent_adoption_required",
    )?;
    Ok(source)
}
pub fn preview(target: &Capture, artifact: &Artifact) -> Result<V> {
    let source = source(target, artifact)?;
    let (combined, raw) = combine(target, &source)?;
    let state = R::reduce_bytes(
        &raw_for(&combined, &raw),
        Some(map(&map(&target.state)?["rules"])?),
        None,
    )?;
    let targets = map(&map(&target.state)?["subjects"])?;
    let mut subjects = Map::new();
    for (subject, incoming) in map(&map(&source.state)?["subjects"])? {
        subjects.insert(
            subject.clone(),
            obj([
                ("requires_choice", V::Bool(targets.contains_key(subject))),
                (
                    "target_heads",
                    targets
                        .get(subject)
                        .map(map)
                        .transpose()?
                        .and_then(|m| m.get("heads"))
                        .cloned()
                        .unwrap_or(V::List(vec![])),
                ),
                ("incoming_heads", map(incoming)?["heads"].clone()),
                (
                    "combined_heads",
                    map(&map(&map(&state)?["subjects"])?[subject])?["heads"].clone(),
                ),
                (
                    "claims",
                    strings(
                        combined
                            .iter()
                            .filter(|(_, v)| {
                                map(v).is_ok_and(|m| {
                                    string_is(&m["subject"], subject)
                                        && !string_is(&m["kind"], "act")
                                })
                            })
                            .map(|(id, _)| id.clone()),
                    ),
                ),
            ]),
        );
    }
    Ok(obj([
        ("artifact_revision", artifact.revision.clone()),
        ("target_authority", target.marker.clone()),
        ("target_baseline", target.baseline.clone()),
        ("subjects", V::Map(subjects)),
    ]))
}
pub fn prepare(
    target: &Capture,
    artifact: &Artifact,
    choices: &V,
    options: &Options,
) -> Result<PreparedMutation> {
    prepare_requires(
        target,
        artifact,
        choices,
        options,
        Some(&strings([crate::history_paths::CAPABILITY.into()])),
    )
}
fn prepare_requires(
    target: &Capture,
    artifact: &Artifact,
    choices: &V,
    options: &Options,
    requires: Option<&V>,
) -> Result<PreparedMutation> {
    let source = source(target, artifact)?;
    let preview = preview(target, artifact)?;
    token(&s(&options.operation))?;
    require(
        !target.commits.contains_key(&options.operation),
        "operation_already_prepared",
    )?;
    require(
        !options.by.is_empty() && !options.recorded_at.is_empty(),
        "invalid_adopter",
    )?;
    let choices = map(choices).map_err(|_| error("invalid_adoption_choices"))?;
    let subjects = map(&map(&preview)?["subjects"])?;
    require(
        subjects.iter().all(|(name, v)| {
            map(v).unwrap()["requires_choice"] != V::Bool(true) || choices.contains_key(name)
        }) && choices.keys().all(|name| subjects.contains_key(name)),
        "adoption_choice_required",
    )?;
    require(
        map(&source.state)?["rules"] == map(&target.state)?["rules"],
        "rules_mismatch",
    )?;
    let (mut combined, mut raw) = combine(target, &source)?;
    let rules = map(&map(&target.state)?["rules"])?;
    let before = R::reduce_bytes(&raw_for(&combined, &raw), Some(rules), None)?;
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
        require(
            list(&map(&subjects[subject])?["claims"])?.contains(chosen),
            "invalid_adoption_choice",
        )?;
        let state = map(&map(&map(&before)?["subjects"])?[subject])?;
        let competing = list(&state["heads"])?
            .iter()
            .chain(list(&state["disputed_acts"])?)
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
                    s(&format!(
                        "explicit adoption of {}",
                        text(&artifact.revision)?
                    )),
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
    let state = R::reduce_bytes(&raw_for(&combined, &raw), Some(rules), None)?;
    for (subject, chosen) in choices {
        let state = map(&map(&map(&state)?["subjects"])?[subject])?;
        let heads = list(&state["heads"])?;
        let meaning = R::claim_meaning(map(&combined[text(chosen)?])?)?;
        require(
            string_is(&state["acceptance"], "accepted") && heads.contains(chosen),
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
    let incoming_template = View::template(&source.commits)?;
    let reasoning = |v: &V| {
        map(v)
            .ok()
            .and_then(|m| m.get("meta"))
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get("reasoning"))
            .cloned()
            .unwrap_or(V::Null)
    };
    require(
        map(&template)?.get("schema").cloned().unwrap_or_else(empty)
            == map(&incoming_template)?
                .get("schema")
                .cloned()
                .unwrap_or_else(empty)
            && reasoning(&template) == reasoning(&incoming_template),
        "adoption_profile_mismatch",
    )?;
    let mut inventory = Map::new();
    for (id, value) in &source.objects {
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
    let adoption = obj([
        ("version", n("1")),
        ("artifact_revision", artifact.revision.clone()),
        ("objects", V::Map(inventory)),
        ("choices", V::Map(choices.clone())),
        ("by", s(&options.by)),
        ("recorded_at", s(&options.recorded_at)),
        ("resolutions", strings(resolutions.keys().cloned())),
    ]);
    let mut cap_doc = template.clone();
    if let Some(meta) = map_mut(&mut cap_doc)?.get_mut("meta") {
        map_mut(meta)?.remove("history");
    }
    let cap = Cap::document_capabilities(&cap_doc)?;
    let receipt = T::semantic_receipt(
        text(&map(&cap)?["profile"])?,
        &cap,
        &obj([("baseline", target.baseline.clone())]),
        &obj([("history_adoption", adoption)]),
    )?;
    let mut adopted = source.objects;
    adopted.extend(resolutions);
    let pairs = adopted
        .into_iter()
        .map(|(id, value)| {
            let key = (text(&map(&value)?["subject"])?.into(), id);
            Ok((value, raw[&key].clone()))
        })
        .collect::<Result<_>>()?;
    let mutation = P::prepare_commit_pairs(
        target,
        &options.operation,
        pairs,
        &template,
        &receipt,
        requires,
        &[],
    )?;
    history_adapter::from_store_capture(&crate::history_prospective::capture_after(
        target, &mutation,
    )?)?;
    Ok(mutation)
}
pub fn verify(live: &Capture, mutation: &PreparedMutation, artifact: &Artifact) -> Result<()> {
    let data = mutation.to_data();
    let data = map(&data)?;
    require(data["authority"] == live.marker, "authority_mismatch")?;
    let manifest = Y::decode_document(
        mutation
            .files()
            .iter()
            .find(|f| f.role == "history_commit")
            .and_then(|f| f.after.as_deref())
            .ok_or_else(|| error("invalid_mutation"))?,
    )?;
    A::validate_commit(&manifest)?;
    let manifest = map(&manifest)?;
    let mut commits = Files::new();
    let mut todo = map(&manifest["parents"])?
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let mut visits = 0;
    while let Some(op) = todo.pop() {
        visits += 1;
        require(visits <= 100_000, "history_limit")?;
        if commits.contains_key(&op) {
            continue;
        }
        let raw = live
            .commits
            .get(&op)
            .ok_or_else(|| error("incomplete_commit"))?;
        let parent = Y::decode_document(raw)?;
        A::validate_commit(&parent)?;
        todo.extend(map(&map(&parent)?["parents"])?.keys().cloned());
        commits.insert(op, raw.clone());
    }
    let mut before = live.clone();
    before.commits = commits;
    before.objects = A::committed_objects(&live.marker, &before.commits, &live.object_bytes)?;
    before.object_bytes = raw_for(&before.objects, &live.object_bytes);
    before.state = R::reduce_bytes(
        &before.object_bytes,
        Some(map(&map(&live.state)?["rules"])?),
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
        "history_adoption",
    )?)?;
    let options = Options {
        operation: text(&data["operation"])?.into(),
        recorded_at: text(field(adoption, "recorded_at")?)?.into(),
        by: text(field(adoption, "by")?)?.into(),
    };
    let expected = prepare_requires(
        &before,
        artifact,
        field(adoption, "choices")?,
        &options,
        manifest.get("requires"),
    )?;
    require(
        expected.to_bytes()? == mutation.to_bytes()?,
        "adoption_mutation_mismatch",
    )
}
pub fn commit(
    store: &Store,
    mutation: &PreparedMutation,
    artifact: &Artifact,
    callback: F::Verify<'_>,
) -> Result<V> {
    store.commit(mutation, &mut |data| {
        verify(&store.capture()?, mutation, artifact)?;
        callback(data)
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde_json::Value as J;
    use std::{fs, path::Path, process::Command};
    pub(crate) fn cases() -> J {
        serde_json::from_str(include_str!(
            "../tests/fixtures/history-contribution-adoption.json"
        ))
        .unwrap()
    }
    pub(crate) fn files(value: &J) -> Files {
        value
            .as_object()
            .unwrap()
            .iter()
            .map(|(p, v)| (p.clone(), STANDARD.decode(v.as_str().unwrap()).unwrap()))
            .collect()
    }
    pub(crate) fn inputs(case: &J) -> (Capture, Artifact, V, Options) {
        let mut target = B::capture(&files(&case["target"]), None).unwrap();
        target.layout = H::Layout::for_entry("GROUNDING.yaml").unwrap();
        let artifact = V::from_tagged(&case["artifact"]).unwrap();
        let a = map(&artifact).unwrap();
        (
            target,
            Artifact {
                manifest: a["manifest"].clone(),
                revision: a["revision"].clone(),
                files: files(&case["artifact_files"]),
            },
            V::from_tagged(&case["choices"]).unwrap(),
            Options {
                operation: case["operation"].as_str().unwrap().into(),
                recorded_at: case["recorded_at"].as_str().unwrap().into(),
                by: case["by"].as_str().unwrap().into(),
            },
        )
    }
    pub(crate) fn write(root: &Path, files: &Files) {
        for (p, b) in files {
            let path = root.join(p);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b).unwrap();
        }
    }
    pub(crate) fn git(root: &Path, args: &[&str]) -> String {
        let result = Command::new("git")
            .args(["-c", "core.longpaths=true", "-c", "core.autocrlf=false"])
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap().trim().into()
    }
    pub(crate) fn install(root: &Path, case: &J, pending: bool) -> Store {
        if pending {
            git(root, &["init", "-q", "-b", "main"]);
            git(root, &["config", "user.email", "fixture@example.test"]);
            git(root, &["config", "user.name", "fixture"]);
            let ledger = files(&case["ledger"]);
            write(root, &ledger);
            git(root, &["add", "--all"]);
            git(root, &["commit", "-qm", "fixture pending evidence"]);
            git(root, &["update-ref", crate::pending_state::REF, "HEAD"]);
            for p in ledger.keys() {
                fs::remove_file(root.join(p)).unwrap();
            }
        }
        write(root, &files(&case["target_files"]));
        Store::new(&root.join("GROUNDING.yaml")).unwrap()
    }
    #[test]
    fn complete_preview_and_mutation_bytes_match_frozen_python() {
        for case in cases()["cases"].as_array().unwrap() {
            let (target, artifact, choices, options) = inputs(case);
            if let Some(expected) = case.get("preview_error") {
                assert_eq!(
                    preview(&target, &artifact).unwrap_err().0,
                    expected.as_str().unwrap()
                );
            } else {
                assert_eq!(
                    preview(&target, &artifact).unwrap(),
                    V::from_tagged(&case["preview"]).unwrap(),
                    "{}",
                    case["name"]
                );
            }
            let result = prepare(&target, &artifact, &choices, &options);
            if let Some(expected) = case.get("mutation") {
                assert_eq!(
                    result.unwrap().to_bytes().unwrap(),
                    STANDARD.decode(expected.as_str().unwrap()).unwrap(),
                    "{}",
                    case["name"]
                );
            } else {
                assert_eq!(
                    result.unwrap_err().0,
                    case["error"].as_str().unwrap(),
                    "{}",
                    case["name"]
                );
            }
        }
    }
    #[test]
    fn full_contribution_validation_and_exact_replay_preserve_every_image() {
        for case in cases()["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c.get("mutation").is_some())
        {
            let dir = tempfile::tempdir().unwrap();
            let store = install(dir.path(), case, false);
            let before = store.capture().unwrap();
            let bundle = crate::pending_state::Bundle {
                value: V::from_tagged(&case["bundle"]).unwrap(),
                files: files(&case["bundle_files"]),
            };
            let artifact = Artifact::from_contribution(&bundle).unwrap();
            let (_, _, choices, options) = inputs(case);
            let mutation = prepare(&before, &artifact, &choices, &options).unwrap();
            for _ in 0..2 {
                commit(&store, &mutation, &artifact, &mut |_| Ok(())).unwrap();
            }
            let after = store.capture().unwrap();
            for (key, raw) in &before.object_bytes {
                assert_eq!(after.object_bytes.get(key), Some(raw));
            }
            for file in mutation.files() {
                assert_eq!(
                    fs::read(dir.path().join(&file.path)).unwrap(),
                    file.after.clone().unwrap()
                );
            }
            assert_eq!(
                map(&after.document).unwrap()["record"],
                map(&before.document).unwrap()["record"]
            );
            for (subject, chosen) in map(&choices).unwrap() {
                assert!(
                    list(
                        &map(&map(&map(&after.state).unwrap()["subjects"]).unwrap()[subject])
                            .unwrap()["heads"]
                    )
                    .unwrap()
                    .contains(chosen)
                );
            }
        }
    }
    #[test]
    fn artifact_corruption_private_content_and_forged_intent_refuse() {
        let all = cases();
        let case = &all["cases"][0];
        let (target, artifact, choices, options) = inputs(case);
        let mut bad = artifact.clone();
        let path = bad
            .files
            .keys()
            .find(|p| p.starts_with("objects/"))
            .unwrap()
            .clone();
        bad.files
            .get_mut(&path)
            .unwrap()
            .extend(b"# changed bytes\n");
        assert!(preview(&target, &bad).is_err());
        let mut bundle = crate::pending_state::Bundle {
            value: V::from_tagged(&case["bundle"]).unwrap(),
            files: files(&case["bundle_files"]),
        };
        let manifest = map_mut(
            map_mut(&mut bundle.value)
                .unwrap()
                .get_mut("manifest")
                .unwrap(),
        )
        .unwrap();
        map_mut(manifest.get_mut("document").unwrap())
            .unwrap()
            .insert("private".into(), V::Bool(true));
        let digest = V::Map(manifest.clone()).digest().unwrap();
        map_mut(&mut bundle.value)
            .unwrap()
            .insert("revision".into(), s(&digest));
        assert!(Artifact::from_contribution(&bundle).is_err());
        let mutation = prepare(&target, &artifact, &choices, &options).unwrap();
        let data = mutation.to_data();
        let receipt = map(&map(&data).unwrap()["receipt"]).unwrap();
        let mut after = receipt["after"].clone();
        map_mut(
            map_mut(&mut after)
                .unwrap()
                .get_mut("history_adoption")
                .unwrap(),
        )
        .unwrap()
        .insert("by".into(), s("forged"));
        let receipt = T::semantic_receipt(
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
        let pairs = mutation
            .files()
            .iter()
            .filter(|f| f.role == "history_object")
            .map(|f| {
                let raw = f.after.clone().unwrap();
                (Y::decode_document(&raw).unwrap(), raw)
            })
            .collect();
        let forged = P::prepare_commit_pairs(
            &target,
            &options.operation,
            pairs,
            &map(&manifest).unwrap()["view_template"],
            &receipt,
            map(&manifest).unwrap().get("requires"),
            &[],
        )
        .unwrap();
        assert_eq!(
            verify(&target, &forged, &artifact).unwrap_err().0,
            "adoption_mutation_mismatch"
        );
    }
}

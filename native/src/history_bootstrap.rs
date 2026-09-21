//! Atomic first-generation authoring. The first claim has its actual intent,
//! writer and recording time; no migration provenance is invented.
use crate::{
    Result,
    history_authoring::{self as W, Options, empty, n, obj, s},
    history_authoring_audit::ReplayAudit,
    history_authority as A, history_capture as H,
    history_contract::*,
    history_emit as E, history_paths as P, history_preparation as Prep, history_reduce as R,
    history_store::Store,
    history_transaction::{self as T, FileImage, PreparedMutation},
    history_transaction_fs as F,
    history_view::{self as View, list, map_mut, truth},
    reasoning_authoring::{self as Authoring, World},
    reasoning_runtime::{OperationalBounds, Runtime},
    require,
    value::TypedValue as V,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
pub const KIND: &str = "new-record-bootstrap/v1";
#[derive(Clone)]
pub struct BootstrapOptions {
    pub operation: String,
    pub recorded_at: String,
    pub recording_day: String,
    pub record_id: String,
    pub by: V,
}
fn entry(path: &Path) -> Result<PathBuf> {
    let path = crate::source_inventory::absolute(path)?;
    let path = if path.exists() {
        path.canonicalize()?
    } else {
        path.parent()
            .ok_or_else(|| error("invalid_path"))?
            .canonicalize()?
            .join(path.file_name().ok_or_else(|| error("invalid_path"))?)
    };
    require(
        path.file_name().and_then(|p| p.to_str()) == Some("GROUNDING.yaml"),
        "legacy_record_birth_unsupported",
    )?;
    Ok(path)
}
fn no_link(path: &Path) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        require(!meta.file_type().is_symlink(), "invalid_history_path")?;
    }
    Ok(())
}
fn empty_directory(path: &Path, code: &str) -> Result<()> {
    no_link(path)?;
    require(
        !path.exists() || path.is_dir() && fs::read_dir(path)?.next().is_none(),
        code,
    )
}
fn verify_environment(entry: &Path, mutation: Option<&PreparedMutation>) -> Result<()> {
    let root = entry.parent().unwrap();
    let layout = T::Layout::for_entry("GROUNDING.yaml")?;
    let after = |role: &str| {
        mutation
            .and_then(|m| m.files().iter().find(|i| i.role == role))
            .and_then(|i| i.after.as_deref())
    };
    no_link(entry)?;
    require(
        F::read(entry)?
            .as_deref()
            .is_none_or(|raw| Some(raw) == after("record")),
        "history_bootstrap_source_present",
    )?;
    let authority = F::target(root, &layout.authority)?;
    require(
        F::read(&authority)?
            .as_deref()
            .is_none_or(|raw| Some(raw) == after("history_authority")),
        "history_bootstrap_authority_present",
    )?;
    let allowed = mutation
        .map(|m| {
            m.files()
                .iter()
                .filter(|i| ["history_object", "history_commit"].contains(&i.role.as_str()))
                .map(|i| Ok((F::target(root, &i.path)?, i.after.clone())))
                .collect::<Result<BTreeMap<_, _>>>()
        })
        .transpose()?
        .unwrap_or_default();
    let mut visits = 0;
    for role in [&layout.objects, &layout.commits] {
        let dir = F::target(root, role)?;
        if !dir.exists() {
            continue;
        }
        require(dir.is_dir(), "invalid_history_path")?;
        let mut pending = vec![dir];
        while let Some(dir) = pending.pop() {
            for item in fs::read_dir(dir)? {
                let path = item?.path();
                visits += 1;
                require(visits <= 100_000, "history_limit")?;
                no_link(&path)?;
                if path.is_dir() {
                    pending.push(path);
                } else {
                    require(
                        path.is_file() && allowed.contains_key(&path),
                        "existing_history_evidence",
                    )?;
                    require(
                        F::read(&path)? == allowed[&path],
                        "existing_history_evidence",
                    )?;
                }
            }
        }
    }
    let hypotheses = F::target(root, &layout.hypotheses)?;
    require(
        !hypotheses.exists() || hypotheses.is_dir(),
        "invalid_history_path",
    )?;
    if hypotheses.is_dir() {
        for item in fs::read_dir(hypotheses)? {
            let name = item?.file_name();
            let name = name.to_str().ok_or_else(|| error("invalid_history_path"))?;
            require(
                name.starts_with('.') || !name.ends_with(".yaml") && !name.ends_with(".yml"),
                "existing_hypothesis_requires_record",
            )?;
        }
    }
    require(
        !F::target(root, &layout.replaced)?.exists(),
        "existing_record_evidence",
    )?;
    empty_directory(
        &F::target(root, &layout.cancellations)?,
        "existing_history_evidence",
    )?;
    let alternate = T::Layout::for_entry("PROVENANCE.yaml")?;
    for path in [&alternate.authority, &alternate.replaced] {
        require(!F::target(root, path)?.exists(), "existing_record_evidence")?;
    }
    for path in [
        &alternate.objects,
        &alternate.commits,
        &alternate.cancellations,
        &layout.retained,
    ] {
        empty_directory(&F::target(root, path)?, "existing_history_evidence")?;
    }
    Ok(())
}
fn act(claim: &V, options: &Options, kind: &str, because: String) -> Result<V> {
    let c = map(claim)?;
    W::make_object(
        text(&c["subject"])?,
        "act",
        obj([
            ("act", s(kind)),
            ("of", c["id"].clone()),
            ("over", V::List(vec![])),
            ("because", s(&because)),
        ]),
        V::List(vec![c["id"].clone()]),
        None,
        empty(),
        empty(),
        options,
    )
}
pub fn prepare(
    path: &Path,
    action: &V,
    policy: &V,
    options: &BootstrapOptions,
    runtime: Option<&Runtime>,
) -> Result<PreparedMutation> {
    prepare_inner(path, action, policy, options, runtime, None, false)
}
#[allow(clippy::too_many_arguments)]
fn prepare_inner(
    path: &Path,
    action: &V,
    policy: &V,
    options: &BootstrapOptions,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
    replay: bool,
) -> Result<PreparedMutation> {
    let entry = entry(path)?;
    let root = entry.parent().unwrap();
    require(!options.recorded_at.is_empty(), "missing_recording_time")?;
    if !replay {
        verify_environment(&entry, None)?;
    }
    map(policy)?;
    let mut action = action.clone();
    let a = map_mut(&mut action)?;
    require(
        a.get("kind") == Some(&s("add")) && !a.get("section").is_some_and(truth),
        "unsupported_history_bootstrap_action",
    )?;
    require(
        a.get("profile")
            .is_none_or(|v| *v == V::Null || string_is(v, "core/v1")),
        "history_profile_migration_required",
    )?;
    if !a.get("as_of").is_some_and(truth) {
        require(!options.recording_day.is_empty(), "missing_recording_time")?;
        a.insert("as_of".into(), s(&options.recording_day));
    }
    let intent = action.clone();
    let day = map(&action)?["as_of"].clone();
    let before_doc = Authoring::declare_document(&obj([("meta", obj([("updated", day)]))]))?;
    let mut before_world = World::new(&before_doc, None, runtime, OperationalBounds::default())?;
    let fields = before_world.fields().clone();
    let (action, notes) = before_world.normalize(&action, None)?;
    let refusals = before_world.validate(&action)?;
    require(
        refusals.is_empty(),
        &format!("refused - {}", refusals.join("\n          ")),
    )?;
    let a = map(&action)?;
    let subject = text(field(a, "id")?)?;
    let body = field(a, "body")?.clone();
    let collection = crate::reasoning_authoring_guards::collection_for(
        &before_world,
        subject,
        &body,
        a.get("into")
            .filter(|v| **v != V::Null)
            .map(text)
            .transpose()?,
    )?;
    let mut authored = obj([
        ("collection", s(&collection)),
        ("fields", V::Map(fields.clone())),
        ("profile", s("core/v1")),
    ]);
    let hypothesis = a.get("hypothesis").filter(|v| **v != V::Null);
    let head = obj([("born", a["as_of"].clone())]);
    if let Some(name) = hypothesis {
        hypothesis_name(name)?;
        map_mut(&mut authored)?.insert(
            "hypothesis".into(),
            obj([
                ("version", n("1")),
                ("name", name.clone()),
                ("head", head.clone()),
            ]),
        );
    }
    let deps = map(&body)
        .ok()
        .and_then(|b| b.get(text(&fields["deps"]).unwrap()))
        .cloned()
        .unwrap_or(V::List(vec![]));
    let deps = list(&deps).map_err(|_| error("invalid_bootstrap_dependencies"))?;
    for dep in deps {
        text(dep).map_err(|_| error("invalid_bootstrap_dependencies"))?;
    }
    require(
        deps.is_empty() || hypothesis.is_some() && !Authoring::blocked_text(&body).is_empty(),
        "unresolved_history_subject",
    )?;
    let gaps = V::Map(
        deps.iter()
            .map(|v| (text(v).unwrap().into(), s("unavailable")))
            .collect(),
    );
    let marker = A::authority(&options.record_id, "history", &n("1"), Map::new())?;
    let initial_state = R::reduce(&Map::new(), None, None)?;
    let baseline = H::baseline(&marker, &A::Files::new(), &initial_state)?;
    let old_marker = A::authority(&options.record_id, "legacy", &n("0"), Map::new())?;
    let ordinary_options = Options {
        operation: options.operation.clone(),
        recorded_at: options.recorded_at.clone(),
        recording_day: options.recording_day.clone(),
        by: options.by.clone(),
        strict: true,
        paths: P::Scheme::Hashed,
        receipt_version: None,
    };
    let claim = W::make_object(
        subject,
        if map(&body).is_ok_and(|b| b.contains_key(text(&fields["deps"]).unwrap())) {
            "judgment"
        } else {
            "reading"
        },
        body.clone(),
        V::List(vec![]),
        Some(authored),
        empty(),
        gaps,
        &ordinary_options,
    )?;
    let because = a
        .get("why")
        .filter(|v| truth(v))
        .map(|v| crate::source_text::python_str(&crate::history_yaml::SourceValue::from_typed(v)))
        .unwrap_or_else(|| {
            if hypothesis.is_some() {
                "explicit named hypothesis".into()
            } else {
                "explicit add".into()
            }
        });
    let disposition = act(
        &claim,
        &ordinary_options,
        if hypothesis.is_some() {
            "propose"
        } else {
            "accept"
        },
        because,
    )?;
    let mut after_doc = before_doc.clone();
    map_mut(&mut after_doc)?.insert(
        collection.clone(),
        V::Map(Map::from([(subject.into(), body)])),
    );
    if hypothesis.is_none() {
        after_doc = W::destination(&after_doc)?;
    }
    let mut before = W::evidence(&before_doc, &mut before_world, audit)?;
    let mut after_world = World::new(&after_doc, None, runtime, OperationalBounds::default())?;
    let mut after = W::evidence(&after_doc, &mut after_world, audit)?;
    let objects = [claim, disposition];
    let mut ids = objects
        .iter()
        .map(|o| map(o).unwrap()["id"].clone())
        .collect::<Vec<_>>();
    ids.sort_by(|a, b| text(a).unwrap().cmp(text(b).unwrap()));
    let layout = T::Layout::for_entry("GROUNDING.yaml")?;
    let archive = obj([("path", s(&layout.replaced)), ("sha256", V::Null)]);
    let bootstrap = obj([("version", n("1")), ("kind", s(KIND))]);
    if let Some(name) = hypothesis {
        map_mut(&mut before)?.insert(
            "hypothesis_authoring".into(),
            obj([
                ("version", n("1")),
                ("kind", s("edit")),
                ("name", name.clone()),
                ("head", head),
                ("action", intent.clone()),
                ("operation", s(&options.operation)),
                ("recorded_at", s(&options.recorded_at)),
                ("by", options.by.clone()),
                ("archive", archive),
                ("physical", empty()),
                ("baseline", baseline.clone()),
                ("bootstrap", bootstrap),
            ]),
        );
        map_mut(&mut after)?.insert(
            "hypothesis_authoring".into(),
            obj([("objects", V::List(ids))]),
        );
    } else {
        map_mut(&mut before)?.insert(
            "authoring".into(),
            obj([
                ("version", n("1")),
                ("action", intent.clone()),
                ("by", options.by.clone()),
                ("recorded_at", s(&options.recorded_at)),
                ("archive", archive),
                ("baseline", baseline.clone()),
                ("bootstrap", bootstrap),
            ]),
        );
        map_mut(&mut after)?.insert(
            "authoring".into(),
            obj([
                ("objects", V::List(ids)),
                ("notes", V::List(notes.into_iter().map(V::Text).collect())),
            ]),
        );
    }
    let cap = crate::reasoning_fields::capabilities(&after_doc, None)?;
    let receipt = T::semantic_receipt("core/v1", &cap, &before, &after)?;
    let pairs = objects
        .iter()
        .map(|o| Ok((o.clone(), E::encode_document(o)?)))
        .collect::<Result<Vec<_>>>()?;
    let mut template = A::document_template(if hypothesis.is_some() {
        &before_doc
    } else {
        &after_doc
    })?;
    map_mut(&mut template)?
        .entry(collection)
        .or_insert_with(empty);
    let mut document = template.clone();
    map_mut(map_mut(&mut document)?.get_mut("meta").unwrap())?
        .insert("history".into(), baseline.clone());
    let raw = E::encode_document(&document)?;
    let mut captured = H::Capture {
        root: root.into(),
        layout: H::Layout::for_entry("GROUNDING.yaml")?,
        entry_bytes: raw.clone(),
        document: document.clone(),
        view_alternatives: vec![View::Alternative {
            name: "view".into(),
            bytes: raw,
            document,
        }],
        authority_bytes: E::encode_document(&marker)?,
        marker: marker.clone(),
        commits: A::Files::new(),
        object_bytes: A::ObjectBytes::new(),
        objects: Map::new(),
        state: initial_state,
        baseline: baseline.clone(),
        inactive_generations: BTreeMap::new(),
        cancellation_bytes: A::Files::new(),
        storage_bytes: A::Files::new(),
        object_paths: A::ObjectPaths::new(),
        inventory: BTreeMap::new(),
    };
    let requires = ordinary_options.requires().unwrap();
    let draft = Prep::make_commit(
        &marker,
        &options.operation,
        &Map::new(),
        &baseline,
        &pairs,
        &receipt,
        b"",
        Some(&template),
        Some(&requires),
    )?;
    let commits = A::Files::from([(options.operation.clone(), E::encode_document(&draft)?)]);
    let selected = objects
        .iter()
        .map(|o| (text(&map(o).unwrap()["id"]).unwrap().into(), o.clone()))
        .collect::<Map>();
    let object_bytes = pairs
        .iter()
        .map(|(o, raw)| {
            (
                (
                    text(&map(o).unwrap()["subject"]).unwrap().into(),
                    text(&map(o).unwrap()["id"]).unwrap().into(),
                ),
                raw.clone(),
            )
        })
        .collect::<A::ObjectBytes>();
    let rendered = View::render(&captured, &selected, &object_bytes, &commits)?;
    let manifest = Prep::make_commit(
        &marker,
        &options.operation,
        &Map::new(),
        &baseline,
        &pairs,
        &receipt,
        &rendered,
        Some(&template),
        Some(&requires),
    )?;
    let manifest_raw = E::encode_document(&manifest)?;
    captured.entry_bytes = rendered.clone();
    captured.document = crate::history_yaml::decode_document(&rendered)?;
    captured.commits = A::Files::from([(options.operation.clone(), manifest_raw.clone())]);
    captured.objects = selected;
    captured.state = R::reduce_bytes(&object_bytes, None, None)?;
    captured.baseline = H::baseline(&marker, &captured.commits, &captured.state)?;
    captured.object_bytes = object_bytes;
    crate::history_adapter::from_store_capture(&captured)?;
    let mut files = vec![
        FileImage {
            path: "GROUNDING.yaml".into(),
            role: "record".into(),
            before: None,
            after: Some(rendered),
        },
        FileImage {
            path: layout.authority.clone(),
            role: "history_authority".into(),
            before: None,
            after: Some(E::encode_document(&marker)?),
        },
    ];
    for (o, raw) in pairs {
        let o = map(&o)?;
        files.push(FileImage {
            path: format!(
                "{}/{}",
                layout.objects,
                P::object_path(text(&o["subject"])?, text(&o["id"])?, P::Scheme::Hashed)?
            ),
            role: "history_object".into(),
            before: None,
            after: Some(raw),
        });
    }
    files.push(FileImage {
        path: format!("{}/{}.yaml", layout.commits, options.operation),
        role: "history_commit".into(),
        before: None,
        after: Some(manifest_raw),
    });
    let baseline = obj([
        ("kind", s("history-authority-transition/v1")),
        ("direction", s("activate")),
        (
            "bootstrap",
            obj([
                ("version", n("1")),
                ("kind", s(KIND)),
                ("action", intent),
                ("by", options.by.clone()),
                ("recorded_at", s(&options.recorded_at)),
                ("record_id", s(&options.record_id)),
                ("policy", policy.clone()),
            ]),
        ),
        ("transaction_root", s(crate::source_inventory::name(root)?)),
        ("source_absent", V::Bool(true)),
        ("record_members", obj([("GROUNDING.yaml", V::Null)])),
        ("hypothesis_members", empty()),
        ("retained_files", empty()),
        ("history_baseline", baseline),
    ]);
    PreparedMutation::prepare(
        &options.operation,
        &old_marker,
        &baseline,
        files,
        &receipt,
        "GROUNDING.yaml",
        Some(&obj([("version", n("1")), ("after", marker)])),
    )
}
pub fn verify_prepared(
    path: &Path,
    mutation: &PreparedMutation,
    policy: &V,
    runtime: Option<&Runtime>,
) -> Result<()> {
    let entry = entry(path)?;
    let data = mutation.to_data();
    let d = map(&data)?;
    let base = map(&d["baseline"])?;
    let b = map(base
        .get("bootstrap")
        .ok_or_else(|| error("invalid_history_bootstrap"))?)?;
    require(
        b.get("version") == Some(&n("1")) && b.get("kind") == Some(&s(KIND)),
        "invalid_history_bootstrap",
    )?;
    require(
        base.get("source_absent") == Some(&V::Bool(true))
            && base.get("record_members") == Some(&obj([("GROUNDING.yaml", V::Null)]))
            && base.get("transaction_root")
                == Some(&s(crate::source_inventory::name(entry.parent().unwrap())?))
            && b.get("policy") == Some(policy),
        "history_bootstrap_changed",
    )?;
    let action = field(b, "action")?;
    let options = BootstrapOptions {
        operation: text(&d["operation"])?.into(),
        recorded_at: text(field(b, "recorded_at")?)?.into(),
        recording_day: map(action)?
            .get("as_of")
            .map(|v| {
                crate::source_text::python_str(&crate::history_yaml::SourceValue::from_typed(v))
            })
            .unwrap_or_default(),
        record_id: text(field(b, "record_id")?)?.into(),
        by: b.get("by").cloned().unwrap_or(V::Null),
    };
    let audit = ReplayAudit::from_parents(&d["receipt"], &A::Files::new())?;
    let expected = prepare_inner(
        &entry,
        action,
        policy,
        &options,
        runtime,
        Some(&audit),
        true,
    )?;
    require(
        expected.to_data() == data,
        "history_bootstrap_intent_mismatch",
    )?;
    verify_environment(&entry, Some(mutation))
}
fn verify_result(entry: &Path, mutation: &PreparedMutation) -> Result<()> {
    let captured = Store::new(entry)?.capture()?;
    let data = mutation.to_data();
    require(
        captured.commits.len() == 1
            && captured
                .commits
                .contains_key(text(&map(&data)?["operation"])?)
            && mutation
                .files()
                .iter()
                .any(|i| i.role == "record" && i.after.as_ref() == Some(&captured.entry_bytes)),
        "history_bootstrap_incomplete",
    )
}
pub fn publish(
    path: &Path,
    mutation: &PreparedMutation,
    policy: &V,
    runtime: Option<&Runtime>,
    verify: &mut dyn FnMut(&V) -> Result<()>,
) -> Result<()> {
    let entry = entry(path)?;
    let root = entry.parent().unwrap();
    let layout = T::Layout::for_entry("GROUNDING.yaml")?;
    let mut checked = |data: &V| {
        verify_prepared(&entry, mutation, policy, runtime)?;
        verify(data)?;
        verify_prepared(&entry, mutation, policy, runtime)
    };
    F::publish_transition(root, &layout.journal, mutation, &mut checked, None)?;
    verify_result(&entry, mutation)
}
pub fn recover(
    path: &Path,
    policy: &V,
    runtime: Option<&Runtime>,
    direction: F::Direction,
    verify: &mut dyn FnMut(&V) -> Result<()>,
) -> Result<PreparedMutation> {
    let entry = entry(path)?;
    let root = entry.parent().unwrap();
    let layout = T::Layout::for_entry("GROUNDING.yaml")?;
    let raw =
        F::read(&F::target(root, &layout.journal)?)?.ok_or_else(|| error("no_recovery_pending"))?;
    let mutation = PreparedMutation::from_bytes(&raw)?;
    let mut checked = |data: &V| {
        require(data == &mutation.to_data(), "concurrent_edit")?;
        verify_prepared(&entry, &mutation, policy, runtime)?;
        verify(data)?;
        verify_prepared(&entry, &mutation, policy, runtime)
    };
    let result = if direction == F::Direction::Before {
        F::rollback_bootstrap(root, &layout.journal, &mut checked)?
    } else {
        F::recover_transition(root, &layout.journal, direction, &mut checked, None)?
    };
    if direction == F::Direction::After {
        verify_result(&entry, &result)?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cases() -> serde_json::Value {
        serde_json::from_str(include_str!("../tests/fixtures/history-bootstrap.json")).unwrap()
    }
    fn runtime(cache: &Path) -> Runtime {
        Runtime::open(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/reasoning/native")
                .join(format!(
                    "{}.kpopper-runtime",
                    crate::reasoning_runtime::target_name().unwrap()
                )),
            cache,
            OperationalBounds::default(),
        )
        .unwrap()
    }
    fn expected(case: &serde_json::Value, root: &Path) -> V {
        fn restore(v: &mut V, root: &str) {
            match v {
                V::Text(s) if s == "$ROOT" => *s = root.into(),
                V::Map(m) => {
                    for v in m.values_mut() {
                        restore(v, root)
                    }
                }
                V::List(a) => {
                    for v in a {
                        restore(v, root)
                    }
                }
                _ => {}
            }
        }
        let mut value = V::from_tagged(&case["output"]).unwrap();
        restore(&mut value, root.to_str().unwrap());
        map_mut(&mut value).unwrap().remove("digest");
        let digest = value.digest().unwrap();
        map_mut(&mut value)
            .unwrap()
            .insert("digest".into(), s(&digest));
        value
    }
    fn options(case: &serde_json::Value, expected: &V) -> (V, BootstrapOptions) {
        let options = V::from_tagged(&case["options"]).unwrap();
        let o = map(&options).unwrap();
        let expected = map(expected).unwrap();
        let base = map(&expected["baseline"]).unwrap();
        let action = &map(&base["bootstrap"]).unwrap()["action"];
        (
            o["policy"].clone(),
            BootstrapOptions {
                operation: text(&o["operation"]).unwrap().into(),
                recorded_at: text(&o["recorded_at"]).unwrap().into(),
                record_id: text(&o["record_id"]).unwrap().into(),
                recording_day: crate::source_text::python_str(
                    &crate::history_yaml::SourceValue::from_typed(&map(action).unwrap()["as_of"]),
                ),
                by: o.get("by").cloned().unwrap_or(V::Null),
            },
        )
    }
    #[test]
    fn first_claim_and_hypothesis_envelopes_match_final_python() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        for case in cases().as_array().unwrap() {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let entry = root.join("GROUNDING.yaml");
            let expected = expected(case, &root);
            let (policy, options) = options(case, &expected);
            let action = V::from_tagged(&case["action"]).unwrap();
            let audit = ReplayAudit::oracle(&map(&expected).unwrap()["receipt"]).unwrap();
            let mutation = prepare_inner(
                &entry,
                &action,
                &policy,
                &options,
                Some(&runtime),
                Some(&audit),
                false,
            )
            .unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
            assert_eq!(mutation.to_data(), expected, "{}", case["name"]);
            assert!(!entry.exists());
        }
    }
    #[test]
    fn native_bootstrap_replays_before_any_publication_and_retains_original_intent() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        for case in cases().as_array().unwrap() {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let entry = root.join("GROUNDING.yaml");
            let expected = expected(case, &root);
            let (policy, options) = options(case, &expected);
            let action = V::from_tagged(&case["action"]).unwrap();
            let mutation = prepare(&entry, &action, &policy, &options, Some(&runtime)).unwrap();
            publish(&entry, &mutation, &policy, Some(&runtime), &mut |_| Ok(())).unwrap();
            let capture = Store::new(&entry).unwrap().capture().unwrap();
            assert_eq!(capture.commits.len(), 1);
            let id = text(&map(&action).unwrap()["id"]).unwrap();
            let state = &map(&map(&capture.state).unwrap()["subjects"]).unwrap()[id];
            let proposed = map(&action)
                .unwrap()
                .get("hypothesis")
                .is_some_and(|v| *v != V::Null);
            assert_eq!(
                map(state).unwrap()["acceptance"],
                s(if proposed { "proposed" } else { "accepted" })
            );
        }
    }
    fn native_case(root: &Path, runtime: &Runtime) -> (V, PreparedMutation) {
        let corpus = cases();
        let case = corpus
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "test_first_claim_is_direct_authored_history_not_legacy_import")
            .unwrap();
        let expected = expected(case, root);
        let (policy, options) = options(case, &expected);
        let action = V::from_tagged(&case["action"]).unwrap();
        let mutation = prepare(
            &root.join("GROUNDING.yaml"),
            &action,
            &policy,
            &options,
            Some(runtime),
        )
        .unwrap();
        (policy, mutation)
    }
    fn stage(root: &Path, mutation: &PreparedMutation, phase: usize) {
        let journal = root.join(T::Layout::for_entry("GROUNDING.yaml").unwrap().journal);
        fs::create_dir_all(journal.parent().unwrap()).unwrap();
        fs::write(journal, mutation.to_bytes().unwrap()).unwrap();
        let mut immutable = 0;
        for item in mutation.files() {
            let publish = if ["history_object", "history_commit"].contains(&item.role.as_str()) {
                immutable += 1;
                phase >= 2 || phase == 1 && immutable == 1
            } else if item.role == "record" {
                phase >= 3
            } else {
                phase >= 4
            };
            if publish {
                let path = root.join(&item.path);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, item.after.as_ref().unwrap()).unwrap();
            }
        }
    }
    #[test]
    fn interrupted_birth_completes_or_removes_only_its_own_new_objects() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        for direction in [F::Direction::After, F::Direction::Before] {
            for phase in 0..5 {
                let temp = tempfile::tempdir().unwrap();
                let root = temp.path().canonicalize().unwrap();
                let entry = root.join("GROUNDING.yaml");
                let (policy, mutation) = native_case(&root, &runtime);
                stage(&root, &mutation, phase);
                assert_eq!(
                    Store::new(&entry).unwrap().capture().unwrap_err().0,
                    "recovery_required"
                );
                recover(&entry, &policy, Some(&runtime), direction, &mut |_| Ok(())).unwrap();
                if direction == F::Direction::After {
                    verify_result(&entry, &mutation).unwrap();
                } else {
                    for image in mutation.files() {
                        assert!(!root.join(&image.path).exists(), "retained {}", image.path);
                    }
                    let (_, retry) = native_case(&root, &runtime);
                    publish(&entry, &retry, &policy, Some(&runtime), &mut |_| Ok(())).unwrap();
                }
            }
        }
    }
    #[test]
    fn rollback_restarts_after_partial_cleanup_and_refuses_changed_images() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        for corrupt in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let entry = root.join("GROUNDING.yaml");
            let (policy, mutation) = native_case(&root, &runtime);
            stage(&root, &mutation, 4);
            for item in mutation
                .files()
                .iter()
                .filter(|i| ["record", "history_authority"].contains(&i.role.as_str()))
            {
                fs::remove_file(root.join(&item.path)).unwrap();
            }
            let first = mutation
                .files()
                .iter()
                .find(|i| i.role == "history_object")
                .unwrap();
            if corrupt {
                fs::write(root.join(&first.path), b"changed").unwrap();
            } else {
                fs::remove_file(root.join(&first.path)).unwrap();
            }
            let result = recover(
                &entry,
                &policy,
                Some(&runtime),
                F::Direction::Before,
                &mut |_| Ok(()),
            );
            if corrupt {
                assert!(result.is_err());
                assert!(
                    root.join(T::Layout::for_entry("GROUNDING.yaml").unwrap().journal)
                        .exists()
                );
            } else {
                result.unwrap();
                assert!(
                    mutation
                        .files()
                        .iter()
                        .all(|i| !root.join(&i.path).exists())
                );
            }
        }
    }
    #[test]
    fn rehashed_intent_and_late_unowned_evidence_cannot_pass_noop_verifiers() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let entry = root.join("GROUNDING.yaml");
        let (policy, mutation) = native_case(&root, &runtime);
        let mut forged = mutation.to_data();
        let base = map_mut(map_mut(&mut forged).unwrap().get_mut("baseline").unwrap()).unwrap();
        let bootstrap = map_mut(base.get_mut("bootstrap").unwrap()).unwrap();
        let action = map_mut(bootstrap.get_mut("action").unwrap()).unwrap();
        map_mut(action.get_mut("body").unwrap())
            .unwrap()
            .insert("v".into(), n("999"));
        map_mut(&mut forged).unwrap().remove("digest");
        let hash = forged.digest().unwrap();
        map_mut(&mut forged)
            .unwrap()
            .insert("digest".into(), s(&hash));
        let forged = PreparedMutation::from_bytes(
            &serde_json::to_vec(&forged.to_tagged().unwrap()).unwrap(),
        )
        .unwrap();
        assert!(publish(&entry, &forged, &policy, Some(&runtime), &mut |_| Ok(())).is_err());
        assert!(!entry.exists());
        let result = publish(&entry, &mutation, &policy, Some(&runtime), &mut |_| {
            let path = root.join(".kpopper/history/unowned.yaml");
            fs::create_dir_all(path.parent().unwrap())?;
            fs::write(path, b"unowned")?;
            Ok(())
        });
        assert!(result.is_err());
        assert!(!entry.exists());
        assert!(root.join(".kpopper/history/unowned.yaml").exists());
    }
    #[test]
    fn existing_record_and_companion_evidence_remain_untouched() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        for path in [
            "GROUNDING.yaml",
            ".kpopper/history.yaml",
            ".kpopper/history/orphan.bin",
            ".kpopper/history-commits/extra.yaml",
            ".kpopper/history-cancellations/other.yaml",
            ".kpopper/hypotheses/new.yaml",
            ".kpopper/replaced.yaml",
            "PROVENANCE.history.yaml",
            "PROVENANCE.replaced.yaml",
            "PROVENANCE.history/orphan",
            ".kpopper-history-migration/old.txt",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let entry = root.join("GROUNDING.yaml");
            let (policy, mutation) = native_case(&root, &runtime);
            let target = root.join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(&target, b"existing evidence").unwrap();
            let prior = fs::read(&entry).ok();
            assert!(
                verify_prepared(&entry, &mutation, &policy, Some(&runtime)).is_err(),
                "accepted {path}"
            );
            assert_eq!(fs::read(&target).unwrap(), b"existing evidence");
            assert_eq!(fs::read(&entry).ok(), prior);
        }
    }
    #[cfg(unix)]
    #[test]
    fn history_symlinks_refuse_and_outside_evidence_is_preserved() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let entry = root.join("GROUNDING.yaml");
        let (policy, mutation) = native_case(&root, &runtime);
        fs::write(outside.path().join("sentinel"), b"outside").unwrap();
        fs::create_dir_all(root.join(".kpopper")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join(".kpopper/history")).unwrap();
        assert!(verify_prepared(&entry, &mutation, &policy, Some(&runtime)).is_err());
        assert!(!entry.exists());
        assert_eq!(
            fs::read(outside.path().join("sentinel")).unwrap(),
            b"outside"
        );
    }
}

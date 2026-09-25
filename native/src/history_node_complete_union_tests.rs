//! Full-history compatibility, provenance, source-order and recovery regressions.
//! Fixtures use legacy capture and contribution wrappers, then migrate into compact targets.
use super::*;
use crate::reasoning_runtime::Runtime;
use crate::{
    domain_profile_package::Package, history_authority as Authority, history_node_writer as W,
};
use crate::{
    history_authoring::{self as A, strings},
    history_view::list,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value as J, json};
use std::collections::BTreeSet;
use std::path::PathBuf;

const AT: &str = "2026-09-25T12:00:00+00:00";
fn value(j: J) -> V {
    V::from_json(&j).unwrap()
}
fn case() -> J {
    let cases: J = serde_json::from_str(include_str!(
        "../tests/fixtures/pending-equivalence-oracle.json"
    ))
    .unwrap();
    cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "v3-history-union")
        .unwrap()
        .clone()
}
fn decoded(value: &J) -> Files {
    value
        .as_object()
        .unwrap()
        .iter()
        .map(|(p, v)| (p.clone(), STANDARD.decode(v.as_str().unwrap()).unwrap()))
        .collect()
}
fn bundle() -> (V, Files) {
    let case = case();
    (
        V::from_tagged(&case["bundle"]).unwrap(),
        decoded(&case["files"]),
    )
}
fn runtime(cache: &Path) -> Runtime {
    let archive = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/reasoning/native")
        .join(format!(
            "{}.kpopper-runtime",
            crate::reasoning_runtime::target_name().unwrap()
        ));
    Runtime::open(
        &archive,
        cache,
        crate::reasoning_runtime::OperationalBounds::default(),
    )
    .unwrap()
}
/// Legacy files written as a live record, migrated into a compact record.
fn migrate(files: &Files) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let legacy = temp.path().join("legacy");
    for (name, raw) in files {
        let path = if name == "entry.yaml" {
            legacy.join("GROUNDING.yaml")
        } else if name == "authority.yaml" {
            legacy.join(".kpopper/history.yaml")
        } else if let Some(n) = name.strip_prefix("commits/") {
            legacy.join(".kpopper/history-commits").join(n)
        } else if let Some(n) = name.strip_prefix("cancellations/") {
            legacy.join(".kpopper/history-cancellations").join(n)
        } else if let Some(n) = name.strip_prefix("objects/") {
            legacy.join(".kpopper/history").join(n)
        } else {
            continue;
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, raw).unwrap();
    }
    let compact = temp.path().join("compact");
    crate::history_node_migration::Plan::prepare(&legacy.join("GROUNDING.yaml"))
        .unwrap()
        .publish(&compact)
        .unwrap();
    (temp, compact)
}
/// The fixture's legacy target, migrated.
fn target() -> (tempfile::TempDir, PathBuf) {
    migrate(&decoded(&case()["target_history"]))
}
fn tree(root: &Path) -> Vec<(String, Option<Vec<u8>>)> {
    let mut out = vec![];
    let mut todo = vec![root.to_owned()];
    while let Some(dir) = todo.pop() {
        for item in std::fs::read_dir(&dir).unwrap() {
            let path = item.unwrap().path();
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                out.push((name, None));
                todo.push(path);
            } else {
                out.push((name, Some(std::fs::read(&path).unwrap())));
            }
        }
    }
    out.sort();
    out
}
fn code(result: Result<()>) -> String {
    result.err().map(|e| e.0).unwrap_or_default()
}
fn union(root: &Path, bundle: &V, files: &Files) -> Result<()> {
    union_complete(root, bundle, files, &mut || Ok(()))
}
/// Re-seal a v3 contribution after changing its history artifact manifest.
fn reseal(bundle: &V, edit: impl FnOnce(&mut Map)) -> V {
    let mut bundle = bundle.clone();
    let V::Map(b) = &mut bundle else { panic!() };
    let V::Map(m) = b.get_mut("manifest").unwrap() else {
        panic!()
    };
    let V::Map(h) = m.get_mut("history").unwrap() else {
        panic!()
    };
    let V::Map(artifact) = h.get_mut("manifest").unwrap() else {
        panic!()
    };
    edit(artifact);
    let revision = V::Map(artifact.clone()).digest().unwrap();
    h.insert("revision".into(), s(&revision));
    let revision = V::Map(m.clone()).digest().unwrap();
    b.insert("revision".into(), s(&revision));
    bundle
}
fn subject(capture: &Capture) -> V {
    map(&map(capture.document()).unwrap()["readings"]).unwrap()["p.input"].clone()
}

/// Edit one union transaction's action in a copy of a snapshot.

// ---- Synthesized complete legacy histories ----

fn scope() -> J {
    json!({"environment":"fixture","kind":"project"})
}
fn fixture_commit() -> Map {
    map(&Y::decode_document(&bundle().1["history-closure/commits/initial.yaml"]).unwrap())
        .unwrap()
        .clone()
}
fn default_rules() -> V {
    map(&crate::history_reduce::reduce_bytes(&Default::default(), None, None).unwrap())
        .unwrap()["rules"]
        .clone()
}
/// A typed core/v1 reading in the fixture's exact authored shape.
fn reading(subject: &str, op: &str, v: J) -> V {
    let mut o = json!({"authored":{"collection":"readings","fields":{"deps":"rests_on","predicate":"wrong_if","snapshot":"seen","value":"v"},"profile":"core/v1"},
        "body":{"scope":scope(),"v":v},"by":"writer","id_scheme":"typed-history/v2","kind":"reading","on":"2026-09-17",
        "op":op,"pins":{},"saw":[],"schema_version":2,"subject":subject});
    o["id"] = json!(crate::identity::typed_object_identity(&value(o.clone())).unwrap());
    value(o)
}
fn id_of(object: &V) -> String {
    text(&map(object).unwrap()["id"]).unwrap().to_owned()
}
#[derive(Clone)]
struct Step {
    op: &'static str,
    generation: u32,
    parents: Vec<&'static str>,
    /// Typed objects with optional exact printed bytes.
    objects: Vec<(V, Option<Vec<u8>>)>,
    template: V,
    requires: Vec<&'static str>,
    receipt: V,
}
fn step(
    op: &'static str,
    generation: u32,
    parents: &[&'static str],
    objects: Vec<V>,
    template: &V,
) -> Step {
    Step {
        op,
        generation,
        parents: parents.to_vec(),
        objects: objects.into_iter().map(|o| (o, None)).collect(),
        template: template.clone(),
        requires: vec![],
        receipt: fixture_commit()["receipt"].clone(),
    }
}
/// A complete legacy history of `steps` under a history marker at `generation`, with the
/// view the legacy renderer generates for its current generation.
fn legacy(generation: u32, steps: &[Step]) -> Files {
    let marker = value(
        json!({"authority":"history","generation":generation,"profile":"history/v1","record_id":"pending-authority","version":1}),
    );
    let mut commits = Files::new();
    let mut storage = Files::new();
    for step in steps {
        let mut items = BTreeMap::new();
        for (object, raw) in &step.objects {
            let subject = text(&map(object).unwrap()["subject"]).unwrap().to_owned();
            let id = id_of(object);
            let raw = raw
                .clone()
                .unwrap_or_else(|| crate::history_emit::encode_document(object).unwrap());
            items.insert(
                id.clone(),
                obj([
                    ("id", s(&id)),
                    ("sha256", s(&sha256(&raw))),
                    ("subject", s(&subject)),
                ]),
            );
            storage.insert(format!("{subject}/{id}.yaml"), raw);
        }
        let mut manifest = map(&obj([
            ("version", A::n("1")),
            ("record_id", s("pending-authority")),
            ("authority_generation", A::n(&step.generation.to_string())),
            ("operation", s(step.op)),
            (
                "parents",
                V::Map(
                    step.parents
                        .iter()
                        .map(|p| (p.to_string(), s(&sha256(&commits[*p]))))
                        .collect(),
                ),
            ),
            ("baseline_digest", s(&sha256(b"baseline"))),
            ("objects", V::List(items.into_values().collect())),
            ("receipt", step.receipt.clone()),
            ("view_sha256", s(&sha256(b"view"))),
            ("view_template", step.template.clone()),
        ]))
        .unwrap()
        .clone();
        if !step.requires.is_empty() {
            manifest.insert(
                "requires".into(),
                strings(step.requires.iter().map(|r| r.to_string())),
            );
        }
        commits.insert(
            step.op.into(),
            crate::history_emit::encode_document(&V::Map(manifest)).unwrap(),
        );
    }
    let (object_bytes, object_paths) = Authority::objects_from_storage(&commits, &storage).unwrap();
    let mut generations =
        Authority::committed_generations(&marker, &commits, &object_bytes, &Files::new()).unwrap();
    let current = generations.remove(&generation.to_string()).unwrap();
    let captured = crate::history_capture::Capture {
        root: PathBuf::new(),
        layout: crate::history_capture::Layout::for_entry("entry.yaml").unwrap(),
        entry_bytes: vec![],
        document: V::Map(Map::new()),
        view_alternatives: vec![],
        authority_bytes: vec![],
        marker: marker.clone(),
        commits: current.commits.clone(),
        object_bytes: object_bytes.clone(),
        objects: current.objects.clone(),
        state: obj([("rules", default_rules())]),
        baseline: V::Null,
        inactive_generations: generations,
        cancellation_bytes: Files::new(),
        storage_bytes: storage.clone(),
        object_paths,
        inventory: BTreeMap::new(),
    };
    let entry =
        crate::history_view::render(&captured, &current.objects, &object_bytes, &current.commits)
            .unwrap();
    let mut files = Files::from([
        (
            "authority.yaml".into(),
            Y::encode_document(&marker).unwrap(),
        ),
        ("entry.yaml".into(), entry),
    ]);
    files.extend(
        commits
            .into_iter()
            .map(|(op, raw)| (format!("commits/{op}.yaml"), raw)),
    );
    files.extend(
        storage
            .into_iter()
            .map(|(p, raw)| (format!("objects/{p}"), raw)),
    );
    files
}
/// A complete artifact (version 1, or 3 with retained generations) wrapped as a v3 bundle.
fn contribute(files: &Files) -> (V, Files) {
    contribute_with(files, &Files::new())
}
/// The same, carrying the exact locator evidence the artifact's objects name.
fn contribute_with(files: &Files, evidence: &Files) -> (V, Files) {
    let rules = default_rules();
    let captured = crate::history_bundle::capture(files, Some(&rules)).unwrap();
    let subjects = captured
        .objects
        .values()
        .chain(
            captured
                .inactive_generations
                .values()
                .flat_map(|g| g.objects.values()),
        )
        .map(|o| text(&map(o).unwrap()["subject"]).unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    let captured_scope = captured
        .objects
        .values()
        .find_map(|o| {
            map(o)
                .ok()?
                .get("body")
                .and_then(|b| map(b).ok())?
                .get("scope")
                .cloned()
        })
        .unwrap_or_else(|| value(scope()));
    let generations = !captured.inactive_generations.is_empty();
    let mut requires = vec![s("history-closure/v1")];
    if generations {
        requires.push(s("history-generations/v1"));
    }
    if !captured.cancellation_bytes.is_empty() {
        requires.push(s("generation-cancellation/v1"));
    }
    if captured.storage_bytes.keys().any(|p| {
        crate::history_paths::parse_object_path(p)
            .is_ok_and(|v| v.0 == crate::history_paths::Scheme::Hashed)
    }) {
        requires.push(s(crate::history_paths::CAPABILITY));
    }
    let mut manifest = map(&obj([
        ("version", A::n(if generations { "3" } else { "1" })),
        ("requires", V::List(requires)),
        ("roots", strings(subjects)),
        ("scope", captured_scope),
        ("shareability", s("project")),
        ("rules", rules),
        ("baseline", captured.baseline.clone()),
        (
            "files",
            V::Map(
                files
                    .iter()
                    .map(|(p, raw)| (p.clone(), s(&sha256(raw))))
                    .collect(),
            ),
        ),
    ]))
    .unwrap()
    .clone();
    if generations {
        manifest.insert(
            "inactive_generations".into(),
            V::Map(
                captured
                    .inactive_generations
                    .iter()
                    .map(|(g, held)| (g.clone(), s(&held.digest)))
                    .collect(),
            ),
        );
    }
    let manifest = V::Map(manifest);
    let artifact = obj([
        ("revision", s(&manifest.digest().unwrap())),
        ("manifest", manifest),
    ]);
    crate::history_contribution_prepare::wrap(&artifact, files, evidence).unwrap()
}
fn package(version: &str) -> (Binding, Package) {
    let contract = serde_json::to_vec(&json!({"format":"domain-contract/v1","types":{"reading":{"collection":"readings","required":true,"fields":{"v":{"type":"integer","minimum":0,"required":true}}}}})).unwrap();
    let manifest = serde_json::to_vec(&json!({"format":"kpopper-domain-package/v1","id":"integer-readings","version":version,"title":"Integer readings","maintainer":"Test","license":"MIT","maturity":"experimental","requires":["domain-contract/v1"],"contract":"contract.json","files":{"contract.json":sha256(&contract)}})).unwrap();
    let package = Package::capture(
        &manifest,
        BTreeMap::from([("contract.json".into(), contract)]),
    )
    .unwrap();
    (Binding::new(&package, BTreeMap::new()).unwrap().0, package)
}
/// A legacy domain adoption commit carrying its package, and the bound template after it.
fn bind(op: &'static str, parent: &'static str, before: &Files, version: &str) -> (Step, V) {
    let mut document = Y::decode_document(&before["entry.yaml"]).unwrap();
    let V::Map(d) = &mut document else { panic!() };
    let V::Map(meta) = d.get_mut("meta").unwrap() else {
        panic!()
    };
    meta.remove("history");
    let (mut binding, package) = package(version);
    binding.package_commit = Some(op.into());
    let after = crate::history_domain::bind_document(&document, &binding, &package).unwrap();
    let template = Authority::document_template(&after).unwrap();
    let intent = obj([
        ("version", A::n("10")),
        ("kind", s("domain-bind")),
        (
            "action",
            obj([
                ("kind", s("domain-bind")),
                ("binding", binding.value().unwrap()),
                ("because", s("fixture contract")),
            ]),
        ),
        ("recorded_at", s(AT)),
        ("by", s("fixture")),
        (
            "archive",
            obj([("path", s(".kpopper/replaced.yaml")), ("sha256", V::Null)]),
        ),
        (
            "domain_package_capture",
            V::from_json(&package.to_capture()).unwrap(),
        ),
    ]);
    let receipt = crate::history_transaction::semantic_receipt(
        "core/v1",
        &crate::reasoning_fields::capabilities(&after, None).unwrap(),
        &obj([("document", document), ("authoring", intent)]),
        &obj([
            ("document", after),
            ("authoring", obj([("objects", V::List(vec![]))])),
        ]),
    )
    .unwrap();
    let mut step = step(op, 1, &[parent], vec![], &template);
    step.receipt = receipt;
    (step, template)
}

// ---- Behavior ----

#[test]
fn complete_same_authority_history_unions_once_and_is_accepted() {
    let (_temp, root) = target();
    let (bundle, files) = bundle();
    let before = Capture::read(&root).unwrap();
    assert!(!accepted(&before, &bundle, &files, &Files::new()).unwrap());
    assert_ne!(map(&subject(&before)).unwrap()["v"], A::n("7"));
    let transactions = before.snapshot.transactions.len();

    union(&root, &bundle, &files).unwrap();
    let after = Capture::read(&root).unwrap();
    assert!(after.snapshot.transactions.len() > transactions);
    assert_eq!(map(&subject(&after)).unwrap()["v"], A::n("7"));
    let retained = Retained::from_bundle(&bundle, &files).unwrap();
    for (id, object) in &retained.objects {
        assert_eq!(&after.history.object(id).unwrap(), object);
    }
    // No act was synthesized: the union adds exactly the contribution's missing objects.
    assert_eq!(after.object_count(), retained.objects.len());
    assert!(accepted(&after, &bundle, &files, &Files::new()).unwrap());
    // Publisher acceptance routes complete v3 here instead of the selective closure reader.
    let equivalent = |c: &Capture| {
        crate::pending_bundle::equivalent_node(&bundle, &files, c, c.document(), &Files::new())
    };
    assert!(!equivalent(&before).unwrap());
    assert!(equivalent(&after).unwrap());
    // Accepted input is a no-op, byte for byte.
    let settled = tree(&root);
    union_complete(&root, &bundle, &files, &mut || Ok(())).unwrap();
    assert_eq!(tree(&root), settled);
    let copy = P::export(&root).unwrap().reconstruct().unwrap();
    assert!(
        accepted(
            &Capture::read(copy.path()).unwrap(),
            &bundle,
            &files,
            &Files::new()
        )
        .unwrap()
    );
}

#[test]
fn crashed_union_recovers_through_the_compact_journal() {
    for phase in [
        P::Phase::Journal,
        P::Phase::Append(0),
        P::Phase::Evidence(0),
        P::Phase::Commit,
        P::Phase::View,
    ] {
        let (_temp, root) = target();
        let (bundle, files) = bundle();
        let prepared = prepare(&root, &bundle, &files).unwrap().unwrap();
        assert!(
            W::publish(&root, &prepared, None, |at| if at == phase {
                Err(crate::Error("crash".into()))
            } else {
                Ok(())
            })
            .is_err()
        );
        W::recover(&root, None).unwrap();
        W::publish(&root, &prepared, None, |_| Ok(())).unwrap();
        let after = Capture::read(&root).unwrap();
        assert!(accepted(&after, &bundle, &files, &Files::new()).unwrap());
    }
}

#[test]
fn scoped_private_and_tampered_bundles_refuse_before_writing() {
    let (_temp, root) = target();
    let (bundle, files) = bundle();
    let before = tree(&root);
    let subset = reseal(&bundle, |a| {
        a.insert("version".into(), A::n("2"));
    });
    assert!(
        code(union(&root, &subset, &files)).starts_with("complete_union_requires_full_history")
    );
    assert!(!is_complete(&subset).unwrap());
    // Version 3 without retained generations is not a valid artifact.
    let generations = reseal(&bundle, |a| {
        a.insert("version".into(), A::n("3"));
    });
    assert!(union(&root, &generations, &files).is_err());
    let private = reseal(&bundle, |a| {
        a.insert("shareability".into(), s("private"));
    });
    assert!(union(&root, &private, &files).is_err());
    let mut tampered = files.clone();
    let object = tampered
        .keys()
        .find(|p| p.starts_with("history-closure/objects/"))
        .unwrap()
        .clone();
    tampered.get_mut(&object).unwrap().extend(b"# tampered\n");
    assert!(union(&root, &bundle, &tampered).is_err());
    // A stale caller guard refuses after admission, before any durable write.
    assert_eq!(
        code(union_complete(&root, &bundle, &files, &mut || {
            Err(crate::Error("pending_changed".into()))
        })),
        "pending_changed"
    );
    assert_eq!(tree(&root), before);
}

#[test]
fn foreign_authority_and_collisions_fail_closed() {
    let (bundle, files) = bundle();
    // A different compact record, even with compatible content, is foreign authority.
    let other = tempfile::tempdir().unwrap();
    std::fs::create_dir(other.path().join(".kpopper")).unwrap();
    std::fs::write(other.path().join(".kpopper/history.yaml"), "version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: fixture\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
    std::fs::write(
        other.path().join("GROUNDING.yaml"),
        "meta:\n  purpose: Fixture\nschema:\n  deps: rests_on\n  snapshot: seen\n  predicate: wrong_if\nknown: {}\n",
    )
    .unwrap();
    let seed = W::prepare(
        other.path(),
        &value(json!({"kind":"add","id":"p.other","body":{"v":1}})),
        &A::Options {
            operation: "seed".into(),
            recorded_at: AT.into(),
            recording_day: "2026-09-25".into(),
            by: s("fixture"),
            strict: false,
            paths: crate::history_paths::Scheme::Hashed,
            receipt_version: None,
        },
        None,
    )
    .unwrap();
    W::publish(other.path(), &seed, None, |_| Ok(())).unwrap();
    let before = tree(other.path());
    assert_eq!(
        code(union(other.path(), &bundle, &files)),
        "complete_union_authority_mismatch"
    );
    assert_eq!(tree(other.path()), before);
    assert!(
        !accepted(
            &Capture::read(other.path()).unwrap(),
            &bundle,
            &files,
            &Files::new()
        )
        .unwrap()
    );
}

#[test]
fn noncanonical_incoming_source_order_is_imported_exactly() {
    let template = fixture_commit()["view_template"].clone();
    let input = step(
        "input",
        1,
        &[],
        vec![reading("p.input", "input", json!(1))],
        &template,
    );
    let extra = reading("p.extra", "extra", json!(3));
    let canonical =
        String::from_utf8(crate::history_emit::encode_document(&extra).unwrap()).unwrap();
    let printed = canonical.replace(
        "body:\n  scope:\n    environment: fixture\n    kind: project\n  v: 3\n",
        "body:\n  v: 3\n  scope:\n    environment: fixture\n    kind: project\n",
    );
    assert_ne!(printed, canonical);
    let mut next = step("extra", 1, &["input"], vec![], &template);
    next.objects = vec![(extra.clone(), Some(printed.clone().into_bytes()))];
    let witness = crate::history_node_source_order::encode(
        &crate::history_node_source_order::without_saw(
            Y::decode_source_document(printed.as_bytes()).unwrap(),
        )
        .unwrap(),
    );
    assert_ne!(witness, V::Null);

    let (_temp, root) = migrate(&legacy(1, std::slice::from_ref(&input)));
    let (bundle, files) = contribute(&legacy(1, &[input, next]));
    union(&root, &bundle, &files).unwrap();
    let copy = P::export(&root).unwrap().reconstruct().unwrap();
    for capture in [
        Capture::read(&root).unwrap(),
        Capture::read(copy.path()).unwrap(),
    ] {
        assert_eq!(
            capture.history.source_orders().get(&id_of(&extra)),
            Some(&witness)
        );
        assert!(accepted(&capture, &bundle, &files, &Files::new()).unwrap());
    }
}

#[test]
fn retained_generations_union_only_against_the_same_archived_generation() {
    let template = fixture_commit()["view_template"].clone();
    let old = |v: i64| {
        step(
            "g1",
            1,
            &[],
            vec![reading("p.old", "g1", json!(v))],
            &template,
        )
    };
    let current = step(
        "g2",
        2,
        &[],
        vec![reading("p.input", "g2", json!(2))],
        &template,
    );
    let next = step(
        "g3",
        2,
        &["g2"],
        vec![reading("p.extra", "g3", json!(3))],
        &template,
    );
    let (_temp, root) = migrate(&legacy(2, &[old(1), current.clone()]));
    let (bundle, files) = contribute(&legacy(2, &[old(1), current.clone(), next.clone()]));
    assert!(is_complete(&bundle).unwrap());
    // A different retained generation is never dropped or reconciled implicitly.
    let (other, other_files) = contribute(&legacy(2, &[old(9), current, next]));
    let before = tree(&root);
    assert!(
        code(union(&root, &other, &other_files)).starts_with("complete_union_generation_mismatch")
    );
    assert_eq!(tree(&root), before);
    assert!(
        !accepted(
            &Capture::read(&root).unwrap(),
            &other,
            &other_files,
            &Files::new()
        )
        .unwrap()
    );
    union(&root, &bundle, &files).unwrap();
    let after = Capture::read(&root).unwrap();
    assert!(accepted(&after, &bundle, &files, &Files::new()).unwrap());
}

#[test]
fn temporal_history_imports_preserve_original_observations() {
    let cache = tempfile::tempdir().unwrap();
    let runtime = runtime(cache.path());
    // Author typed temporal objects with the compact writer, then retain them as legacy history.
    let author = tempfile::tempdir().unwrap();
    std::fs::create_dir(author.path().join(".kpopper")).unwrap();
    std::fs::write(author.path().join(".kpopper/history.yaml"), "version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: authoring\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
    std::fs::write(
        author.path().join("GROUNDING.yaml"),
        Y::encode_document(&value(json!({"meta":{"purpose":"Fixture","reasoning":{"version":2,"profile":"core/v1","requires":["arithmetic/v1"]}},
            "schema":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"},"readings":{},"judgments":{}}))).unwrap(),
    )
    .unwrap();
    for (op, action) in [
        (
            "input",
            json!({"kind":"add","id":"p.input","into":"readings","body":{"v":1,"scope":scope()}}),
        ),
        (
            "judgment",
            json!({"kind":"add","id":"d.ready","into":"judgments","body":{"verdict":"ready","rests_on":["p.input"],
            "wrong_if":{"expr":"p.input > 5"},"temporal":{"version":1,"applicability":"current"},"scope":scope()}}),
        ),
        (
            "extra",
            json!({"kind":"add","id":"p.extra","into":"readings","body":{"v":3,"scope":scope()}}),
        ),
    ] {
        let options = A::Options {
            operation: op.into(),
            recorded_at: AT.into(),
            recording_day: "2026-09-25".into(),
            by: s("writer"),
            strict: false,
            paths: crate::history_paths::Scheme::Hashed,
            receipt_version: None,
        };
        let p = W::prepare(author.path(), &value(action), &options, Some(&runtime)).unwrap();
        W::publish(author.path(), &p, Some(&runtime), |_| Ok(())).unwrap();
    }
    let authored = Capture::read(author.path()).unwrap();
    let object = |subject: &str| {
        let (id, _) = authored
            .history
            .objects()
            .iter()
            .find(|(_, o)| string_is(&map(o).unwrap()["subject"], subject))
            .unwrap();
        authored.history.object(id).unwrap()
    };
    let mut document = authored.document().clone();
    let V::Map(d) = &mut document else { panic!() };
    let V::Map(meta) = d.get_mut("meta").unwrap() else {
        panic!()
    };
    meta.remove("node_history");
    meta.remove("node_publication");
    let template = Authority::document_template(&document).unwrap();
    let input = step("input", 1, &[], vec![object("p.input")], &template);
    let mut judgment = step(
        "judgment",
        1,
        &["input"],
        vec![object("d.ready")],
        &template,
    );
    judgment.requires = vec![crate::history_authority::TEMPORAL_APPLICABILITY];
    let extra = step(
        "extra",
        1,
        &["judgment"],
        vec![object("p.extra")],
        &template,
    );
    let steps = [input, judgment, extra];
    let (bundle, files) = contribute(&legacy(1, &steps));
    let original = crate::history_adapter::from_store_capture(
        &crate::pending_bundle::contribution_history(&bundle, &files).unwrap(),
    )
    .unwrap();
    let original_temporal = map(original.projection()).unwrap()["temporal"].clone();
    // Into a plain target (the union makes the world temporal) and into a temporal target.
    for held in [1, 2] {
        let (_temp, root) = migrate(&legacy(1, &steps[..held]));
        union(&root, &bundle, &files).unwrap();
        let after = Capture::read(&root).unwrap();
        assert!(accepted(&after, &bundle, &files, &Files::new()).unwrap());
        let temporal = after.temporal.clone().expect("temporal capture");
        for observation in list(&map(&original_temporal).unwrap()["observations"]).unwrap() {
            assert!(
                list(&map(&temporal).unwrap()["observations"])
                    .unwrap()
                    .contains(observation),
                "scoped temporal observation differs"
            );
        }
    }
}

#[test]
fn same_domain_contract_unions_and_different_or_missing_contracts_refuse() {
    let template = fixture_commit()["view_template"].clone();
    let input = step(
        "input",
        1,
        &[],
        vec![reading("p.input", "input", json!(1))],
        &template,
    );
    let unbound = legacy(1, std::slice::from_ref(&input));
    let (adopt, bound) = bind("bind", "input", &unbound, "1.0.0");
    let extra = step(
        "extra",
        1,
        &["bind"],
        vec![reading("p.extra", "extra", json!(3))],
        &bound,
    );
    let target_files = legacy(1, &[input.clone(), adopt.clone()]);
    let (bundle, files) = contribute(&legacy(1, &[input.clone(), adopt, extra]));

    let (_temp, root) = migrate(&target_files);
    union(&root, &bundle, &files).unwrap();
    let after = Capture::read(&root).unwrap();
    assert!(accepted(&after, &bundle, &files, &Files::new()).unwrap());
    let mut expected = package("1.0.0").0;
    expected.package_commit = Some("bind".into());
    assert_eq!(
        Domain::from_document(after.document()).unwrap(),
        Some(expected)
    );

    // A different contract version on the incoming side.
    let (other_adopt, other_bound) = bind("bind2", "input", &unbound, "2.0.0");
    let other_extra = step(
        "extra2",
        1,
        &["bind2"],
        vec![reading("p.extra", "extra2", json!(3))],
        &other_bound,
    );
    let (other, other_files) = contribute(&legacy(1, &[input, other_adopt, other_extra]));
    let (_temp2, root2) = migrate(&target_files);
    let before = tree(&root2);
    assert_eq!(
        code(union(&root2, &other, &other_files)),
        "incompatible_domain_profiles"
    );
    assert_eq!(tree(&root2), before);
    // A union never adopts a contract the target has not bound.
    let (_temp3, root3) = migrate(&unbound);
    let before = tree(&root3);
    assert_eq!(
        code(union(&root3, &bundle, &files)),
        "incompatible_domain_profiles"
    );
    assert_eq!(tree(&root3), before);
}

#[test]
fn full_materialization_keeps_original_authority_and_exact_objects() {
    let (bundle, files) = bundle();
    let out = tempfile::tempdir().unwrap();
    let document = materialize_complete(out.path(), &bundle, &files).unwrap();
    let captured = Capture::read(out.path()).unwrap();
    let source = crate::pending_bundle::contribution_history(&bundle, &files).unwrap();
    assert!(same_authority(&captured.snapshot.authority, &source.marker).unwrap());
    assert_eq!(&document, captured.document());
    for (id, object) in &source.objects {
        assert_eq!(captured.history.object(id).unwrap(), *object);
    }
    let before = tree(out.path());
    assert!(materialize_complete(out.path(), &bundle, &files).is_err());
    assert_eq!(tree(out.path()), before);
}
#[test]
fn full_materialization_refuses_tampered_evidence_before_writing() {
    let (bundle, mut files) = bundle();
    files.values_mut().next().unwrap().push(b' ');
    let out = tempfile::tempdir().unwrap();
    assert!(materialize_complete(out.path(), &bundle, &files).is_err());
    assert!(tree(out.path()).is_empty());
}
#[test]
fn full_materialization_retains_cancelled_generations_and_receipts() {
    let cases: J =
        serde_json::from_str(include_str!("../tests/fixtures/history-branch.json")).unwrap();
    let original = decoded(&cases[2]["files"]);
    let source_root = tempfile::tempdir().unwrap();
    for (path, raw) in &original {
        crate::history_transaction_fs::publish_immutable(source_root.path(), path, raw).unwrap();
    }
    let source = crate::history_store::Store::new(&source_root.path().join("GROUNDING.yaml"))
        .unwrap()
        .capture()
        .unwrap();
    let mut history_files = Files::from([
        ("entry.yaml".into(), source.entry_bytes.clone()),
        ("authority.yaml".into(), source.authority_bytes.clone()),
    ]);
    for (op, raw) in source.commits.iter().chain(
        source
            .inactive_generations
            .values()
            .flat_map(|g| g.commits.iter()),
    ) {
        history_files.insert(format!("commits/{op}.yaml"), raw.clone());
    }
    for (path, raw) in &source.storage_bytes {
        history_files.insert(format!("objects/{path}"), raw.clone());
    }
    for (path, raw) in &source.cancellation_bytes {
        history_files.insert(format!("cancellations/{path}"), raw.clone());
    }
    let mut required = BTreeSet::new();
    for raw in history_files.values() {
        let value = Y::decode_document(raw).unwrap();
        let mut pending = vec![&value];
        while let Some(value) = pending.pop() {
            match value {
                V::Map(m) => {
                    if let Some(file) = m.get("file") {
                        required.insert(text(file).unwrap().to_owned());
                    }
                    pending.extend(m.values());
                }
                V::List(v) => pending.extend(v),
                _ => {}
            }
        }
    }
    if let Some(import) = map(&map(&source.document).unwrap()["meta"])
        .unwrap()
        .get("history_import")
    {
        for member in list(&map(import).unwrap()["members"]).unwrap() {
            required.insert(text(&map(member).unwrap()["path"]).unwrap().to_owned());
        }
    }
    let evidence = required
        .iter()
        .map(|path| (path.clone(), original[path].clone()))
        .collect();
    let manifest = obj([
        ("version", A::n("3")),
        (
            "requires",
            V::List(vec![
                s("history-closure/v1"),
                s("history-generations/v1"),
                s("generation-cancellation/v1"),
                s(crate::history_paths::CAPABILITY),
            ]),
        ),
        (
            "roots",
            strings(
                source
                    .objects
                    .values()
                    .map(|o| text(&map(o).unwrap()["subject"]).unwrap().to_owned())
                    .collect::<BTreeSet<_>>(),
            ),
        ),
        (
            "scope",
            obj([("kind", s("project")), ("environment", s("fixture"))]),
        ),
        ("shareability", s("project")),
        ("rules", map(&source.state).unwrap()["rules"].clone()),
        ("baseline", source.baseline.clone()),
        (
            "files",
            V::Map(
                history_files
                    .iter()
                    .map(|(p, b)| (p.clone(), s(&sha256(b))))
                    .collect(),
            ),
        ),
        (
            "inactive_generations",
            V::Map(
                source
                    .inactive_generations
                    .iter()
                    .map(|(g, v)| (g.clone(), s(&v.digest)))
                    .collect(),
            ),
        ),
    ]);
    let artifact = obj([
        ("revision", s(&manifest.digest().unwrap())),
        ("manifest", manifest),
    ]);
    let (bundle, files) =
        crate::history_contribution_prepare::wrap(&artifact, &history_files, &evidence).unwrap();
    let output = tempfile::tempdir().unwrap();
    materialize_complete(output.path(), &bundle, &files).unwrap();
    let captured = Capture::read(output.path()).unwrap();
    assert!(!source.inactive_generations.is_empty());
    for (id, object) in source.objects.iter().chain(
        source
            .inactive_generations
            .values()
            .flat_map(|g| g.objects.iter()),
    ) {
        assert_eq!(
            captured
                .object(text(&map(object).unwrap()["subject"]).unwrap(), id)
                .unwrap(),
            *object
        );
    }
    let copy = P::export(output.path()).unwrap().reconstruct().unwrap();
    let detached = Capture::read(copy.path()).unwrap();
    for (id, object) in source
        .inactive_generations
        .values()
        .flat_map(|g| g.objects.iter())
    {
        assert_eq!(
            detached
                .object(text(&map(object).unwrap()["subject"]).unwrap(), id)
                .unwrap(),
            *object
        );
    }
}

#[test]
fn parallel_complete_imports_can_be_merged_without_aliasing_storage_operations() {
    let cache = tempfile::tempdir().unwrap();
    let runtime = runtime(cache.path());
    let (_temp, left) = target();
    let right = P::export(&left).unwrap().reconstruct().unwrap();
    for (root, id) in [(left.as_path(), "p.left"), (right.path(), "p.right")] {
        let options = A::Options {
            operation: id.replace('.', "-"),
            recorded_at: AT.into(),
            recording_day: "2026-09-25".into(),
            by: s("fixture"),
            strict: false,
            paths: crate::history_paths::Scheme::Hashed,
            receipt_version: None,
        };
        let p = W::prepare(
            root,
            &value(json!({"kind":"add", "id":id, "body":{"v":1}})),
            &options,
            Some(&runtime),
        )
        .unwrap();
        W::publish(root, &p, Some(&runtime), |_| Ok(())).unwrap();
    }
    let (bundle, files) = bundle();
    union_complete(&left, &bundle, &files, &mut || Ok(())).unwrap();
    union_complete(right.path(), &bundle, &files, &mut || Ok(())).unwrap();
    let p = crate::history_node_branch::prepare(
        &left,
        &[P::export(right.path()).unwrap()],
        "merge-parallel",
    )
    .unwrap();
    W::publish(&left, &p, None, |_| Ok(())).unwrap();
    let result = Capture::read(&left).unwrap();
    let incoming = crate::pending_bundle::contribution_history(&bundle, &files).unwrap();
    for (id, object) in incoming.objects {
        assert_eq!(result.history.object(&id).unwrap(), object);
    }
    assert!(
        crate::reasoning_snapshot::entries(result.document())
            .unwrap()
            .contains_key("p.left")
    );
    assert!(
        crate::reasoning_snapshot::entries(result.document())
            .unwrap()
            .contains_key("p.right")
    );
}

#[test]
fn paired_legacy_historical_assessment_survives_exact_full_union() {
    use crate::reasoning_snapshot::{CaptureOptions, Snapshot};
    let cases: J =
        serde_json::from_str(include_str!("../tests/fixtures/temporal-capture.json")).unwrap();
    let normalized = |case: &J| {
        case["files"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(path, raw)| {
                let path = if path == "GROUNDING.yaml" {
                    "entry.yaml".into()
                } else if path == ".kpopper/history.yaml" {
                    "authority.yaml".into()
                } else if let Some(p) = path.strip_prefix(".kpopper/history-commits/") {
                    format!("commits/{p}")
                } else if let Some(p) = path.strip_prefix(".kpopper/history/") {
                    format!("objects/{p}")
                } else {
                    panic!("unexpected fixture member {path}")
                };
                (path, raw.as_str().unwrap().as_bytes().to_vec())
            })
            .collect::<Files>()
    };
    let initial = normalized(&cases[0]);
    let full = normalized(&cases[2]);
    let old = crate::history_bundle::capture(&full, None).unwrap();
    let old_projection = crate::history_adapter::from_store_capture(&old).unwrap();
    let (_temp, target) = migrate(&initial);
    // This historical oracle predates contribution scope annotations. Replay its
    // exact existing history through the same conversion primitive, unchanged.
    let (_source_temp, source) = migrate(&full);
    let p = crate::history_node_branch::prepare(
        &target,
        &[P::export(&source).unwrap()],
        "full-temporal-union",
    )
    .unwrap();
    W::publish(&target, &p, None, |_| Ok(())).unwrap();
    let new = Capture::read(&target).unwrap();
    let new_projection = crate::history_node_projection::capture(&new).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let runtime = runtime(cache.path());
    let assess = |projection: &crate::history_projection::CapturedHistory| {
        let snapshot = Snapshot::from_data(
            projection.document(),
            CaptureOptions {
                context: Some(obj([
                    ("read_mode", s("supplied")),
                    ("source_collection", s("caller-owned")),
                    ("history", projection.projection().clone()),
                ])),
                as_of: Some(s("2026-09-20")),
                ..Default::default()
            },
        )
        .unwrap();
        crate::reasoning_history_assessment::assess(
            &snapshot,
            None,
            "focused-review/v1",
            Some(&runtime),
            crate::reasoning_runtime::OperationalBounds::default(),
            None,
        )
        .unwrap()
    };
    let before = assess(&old_projection);
    let after = assess(&new_projection);
    let before_subjects = &map(&before).unwrap()["history_subjects"];
    let after_subjects = &map(&after).unwrap()["history_subjects"];
    let old_temporal = map(&map(before_subjects).unwrap()["p.ready"]).unwrap()["temporal"].clone();
    let new_temporal = map(&map(after_subjects).unwrap()["p.ready"]).unwrap()["temporal"].clone();
    let old_episodes = list(&map(&old_temporal).unwrap()["episodes"]).unwrap();
    let new_episodes = list(&map(&new_temporal).unwrap()["episodes"]).unwrap();
    let key = |v: &V| {
        let m = map(v).unwrap();
        (m["operation"].clone(), m["phase"].clone())
    };
    for episode in old_episodes {
        let matches = new_episodes
            .iter()
            .filter(|e| key(e) == key(episode))
            .collect::<Vec<_>>();
        assert_eq!(matches, vec![episode]);
    }
    for episode in new_episodes
        .iter()
        .filter(|e| !old_episodes.iter().any(|o| key(o) == key(e)))
    {
        let e = map(episode).unwrap();
        let op = text(&e["operation"]).unwrap();
        assert!(op == "full-temporal-union" || op.starts_with("migration-"));
        assert_eq!(e["verification"], s("verified"));
        assert_eq!(e["evidence_kind"], s("reconstructed_committed_world"));
        assert_eq!(map(&e["result"]).unwrap()["status"], s("does_not_hold"));
    }
    let mut normalized = after_subjects.clone();
    let temporal = crate::history_view::map_mut(
        crate::history_view::map_mut(
            crate::history_view::map_mut(&mut normalized)
                .unwrap()
                .get_mut("p.ready")
                .unwrap(),
        )
        .unwrap()
        .get_mut("temporal")
        .unwrap(),
    )
    .unwrap();
    temporal.insert("episodes".into(), V::List(old_episodes.to_vec()));
    assert_eq!(&normalized, before_subjects);
    let copy = P::export(&target).unwrap().reconstruct().unwrap();
    let copy_projection =
        crate::history_node_projection::capture(&Capture::read(copy.path()).unwrap()).unwrap();
    assert_eq!(
        &map(&assess(&copy_projection)).unwrap()["history_subjects"],
        after_subjects
    );
    let original_observations =
        list(&map(&map(old_projection.projection()).unwrap()["temporal"]).unwrap()["observations"])
            .unwrap();
    let observations =
        list(&map(&map(new_projection.projection()).unwrap()["temporal"]).unwrap()["observations"])
            .unwrap();
    for original in original_observations {
        assert_eq!(
            observations
                .iter()
                .filter(|o| key(o) == key(original))
                .collect::<Vec<_>>(),
            vec![original]
        );
    }
}

#[test]
fn reserved_evidence_names_record_storage_and_git_control_case_insensitively() {
    for path in [
        ".kpopper/history-commits/x.yaml",
        ".kpopper/history/p.x/0.yaml",
        ".KPOPPER/hypotheses/x.yaml",
        ".kpopper/project.json",
        ".kpopper-history-migration/op/GROUNDING.yaml",
        "GROUNDING.yaml",
        "Provenance.yaml",
        "docs/.gitattributes",
        ".gitignore",
    ] {
        assert!(reserved_evidence(path), "{path}");
    }
    for path in [
        "evidence/vendor.txt",
        "evidence/reports/report.txt",
        "docs/kpopper.txt",
        "docs/GROUNDING.yaml.txt",
    ] {
        assert!(!reserved_evidence(path), "{path}");
    }
}

/// A verified reading whose body names locator evidence at `locator`.
fn located_reading(locator: &str) -> V {
    let mut o = json!({"authored":{"collection":"readings","fields":{"deps":"rests_on","predicate":"wrong_if","snapshot":"seen","value":"v"},"profile":"core/v1"},
        "body":{"scope":scope(),"v":1,"note":{"file":locator}},"by":"writer","id_scheme":"typed-history/v2","kind":"reading","on":"2026-09-17",
        "op":"input","pins":{},"saw":[],"schema_version":2,"subject":"p.input"});
    o["id"] = json!(crate::identity::typed_object_identity(&value(o.clone())).unwrap());
    value(o)
}

#[test]
fn locator_evidence_at_record_control_paths_is_archived_and_never_becomes_history() {
    let template = fixture_commit()["view_template"].clone();
    for (locator, raw) in [
        // A physical layer the legacy reader would otherwise import as a named proposal.
        (
            ".kpopper/hypotheses/planted.yaml",
            b"hypothesis: {claim: planted}\nreadings:\n  p.input: {v: 99}\n".to_vec(),
        ),
        // A replaced-version sidecar the legacy reader would otherwise retain as evidence.
        (
            ".kpopper/replaced.yaml",
            b"p.input:\n- v: 0\n  ended: planted\n".to_vec(),
        ),
        // Project configuration.
        (
            ".kpopper/project.json",
            b"{\"version\":1,\"mode\":\"advanced\"}".to_vec(),
        ),
    ] {
        let object = located_reading(locator);
        let history = legacy(1, &[step("input", 1, &[], vec![object.clone()], &template)]);
        let evidence = Files::from([(locator.to_owned(), raw.clone())]);
        let (bundle, files) = contribute_with(&history, &evidence);
        let revision = text(&map(&bundle).unwrap()["revision"])
            .unwrap()
            .to_owned();
        let out = tempfile::tempdir().unwrap();
        materialize_complete(out.path(), &bundle, &files)
            .unwrap_or_else(|e| panic!("{locator}: {e}"));
        // Retained exactly in archive space; never an active storage or control file.
        assert!(
            out.path().join(locator).symlink_metadata().is_err(),
            "{locator} became active"
        );
        assert_eq!(
            std::fs::read(
                out.path()
                    .join(format!(".kpopper-contributions/{revision}/evidence/{locator}"))
            )
            .unwrap(),
            raw,
            "{locator}"
        );
        // Exact membership: only the verified reading, with its locator unchanged.
        let capture = Capture::read(out.path()).unwrap();
        assert_eq!(
            capture.history.objects().keys().collect::<Vec<_>>(),
            vec![&id_of(&object)],
            "{locator}"
        );
        assert_eq!(capture.object("p.input", &id_of(&object)).unwrap(), object);
        assert_eq!(
            map(&subject(&capture)).unwrap()["note"],
            value(json!({"file": locator}))
        );
    }
}

#[test]
fn actual_legacy_import_bound_originals_survive_full_materialization() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source");
    std::fs::create_dir_all(source.join(".kpopper")).unwrap();
    std::fs::write(source.join("GROUNDING.yaml"),
        "known:\n  p.input: {v: 1, scope: {kind: project, environment: fixture}}\n").unwrap();
    std::fs::write(source.join(".kpopper/replaced.yaml"),
        "p.input:\n- {v: 0, scope: {kind: project, environment: fixture}, ended: superseded, day: 2026-09-20}\n").unwrap();
    let original = temporary.path().join("legacy");
    crate::history_migration::Plan::prepare(&source.join("GROUNDING.yaml"), &source,
        crate::history_migration::Options {
            operation: "import-originals".into(), recorded_at: AT.into(),
            record_id: Some("bound-originals".into()), read_mode: crate::source_capture::ReadMode::Frozen,
            route: false, as_of: None,
        }, None).unwrap().publish(&original).unwrap();
    let held = crate::history_store::Store::new(&original.join("GROUNDING.yaml")).unwrap().capture().unwrap();
    let mut core = Files::from([("entry.yaml".into(), held.entry_bytes.clone()),
        ("authority.yaml".into(), held.authority_bytes.clone())]);
    for (operation, bytes) in &held.commits { core.insert(format!("commits/{operation}.yaml"), bytes.clone()); }
    for (path, bytes) in &held.storage_bytes { core.insert(format!("objects/{path}"), bytes.clone()); }
    let members = super::bound_members(&held).unwrap();
    assert!(!members.is_empty());
    let mut required = members.keys().cloned().collect::<BTreeSet<_>>();
    for bytes in core.values() {
        required.extend(crate::pending_bundle::required_files(&Y::decode_document(bytes).unwrap()).unwrap());
    }
    let evidence = required.into_iter().map(|path| {
        let raw = std::fs::read(original.join(&path)).unwrap(); (path, raw)
    }).collect();
    let (bundle, files) = contribute_with(&core, &evidence);
    let output = temporary.path().join("output");
    std::fs::create_dir(&output).unwrap();
    materialize_complete(&output, &bundle, &files).unwrap();
    let copied = Capture::read(&output).unwrap();
    assert_eq!(copied.history.objects().keys().collect::<Vec<_>>(), held.objects.keys().collect::<Vec<_>>());
    for (id, object) in &held.objects { assert_eq!(copied.history.object(id).unwrap(), *object); }
    let archived = copied.snapshot.legacy.values().next().unwrap();
    assert_eq!(archived.captured.commits, held.commits);
}

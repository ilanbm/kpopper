use super::*;
use crate::{
    history_authoring::Options,
    history_node_capture::Capture,
    history_node_writer as W,
    history_paths::Scheme,
    history_yaml as Y,
    reasoning_runtime::{OperationalBounds, Runtime},
    reasoning_snapshot::{CaptureOptions, Snapshot},
};
use std::{fs, path::Path};

fn retain(root: &Path, files: &serde_json::Value) {
    for (name, raw) in files.as_object().unwrap() {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, raw.as_str().unwrap()).unwrap();
    }
}
fn runtime(root: &Path) -> Runtime {
    Runtime::open(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!(
                "{}.kpopper-runtime",
                crate::reasoning_runtime::target_name().unwrap()
            )),
        root,
        OperationalBounds::default(),
    )
    .unwrap()
}
fn write(root: &Path, runtime: &Runtime, operation: &str, action: serde_json::Value) {
    let options = Options {
        operation: operation.into(),
        recorded_at: "2026-09-25T12:00:00Z".into(),
        recording_day: "2026-09-25".into(),
        by: s("template-test"),
        strict: false,
        paths: Scheme::Hashed,
        receipt_version: None,
    };
    let prepared = W::prepare(
        root,
        &V::from_json(&action).unwrap(),
        &options,
        Some(runtime),
    )
    .unwrap();
    W::publish(root, &prepared, Some(runtime), |_| Ok(())).unwrap();
}
fn assess(history: &crate::history_projection::CapturedHistory, runtime: &Runtime) -> V {
    let snapshot = Snapshot::from_data(
        history.document(),
        CaptureOptions {
            context: Some(V::Map(Map::from([
                ("read_mode".into(), s("supplied")),
                ("source_collection".into(), s("caller-owned")),
                ("history".into(), history.projection().clone()),
            ]))),
            as_of: Some(s("2026-09-20")),
            ..Default::default()
        },
    )
    .unwrap();
    crate::reasoning_history_assessment::assess(
        &snapshot,
        None,
        "focused-review/v1",
        Some(runtime),
        OperationalBounds::default(),
        None,
    )
    .unwrap()
}
fn preserves(original: &V, current: &V, rows: &str) {
    let current = crate::history_view::list(&map(current).unwrap()[rows]).unwrap();
    for row in crate::history_view::list(&map(original).unwrap()[rows]).unwrap() {
        let fields = map(row).unwrap();
        let found = current
            .iter()
            .filter(|candidate| {
                let candidate = map(candidate).unwrap();
                candidate["operation"] == fields["operation"]
                    && candidate["phase"] == fields["phase"]
            })
            .collect::<Vec<_>>();
        assert_eq!(found, vec![row]);
    }
}
#[test]
fn legacy_template_derivation_uses_the_verified_manifest() {
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/temporal-capture.json")).unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source");
    let target = temporary.path().join("target");
    retain(&source, &fixtures[2]["files"]);
    crate::history_node_migration::Plan::prepare(&source.join("GROUNDING.yaml"))
        .unwrap()
        .publish(&target)
        .unwrap();
    let captured = Capture::read(&target).unwrap();
    let original = crate::history_store::Store::new(&source.join("GROUNDING.yaml"))
        .unwrap()
        .capture()
        .unwrap();
    for (operation, raw) in &original.commits {
        let manifest = Y::decode_document(raw).unwrap();
        assert_eq!(
            after_template(&captured.snapshot, operation).unwrap(),
            map(&manifest).unwrap()["view_template"],
            "legacy operation {operation}"
        );
    }
}
#[test]
fn native_write_on_migrated_target_then_full_union_uses_original_templates() {
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/temporal-capture.json")).unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let initial = temporary.path().join("initial");
    let source = temporary.path().join("source");
    retain(&initial, &fixtures[0]["files"]);
    retain(&source, &fixtures[2]["files"]);
    let target = temporary.path().join("target");
    let incoming = temporary.path().join("incoming");
    crate::history_node_migration::Plan::prepare(&initial.join("GROUNDING.yaml"))
        .unwrap()
        .publish(&target)
        .unwrap();
    crate::history_node_migration::Plan::prepare(&source.join("GROUNDING.yaml"))
        .unwrap()
        .publish(&incoming)
        .unwrap();
    let original = crate::history_store::Store::new(&source.join("GROUNDING.yaml"))
        .unwrap()
        .capture()
        .unwrap();
    let original_projection = crate::history_adapter::from_store_capture(&original).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let runtime = runtime(cache.path());
    write(
        &target,
        &runtime,
        "native-before",
        serde_json::json!({"kind":"add","id":"p.local","into":"readings","body":{"v":4}}),
    );
    let union = crate::history_node_branch::prepare(
        &target,
        &[P::export(&incoming).unwrap()],
        "mixed-union",
    )
    .unwrap();
    W::publish(&target, &union, Some(&runtime), |_| Ok(())).unwrap();
    write(
        &target,
        &runtime,
        "native-after",
        serde_json::json!({"kind":"set","id":"p.local","value":5}),
    );
    let captured = Capture::read(&target).unwrap();
    let projection = crate::history_node_projection::capture(&captured).unwrap();
    for (id, object) in &original.objects {
        assert_eq!(
            captured
                .object(text(&map(object).unwrap()["subject"]).unwrap(), id)
                .unwrap(),
            *object
        );
    }
    preserves(
        &map(original_projection.projection()).unwrap()["temporal"],
        &map(projection.projection()).unwrap()["temporal"],
        "observations",
    );
    let old_report = assess(&original_projection, &runtime);
    let new_report = assess(&projection, &runtime);
    let temporal = |report: &V| {
        map(&map(&map(report).unwrap()["history_subjects"]).unwrap()["p.ready"]).unwrap()["temporal"].clone()
    };
    let before = temporal(&old_report);
    let after = temporal(&new_report);
    assert_eq!(
        map(&before).unwrap()["status"],
        map(&after).unwrap()["status"]
    );
    preserves(&before, &after, "episodes");
    let copy = P::export(&target).unwrap().reconstruct().unwrap();
    let rebuilt = Capture::read(copy.path()).unwrap();
    assert_eq!(
        assess(
            &crate::history_node_projection::capture(&rebuilt).unwrap(),
            &runtime
        ),
        new_report
    );
    let mut role = captured.snapshot.clone();
    let mut document = Y::decode_document(role.current.as_deref().unwrap()).unwrap();
    map_mut(map_mut(&mut document).unwrap().get_mut("schema").unwrap())
        .unwrap()
        .insert("deps".into(), s("different_dependencies"));
    role.current = Some(Y::encode_document(&document).unwrap());
    assert!(
        Capture::from_union(role).is_err(),
        "incompatible authored roles must refuse"
    );
    let mut metadata = captured.snapshot.clone();
    let mut document = Y::decode_document(metadata.current.as_deref().unwrap()).unwrap();
    map_mut(map_mut(&mut document).unwrap().get_mut("meta").unwrap())
        .unwrap()
        .insert("schema".into(), s("other metadata"));
    metadata.current = Some(Y::encode_document(&document).unwrap());
    assert!(
        Capture::from_union(metadata).is_err(),
        "non-role metadata must remain exact"
    );
    let mut profile = captured.snapshot.clone();
    let mut document = Y::decode_document(profile.current.as_deref().unwrap()).unwrap();
    map_mut(
        map_mut(map_mut(&mut document).unwrap().get_mut("meta").unwrap())
            .unwrap()
            .get_mut("reasoning")
            .unwrap(),
    )
    .unwrap()
    .insert("profile".into(), s("incompatible/v1"));
    profile.current = Some(Y::encode_document(&document).unwrap());
    assert!(
        Capture::from_union(profile).is_err(),
        "incompatible profile must refuse"
    );
    // Both the temporal reader and later compact template deltas must use
    // the same verified original manifest, even if receipt.after is empty.
    for (operation, raw) in &original.commits {
        let manifest = Y::decode_document(raw).unwrap();
        assert_eq!(
            after_template(&captured.snapshot, operation).unwrap(),
            map(&manifest).unwrap()["view_template"],
            "legacy operation {operation}"
        );
    }
}

#[test]
fn empty_world_does_not_infer_unwitnessed_role_equivalence() {
    let objects = Map::new();
    let state = crate::history_reduce::reduce_bytes(&Default::default(), None, None).unwrap();
    let implicit = V::from_json(&serde_json::json!({"meta":{},"readings":{}})).unwrap();
    let explicit = V::from_json(&serde_json::json!({"meta":{},"readings":{},
        "schema":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"}}))
    .unwrap();
    assert_ne!(
        template_in_world(&implicit, &objects, &state).unwrap(),
        template_in_world(&explicit, &objects, &state).unwrap()
    );
}

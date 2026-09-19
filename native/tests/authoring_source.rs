mod support;
use kpop_native::{authoring_source::AuthoringSource, value::TypedValue as V};
use support::pending::fixture;

#[test]
fn actual_pending_disposition_controls_profile_preparation() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/pending-state.json")).unwrap();
    let action=V::from_json(&serde_json::json!({"kind":"add","id":"p.new","body":{"v":2},"profile":"core/v1","as_of":"2020-01-01"})).unwrap();
    for scenario in ["one", "accepted", "retired", "unrelated"] {
        let case = cases
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == scenario)
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fixture(&root, case);
        let entry = root.join(case["entry"].as_str().unwrap());
        let result = AuthoringSource::capture(&[entry], &root, &action, None, None);
        if ["one", "accepted"].contains(&scenario) {
            let error = match result {
                Err(e) => e,
                Ok(_) => panic!("promoted over active pending {scenario}"),
            };
            assert!(
                error
                    .0
                    .starts_with("pending_profile_reconciliation_required"),
                "{scenario}: {error}"
            );
        } else {
            let mut prepared = result.unwrap_or_else(|e| panic!("{scenario}: {e}"));
            assert!(prepared.world().is_some());
            let V::Map(data) = prepared.source().snapshot().to_data() else {
                unreachable!()
            };
            assert_eq!(data["as_of"], V::Null);
            prepared.verify().unwrap();
        }
    }
}

#[cfg(unix)]
#[test]
fn writer_source_holds_policy_and_rechecks_exact_bytes() {
    use fs2::FileExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let entry = root.join("GROUNDING.yaml");
    std::fs::write(&entry, "known: {p.a: {v: 1}}\n").unwrap();
    let action = V::from_json(&serde_json::json!({"kind":"set","id":"p.a","value":2})).unwrap();
    let mut prepared =
        AuthoringSource::capture(std::slice::from_ref(&entry), &root, &action, None, None).unwrap();
    assert!(prepared.world().is_none());
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join(".kpopper/project.lock"))
        .unwrap();
    assert!(FileExt::try_lock_exclusive(&lock).is_err());
    std::fs::write(&entry, "# same meaning\nknown: {p.a: {v: 1}}\n").unwrap();
    assert_eq!(prepared.verify().unwrap_err().0, "snapshot_changed");
    drop(prepared);
    FileExt::try_lock_exclusive(&lock).unwrap();
}

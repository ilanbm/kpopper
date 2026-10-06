use chrono::{TimeZone, Utc};
use kpop_native::followup_store::{Store, digest};
use serde_json::json;
use std::fs;
#[test]
fn an_interrupted_unreferenced_segment_can_be_recovered_from_retained_hot_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    fs::create_dir(&workspace).unwrap();
    fs::write(
        workspace.join("PROVENANCE.yaml"),
        "meta:\n  name: Retention\nknown:\n  source.value:\n    name: Source value\n    v: 1\n",
    )
    .unwrap();
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &temp.path().join("state"), now).unwrap();
    store.setup(None, "UTC", None, true).unwrap();
    let declaration = json!({"schema":"kpopper.maintenance-declaration/v1","kind":"source","id":"source-check","title":"Source check","why":"Inspect source","how":"Read source","scope":"Read source only","related":["source.value"],"cadence_days":1,"timezone":"UTC","check_time":"09:00","use_policy":"allow_cached_until_expiry","evidence_requirement":"host_attested","first_due_at":"2026-10-06T12:00:00Z","source":{"source_id":"source","locator":"https://example.test/status","publisher":"Example","selection":"Status","adapter":"host/v1","evidence_format":"text","tool_policy":"authorized read tool","allowed_roots":["https://example.test/"],"max_age_hours":24}});
    let spec = kpop_native::maintenance_contract::compile(&declaration).unwrap()["spec"].clone();
    store.add(spec.clone()).unwrap();
    let report = json!({"id":"source-check","policy_digest":spec["maintenance"]["policy_digest"],"source_ref":spec["maintenance"]["source_ref"],"evidence":"fixture://success","value":"current","inspection":spec["maintenance"]["inspection"],"inspected_at":"2026-10-06T12:00:00Z"});
    let mut attempt = serde_json::Value::Null;
    for _ in 0..66 {
        attempt = store.inspect_maintenance(report.clone()).unwrap()["attempt"].clone();
    }
    let data = store.load(true).unwrap().unwrap();
    let mut attempts = data["items"]["source-check"]["attempts"]
        .as_array()
        .unwrap()
        .clone();
    attempts.push(attempt);
    let segment = json!({"schema":"kpopper.followup-attempts/v1","workspace_key":data["workspace_key"],"id":"source-check","previous":null,"attempts":attempts[..35]});
    let hash = digest(&segment).unwrap();
    let directory = store.root.join("attempt-history");
    fs::create_dir(&directory).unwrap();
    let path = directory.join(format!("{hash}.json"));
    fs::write(&path, b"{\"schema\":").unwrap();
    let result = store.inspect_maintenance(report);
    assert!(
        result.is_ok(),
        "recoverable hot evidence must not permanently block recurrence: {result:?}"
    );
    let retained = kpop_native::followup_store::parse_input(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(digest(&retained).unwrap(), hash);
    assert!(
        fs::read_dir(directory)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains(".corrupt-"))
    );
}

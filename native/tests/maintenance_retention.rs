use chrono::{TimeZone, Utc};
use kpop_native::followup_store::{Store, digest};
use serde_json::json;
use std::fs;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::path::Path;

#[cfg(windows)]
fn windows_path_units(path: &Path) -> usize {
    path.as_os_str().encode_wide().count()
}

#[test]
fn an_interrupted_unreferenced_segment_can_be_recovered_from_retained_hot_evidence() {
    #[cfg(windows)]
    let temp = {
        let system_temp = std::env::temp_dir();
        let current = std::env::current_dir().unwrap();
        let short_parent = if windows_path_units(&current) < windows_path_units(&system_temp) {
            current
        } else {
            system_temp
        };
        tempfile::tempdir_in(short_parent).unwrap()
    };
    #[cfg(not(windows))]
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    fs::create_dir(&workspace).unwrap();
    fs::write(
        workspace.join("PROVENANCE.yaml"),
        "meta:\n  name: Retention\nknown:\n  source.value:\n    name: Source value\n    v: 1\n",
    )
    .unwrap();
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let initial_state_home = temp.path().join("state");
    let initial_store = Store::at_in_state(&workspace, &initial_state_home, now).unwrap();
    #[cfg(windows)]
    let store = {
        // Put the normal segment path at a fixed distance below MAX_PATH,
        // independent of the runner's username and TEMP directory length.
        let ordinary_suffix = initial_store
            .root
            .strip_prefix(&initial_state_home)
            .unwrap()
            .join("attempt-history")
            .join(format!("{}.json", "0".repeat(64)));
        // The eventual path contains a separator before and after the state
        // component, plus at least one character in that component.
        let fixed_prefix_units = windows_path_units(temp.path()) + 2;
        let base_units = fixed_prefix_units + windows_path_units(&ordinary_suffix);
        let target_ordinary_units: usize = (base_units + 1).max(245);
        assert!(
            target_ordinary_units < 260,
            "short temp root still leaves no room below MAX_PATH: {} units before state padding",
            base_units
        );
        let padding_units = target_ordinary_units - base_units;
        assert!(padding_units <= 255, "state padding exceeds a Windows path component");
        let state_home = temp.path().join("s".repeat(padding_units));
        let store = Store::at_in_state(&workspace, &state_home, now).unwrap();
        assert_eq!(
            windows_path_units(&store.root.join("attempt-history").join(format!("{}.json", "0".repeat(64)))),
            target_ordinary_units
        );
        store
    };
    #[cfg(not(windows))]
    let store = initial_store;
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
    #[cfg(windows)]
    {
        // The regular digest name fits MAX_PATH here; the quarantine suffix
        // crosses it. Keep this regression tied to the path operation that
        // needs the Windows extended-length form.
        let quarantine = directory.join(format!("{hash}.corrupt-{}.json", "0".repeat(36)));
        assert!(
            windows_path_units(&path) < 260,
            "regular digest path must fit MAX_PATH: {}",
            path.display()
        );
        assert!(
            windows_path_units(&quarantine) > 260,
            "fixture must exercise a quarantine path beyond MAX_PATH: {}",
            quarantine.display()
        );
    }
    let result = store.inspect_maintenance(report);
    assert!(
        result.is_ok(),
        "recoverable hot evidence must not permanently block recurrence at {}: {result:?}",
        directory.display()
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

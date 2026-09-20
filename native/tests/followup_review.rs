use chrono::{TimeZone, Utc};
use kpop_native::{followup_store::Store, followup_triggers as T};
use serde_json::{Value as J, json};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn resources() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    fs::create_dir_all(temp.path().join("reasoning")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.zip")),
        temp.path().join("reasoning").join(format!("{target}.zip")),
    )
    .unwrap();
    let source = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap())
                .join(".cache/kpopper/lean")
                .join(&target)
                .join(env!("KPOP_ORDINARY_SOURCE_SHA256"))
        });
    let dest = temp.path().join("ordinary").join(target);
    fs::create_dir_all(&dest).unwrap();
    for name in [
        "build.json",
        if cfg!(windows) {
            "epistemic-core.exe"
        } else {
            "epistemic-core"
        },
    ] {
        fs::copy(source.join(name), dest.join(name)).unwrap();
    }
    temp
}

fn invoke(
    root: &Path,
    state: &Path,
    runtime: &Path,
    args: &[&str],
    python: Option<(&Path, &Path)>,
) -> Output {
    let mut cmd = if let Some((python, oracle)) = python {
        let mut c = Command::new(python);
        c.arg(oracle.join("scripts/kpopper"));
        c
    } else {
        Command::new(env!("CARGO_BIN_EXE_kpop-native"))
    };
    cmd.current_dir(root)
        .arg("followups")
        .args(args)
        .env("XDG_STATE_HOME", state)
        .env("KPOPPER_NATIVE_RESOURCES", runtime)
        .env("KPOPPER_NATIVE_CACHE", runtime.join("cache"))
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .output()
        .unwrap()
}
fn success(out: Output) -> J {
    assert!(
        out.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn setup(root: &Path, state: &Path, runtime: &Path, case: &J) -> J {
    fs::write(
        root.join("GROUNDING.yaml"),
        case["record"].as_str().unwrap(),
    )
    .unwrap();
    success(invoke(
        root,
        state,
        runtime,
        &["setup", "--private", "--timezone", "UTC"],
        None,
    ));
    let related = case["values"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let spec = json!({"id":"watch","title":"Check the recorded basis","why":"Keep the evidence current","how":"Inspect the record","scope":"Read only","related":related,"when":{"changed":related[0]}});
    let file = root.join("spec.json");
    fs::write(&file, serde_json::to_vec(&spec).unwrap()).unwrap();
    success(invoke(
        root,
        state,
        runtime,
        &["add", "--file", file.to_str().unwrap()],
        None,
    ))
}

#[test]
fn native_baselines_match_all_python_graph_values_and_reject_tampering() {
    let cases: J =
        serde_json::from_str(include_str!("fixtures/followup-graph-oracle.json")).unwrap();
    let runtime = resources();
    for (name, case) in cases.as_object().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        fs::create_dir(&root).unwrap();
        let state = temp.path().join("state");
        let added = setup(&root, &state, runtime.path(), case);
        assert_eq!(
            added["baseline"], case["values"],
            "{name}: complete retained readings"
        );
        let scan = success(invoke(&root, &state, runtime.path(), &["scan"], None));
        assert_eq!(scan["items"][0]["state"], "waiting", "{name}: {scan}");
        if name == "core_value" {
            let store = Store::at_in_state(
                &root,
                &state,
                Utc.with_ymd_and_hms(2026, 9, 20, 0, 0, 0).unwrap(),
            )
            .unwrap();
            let mut ledger = store.load(true).unwrap().unwrap();
            ledger["items"]["watch"]["baseline"]["p.x"]["core"]["value"]["numerator"] =
                json!("999");
            fs::write(&store.path, serde_json::to_vec(&ledger).unwrap()).unwrap();
            let scan = success(invoke(&root, &state, runtime.path(), &["scan"], None));
            assert_eq!(scan["items"][0]["state"], "unknown", "{scan}");
        }
        if name == "computed" {
            fs::write(
                root.join("GROUNDING.yaml"),
                case["record"]
                    .as_str()
                    .unwrap()
                    .replace("p.a + p.b", "p.a + 4"),
            )
            .unwrap();
            // The watched first id is m.total; only the formula changed.
            let scan = success(invoke(&root, &state, runtime.path(), &["scan"], None));
            assert_eq!(scan["items"][0]["state"], "ready", "{scan}");
        }
    }
}

#[test]
#[ignore = "requires explicit immutable KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT"]
fn python_and_native_can_scan_each_others_complete_baselines() {
    let python =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("set oracle Python"));
    let oracle = PathBuf::from(
        std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("set immutable oracle root"),
    );
    let cases: J =
        serde_json::from_str(include_str!("fixtures/followup-graph-oracle.json")).unwrap();
    let runtime = resources();
    for (name, case) in cases.as_object().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        fs::create_dir(&root).unwrap();
        let state = temp.path().join("state");
        setup(&root, &state, runtime.path(), case);
        let native = success(invoke(&root, &state, runtime.path(), &["scan"], None));
        let py = success(invoke(
            &root,
            &state,
            runtime.path(),
            &["scan"],
            Some((&python, &oracle)),
        ));
        assert_eq!(native, py, "{name}: Python scan of native ledger");
        // Restore a Python-captured baseline verbatim into the same durable ledger.
        let store = Store::at_in_state(
            &root,
            &state,
            Utc.with_ymd_and_hms(2026, 9, 20, 0, 0, 0).unwrap(),
        )
        .unwrap();
        let mut ledger = store.load(true).unwrap().unwrap();
        ledger["items"]["watch"]["baseline"] = case["values"].clone();
        fs::write(&store.path, serde_json::to_vec(&ledger).unwrap()).unwrap();
        let native = success(invoke(&root, &state, runtime.path(), &["scan"], None));
        let py = success(invoke(
            &root,
            &state,
            runtime.path(),
            &["scan"],
            Some((&python, &oracle)),
        ));
        assert_eq!(native, py, "{name}: native scan of Python baseline");
    }
}

#[test]
fn timestamp_grammar_and_age_bounds_match_python() {
    for (input, zone, expected) in [
        ("2027-01-01T09:00+02:00", "UTC", "2027-01-01T07:00:00Z"),
        (
            "2027-01-01t09:00:00.1z",
            "UTC",
            "2027-01-01T09:00:00.100000Z",
        ),
        (
            "2027-01-01 09:00:00.123000+00:00",
            "UTC",
            "2027-01-01T09:00:00.123000Z",
        ),
        ("2011-12-30", "Pacific/Apia", "2011-12-30T10:00:00Z"),
    ] {
        assert_eq!(T::stamp(T::parse_time(input, zone).unwrap()), expected);
    }
    for bad in [
        "2026-09-10T12:00:00.123456789Z",
        "2027-01-01T09:00:60Z",
        "0000-01-01",
    ] {
        assert!(T::parse_time(bad, "UTC").is_err(), "{bad}");
    }
    let excessive = json!({"external":{"ref":"release","equals":true,"max_age_hours":1.0e18}});
    assert_eq!(
        T::validate(&excessive, &[]).unwrap_err().0,
        "max_age_hours is too large"
    );
    let large = json!({"external":{"ref":"release","equals":true,"max_age_hours":1.0e10}});
    T::validate(&large, &[]).unwrap();
    let observations = serde_json::Map::from_iter([(
        "release".into(),
        json!({"value":true,"observed_at":"2026-09-01T00:00Z","evidence":"captured"}),
    )]);
    let evaluation = T::evaluate(
        &large,
        &Default::default(),
        &Default::default(),
        &BTreeSet::new(),
        &observations,
        Utc.with_ymd_and_hms(2026, 9, 20, 0, 0, 0).unwrap(),
        "UTC",
        &BTreeSet::new(),
        &BTreeSet::new(),
    )
    .unwrap();
    assert_eq!(evaluation.value, None);
    assert_eq!(evaluation.inputs["external"]["0"]["status"], "invalid");
    assert!(!T::unavailable(Some(&json!({"alpha":1})), false));
}

#[test]
fn writes_keep_float_types_and_refuse_invalid_times_and_unbounded_ages() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("GROUNDING.yaml"), "known:\n  p.x: {v: 1}\n").unwrap();
    let state = temp.path().join("state");
    let now = Utc.with_ymd_and_hms(2026, 9, 20, 0, 0, 0).unwrap();
    let store = Store::at_in_state(&root, &state, now).unwrap();
    store.setup(None, "UTC", None, true).unwrap();
    store.observe(json!({"ref":"large","value":1.0e20,"observed_at":"2026-09-19T12:00Z","evidence":"reading 1e20 is a string here"})).unwrap();
    let yaml = kpop_native::history_yaml::decode_document(&fs::read(&store.path).unwrap()).unwrap();
    let kpop_native::value::TypedValue::Map(values) = yaml else {
        panic!("ledger must be a map")
    };
    let kpop_native::value::TypedValue::Map(observations) = &values["observations"] else {
        panic!("observations must be a map")
    };
    let kpop_native::value::TypedValue::Map(observation) = &observations["large"] else {
        panic!("observation must be a map")
    };
    assert!(matches!(
        observation["value"],
        kpop_native::value::TypedValue::Float(_)
    ));
    assert_eq!(
        store.load(true).unwrap().unwrap()["observations"]["large"]["evidence"],
        "reading 1e20 is a string here"
    );
    assert_eq!(
        kpop_native::followup_store::digest(
            &json!({"tiny":1e-7,"large":1e20,"zero":-0.0,"text":"1e-07"})
        )
        .unwrap(),
        "5e57da58a961618b3b0b5a51b8e4d809a358fa263855de3df953676064ab22a5"
    );
    let before = fs::read(&store.path).unwrap();
    assert!(store.observe(json!({"ref":"bad","value":true,"observed_at":"2026-09-19T12:00:00.123456789Z","evidence":"bad timestamp"})).is_err());
    let bad = json!({"id":"bad","title":"Bad age","why":"test","how":"inspect","scope":"Read only","related":["p.x"],"when":{"external":{"ref":"large","equals":true,"max_age_hours":1e18}}});
    assert_eq!(store.add(bad).unwrap_err().0, "max_age_hours is too large");
    assert_eq!(fs::read(&store.path).unwrap(), before);
    fs::remove_file(&store.path).unwrap();
    let registered = Store::at_in_state(&root, &state, now).unwrap();
    let error=registered.observe(json!({"ref":"new","value":true,"observed_at":"2026-09-19T12:00Z","evidence":"capture"})).unwrap_err().0;
    assert!(
        error.contains("Registered followups ledger is unavailable")
            && error.contains("restore it from backup"),
        "{error}"
    );
}

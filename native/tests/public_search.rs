use kpop_native::{
    public_search::{self, Options},
    source_capture::ReadMode,
};
use serde_json::{Value as J, json};
use std::{fs, path::Path, process::Command};

#[test]
fn invalid_read_mode_preserves_python_argument_error_precedence() {
    let cases: J =
        serde_json::from_slice(include_bytes!("fixtures/search-mode-errors.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let root = tempfile::tempdir().unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
            .args(
                case["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap()),
            )
            .current_dir(root.path())
            .env("KPOPPER_READ_MODE", "bogus")
            .output()
            .unwrap();
        assert_eq!(output.status.code().map(i64::from), case["code"].as_i64());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), case["stdout"]);
        assert_eq!(String::from_utf8(output.stderr).unwrap(), case["stderr"]);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
fn expand(v: &mut J, from: &str, to: &str) {
    match v {
        J::String(s) => *s = s.replace(from, to),
        J::Array(a) => a.iter_mut().for_each(|v| expand(v, from, to)),
        J::Object(m) => {
            let old = std::mem::take(m);
            for (k, mut v) in old {
                expand(&mut v, from, to);
                m.insert(k.replace(from, to), v);
            }
        }
        _ => {}
    }
}
fn difference(a: &J, b: &J, path: &str) -> Option<String> {
    if a == b {
        return None;
    }
    match (a, b) {
        (J::Object(a), J::Object(b)) => {
            for k in a.keys().chain(b.keys()) {
                if let Some(d) = difference(
                    a.get(k).unwrap_or(&J::Null),
                    b.get(k).unwrap_or(&J::Null),
                    &format!("{path}/{k}"),
                ) {
                    return Some(d);
                }
            }
        }
        (J::Array(a), J::Array(b)) => {
            if a.len() != b.len() {
                return Some(format!("{path} length {} != {}", a.len(), b.len()));
            }
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                if let Some(d) = difference(a, b, &format!("{path}/{i}")) {
                    return Some(d);
                }
            }
        }
        _ => {}
    }
    Some(format!(
        "{path}: {:?} != {:?}",
        a.to_string().chars().take(240).collect::<String>(),
        b.to_string().chars().take(240).collect::<String>()
    ))
}
fn encode(value: &J) -> String {
    match value {
        J::Array(a) => format!("[{}]", a.iter().map(encode).collect::<Vec<_>>().join(", ")),
        J::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}: {}", serde_json::to_string(k).unwrap(), encode(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => value.to_string(),
    }
}
fn native_runtime() -> &'static kpop_native::reasoning_runtime::Runtime {
    static R: std::sync::OnceLock<kpop_native::reasoning_runtime::Runtime> =
        std::sync::OnceLock::new();
    R.get_or_init(|| {
        let root = std::path::PathBuf::from(std::env::var_os("KPOPPER_NATIVE_RESOURCES").unwrap());
        let cache = std::path::PathBuf::from(std::env::var_os("KPOPPER_NATIVE_CACHE").unwrap());
        kpop_native::reasoning_runtime::Runtime::open(
            &root.join("reasoning").join(format!(
                "{}.zip",
                kpop_native::reasoning_runtime::target_name().unwrap()
            )),
            &cache,
            Default::default(),
        )
        .unwrap()
    })
}
fn normalize_runtime(case: &mut J, root: &Path) {
    use kpop_native::{
        reasoning_context::CapturedAssessment, reasoning_history_assessment as H,
        value::TypedValue as V,
    };
    if case["expected"]["core_context"].is_null() {
        return;
    }
    let original = CapturedAssessment::from_data(&case["expected"]["core_context"]).unwrap();
    let old_revision = original.findings_revision().to_owned();
    let mut base = original.base_assessment().clone();
    let runtime = native_runtime();
    fn replace(value: &mut V, runtime: &kpop_native::reasoning_runtime::Runtime) {
        match value {
            V::Map(m) => {
                if let Some(V::Map(old)) = m.get("implementation") {
                    let V::Map(mut actual) = V::from_json(&runtime.implementation).unwrap() else {
                        panic!()
                    };
                    assert_eq!(
                        old.keys().collect::<Vec<_>>(),
                        actual.keys().collect::<Vec<_>>()
                    );
                    for key in ["source_sha256", "lean_version"] {
                        assert_eq!(old[key], actual[key], "runtime changed {key}");
                    }
                    actual.insert("protocol".into(), old["protocol"].clone());
                    let actual = V::Map(actual);
                    let fingerprint = actual.digest().unwrap();
                    m.insert("implementation".into(), actual);
                    let Some(V::Map(assurance)) = m.get_mut("assurance") else {
                        panic!()
                    };
                    assurance.insert("implementation".into(), V::Text(fingerprint));
                }
                for value in m.values_mut() {
                    replace(value, runtime)
                }
            }
            V::List(a) => a.iter_mut().for_each(|v| replace(v, runtime)),
            _ => {}
        }
    }
    replace(&mut base, runtime);
    let V::Map(m) = &mut base else { panic!() };
    m.remove("assessment_revision");
    let digest = base.digest().unwrap();
    let V::Map(m) = &mut base else { panic!() };
    m.insert("assessment_revision".into(), V::Text(digest));
    let original_report = original.assessment();
    let V::Map(report) = original_report else {
        panic!()
    };
    let display = match &report["display_selection"] {
        V::List(a) => a
            .iter()
            .map(|v| {
                let V::Text(s) = v else { panic!() };
                s.clone()
            })
            .collect::<Vec<_>>(),
        _ => panic!(),
    };
    let mut episodes = std::collections::BTreeMap::new();
    if let V::Map(subjects) = &report["history_subjects"] {
        for (id, subject) in subjects {
            if let V::Map(s) = subject
                && let Some(V::Map(t)) = s.get("temporal")
                && let Some(episode) = t.get("episodes")
            {
                episodes.insert(id.clone(), episode.clone());
            }
        }
    }
    let snapshot = original.snapshot().clone();
    let report =
        H::from_v2_temporal(&snapshot, &base, Some(&display), Some(&V::Map(episodes))).unwrap();
    let normalized = CapturedAssessment::new(snapshot, &report).unwrap();
    let mut old_view = original.view().clone();
    let mut new_view = normalized.view().clone();
    for view in [&mut old_view, &mut new_view] {
        let V::Map(m) = view else { panic!() };
        m.remove("findings_revision");
    }
    assert_eq!(
        old_view, new_view,
        "provenance normalization altered consumer findings"
    );
    // Compare complete native packets against the retained Python packet after
    // only replacing verified runtime provenance and recomputing dependent IDs.
    let record = std::path::PathBuf::from(case["options"]["record"].as_str().unwrap());
    let mode = if case["options"]["mode"] == "frozen" {
        ReadMode::Frozen
    } else {
        ReadMode::Live
    };
    let capture = kpop_native::source_capture::capture_source_with_runtime(
        &[record],
        &root.join("source"),
        mode,
        None,
        Some(runtime),
    )
    .unwrap();
    let native = CapturedAssessment::from_snapshot(
        capture.snapshot().unwrap().clone(),
        None,
        "focused-review/v1",
        Some(runtime),
        Default::default(),
        None,
    )
    .unwrap();
    assert_eq!(
        native.assessment(),
        normalized.assessment(),
        "complete normalized core assessment differs"
    );
    expand(case, &old_revision, normalized.findings_revision());
}
fn git_fixture(root: &Path, case: &J) {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use std::io::Write;
    let git = |args: &[&str], input: Option<Vec<u8>>| {
        let mut c = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = input {
            c.stdin.take().unwrap().write_all(&input).unwrap();
        }
        let output = c.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    git(
        &[
            "init",
            "-b",
            "trunk",
            &format!(
                "--object-format={}",
                case["object_format"].as_str().unwrap()
            ),
        ],
        None,
    );
    for object in case["objects"].as_array().unwrap() {
        let hash = git(
            &[
                "hash-object",
                "-w",
                "--stdin",
                "-t",
                object["kind"].as_str().unwrap(),
            ],
            Some(STANDARD.decode(object["raw"].as_str().unwrap()).unwrap()),
        );
        assert_eq!(hash, object["oid"]);
    }
    if let Some(head) = case["head"].as_str() {
        git(
            &["update-ref", "refs/kpopper/pending_grounding", head],
            None,
        );
    }
    let project = kpop_native::project_modes::Project::open(root).unwrap();
    if let Some(raw) = case["config"].as_str() {
        fs::create_dir_all(&project.state).unwrap();
        fs::write(&project.config_path, raw).unwrap();
    }
    if let Some(raw) = case["publication"].as_str() {
        fs::create_dir_all(&project.state).unwrap();
        fs::write(project.state.join("publication.json"), raw).unwrap();
    }
    if let Some(target) = case["target_ref"].as_str() {
        git(&["update-ref", "refs/remotes/origin/trunk", target], None);
    }
}
fn setup(case: &mut J, root: &Path) {
    expand(case, "$ROOT", root.to_str().unwrap());
    fs::create_dir_all(root.join("source")).unwrap();
    if !case["git_fixture"].is_null() {
        git_fixture(&root.join("source"), &case["git_fixture"]);
    }
    for (path, value) in case["input"].as_object().unwrap() {
        let p = root.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, value.as_str().unwrap().as_bytes()).unwrap();
    }
    normalize_runtime(case, root);
    if !case["preimage"].is_null() {
        let digest = kpop_native::identity::sha256(encode(&case["preimage"]).as_bytes());
        expand(case, "$REVISION", &digest);
    }
}
fn options(case: &J) -> Options {
    let o = &case["options"];
    Options {
        record: o["record"].as_str().map(Into::into),
        state_dir: o["state_dir"].as_str().map(Into::into),
        source_roots: o["source_roots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().into())
            .collect(),
        profile: o["profile"].as_str().map(str::to_owned),
        ..Default::default()
    }
}
#[test]
#[ignore = "requires KPOPPER_NATIVE_RESOURCES and KPOPPER_NATIVE_CACHE"]
fn complete_corpora_and_revisions_match_python() {
    if std::env::var_os("KPOPPER_NATIVE_RESOURCES").is_none() {
        eprintln!("set KPOPPER_NATIVE_RESOURCES for complete search corpus oracles");
        return;
    }
    let fixture: J = serde_json::from_str(include_str!("fixtures/search-corpus.json")).unwrap();
    let mut failures = vec![];
    for original in fixture["cases"].as_array().unwrap() {
        if std::env::var("KPOP_SEARCH_CASE").is_ok_and(|f| original["name"] != f) {
            continue;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let mut case = original.clone();
        setup(&mut case, &root);
        let mode = if case["options"]["mode"] == "frozen" {
            ReadMode::Frozen
        } else {
            ReadMode::Live
        };
        let actual = public_search::corpus(&options(&case), &root.join("source"), mode);
        let (expected, actual) = match actual {
            Ok(data) => (case["expected"]["corpus"].clone(), data),
            Err(e) => (
                case["expected"]["failure"].clone(),
                json!({"message":e.message,"capture_failure":e.capture_failure}),
            ),
        };
        if let Some(diff) = difference(&actual, &expected, "") {
            failures.push(format!("{}: {diff}", case["name"]));
            let output = std::env::temp_dir().join(format!(
                "kpop-search-{}.json",
                case["name"].as_str().unwrap()
            ));
            fs::write(
                output,
                serde_json::to_vec_pretty(&json!({"actual":actual,"expected":expected})).unwrap(),
            )
            .unwrap();
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[test]
#[ignore = "requires KPOPPER_NATIVE_RESOURCES and KPOPPER_NATIVE_CACHE"]
fn actual_search_and_paged_read_cli_match_python() {
    let fixture: J = serde_json::from_str(include_str!("fixtures/search-corpus.json")).unwrap();
    let mut failures = vec![];
    for original in fixture["cases"].as_array().unwrap() {
        if std::env::var("KPOP_SEARCH_CASE").is_ok_and(|f| original["name"] != f) {
            continue;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let mut case = original.clone();
        setup(&mut case, &root);
        for label in ["cli", "read_cli"] {
            let expected = &case["expected"][label];
            if expected.is_null() {
                continue;
            }
            let output = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
                .args(
                    expected["argv"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap()),
                )
                .current_dir(root.join("source"))
                .env("XDG_STATE_HOME", root.join("statehome"))
                .env_remove("KPOPPER_READ_MODE")
                .output()
                .unwrap();
            let actual = json!({"exit":output.status.code(),"stdout":String::from_utf8(output.stdout).unwrap(),"stderr":String::from_utf8(output.stderr).unwrap()});
            let wanted = json!({"exit":expected["exit"],"stdout":expected["stdout"],"stderr":expected["stderr"]});
            if let Some(diff) = difference(&actual, &wanted, "") {
                failures.push(format!("{} {label}: {diff}", case["name"]));
            }
        }
        for (path, content) in case["input"].as_object().unwrap() {
            assert_eq!(
                fs::read(root.join(path)).unwrap(),
                content.as_str().unwrap().as_bytes(),
                "search changed {path}"
            );
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
#[ignore = "requires KPOPPER_NATIVE_RESOURCES and KPOPPER_NATIVE_CACHE"]
fn nonfinite_corpora_search_and_paged_read_match_python() {
    let fixture: J = serde_json::from_str(include_str!("fixtures/search-nonfinite.json")).unwrap();
    let mut failures = vec![];
    for original in fixture["cases"].as_array().unwrap() {
        if std::env::var("KPOP_SEARCH_CASE").is_ok_and(|f| original["name"] != f) {
            continue;
        }
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let mut case = original.clone();
        setup(&mut case, &root);
        let mode = if case["options"]["mode"] == "frozen" {
            ReadMode::Frozen
        } else {
            ReadMode::Live
        };
        let result = public_search::corpus(&options(&case), &root.join("source"), mode);
        let (actual, expected) = match result {
            Ok(value) => (value, case["expected"]["corpus"].clone()),
            Err(e) => (
                json!({"message":e.message,"capture_failure":e.capture_failure}),
                case["expected"]["failure"].clone(),
            ),
        };
        if let Some(diff) = difference(&actual, &expected, "") {
            failures.push(format!("{} corpus: {diff}", case["name"]));
            fs::write(
                std::env::temp_dir().join(format!(
                    "kpop-search-{}.json",
                    case["name"].as_str().unwrap()
                )),
                serde_json::to_vec_pretty(&json!({"actual":actual,"expected":expected})).unwrap(),
            )
            .unwrap();
        }
        for label in ["cli", "read_cli"] {
            let expected = &case["expected"][label];
            if expected.is_null() {
                continue;
            }
            let result = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
                .args(
                    expected["argv"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap()),
                )
                .current_dir(root.join("source"))
                .env("XDG_STATE_HOME", root.join("statehome"))
                .env_remove("KPOPPER_READ_MODE")
                .output()
                .unwrap();
            let actual = json!({"exit":result.status.code(),"stdout":String::from_utf8(result.stdout).unwrap(),"stderr":String::from_utf8(result.stderr).unwrap()});
            let wanted = json!({"exit":expected["exit"],"stdout":expected["stdout"],"stderr":expected["stderr"]});
            if let Some(diff) = difference(&actual, &wanted, "") {
                failures.push(format!("{} {label}: {diff}", case["name"]));
            }
        }
        for (path, value) in case["input"].as_object().unwrap() {
            assert_eq!(
                fs::read(root.join(path)).unwrap(),
                value.as_str().unwrap().as_bytes(),
                "search changed {path}"
            );
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
#[ignore = "requires KPOPPER_NATIVE_RESOURCES and KPOPPER_NATIVE_CACHE"]
fn nonfinite_reads_refuse_changed_captured_bytes() {
    let fixture: J = serde_json::from_str(include_str!("fixtures/search-nonfinite.json")).unwrap();
    let mut case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "nonfinite-nan")
        .unwrap()
        .clone();
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    setup(&mut case, &root);
    let mut options = options(&case);
    let corpus = public_search::corpus(&options, &root.join("source"), ReadMode::Live).unwrap();
    let path = root.join("source/GROUNDING.yaml");
    let old = fs::read_to_string(&path).unwrap();
    fs::write(path, old.replace(".nan", ".inf")).unwrap();
    options.reference = Some("node:p.value".into());
    options.revision = Some(corpus["revision"].as_str().unwrap().into());
    assert_eq!(
        public_search::run(&options, &root.join("source"), ReadMode::Live)
            .unwrap_err()
            .message,
        "record, capture state or source changed; search again"
    );
}

#[test]
#[ignore = "requires KPOPPER_NATIVE_RESOURCES and KPOPPER_NATIVE_CACHE"]
fn reads_rebuild_corpus_and_refuse_changed_records_sources_captures_or_grants() {
    let fixture: J = serde_json::from_str(include_str!("fixtures/search-corpus.json")).unwrap();
    for mutation in ["record", "source", "capture", "grant"] {
        let name = match mutation {
            "capture" => "capture-valid",
            "grant" => "ordinary-extra-root",
            _ => "ordinary-basic",
        };
        let mut case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap()
            .clone();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        setup(&mut case, &root);
        let mut options = options(&case);
        let data = public_search::corpus(&options, &root.join("source"), ReadMode::Live).unwrap();
        let reference = data["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["kind"] == "source")
            .unwrap()["ref"]
            .as_str()
            .unwrap()
            .to_owned();
        match mutation {
            "record" => {
                let path = root.join("source/GROUNDING.yaml");
                let mut raw = fs::read_to_string(&path).unwrap();
                raw.push_str("# source observation changed\n");
                fs::write(path, raw).unwrap();
            }
            "source" => {
                fs::write(root.join("source/evidence.md"), "Changed source evidence\n").unwrap()
            }
            "capture" => {
                let path = root.join("private/events/event1.json");
                let mut event: J = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                event["state"] = json!("needs_primary");
                fs::write(path, serde_json::to_vec(&event).unwrap()).unwrap();
            }
            "grant" => options.source_roots.clear(),
            _ => unreachable!(),
        }
        options.reference = Some(reference);
        options.revision = Some(data["revision"].as_str().unwrap().into());
        assert_eq!(
            public_search::run(&options, &root.join("source"), ReadMode::Live)
                .unwrap_err()
                .message,
            "record, capture state or source changed; search again",
            "{mutation}"
        );
    }
}

#[test]
#[ignore = "requires KPOPPER_NATIVE_RESOURCES and KPOPPER_NATIVE_CACHE"]
fn source_size_utf8_and_binary_bounds_are_explicit() {
    let fixture: J = serde_json::from_str(include_str!("fixtures/search-corpus.json")).unwrap();
    for (raw, reason) in [
        (
            vec![b'a'; 1024 * 1024 + 1],
            "source exceeds the 1 MiB search limit; read it directly",
        ),
        (
            vec![0xff],
            "'utf-8' codec can't decode byte 0xff in position 0: invalid start byte",
        ),
        (
            vec![0xe2, 0x82],
            "'utf-8' codec can't decode bytes in position 0-1: unexpected end of data",
        ),
        (b"a\0b".to_vec(), "binary content is not indexed"),
    ] {
        let mut case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "ordinary-basic")
            .unwrap()
            .clone();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        setup(&mut case, &root);
        fs::write(root.join("source/evidence.md"), raw).unwrap();
        let data =
            public_search::corpus(&options(&case), &root.join("source"), ReadMode::Live).unwrap();
        assert_eq!(
            data["unindexed"],
            json!([{"ref":"source:s.note","reason":reason}])
        );
        assert!(
            !data["rows"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["ref"] == "source:s.note")
        );
    }
}

#[test]
#[ignore = "opt-in immutable Python comparison; set KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT"]
fn live_python_ordinary_cli_matches_without_normalization() {
    let python =
        std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("KPOP_SESSION_ORACLE_PYTHON");
    let baseline = std::path::PathBuf::from(
        std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("KPOP_SESSION_ORACLE_ROOT"),
    );
    let fixture: J = serde_json::from_str(include_str!("fixtures/search-corpus.json")).unwrap();
    for name in [
        "ordinary-basic",
        "ordinary-statuses",
        "capture-valid",
        "pending-two-live",
        "ordinary-extra-root",
    ] {
        let mut case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap()
            .clone();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        setup(&mut case, &root);
        for label in ["cli", "read_cli"] {
            let expected = &case["expected"][label];
            if expected.is_null() {
                continue;
            }
            let args = expected["argv"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect::<Vec<_>>();
            let native = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
                .args(&args)
                .current_dir(root.join("source"))
                .env("XDG_STATE_HOME", root.join("statehome"))
                .env_remove("KPOPPER_READ_MODE")
                .output()
                .unwrap();
            let reference = Command::new(&python)
                .arg(baseline.join("scripts/cli.py"))
                .args(&args)
                .current_dir(root.join("source"))
                .env("XDG_STATE_HOME", root.join("statehome"))
                .env_remove("KPOPPER_READ_MODE")
                .output()
                .unwrap();
            assert_eq!(
                native.status.code(),
                reference.status.code(),
                "{name} {label}"
            );
            assert_eq!(native.stdout, reference.stdout, "{name} {label} stdout");
            assert_eq!(native.stderr, reference.stderr, "{name} {label} stderr");
        }
    }
}

#[test]
#[ignore = "requires KPOPPER_NATIVE_RESOURCES and KPOPPER_NATIVE_CACHE"]
fn cli_errors_and_negative_query_match_python() {
    let corpus: J = serde_json::from_str(include_str!("fixtures/search-corpus.json")).unwrap();
    let basic = corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "ordinary-basic")
        .unwrap();
    let errors: J = serde_json::from_str(include_str!("fixtures/search-errors.json")).unwrap();
    for original in errors.as_array().unwrap() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let mut case = basic.clone();
        setup(&mut case, &root);
        let mut row = original.clone();
        expand(&mut row, "$ROOT", root.to_str().unwrap());
        expand(
            &mut row,
            "$REVISION",
            case["expected"]["corpus"]["revision"].as_str().unwrap(),
        );
        let output = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
            .args(
                row["argv"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap()),
            )
            .current_dir(root.join("source"))
            .env("XDG_STATE_HOME", root.join("statehome"))
            .env_remove("KPOPPER_READ_MODE")
            .output()
            .unwrap();
        let actual = json!({"exit":output.status.code(),"stdout":String::from_utf8(output.stdout).unwrap(),"stderr":String::from_utf8(output.stderr).unwrap()});
        assert_eq!(
            actual,
            json!({"exit":row["exit"],"stdout":row["stdout"],"stderr":row["stderr"]}),
            "{}",
            row["name"]
        );
    }
}

use base64::{Engine, engine::general_purpose::STANDARD};
fn fresh_implementation(value: &mut V, runtime: &kpop_native::reasoning_runtime::Runtime) {
    match value {
        V::Map(m) => {
            if let Some(V::Map(old)) = m.get("implementation") {
                let current = V::from_json(&runtime.implementation).unwrap();
                let V::Map(mut current) = current else {
                    unreachable!()
                };
                for key in ["source_sha256", "lean_version"] {
                    assert_eq!(old.get(key), current.get(key));
                }
                current.insert("protocol".into(), old["protocol"].clone());
                let implementation = V::Map(current);
                let fingerprint = implementation.digest().unwrap();
                m.insert("implementation".into(), implementation);
                let V::Map(assurance) = m.get_mut("assurance").unwrap() else {
                    unreachable!()
                };
                assurance.insert("implementation".into(), V::Text(fingerprint));
            }
            for value in m.values_mut() {
                fresh_implementation(value, runtime);
            }
        }
        V::List(values) => {
            for value in values {
                fresh_implementation(value, runtime);
            }
        }
        _ => {}
    }
}
use kpop_native::{
    pending_state::{self as P, Ledger},
    project_modes::Project,
    value::TypedValue as V,
};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};
fn git(root: &Path, args: &[&str], input: Option<&[u8]>) -> Vec<u8> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    if let Some(input) = input {
        child.stdin.take().unwrap().write_all(input).unwrap();
    }
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}
pub fn fixture(root: &Path, case: &serde_json::Value) {
    git(
        root,
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
        let raw = STANDARD.decode(object["raw"].as_str().unwrap()).unwrap();
        let got = git(
            root,
            &[
                "hash-object",
                "-w",
                "--stdin",
                "-t",
                object["kind"].as_str().unwrap(),
            ],
            Some(&raw),
        );
        assert_eq!(
            String::from_utf8(got).unwrap().trim(),
            object["oid"].as_str().unwrap()
        );
    }
    if let Some(head) = case["head"].as_str() {
        git(root, &["update-ref", P::REF, head], None);
    }
    for (path, raw) in case["files"].as_object().unwrap() {
        std::fs::write(root.join(path), raw.as_str().unwrap()).unwrap();
    }
    let project = Project::open(root).unwrap();
    if let Some(raw) = case["config"].as_str() {
        std::fs::create_dir_all(&project.state).unwrap();
        std::fs::write(&project.config_path, raw).unwrap();
    }
    if let Some(target) = case["target_ref"].as_str() {
        git(
            root,
            &["update-ref", "refs/remotes/origin/trunk", target],
            None,
        );
    }
    if let Some(raw) = case["publication"].as_str() {
        std::fs::create_dir_all(&project.state).unwrap();
        std::fs::write(project.state.join("publication.json"), raw).unwrap();
    }
}
#[test]
fn pinned_git_ledgers_and_publisher_status_match_python() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/pending-state.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fixture(&root, case);
        let project = Project::open(&root).unwrap();
        let ledger = Ledger::capture(&project).unwrap();
        assert_eq!(
            ledger.portable(),
            V::from_tagged(&case["ledger"]).unwrap(),
            "{} ledger",
            case["name"]
        );
        let observation = P::Observation::capture(&project, &project.config().unwrap()).unwrap();
        assert_eq!(
            observation.publication,
            V::from_tagged(&case["status"]).unwrap(),
            "{} status",
            case["name"]
        );
        let cache = tempfile::tempdir().unwrap();
        let archive = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!(
                "{}.zip",
                kpop_native::reasoning_runtime::target_name().unwrap()
            ));
        let runtime = if case["name"] == "target_core" {
            Some(
                kpop_native::reasoning_runtime::Runtime::open(
                    &archive,
                    cache.path(),
                    Default::default(),
                )
                .unwrap(),
            )
        } else {
            None
        };
        let captured = kpop_native::source_capture::capture_source_with_runtime(
            &[root.join(case["entry"].as_str().unwrap_or("GROUNDING.yaml"))],
            &root,
            kpop_native::source_capture::ReadMode::Live,
            Some(V::Text("2026-09-19".into())),
            runtime.as_ref(),
        )
        .unwrap();
        let mut expected = V::from_tagged(&case["snapshot"]).unwrap();
        if let Some(runtime) = &runtime {
            // This is a fresh computation. Retain the verified native runtime's
            // actual implementation identity; never impersonate the Python adapter.
            let V::Map(data) = &mut expected else {
                unreachable!()
            };
            let V::Map(context) = data.get_mut("context").unwrap() else {
                unreachable!()
            };
            let V::Map(target) = context.get_mut("target").unwrap() else {
                unreachable!()
            };
            let V::Map(target_snapshot) = target.get_mut("snapshot").unwrap() else {
                unreachable!()
            };
            let V::Map(core) = target_snapshot.get_mut("core").unwrap() else {
                unreachable!()
            };
            let report = core.get_mut("assessment").unwrap();
            fresh_implementation(report, runtime);
            let V::Map(fields) = report else {
                unreachable!()
            };
            fields.remove("assessment_revision");
            let digest = V::Map(fields.clone()).digest().unwrap();
            fields.insert("assessment_revision".into(), V::Text(digest));
            expected = kpop_native::reasoning_snapshot::Snapshot::from_data(
                &data["document"],
                kpop_native::reasoning_snapshot::CaptureOptions {
                    context: Some(data["context"].clone()),
                    hypotheses: Some(data["hypotheses"].clone()),
                    as_of: Some(data["as_of"].clone()),
                    authored_revision: Some(data["authored_revision"].clone()),
                },
            )
            .unwrap()
            .to_data();
        }
        if captured.snapshot().to_data() != expected {
            eprintln!(
                "{} actual={} expected={}",
                case["name"],
                captured.snapshot().to_json().unwrap(),
                serde_json::to_string(&case["snapshot"]).unwrap()
            );
        }
        assert_eq!(
            captured.snapshot().to_data(),
            expected,
            "{} live snapshot",
            case["name"]
        );
        if let Some(head) = &ledger.head {
            git(&root, &["update-ref", "-d", P::REF], None);
            assert!(Ledger::capture(&project).unwrap().head.is_none());
            assert_eq!(
                Ledger::at(&root, Some(head.clone())).unwrap().portable(),
                ledger.portable()
            );
        }
    }
}

#[test]
fn telemetry_is_private_but_stale_observations_refuse() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/pending-state.json")).unwrap();
    let case = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "one")
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fixture(&root, case);
    let capture = || {
        kpop_native::source_capture::capture_source(
            &[root.join("GROUNDING.yaml")],
            &root,
            kpop_native::source_capture::ReadMode::Live,
            Some(V::Text("2026-09-19".into())),
        )
        .unwrap()
    };
    let before = capture();
    let project = Project::open(&root).unwrap();
    std::fs::create_dir_all(&project.state).unwrap();
    let state = serde_json::json!({"version":1,"states":{},"decisions":{},"paused":false,"pr":null,"expected_head":null,"retry_at":32,"failures":4,"intent":null});
    std::fs::write(
        project.state.join("publication.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    assert_eq!(before.verify().unwrap_err().0, "snapshot_changed");
    let after = capture();
    assert_eq!(before.snapshot().to_data(), after.snapshot().to_data());
    git(&root, &["update-ref", "-d", P::REF], None);
    assert_eq!(after.verify().unwrap_err().0, "snapshot_changed");
}

#[test]
fn corrupt_object_bytes_cannot_reuse_a_committed_blob_identity() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/pending-state.json")).unwrap();
    let case = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "one")
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fixture(&root, case);
    let blobs = case["objects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["kind"] == "blob")
        .take(2)
        .map(|o| o["oid"].as_str().unwrap())
        .collect::<Vec<_>>();
    let object = |oid: &str| root.join(".git/objects").join(&oid[..2]).join(&oid[2..]);
    let replacement = std::fs::read(object(blobs[1])).unwrap();
    std::fs::remove_file(object(blobs[0])).unwrap();
    std::fs::write(object(blobs[0]), replacement).unwrap();
    let project = Project::open(&root).unwrap();
    assert_eq!(
        Ledger::capture(&project).unwrap_err().0,
        "pending_object_mismatch"
    );
}

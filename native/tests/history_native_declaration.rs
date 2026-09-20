//! Actual copied-binary declaration/probe tests; no Python or Lean execution.
use kpop_native::{history_runtime as H, identity::sha256};
use serde_json::{Value as J, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
const NONCE: &str = "0123456789abcdef0123456789abcdef";
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for item in fs::read_dir(path).unwrap() {
            let p = item.unwrap().path();
            if p.is_dir() {
                walk(root, &p, out)
            } else {
                out.insert(p.strip_prefix(root).unwrap().into(), fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}
fn package(root: &Path, complete: bool) -> PathBuf {
    let binary = root.join(if cfg!(windows) {
        "native fixture.exe"
    } else {
        "native fixture"
    });
    fs::copy(env!("CARGO_BIN_EXE_kpop-native"), &binary).unwrap();
    if complete {
        let target = kpop_native::reasoning_runtime::target_name().unwrap();
        let resources = root.join("resources");
        fs::create_dir_all(resources.join("reasoning")).unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/reasoning/native")
                .join(format!("{target}.zip")),
            resources
                .join("reasoning")
                .join(format!("{target}.kpopper-runtime")),
        )
        .unwrap();
        let ordinary = resources.join("ordinary").join(target);
        fs::create_dir_all(&ordinary).unwrap();
        // Intentionally not a runnable program: successful describe proves that
        // archive/file validation does not execute this integrity-only fixture.
        let bytes = b"ordinary program must not execute";
        let name = if cfg!(windows) {
            "epistemic-core.exe"
        } else {
            "epistemic-core"
        };
        fs::write(ordinary.join(name), bytes).unwrap();
        fs::write(ordinary.join("build.json"),serde_json::to_vec(&json!({"source_sha256":env!("KPOP_ORDINARY_SOURCE_SHA256"),"binary_sha256":sha256(bytes)})).unwrap()).unwrap();
    }
    binary
}
fn invoke(binary: &Path, nonce: &str) -> std::process::Output {
    Command::new(binary)
        .args(["history", "capabilities", "--nonce", nonce, "--json"])
        .env("PATH", "")
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_AGENT_CONTEXT")
        .env(
            "KPOPPER_NATIVE_CACHE",
            binary.parent().unwrap().join("must-not-create-cache"),
        )
        .current_dir(binary.parent().unwrap())
        .output()
        .unwrap()
}
fn declaration(binary: &Path) -> J {
    let out = invoke(binary, NONCE);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(out.stderr.is_empty());
    serde_json::from_slice(&out.stdout).unwrap()
}
fn native_inventory(binary: &Path, value: &J) -> J {
    #[cfg(unix)]
    let argv = json!(["/usr/bin/env", "PATH=", binary]);
    #[cfg(not(unix))]
    let argv = json!([binary]);
    json!([{"id":"native","kind":"native-rust/v1","argv":argv,"executable":binary,"resource_root":value["resolved"]["resource_root"]}])
}
fn expected(value: &J) -> J {
    json!({"native":{"artifact":value["artifact"]["digest"],"schemas":value["schemas"]["digest"],"resources":value["resources"]["digest"]}})
}
#[test]
fn copied_cli_without_python_path_is_readonly_and_reports_missing_resources_honestly() {
    for complete in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let binary = package(&root, complete);
        let before = snapshot(&root);
        let value = declaration(&binary);
        H::validate_declaration(&value, NONCE).unwrap();
        assert_eq!(value["version"], 2);
        assert!(value.get("sources").is_none());
        assert_eq!(
            value["artifact"]["executable_sha256"],
            sha256(&fs::read(&binary).unwrap())
        );
        assert_eq!(
            value["resources"]["core"]["status"],
            if complete {
                "archive_validated"
            } else {
                "unavailable"
            }
        );
        assert_eq!(snapshot(&root), before);
        assert!(!root.join("must-not-create-cache").exists());
        let proof = H::probe_launchers(
            &native_inventory(&binary, &value),
            &expected(&value),
            NONCE,
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(proof["version"], 2);
        assert_eq!(proof["kind"], "managed-launcher-probe/v2");
        assert_eq!(snapshot(&root), before);
        fs::create_dir(root.join(".kpopper")).unwrap();
        fs::write(
            root.join(".kpopper/native-feasibility.json"),
            b"not a record manifest",
        )
        .unwrap();
        let feasibility_before = snapshot(&root);
        assert_eq!(declaration(&binary), value);
        assert_eq!(snapshot(&root), feasibility_before);
        let invalid = invoke(&binary, "bad");
        assert!(!invalid.status.success());
        assert_eq!(
            serde_json::from_slice::<J>(&invalid.stdout).unwrap()["code"],
            "invalid_runtime_nonce"
        );
        assert_eq!(snapshot(&root), feasibility_before);
    }
}
#[test]
fn pinned_digest_root_nonce_and_unknown_inventory_cannot_authorize_another_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let binary = package(&root, true);
    let value = declaration(&binary);
    let inventory = native_inventory(&binary, &value);
    let mut changed = expected(&value);
    changed["native"]["artifact"] = json!("0".repeat(64));
    assert!(H::probe_launchers(&inventory, &changed, NONCE, Duration::from_secs(10)).is_err());
    let mut invalid = inventory.clone();
    invalid[0]["kind"] = json!("unknown/v1");
    assert!(
        H::probe_launchers(&invalid, &expected(&value), NONCE, Duration::from_secs(10)).is_err()
    );
    let mut invalid = inventory.clone();
    invalid[0]["resource_root"] = json!(root);
    assert!(
        H::probe_launchers(&invalid, &expected(&value), NONCE, Duration::from_secs(10)).is_err()
    );
    assert!(
        H::probe_launchers(
            &inventory,
            &expected(&value),
            "short",
            Duration::from_secs(10)
        )
        .is_err()
    );
    assert_eq!(
        H::validate_declaration(&value, "other_valid_nonce")
            .unwrap_err()
            .0,
        "runtime_nonce_mismatch"
    );
}
#[cfg(unix)]
#[test]
fn mixed_native_python_inventory_keeps_the_complete_legacy_proof_and_refuses_cross_kind() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let binary = package(&root, true);
    let value = declaration(&binary);
    let shell = Path::new("/bin/sh").canonicalize().unwrap();
    let py = root.join("legacy");
    fs::create_dir(&py).unwrap();
    fs::write(py.join("cli.py"), b"fixture").unwrap();
    let mut legacy: J =
        serde_json::from_str(include_str!("fixtures/history-runtime-applications.json")).unwrap();
    legacy["nonce"] = json!(NONCE);
    legacy["resolved"] = json!({"package_root":py,"cli":py.join("cli.py"),"executable":shell,"argv":["fixture-probe"],"bytecode_write_disabled":true});
    let response = root.join("response.json");
    fs::write(&response, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let item = json!({"id":"python","argv":[shell,"-c","/bin/cat \"$1\"","fixture",response],"package_root":py,"executable":shell});
    let legacy_expected = json!({"sources":legacy["sources"]["digest"],"schemas":legacy["schemas"]["digest"],"native":legacy["native"]["digest"]});
    let only = H::probe_launchers(
        &json!([item]),
        &json!({"python":legacy_expected}),
        NONCE,
        Duration::from_secs(10),
    )
    .unwrap();
    let mut argv = item["argv"].as_array().unwrap().clone();
    argv.extend(["history", "capabilities", "--nonce", NONCE, "--json"].map(|s| json!(s)));
    assert_eq!(
        only,
        json!({"version":1,"kind":"managed-launcher-probe/v1","nonce":NONCE,"scope":"selected_managed_launchers","complete":true,
        "launchers":[{"id":"python","argv":argv,"declaration_digest":sha256(&serde_json::to_vec(&legacy).unwrap()),"declaration":legacy}],
        "assurance":"fresh_nonce_correlation_not_attestation","deployment_stability":"caller_responsibility","unlisted_launchers":"not_covered"})
    );
    let mut inventory = native_inventory(&binary, &value);
    inventory.as_array_mut().unwrap().push(item.clone());
    let mut expectations = expected(&value);
    expectations["python"] = legacy_expected.clone();
    let mixed =
        H::probe_launchers(&inventory, &expectations, NONCE, Duration::from_secs(10)).unwrap();
    assert_eq!(mixed["kind"], "managed-launcher-probe/v2");
    assert_eq!(mixed["launchers"][1], only["launchers"][0]);
    let native_response = root.join("native-response.json");
    fs::write(&native_response, serde_json::to_vec(&value).unwrap()).unwrap();
    let mut wrong = item.clone();
    wrong["argv"][4] = json!(native_response);
    assert_eq!(
        H::probe_launchers(
            &json!([wrong]),
            &json!({"python":legacy_expected}),
            NONCE,
            Duration::from_secs(10)
        )
        .unwrap_err()
        .0,
        "runtime_kind_mismatch"
    );
    let mut wrong = native_inventory(&binary, &value);
    wrong[0]["argv"] = item["argv"].clone();
    assert_eq!(
        H::probe_launchers(&wrong, &expected(&value), NONCE, Duration::from_secs(10))
            .unwrap_err()
            .0,
        "runtime_kind_mismatch"
    );
}

#[test]
fn native_transition_gate_refuses_missing_integrity_before_record_or_authority_writes() {
    use kpop_native::{
        Result,
        history_activation::{self as A, Deployment, DeploymentGuard, Options, Selection},
    };
    struct Exclusion;
    struct Held;
    impl DeploymentGuard for Held {
        fn verify(&self) -> Result<()> {
            Ok(())
        }
    }
    impl Deployment for Exclusion {
        fn exclude<'a>(&'a self, _: &J, _: &J) -> Result<Box<dyn DeploymentGuard + 'a>> {
            Ok(Box::new(Held))
        }
    }
    for missing in ["root", "core", "ordinary"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let binroot = root.join("package");
        fs::create_dir(&binroot).unwrap();
        let binary = package(&binroot, true);
        let target = kpop_native::reasoning_runtime::target_name().unwrap();
        match missing {
            "root" => fs::remove_dir_all(binroot.join("resources")).unwrap(),
            "core" => fs::remove_file(
                binroot
                    .join("resources/reasoning")
                    .join(format!("{target}.kpopper-runtime")),
            )
            .unwrap(),
            "ordinary" => fs::remove_dir_all(binroot.join("resources/ordinary")).unwrap(),
            _ => unreachable!(),
        }
        let value = declaration(&binary);
        let selection = Selection {
            inventory: native_inventory(&binary, &value),
            expected_digests: expected(&value),
        };
        let records = root.join("records");
        fs::create_dir(&records).unwrap();
        let entry = records.join("GROUNDING.yaml");
        let raw = b"known: {p.value: {v: 1}}\n";
        fs::write(&entry, raw).unwrap();
        let result = A::prepare_activation(
            &entry,
            &Options {
                operation: "native-probe-test".into(),
                recorded_at: "2026-09-20T00:00:00+00:00".into(),
                record_id: Some("fixture".into()),
            },
            &selection,
            &Exclusion,
            None,
        );
        assert_eq!(
            result.unwrap_err().0,
            "history_transition_runtime_unsupported",
            "{missing}"
        );
        assert_eq!(fs::read(&entry).unwrap(), raw);
        let layout = kpop_native::history_transaction::Layout::for_entry("GROUNDING.yaml").unwrap();
        assert!(!records.join(layout.authority).exists());
        assert!(!records.join(layout.journal).exists());
        assert!(!records.join(layout.objects).exists());
    }
}

#[test]
fn execution_and_declaration_select_the_same_new_or_historical_archive() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let binary = package(&root, true);
    let record = root.join("GROUNDING.yaml");
    let raw=b"meta: {reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}}\nknown: {p.answer: {v: 42}}\n";
    fs::write(&record, raw).unwrap();
    let run = || {
        Command::new(&binary)
            .args(["--frozen", "assess", "p.answer", "--record"])
            .arg(&record)
            .env_remove("KPOPPER_NATIVE_RESOURCES")
            .env("KPOPPER_NATIVE_CACHE", root.join("test-cache"))
            .current_dir(&root)
            .output()
            .unwrap()
    };
    let modern = run();
    assert!(
        modern.status.success(),
        "{} {}",
        String::from_utf8_lossy(&modern.stdout),
        String::from_utf8_lossy(&modern.stderr)
    );
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    let path = root
        .join("resources/reasoning")
        .join(format!("{target}.kpopper-runtime"));
    let historical = path.with_file_name(format!("{target}.zip"));
    fs::rename(&path, &historical).unwrap();
    assert!(
        declaration(&binary)["resources"]["core"]["path"]
            .as_str()
            .unwrap()
            .ends_with(".zip")
    );
    let legacy = run();
    assert!(legacy.status.success());
    assert_eq!(modern.stdout, legacy.stdout);
    fs::copy(&historical, &path).unwrap();
    let ambiguous = run();
    assert!(!ambiguous.status.success());
    assert!(
        format!(
            "{}{}",
            String::from_utf8_lossy(&ambiguous.stdout),
            String::from_utf8_lossy(&ambiguous.stderr)
        )
        .contains("ambiguous_runtime_archive")
    );
    assert_eq!(fs::read(record).unwrap(), raw);
}

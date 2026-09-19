use kpop_native::history_runtime::{probe_launchers, validate_declaration};
use serde_json::{Value as J, json};
use std::{fs, path::Path, time::Duration};
fn setup(value: &mut J, root: &Path, executable: &Path) {
    value["resolved"]["package_root"] = json!(root);
    value["resolved"]["cli"] = json!(root.join("cli.py"));
    value["resolved"]["executable"] = json!(executable);
}
#[test]
fn runtime_declarations_match_final_python() {
    let corpus: J = serde_json::from_str(include_str!("fixtures/history-runtime.json")).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
    fs::write(root.join("cli.py"), b"fixture").unwrap();
    for case in corpus["cases"].as_array().unwrap() {
        let mut value = case["declaration"].clone();
        setup(&mut value, &root, &executable);
        let actual = validate_declaration(&value, corpus["nonce"].as_str().unwrap());
        match case["error"].as_str() {
            Some(code) => assert_eq!(actual.unwrap_err().0, code, "{}", case["name"]),
            None => actual.unwrap(),
        }
    }
}

#[test]
fn current_main_application_inventory_is_supported_without_relaxing_its_manifest() {
    let mut declaration: J =
        serde_json::from_str(include_str!("fixtures/history-runtime-applications.json")).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::write(root.join("cli.py"), b"fixture").unwrap();
    setup(
        &mut declaration,
        &root,
        &std::env::current_exe().unwrap().canonicalize().unwrap(),
    );
    let nonce = declaration["nonce"].as_str().unwrap().to_owned();
    validate_declaration(&declaration, &nonce).unwrap();
    for change in ["required", "files", "inclusion"] {
        let mut altered = declaration.clone();
        let source = altered["sources"].as_object_mut().unwrap();
        match change {
            "required" => source
                .get_mut("required")
                .unwrap()
                .as_array_mut()
                .unwrap()
                .retain(|p| p != "applications/hub.py"),
            "files" => source
                .get_mut("files")
                .unwrap()
                .as_array_mut()
                .unwrap()
                .retain(|f| f["path"] != "applications/hub.py"),
            _ => source
                .get_mut("inclusion")
                .unwrap()
                .as_array_mut()
                .unwrap()
                .push(json!("unknown/*.py")),
        }
        source.remove("digest");
        let digest = kpop_native::identity::sha256(&serde_json::to_vec(&source).unwrap());
        source.insert("digest".into(), json!(digest));
        assert!(validate_declaration(&altered, &nonce).is_err(), "{change}");
    }
}
#[cfg(unix)]
#[test]
fn selected_launchers_nonce_digest_output_and_timeout_are_checked() {
    let corpus: J = serde_json::from_str(include_str!("fixtures/history-runtime.json")).unwrap();
    let nonce = corpus["nonce"].as_str().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let shell = Path::new("/bin/sh").canonicalize().unwrap();
    fs::write(root.join("cli.py"), b"fixture").unwrap();
    let mut declaration = corpus["cases"][0]["declaration"].clone();
    setup(&mut declaration, &root, &shell);
    let response = root.join("response.json");
    fs::write(&response, serde_json::to_vec(&declaration).unwrap()).unwrap();
    let inventory = json!([{"id":"managed","argv":[shell,"-c","cat \"$1\"","probe",response],"executable":shell,"package_root":root}]);
    let expected = json!({"managed":{"sources":declaration["sources"]["digest"],"schemas":declaration["schemas"]["digest"],"native":declaration["native"]["digest"]}});
    let proof = probe_launchers(&inventory, &expected, nonce, Duration::from_secs(10)).unwrap();
    assert_eq!(proof["complete"], true);
    assert_eq!(proof["launchers"][0]["declaration"], declaration);
    assert!(
        probe_launchers(
            &inventory,
            &expected,
            "different_nonce_0123456789",
            Duration::from_secs(1)
        )
        .is_err()
    );
    let mut forged = expected.clone();
    forged["managed"]["sources"] = json!("0".repeat(64));
    assert!(probe_launchers(&inventory, &forged, nonce, Duration::from_secs(1)).is_err());
    for raw in [
        b"{} {}".as_slice(),
        b"{\"version\":1,\"version\":2}".as_slice(),
        b"NaN".as_slice(),
    ] {
        fs::write(&response, raw).unwrap();
        assert_eq!(
            probe_launchers(&inventory, &expected, nonce, Duration::from_secs(1))
                .unwrap_err()
                .0,
            "invalid_runtime_json"
        );
    }
    let mut limited = inventory.clone();
    limited[0]["argv"] = json!([shell, "-c", "sleep 5"]);
    assert_eq!(
        probe_launchers(&limited, &expected, nonce, Duration::from_millis(30))
            .unwrap_err()
            .0,
        "launcher_probe_failed"
    );
    limited[0]["argv"] = json!([shell, "-c", "head -c 2097153 /dev/zero"]);
    assert_eq!(
        probe_launchers(&limited, &expected, nonce, Duration::from_secs(2))
            .unwrap_err()
            .0,
        "launcher_probe_failed"
    );
    let marker = root.join("ran");
    limited[0]["argv"] = json!([shell, "-c", r#"touch "$1""#, "probe", marker]);
    limited[0]["id"] = json!("invalid id");
    assert!(probe_launchers(&limited, &expected, nonce, Duration::from_secs(1)).is_err());
    assert!(!marker.exists());
}
#[test]
#[ignore = "requires explicit KPOP_TEST_MANAGED_LAUNCHERS deployment selection"]
fn actual_configured_managed_launcher_is_probed() {
    let path = std::env::var_os("KPOP_TEST_MANAGED_LAUNCHERS")
        .expect("explicit managed launcher selection");
    let selection: J = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let nonce = format!(
        "native_probe_{:032x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let proof = probe_launchers(
        &selection["inventory"],
        &selection["expected"],
        &nonce,
        Duration::from_secs(10),
    )
    .unwrap();
    assert_eq!(proof["complete"], true);
    assert!(!proof["launchers"].as_array().unwrap().is_empty());
    println!(
        "{} actual configured launcher(s) verified",
        proof["launchers"].as_array().unwrap().len()
    );
}

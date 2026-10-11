//! The admitted platform's real build/install/read/update boundary.
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use serde_json::Value;
use std::{fs, path::Path, process::Command};

mod common;
use common::WritableCleanup;

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for item in fs::read_dir(from).unwrap() {
        let item = item.unwrap();
        let destination = to.join(item.file_name());
        if item.file_type().unwrap().is_dir() {
            copy_tree(&item.path(), &destination);
        } else {
            fs::copy(item.path(), destination).unwrap();
        }
    }
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn commit(root: &Path) {
    git(root, &["add", "-A"]);
    git(
        root,
        &[
            "-c",
            "user.name=Model fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "Fixture revision",
        ],
    );
}

fn run(binary: &Path, cwd: &Path, arguments: &[&str], succeeds: bool) -> Value {
    let output = Command::new(binary)
        .args(arguments)
        .current_dir(cwd)
        .env("KPOP_BIN", "/unavailable/global/kpop")
        .env("KPOPPER_NATIVE_RESOURCES", "/unavailable/ambient/resources")
        .output()
        .unwrap();
    assert_eq!(
        output.status.success(),
        succeeds,
        "binary={} cwd={} arguments={arguments:?} stdout={} stderr={}",
        binary.display(),
        cwd.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = if succeeds {
        &output.stdout
    } else {
        &output.stderr
    };
    serde_json::from_slice(bytes).unwrap()
}

#[test]
fn real_bundle_build_setup_read_upgrade_and_rollback() {
    let archive = std::env::var("KPOP_MODEL_ENGINE_ARCHIVE")
        .expect("provide the pinned darwin-arm64 native archive");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let _cleanup = WritableCleanup(root.clone());
    let source = root.join("source");
    let template = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/model-plugin");
    copy_tree(&template, &source);
    git(&source, &["init", "-q"]);
    commit(&source);
    let binary = Path::new(env!("CARGO_BIN_EXE_kpop-model"));
    let bundle_a = root.join("bundle-a");
    let first = run(
        binary,
        &root,
        &[
            "build",
            "--source",
            source.to_str().unwrap(),
            "--descriptor",
            "model-package.json",
            "--engine-archive",
            &archive,
            "--output",
            bundle_a.to_str().unwrap(),
        ],
        true,
    );
    let digest_a = first["package_digest"].as_str().unwrap().to_owned();

    let installed = root.join("installed");
    copy_tree(&bundle_a, &installed);
    let cache = root.join("private-cache");
    let caller = root.join("unrelated-project");
    fs::create_dir(&caller).unwrap();
    let sentinel = b"the caller's record must never supply model values\n";
    fs::write(caller.join("GROUNDING.yaml"), sentinel).unwrap();
    let invoke = |arguments: &[&str], succeeds| {
        let mut args = vec![
            "--bundle",
            installed.to_str().unwrap(),
            "--cache",
            cache.to_str().unwrap(),
        ];
        args.extend_from_slice(arguments);
        run(&installed.join("bin/kpop-model"), &caller, &args, succeeds)
    };
    invoke(&["setup"], true);
    invoke(&["setup"], true);
    for operation in ["pull", "context"] {
        let result = invoke(&[operation, "pricing.standard_hourly_rate"], true);
        assert_eq!(result["package"]["digest"], digest_a);
    }
    invoke(&["search", "weekend"], true);
    assert_eq!(fs::read(caller.join("GROUNDING.yaml")).unwrap(), sentinel);

    // Change the model's path as well as the version: rollback must select the
    // old descriptor, not just an older engine paired with current paths.
    let descriptor_path = source.join("model-package.json");
    let mut descriptor: Value =
        serde_json::from_slice(&fs::read(&descriptor_path).unwrap()).unwrap();
    descriptor["version"] = "0.2.0".into();
    descriptor["model"] = "knowledge/venue-v2/GROUNDING.yaml".into();
    fs::rename(
        source.join("knowledge/venue-rates"),
        source.join("knowledge/venue-v2"),
    )
    .unwrap();
    fs::write(
        &descriptor_path,
        serde_json::to_vec_pretty(&descriptor).unwrap(),
    )
    .unwrap();
    commit(&source);
    let bundle_b = root.join("bundle-b");
    run(
        binary,
        &root,
        &[
            "build",
            "--source",
            source.to_str().unwrap(),
            "--descriptor",
            "model-package.json",
            "--engine-archive",
            &archive,
            "--output",
            bundle_b.to_str().unwrap(),
        ],
        true,
    );
    fs::rename(&installed, root.join("previous-installation")).unwrap();
    copy_tree(&bundle_b, &installed);
    let refusal = invoke(&["pull", "pricing.standard_hourly_rate"], false);
    assert!(refusal["error"]
        .as_str()
        .unwrap()
        .contains("differs from this bundle"));
    invoke(&["setup"], true);
    invoke(&["activate", "--digest", &digest_a], true);
    let restored = invoke(&["pull", "pricing.standard_hourly_rate"], true);
    assert_eq!(restored["package"]["digest"], digest_a);
    assert_eq!(restored["package"]["version"], "0.1.0");
    assert_eq!(fs::read(caller.join("GROUNDING.yaml")).unwrap(), sentinel);
}

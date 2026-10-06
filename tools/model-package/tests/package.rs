use kpop_model::{
    manifest::{
        Capability, Descriptor, Engine, FileIdentity, PackageLock, Smoke, ENGINE_SHA256,
        ENGINE_SIZE, ENGINE_VERSION, LOCK_FORMAT, PACKAGE_FORMAT, TARGET,
    },
    package::{build, verify_bundle},
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path};

fn engine_path() -> std::path::PathBuf {
    std::env::var_os("KPOP_MODEL_ENGINE_ARCHIVE")
        .map(Into::into)
        .expect("KPOP_MODEL_ENGINE_ARCHIVE must point to the pinned archive")
}

fn creator_fixture_path() -> std::path::PathBuf {
    std::env::var_os("KPOP_MODEL_SOURCE")
        .map(Into::into)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/model-plugin")
        })
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let source = entry.path();
        let target = to.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source).unwrap();
        if metadata.is_dir() {
            copy_tree(&source, &target);
        } else if metadata.is_file() {
            fs::copy(&source, &target).unwrap();
        } else {
            panic!("unsupported fixture member: {}", source.display());
        }
    }
}

fn git_ok(root: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn committed_source(mutate: impl FnOnce(&Path)) -> tempfile::TempDir {
    let from = creator_fixture_path();
    assert!(
        from.join("model-package.json").is_file(),
        "creator fixture not found at {}",
        from.display()
    );
    let temp = tempfile::tempdir().unwrap();
    copy_tree(&from, temp.path());
    mutate(temp.path());
    git_ok(temp.path(), &["init", "-q"]);
    git_ok(
        temp.path(),
        &["config", "user.email", "package-tests@example.invalid"],
    );
    git_ok(temp.path(), &["config", "user.name", "Package Tests"]);
    git_ok(temp.path(), &["add", "."]);
    git_ok(temp.path(), &["commit", "-qm", "fixture"]);
    temp
}

fn build_fixture(source: &Path, output: &Path) -> kpop_model::Result<String> {
    build(
        source,
        Path::new("model-package.json"),
        &engine_path(),
        None,
        output,
    )
}

fn sha(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}
fn put(root: &Path, n: &str, b: &[u8]) {
    let p = root.join(n);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, b).unwrap()
}
// This fixture exercises package inventory integrity. Native history semantics
// are checked by the real-engine runtime integration suite.
fn fixture() -> tempfile::TempDir {
    let engine = std::path::PathBuf::from(
        std::env::var_os("KPOP_MODEL_ENGINE_ARCHIVE")
            .expect("set KPOP_MODEL_ENGINE_ARCHIVE to the pinned darwin-arm64 release archive"),
    );
    assert!(engine.is_file(), "pinned engine archive is absent");
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    let eb = fs::read(&engine).unwrap();
    assert_eq!(eb.len() as u64, ENGINE_SIZE);
    assert_eq!(sha(&eb), ENGINE_SHA256);
    let d = Descriptor {
        format: PACKAGE_FORMAT.into(),
        id: "demo".into(),
        version: "1.0.0".into(),
        target: TARGET.into(),
        capability: Capability::Read,
        model: "model/GROUNDING.yaml".into(),
        skill: "skills/demo/SKILL.md".into(),
        include: vec![],
        engine: Engine {
            version: ENGINE_VERSION.into(),
            sha256: ENGINE_SHA256.into(),
            size: ENGINE_SIZE,
        },
        adapter: None,
        smoke: Smoke::Read {
            id: "demo.fact".into(),
        },
    };
    let db = serde_json::to_vec_pretty(&d).unwrap();
    put(r, "model-package.json", &db);
    for (n, b) in [
        ("model/GROUNDING.yaml", b"known: {}\n".as_slice()),
        ("model/.gitattributes", b"*.yaml text\n".as_slice()),
        (
            "model/.kpopper/history.yaml",
            b"format: native\n".as_slice(),
        ),
        ("skills/demo/SKILL.md", b"# Demo\n".as_slice()),
        ("bin/kpop-model", b"launcher".as_slice()),
    ] {
        put(r, n, b)
    }
    put(r, "assets/kpopper.tar.gz", &eb);
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(eb.as_slice()));
    let mut members = vec![];
    for ent in ar.entries().unwrap() {
        let mut e = ent.unwrap();
        let ty = e.header().entry_type();
        let path = e.path().unwrap().to_string_lossy().into_owned();
        let size = e.size();
        let mode = format!("0o{:o}", e.header().mode().unwrap());
        let digest = if ty.is_file() {
            use std::io::Read;
            let mut hasher = Sha256::new();
            let mut buffer = [0u8; 65536];
            loop {
                let n = e.read(&mut buffer).unwrap();
                if n == 0 {
                    break;
                }
                hasher.update(&buffer[..n]);
            }
            Some(format!("{:x}", hasher.finalize()))
        } else {
            None
        };
        members.push(serde_json::json!({"path":path,"size":size,"mode":mode,
            "type":if ty.is_file(){"file"}else{"dir"},"sha256":digest}));
    }
    members.sort_by_key(|v| v["path"].as_str().unwrap().to_owned());
    put(
        r,
        "engine-members.json",
        &serde_json::to_vec_pretty(
            &serde_json::json!({"archive_sha256":ENGINE_SHA256,"members":members}),
        )
        .unwrap(),
    );
    let plugin = serde_json::json!({"name":"demo","version":"1.0.0","description":"demo native knowledge model","skills":"./skills/"});
    put(
        r,
        ".codex-plugin/plugin.json",
        &serde_json::to_vec_pretty(&plugin).unwrap(),
    );
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(r.join("bin/kpop-model"), fs::Permissions::from_mode(0o755)).unwrap();
    let mut files = BTreeMap::new();
    for n in [
        "model/GROUNDING.yaml",
        "model/.gitattributes",
        "model/.kpopper/history.yaml",
        "skills/demo/SKILL.md",
        "bin/kpop-model",
        "assets/kpopper.tar.gz",
        "engine-members.json",
        ".codex-plugin/plugin.json",
        "model-package.json",
    ] {
        let p = r.join(n);
        let b = fs::read(&p).unwrap();
        files.insert(
            n.to_owned(),
            FileIdentity {
                sha256: sha(&b),
                size: b.len() as u64,
                executable: n == "bin/kpop-model",
            },
        );
    }
    let l = PackageLock {
        format: LOCK_FORMAT.into(),
        descriptor_sha256: sha(&db),
        source_commit: "a".repeat(40),
        engine: d.engine,
        files,
    };
    put(
        r,
        "model-package.lock.json",
        &serde_json::to_vec(&l).unwrap(),
    );
    t
}
#[test]
fn accepts_complete_bundle_and_rejects_each_member_failure() {
    let t = fixture();
    let r = t.path();
    verify_bundle(r).unwrap_or_else(|e| panic!("valid fixture rejected: {e}"));
    fs::remove_file(r.join("model/GROUNDING.yaml")).unwrap();
    assert!(verify_bundle(r)
        .unwrap_err()
        .to_string()
        .contains("bundle file closure differs"));
}
#[test]
fn rejects_extra_and_corrupt_files() {
    let t = fixture();
    let r = t.path();
    verify_bundle(r).unwrap_or_else(|e| panic!("valid fixture rejected: {e}"));
    put(r, "unexpected", b"x");
    assert!(verify_bundle(r)
        .unwrap_err()
        .to_string()
        .contains("bundle file closure differs"));
    fs::remove_file(r.join("unexpected")).unwrap();
    put(r, "model/GROUNDING.yaml", b"changed");
    assert!(verify_bundle(r)
        .unwrap_err()
        .to_string()
        .contains("bundle member mismatch"));
}
#[test]
fn rejects_lock_path_escape_and_duplicate_file_keys() {
    let t = fixture();
    let r = t.path();
    let p = r.join("model-package.lock.json");
    let mut l: PackageLock = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
    l.files.insert(
        "../escape".into(),
        FileIdentity {
            sha256: sha(b"x"),
            size: 1,
            executable: false,
        },
    );
    put(
        r,
        "model-package.lock.json",
        &serde_json::to_vec(&l).unwrap(),
    );
    assert!(verify_bundle(r)
        .unwrap_err()
        .to_string()
        .contains("unsafe relative path"));
    let b=br#"{"format":"kpop-model-lock/v1","format":"kpop-model-lock/v1","descriptor_sha256":"x","source_commit":"x","engine":{},"files":{"a":{},"a":{}}}"#;
    put(r, "model-package.lock.json", b);
    assert!(verify_bundle(r)
        .unwrap_err()
        .to_string()
        .contains("duplicate JSON key"));
}

#[test]
fn committed_creator_fixture_builds_only_after_native_validation() {
    let source = committed_source(|_| {});
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("package");
    let digest = build_fixture(source.path(), &output).unwrap();
    let (_, _, verified) = verify_bundle(&output).unwrap();
    assert_eq!(digest, verified);
}

#[test]
fn native_build_gate_rejects_removed_history_and_tampered_view() {
    let missing_history = committed_source(|root| {
        let dir = root.join("knowledge/venue-rates/.kpopper/history-commits");
        let mut files: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        fs::remove_file(files.remove(0)).unwrap();
    });
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("history-bad");
    let error = build_fixture(missing_history.path(), &output)
        .unwrap_err()
        .to_string();
    assert!(
        !output.exists(),
        "failed validation must not publish a built package"
    );
    assert!(
        error.contains("history") || error.contains("native") || error.contains("model"),
        "unexpected missing-history error: {error}"
    );

    let tampered_view = committed_source(|root| {
        let p = root.join("knowledge/venue-rates/GROUNDING.yaml");
        let before = fs::read_to_string(&p).unwrap();
        let after = before.replacen("!!int \"10\"", "!!int \"99\"", 1);
        assert_ne!(before, after);
        fs::write(p, after).unwrap();
    });
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("view-bad");
    let error = build_fixture(tampered_view.path(), &output)
        .unwrap_err()
        .to_string();
    assert!(
        !output.exists(),
        "failed validation must not publish a built package"
    );
    assert!(
        error.contains("history") || error.contains("native") || error.contains("model"),
        "unexpected tampered-view error: {error}"
    );
}

#[test]
fn diagnoses_tracked_transient_and_untracked_model_paths() {
    let tracked = committed_source(|root| {
        put(
            root,
            "knowledge/venue-rates/.kpopper/.history-local/journal.json",
            b"{}\n",
        )
    });
    let output = tempfile::tempdir().unwrap();
    let error = build_fixture(tracked.path(), &output.path().join("transient"))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(".kpopper/.history-local/journal.json"),
        "transient diagnostic omitted path: {error}"
    );
    let untracked = committed_source(|_| {});
    let unexpected = "knowledge/venue-rates/unexpected.bin";
    put(untracked.path(), unexpected, b"x");
    let error = build_fixture(untracked.path(), &output.path().join("untracked"))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(unexpected),
        "closure diagnostic omitted path: {error}"
    );
}

mod common;
use common::WritableCleanup;

use kpop_model::runtime::{execute, Operation};

#[test]
fn execute_rejects_a_directory_without_a_verified_package() {
    let temp = tempfile::tempdir().unwrap();
    let _cleanup = WritableCleanup(temp.path().to_path_buf());
    assert!(execute(temp.path(), None, Operation::Setup).is_err());
}

#[test]
fn real_engine_setup_recovers_a_corrupt_managed_generation_and_keeps_active_identity() {
    use kpop_model::{
        package,
        runtime::{execute, Operation},
    };
    use sha2::{Digest, Sha256};
    use std::{fs, path::Path, process::Command};

    fn copy_tree(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).unwrap();
        for entry in fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let from = entry.path();
            let to = dst.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&from, &to);
            } else if entry.file_type().unwrap().is_file() {
                fs::copy(from, to).unwrap();
            }
        }
    }
    fn git(source: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(source)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    let archive = std::path::PathBuf::from(
        std::env::var_os("KPOP_MODEL_ENGINE_ARCHIVE")
            .expect("set KPOP_MODEL_ENGINE_ARCHIVE to the pinned native release"),
    );
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/model-plugin");
    let temp = tempfile::tempdir().unwrap();
    let _cleanup = WritableCleanup(temp.path().to_path_buf());
    let source = temp.path().join("source");
    copy_tree(&repository, &source);
    git(&source, &["init", "-q"]);
    git(&source, &["add", "."]);
    let commit = Command::new("git")
        .arg("-C")
        .arg(&source)
        .args([
            "-c",
            "user.name=Runtime Fixture",
            "-c",
            "user.email=runtime@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ])
        .status()
        .unwrap();
    assert!(commit.success());

    let bundle = temp.path().join("bundle");
    package::build(
        &source,
        Path::new("model-package.json"),
        &archive,
        None,
        &bundle,
    )
    .unwrap();
    let cache = temp.path().canonicalize().unwrap().join("cache");
    let setup = execute(&bundle, Some(&cache), Operation::Setup).unwrap();
    let digest = setup["package"]["digest"].as_str().unwrap().to_owned();
    assert_eq!(setup["result"]["active"], digest);
    let origin = bundle.canonicalize().unwrap();
    let install_key = format!(
        "{:x}",
        Sha256::digest(format!("{}\0venue-rates", origin.display()).as_bytes())
    );
    let namespace = cache.join(install_key);
    let generation = namespace.join("generations").join(&digest);
    let active_before = fs::read(namespace.join("active.json")).unwrap();
    let skill_rel = "skills/venue-rates/SKILL.md";
    let bundle_skill = bundle.join(skill_rel);
    let valid_bundle_skill = fs::read(&bundle_skill).unwrap();
    let installed_skill = generation.join("payload").join(skill_rel);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&installed_skill, fs::Permissions::from_mode(0o644)).unwrap();
    }
    fs::write(&installed_skill, b"damaged active generation").unwrap();

    fs::write(&bundle_skill, b"tampered invoking bundle").unwrap();
    assert!(execute(&bundle, Some(&cache), Operation::Setup).is_err());
    assert!(
        generation.exists(),
        "failed package validation must preserve the active generation"
    );
    assert_eq!(
        fs::read(namespace.join("active.json")).unwrap(),
        active_before
    );
    assert!(fs::read_dir(namespace.join("generations"))
        .unwrap()
        .all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".quarantine-")));

    fs::write(&bundle_skill, valid_bundle_skill).unwrap();
    let recovered = execute(&bundle, Some(&cache), Operation::Setup).unwrap();
    assert_eq!(recovered["result"]["active"], digest);
    assert_eq!(
        fs::read(namespace.join("active.json")).unwrap(),
        active_before
    );
    assert_eq!(
        fs::read(generation.join("payload").join(skill_rel)).unwrap(),
        fs::read(&bundle_skill).unwrap()
    );
    let quarantined = fs::read_dir(namespace.join("generations"))
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(&format!(".quarantine-{digest}-"))
        })
        .expect("damaged generation should be retained in quarantine")
        .path();
    assert_eq!(
        fs::read(quarantined.join("payload").join(skill_rel)).unwrap(),
        b"damaged active generation"
    );
    let doctor = execute(&bundle, Some(&cache), Operation::Doctor).unwrap();
    assert_eq!(doctor["result"]["selected"]["version"], "0.1.0");
    assert_eq!(
        doctor["result"]["selected"]["model"],
        "knowledge/venue-rates/GROUNDING.yaml"
    );
    assert_eq!(
        doctor["result"]["selected"]["skill"],
        "skills/venue-rates/SKILL.md"
    );

    let old_descriptor: serde_json::Value =
        serde_json::from_slice(&fs::read(source.join("model-package.json")).unwrap()).unwrap();
    copy_tree(
        &source.join("knowledge/venue-rates"),
        &source.join("knowledge/venue-rates-v2"),
    );
    fs::create_dir_all(source.join("skills/venue-rates-v2")).unwrap();
    fs::copy(
        source.join("skills/venue-rates/SKILL.md"),
        source.join("skills/venue-rates-v2/SKILL.md"),
    )
    .unwrap();
    let mut new_descriptor = old_descriptor;
    new_descriptor["version"] = serde_json::json!("0.2.0");
    new_descriptor["model"] = serde_json::json!("knowledge/venue-rates-v2/GROUNDING.yaml");
    new_descriptor["skill"] = serde_json::json!("skills/venue-rates-v2/SKILL.md");
    fs::write(
        source.join("model-package.json"),
        serde_json::to_vec_pretty(&new_descriptor).unwrap(),
    )
    .unwrap();
    git(&source, &["add", "."]);
    let commit = Command::new("git")
        .arg("-C")
        .arg(&source)
        .args([
            "-c",
            "user.name=Runtime Fixture",
            "-c",
            "user.email=runtime@example.invalid",
            "commit",
            "-qm",
            "second generation",
        ])
        .status()
        .unwrap();
    assert!(commit.success());
    let next = temp.path().join("next-bundle");
    package::build(
        &source,
        Path::new("model-package.json"),
        &archive,
        None,
        &next,
    )
    .unwrap();
    fs::remove_dir_all(&bundle).unwrap();
    fs::rename(&next, &bundle).unwrap();
    let upgraded = execute(&bundle, Some(&cache), Operation::Setup).unwrap();
    let v2 = upgraded["package"]["digest"].as_str().unwrap().to_owned();
    assert_ne!(v2, digest);
    let activated = execute(
        &bundle,
        Some(&cache),
        Operation::Activate {
            digest: digest.clone(),
        },
    )
    .unwrap();
    assert_eq!(activated["result"]["selected"]["digest"], digest);
    assert_eq!(activated["result"]["selected"]["version"], "0.1.0");
    assert_eq!(
        activated["result"]["selected"]["model"],
        "knowledge/venue-rates/GROUNDING.yaml"
    );
    assert_eq!(
        activated["result"]["selected"]["skill"],
        "skills/venue-rates/SKILL.md"
    );
    let rolled_back = execute(&bundle, Some(&cache), Operation::Doctor).unwrap();
    assert_eq!(rolled_back["result"]["selected"]["version"], "0.1.0");
    assert_eq!(
        rolled_back["result"]["selected"]["model"],
        "knowledge/venue-rates/GROUNDING.yaml"
    );
    assert_eq!(
        rolled_back["result"]["selected"]["skill"],
        "skills/venue-rates/SKILL.md"
    );
    let versions = execute(&bundle, Some(&cache), Operation::Versions).unwrap();
    let old = versions["result"]["generations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["selected"]["digest"] == digest)
        .unwrap();
    assert_eq!(old["selected"]["version"], "0.1.0");
    assert_eq!(
        old["selected"]["model"],
        "knowledge/venue-rates/GROUNDING.yaml"
    );
    assert_eq!(old["selected"]["skill"], "skills/venue-rates/SKILL.md");
}

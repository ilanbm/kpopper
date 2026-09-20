use kpop_native::reasoning_runtime::{OperationalBounds, Runtime, target_name};
use std::{
    fs,
    path::{Path, PathBuf},
};
fn archive() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/reasoning/native")
        .join(format!("{}.kpopper-runtime", target_name().unwrap()))
}
#[test]
fn actual_packaged_lean_matches_pinned_python_across_scalar_and_composition_requests() {
    let cache = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(&archive(), cache.path(), OperationalBounds::default()).unwrap();
    let data: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/reasoning-runtime.json")).unwrap();
    assert_eq!(runtime.manifest["source_sha256"], data["source_sha256"]);
    let cases = data["cases"].as_array().unwrap();
    let requests = cases
        .iter()
        .map(|c| c["request"].clone())
        .collect::<Vec<_>>();
    let results = runtime.request_many(&requests).unwrap();
    for (c, result) in cases.iter().zip(results) {
        assert_eq!(result, c["output"], "{}", c["name"]);
    }
    assert_eq!(
        runtime.implementation["modified_libraries"],
        serde_json::json!([])
    );
    assert_eq!(
        runtime.implementation["adapter_source_sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    let reopened = Runtime::open(&archive(), cache.path(), OperationalBounds::default()).unwrap();
    assert_eq!(runtime.implementation, reopened.implementation);
}
#[test]
fn modified_runtime_members_are_refused_and_replaceable_libraries_disclosed() {
    let cache = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(&archive(), cache.path(), OperationalBounds::default()).unwrap();
    let lib = runtime.manifest["libraries"][0].as_str().unwrap();
    let path = runtime.root.join(lib);
    let old = fs::read(&path).unwrap();
    fs::write(&path, b"replacement").unwrap();
    let request = serde_json::json!({"nodes":{},"declared":[],"expression":{"num":"1"}});
    assert_eq!(
        runtime.request_many(&[request]).unwrap_err().0,
        "runtime changed; reopen before evaluating"
    );
    let changed = Runtime::open(&archive(), cache.path(), OperationalBounds::default()).unwrap();
    assert_eq!(
        changed.implementation["modified_libraries"],
        serde_json::json!([lib])
    );
    fs::write(path, old).unwrap();
    fs::write(&runtime.binary, b"modified executable").unwrap();
    assert!(
        Runtime::open(&archive(), cache.path(), OperationalBounds::default())
            .unwrap_err()
            .0
            .starts_with("runtime member checksum changed:")
    );
}
#[cfg(unix)]
#[test]
fn process_deadline_shared_output_cap_and_owned_group_cleanup_are_bounded() {
    use kpop_native::reasoning_runtime::run_bounded;
    use std::time::{Duration, Instant};
    let shell = Path::new("/bin/sh");
    assert_eq!(
        run_bounded(
            shell,
            &["-c", "cat"],
            b"hello".to_vec(),
            Duration::from_secs(1),
            64
        )
        .unwrap(),
        b"hello"
    );
    let start = Instant::now();
    let error = run_bounded(
        shell,
        &["-c", "sleep 10 & wait"],
        vec![],
        Duration::from_millis(50),
        64,
    )
    .unwrap_err();
    assert_eq!(error.0, "runtime_timeout");
    assert!(start.elapsed() < Duration::from_secs(2));
    let error = run_bounded(
        shell,
        &["-c", "while :; do printf 0123456789 >&2; done"],
        vec![],
        Duration::from_secs(1),
        128,
    )
    .unwrap_err();
    assert_eq!(error.0, "output_limit");
    assert_eq!(
        run_bounded(
            shell,
            &["-c", "exit 1"],
            vec![],
            Duration::from_secs(1),
            128
        )
        .unwrap_err()
        .0,
        "native reasoning process failed"
    );
}
#[test]
fn archive_paths_and_links_are_rejected_before_cache_extraction() {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    for (name, link) in [("../escape", false), ("link", true)] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("runtime.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        if link {
            zip.add_symlink(name, "outside", SimpleFileOptions::default())
                .unwrap();
        } else {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(b"data").unwrap();
        }
        zip.finish().unwrap();
        let error = Runtime::open(
            &path,
            &temp.path().join("cache"),
            OperationalBounds::default(),
        )
        .unwrap_err();
        assert_eq!(
            error.0,
            if link {
                "unsupported runtime archive member"
            } else {
                "runtime archive path escapes its directory"
            }
        );
        assert!(!temp.path().join("cache").exists());
        assert!(!temp.path().join("escape").exists());
    }
}
#[test]
fn batch_limits_refuse_before_launch() {
    let cache = tempfile::tempdir().unwrap();
    let request = serde_json::json!({"nodes":{},"declared":[],"expression":{"num":"1"}});
    let runtime = Runtime::open(
        &archive(),
        cache.path(),
        OperationalBounds {
            batch_requests: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        runtime
            .request_many(&[request.clone(), request.clone()])
            .unwrap_err()
            .0,
        "batch_request_limit"
    );
    let runtime = Runtime::open(
        &archive(),
        cache.path(),
        OperationalBounds {
            input_bytes: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        runtime.request_many(&[request]).unwrap_err().0,
        "batch_input_limit"
    );
}
#[cfg(unix)]
#[test]
fn exited_parent_does_not_leave_the_call_waiting_for_inherited_pipes() {
    use std::time::{Duration, Instant};
    let start = Instant::now();
    let error = kpop_native::reasoning_runtime::run_bounded(
        Path::new("/bin/sh"),
        &["-c", "sleep 10 & exit 0"],
        vec![],
        Duration::from_millis(50),
        128,
    )
    .unwrap_err();
    assert_eq!(error.0, "runtime_timeout");
    assert!(start.elapsed() < Duration::from_secs(2));
}
#[test]
fn foreign_source_manifest_cannot_activate_an_otherwise_valid_archive() {
    use std::io::{Read, Write};
    use zip::write::SimpleFileOptions;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("foreign.zip");
    let mut source = zip::ZipArchive::new(fs::File::open(archive()).unwrap()).unwrap();
    let mut target = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    for index in 0..source.len() {
        let mut member = source.by_index(index).unwrap();
        let name = member.name().to_owned();
        let mut bytes = Vec::new();
        member.read_to_end(&mut bytes).unwrap();
        if name == "manifest.json" {
            let mut manifest: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            manifest["source_sha256"] = serde_json::json!("0".repeat(64));
            bytes = serde_json::to_vec(&manifest).unwrap();
        }
        target
            .start_file(name, SimpleFileOptions::default())
            .unwrap();
        target.write_all(&bytes).unwrap();
    }
    target.finish().unwrap();
    assert_eq!(
        Runtime::open(
            &path,
            &temp.path().join("cache"),
            OperationalBounds::default()
        )
        .unwrap_err()
        .0,
        "packaged runtime does not match its source revision"
    );
    assert!(!temp.path().join("cache").exists());
}

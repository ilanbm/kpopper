use kpop_native::{
    Error, Result,
    history_transaction::PreparedMutation,
    history_transaction_fs::{self as F, Direction, DirectoryGuard},
};
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
fn install(root: &Path, files: &Value) {
    for (name, raw) in files.as_object().unwrap() {
        let p = root.join(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, unhex(raw.as_str().unwrap())).unwrap();
    }
}
fn tree(root: &Path) -> Value {
    fn walk(root: &Path, path: &Path, items: &mut BTreeMap<String, String>) {
        for item in fs::read_dir(path).unwrap() {
            let p = item.unwrap().path();
            if p.is_dir() {
                walk(root, &p, items);
            } else {
                let raw = fs::read(&p).unwrap();
                items.insert(
                    p.strip_prefix(root).unwrap().to_str().unwrap().into(),
                    raw.iter().map(|b| format!("{b:02x}")).collect(),
                );
            }
        }
    }
    let mut items = BTreeMap::new();
    walk(root, root, &mut items);
    serde_json::to_value(items).unwrap()
}
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/history-recovery.json")).unwrap()
}
fn mutation() -> PreparedMutation {
    PreparedMutation::from_bytes(fixture()["mutation"].as_str().unwrap().as_bytes()).unwrap()
}
fn code<T>(result: Result<T>) -> Option<String> {
    result.err().map(|e| e.0.split(':').next().unwrap().into())
}

#[test]
fn recovery_from_every_python_interruption_preserves_exact_images() {
    let f = fixture();
    let journal = f["journal"].as_str().unwrap();
    for c in f["cases"].as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        install(root, &c["before"]);
        assert_eq!(
            code(F::reader_guard(root, journal, || Ok(()))),
            c["reader_error"].as_str().map(str::to_owned),
            "{}",
            c["name"]
        );
        let direction = if c["direction"] == "before" {
            Direction::Before
        } else {
            Direction::After
        };
        let result = F::recover_legacy(root, journal, direction, &mut |_| Ok(()), None, None);
        assert_eq!(
            code(result),
            c["error"].as_str().map(str::to_owned),
            "{}",
            c["name"]
        );
        assert_eq!(tree(root), c["after"], "{}", c["name"]);
    }
}
#[test]
fn publication_matches_python_and_callback_failure_keeps_reader_guard() {
    let f = fixture();
    let m = mutation();
    let journal = f["journal"].as_str().unwrap();
    for fail in [false, true] {
        let c = f["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| {
                c["name"]
                    == if fail {
                        "crash-3-after-edit-False"
                    } else {
                        "crash-4-after-edit-False"
                    }
            })
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        install(temp.path(), &c["initial"]);
        let mut callback = |_: &kpop_native::value::TypedValue| {
            if fail {
                Err(Error("injected".into()))
            } else {
                Ok(())
            }
        };
        let result = F::publish_legacy(
            temp.path(),
            journal,
            &m,
            &mut |_| Ok(()),
            Some(&mut callback),
        );
        assert_eq!(
            code(result),
            c["publication_error"].as_str().map(str::to_owned)
        );
        assert_eq!(tree(temp.path()), c["publication"]);
    }
}
#[test]
fn verifier_collision_and_stale_images_fail_before_publication() {
    let f = fixture();
    let m = mutation();
    let journal = f["journal"].as_str().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    install(root, &f["cases"][0]["initial"]);
    let before = tree(root);
    assert_eq!(
        code(F::publish_legacy(
            root,
            journal,
            &m,
            &mut |_| Err(Error("capability_changed".into())),
            None
        ))
        .as_deref(),
        Some("capability_changed")
    );
    assert_eq!(tree(root), before);
    fs::write(root.join("GROUNDING.yaml"), b"unrelated").unwrap();
    let before = tree(root);
    assert_eq!(
        code(F::publish_legacy(root, journal, &m, &mut |_| Ok(()), None)).as_deref(),
        Some("concurrent_edit")
    );
    assert_eq!(tree(root), before);
    F::publish_immutable(root, "object.yaml", b"original").unwrap();
    F::publish_immutable(root, "object.yaml", b"original").unwrap();
    assert_eq!(
        code(F::publish_immutable(root, "object.yaml", b"different")).as_deref(),
        Some("immutable_collision")
    );
    assert_eq!(fs::read(root.join("object.yaml")).unwrap(), b"original");
}
#[cfg(unix)]
#[test]
fn directory_locks_reenter_refuse_upgrade_and_serialize_threads() {
    use std::sync::mpsc;
    use std::time::Duration;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let writer = DirectoryGuard::acquire(root, true).unwrap();
    let nested = DirectoryGuard::acquire(root, false).unwrap();
    drop(nested);
    let (tx, rx) = mpsc::channel();
    let path = root.to_owned();
    let thread = std::thread::spawn(move || {
        tx.send("started").unwrap();
        let _guard = DirectoryGuard::acquire(&path, false).unwrap();
        tx.send("entered").unwrap();
    });
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "started");
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    drop(writer);
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "entered");
    thread.join().unwrap();
    let _reader = DirectoryGuard::acquire(root, false).unwrap();
    assert_eq!(
        code(DirectoryGuard::acquire(root, true)).as_deref(),
        Some("lock_upgrade_refused")
    );
}
#[cfg(unix)]
#[test]
fn path_alias_is_allowed_but_member_symlinks_are_refused() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir(&root).unwrap();
    let alias = temp.path().join("alias");
    symlink(&root, &alias).unwrap();
    F::publish_immutable(&alias, "a", b"original").unwrap();
    symlink("a", root.join("link")).unwrap();
    assert_eq!(
        code(F::publish_immutable(&root, "link", b"new")).as_deref(),
        Some("symlink_path")
    );
    assert_eq!(fs::read(root.join("a")).unwrap(), b"original");
}

#[test]
fn authority_switch_recovery_matches_python_and_retains_immutable_files() {
    let f: Value =
        serde_json::from_str(include_str!("fixtures/history-transition-recovery.json")).unwrap();
    let journal = f["journal"].as_str().unwrap();
    for c in f["cases"].as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        install(root, &c["before"]);
        let direction = if c["direction"] == "before" {
            Direction::Before
        } else {
            Direction::After
        };
        let result = F::recover_transition(root, journal, direction, &mut |_| Ok(()), None);
        assert_eq!(
            code(result),
            c["error"].as_str().map(str::to_owned),
            "{}",
            c["name"]
        );
        assert_eq!(tree(root), c["after"], "{}", c["name"]);
    }
}

#[test]
fn authority_publication_and_callback_failures_match_python() {
    let f: Value =
        serde_json::from_str(include_str!("fixtures/history-transition-recovery.json")).unwrap();
    let journal = f["journal"].as_str().unwrap();
    for mode in ["activate", "deactivate"] {
        for fail in [false, true] {
            let name = format!("{mode}-{}-after-none", if fail { 2 } else { 3 });
            let c = f["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["name"] == name)
                .unwrap();
            let m =
                PreparedMutation::from_bytes(c["mutation"].as_str().unwrap().as_bytes()).unwrap();
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            install(root, &c["initial"]);
            let mut callback = |_: &kpop_native::value::TypedValue| {
                if fail {
                    Err(Error("injected".into()))
                } else {
                    Ok(())
                }
            };
            assert_eq!(
                code(F::publish_transition(
                    root,
                    journal,
                    &m,
                    &mut |_| Ok(()),
                    Some(&mut callback)
                )),
                c["publication_error"].as_str().map(str::to_owned),
                "{name}"
            );
            assert_eq!(tree(root), c["publication"], "{name}");
        }
    }
}

#[cfg(unix)]
#[test]
fn locks_refuse_descending_acquisition_and_transition_rechecks_verifier_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let z = temp.path().join("z");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&z).unwrap();
    {
        let _guard = DirectoryGuard::acquire(&z, true).unwrap();
        assert_eq!(
            code(DirectoryGuard::acquire(&a, true)).as_deref(),
            Some("lock_order_refused")
        );
    }
    let f: Value =
        serde_json::from_str(include_str!("fixtures/history-transition-recovery.json")).unwrap();
    let c = &f["cases"][0];
    install(&a, &c["initial"]);
    let m = PreparedMutation::from_bytes(c["mutation"].as_str().unwrap().as_bytes()).unwrap();
    let result = F::publish_transition(
        &a,
        f["journal"].as_str().unwrap(),
        &m,
        &mut |_| {
            fs::write(a.join("GROUNDING.yaml"), b"changed")?;
            Ok(())
        },
        None,
    );
    assert_eq!(code(result).as_deref(), Some("concurrent_edit"));
    assert!(!a.join(f["journal"].as_str().unwrap()).exists());
}

#[test]
fn participant_readiness_preserves_unguarded_edits_and_rejects_foreign_replicas() {
    use kpop_native::{history_transaction::FileImage, identity::sha256, value::TypedValue as V};
    for foreign in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let canonical = temp.path().canonicalize().unwrap();
        let root = canonical.as_path();
        fs::create_dir(root.join("a")).unwrap();
        fs::create_dir(root.join("b")).unwrap();
        let authority=V::from_json(&serde_json::json!({"version":1,"record_id":"r","authority":"legacy","generation":0,"profile":"history/v1"})).unwrap();
        let baseline=V::from_json(&serde_json::json!({"transaction_root":root.to_str().unwrap(),"record_members":{"a/GROUNDING.yaml":sha256(b"entry"),"b/part.yaml":sha256(b"part")}})).unwrap();
        let receipt = kpop_native::history_transaction::semantic_receipt(
            "ordinary-reader/v1",
            &V::Map(BTreeMap::new()),
            &V::Map(BTreeMap::new()),
            &V::Map(BTreeMap::new()),
        )
        .unwrap();
        let m = PreparedMutation::prepare(
            "replicas",
            &authority,
            &baseline,
            vec![
                FileImage {
                    path: "a/GROUNDING.yaml".into(),
                    role: "record".into(),
                    before: Some(b"entry".to_vec()),
                    after: Some(b"entry-after".to_vec()),
                },
                FileImage {
                    path: "b/part.yaml".into(),
                    role: "record_member".into(),
                    before: Some(b"part".to_vec()),
                    after: Some(b"part-after".to_vec()),
                },
            ],
            &receipt,
            "a/GROUNDING.yaml",
            None,
        )
        .unwrap();
        let V::Map(data) = m.to_data() else { panic!() };
        let V::Text(digest) = &data["digest"] else {
            panic!()
        };
        let journal = "a/.kpopper/.transaction.json";
        fs::write(root.join("a/GROUNDING.yaml"), b"entry").unwrap();
        fs::write(
            root.join("b/part.yaml"),
            if foreign {
                b"part".as_slice()
            } else {
                b"new independent edit".as_slice()
            },
        )
        .unwrap();
        F::publish_immutable(root, journal, &m.to_bytes().unwrap()).unwrap();
        if foreign {
            F::publish_immutable(root, &format!("b/.history-local/{digest}.json"), b"{}").unwrap();
        }
        let before = tree(root);
        let result = F::recover_legacy(
            root,
            journal,
            Direction::Before,
            &mut |_| Ok(()),
            None,
            None,
        );
        if foreign {
            assert_eq!(code(result).as_deref(), Some("journal_replica_mismatch"));
            assert_eq!(tree(root), before);
        } else {
            result.unwrap();
            assert_eq!(
                fs::read(root.join("b/part.yaml")).unwrap(),
                b"new independent edit"
            );
            assert!(!root.join(journal).exists());
            fs::write(root.join("b/part.yaml"), b"part").unwrap();
            let mut callback = |_: &V| Err(Error("injected".into()));
            assert_eq!(
                code(F::publish_legacy(
                    root,
                    journal,
                    &m,
                    &mut |_| Ok(()),
                    Some(&mut callback)
                ))
                .as_deref(),
                Some("injected")
            );
            assert!(
                root.join(format!("b/.history-local/{digest}.json"))
                    .exists()
            );
            F::recover_legacy(root, journal, Direction::After, &mut |_| Ok(()), None, None)
                .unwrap();
            assert_eq!(fs::read(root.join("b/part.yaml")).unwrap(), b"part-after");
            assert!(
                !root
                    .join(format!("b/.history-local/{digest}.json"))
                    .exists()
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn auxiliary_exception_requires_exclusive_lock_exact_bytes_and_live_owner() {
    let fixtures: Value =
        serde_json::from_str(include_str!("fixtures/history-transactions.json")).unwrap();
    let case = fixtures["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["kind"] == "journal" && c["name"] == "GROUNDING.yaml-auxiliary")
        .unwrap();
    let m = PreparedMutation::from_bytes(case["raw"].as_str().unwrap().as_bytes()).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let layout = kpop_native::history_transaction::Layout::for_entry("GROUNDING.yaml").unwrap();
    let journal = &layout.journal;
    assert_eq!(
        code(F::auxiliary_owner(root, journal, &m)).as_deref(),
        Some("auxiliary_writer_lock_required")
    );
    let _writer = DirectoryGuard::acquire(root, true).unwrap();
    assert_eq!(
        code(F::publish_auxiliary_journal(root, journal, &m)).as_deref(),
        Some("auxiliary_writer_lock_required")
    );
    let owner = F::auxiliary_owner(root, journal, &m).unwrap();
    F::publish_auxiliary_journal(root, journal, &m).unwrap();
    F::reader_guard(root, journal, || Ok(())).unwrap();
    let raw = fs::read(root.join(journal)).unwrap();
    let mut altered = raw.clone();
    altered.push(b' ');
    fs::write(root.join(journal), altered).unwrap();
    assert_eq!(
        code(F::reader_guard(root, journal, || Ok(()))).as_deref(),
        Some("recovery_required")
    );
    assert_eq!(
        code(F::clear_auxiliary_journal(root, journal, &m)).as_deref(),
        Some("recovery_required")
    );
    fs::write(root.join(journal), raw).unwrap();
    drop(owner);
    assert_eq!(
        code(F::reader_guard(root, journal, || Ok(()))).as_deref(),
        Some("recovery_required")
    );
    let _owner = F::auxiliary_owner(root, journal, &m).unwrap();
    F::clear_auxiliary_journal(root, journal, &m).unwrap();
    F::reader_guard(root, journal, || Ok(())).unwrap();
}

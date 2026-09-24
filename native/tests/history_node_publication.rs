use kpop_native::{
    Error, history_node_codec as C, history_node_publication as P, value::TypedValue as V,
};
use std::{collections::BTreeMap, fs, path::Path};
use tempfile::TempDir;
const MARKER: &str = "version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: experiment\ngeneration: 1\nrequires: [node-history/v1]\n";
fn setup() -> TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".kpopper")).unwrap();
    fs::write(root.path().join(".kpopper/history.yaml"), MARKER).unwrap();
    fs::write(
        root.path().join("GROUNDING.yaml"),
        b"known: {a: {v: 0}, b: {v: 0}}\n",
    )
    .unwrap();
    root
}
fn frames(subject: &str) -> (Vec<u8>, C::Version) {
    let initial = C::Event::create(
        subject,
        "create",
        vec![],
        None,
        Some(V::Text("original".into())),
    )
    .unwrap();
    let base = initial.reconstruct(None).unwrap();
    let next = C::Event::create(
        subject,
        "change",
        vec![base.id().into()],
        Some(&base),
        Some(V::Text("changed".into())),
    )
    .unwrap();
    (
        [initial.encode().unwrap(), next.encode().unwrap()].concat(),
        next.reconstruct(Some(&base)).unwrap(),
    )
}
fn prepare(root: &Path) -> P::Prepared {
    P::prepare(
        root,
        "transaction-1",
        P::bind_view(b"known: {a: {v: 1}, b: {v: 1}}\n", "transaction-1").unwrap(),
        BTreeMap::from([("a".into(), frames("a").0), ("b".into(), frames("b").0)]),
    )
    .unwrap()
}
fn stream(root: &Path, subject: &str) -> std::path::PathBuf {
    root.join(".kpopper/history")
        .join(C::subject_path(subject).unwrap())
}
#[test]
fn crash_at_every_boundary_has_one_durable_direction_and_exact_retry() {
    for phase in [
        P::Phase::Journal,
        P::Phase::Append(0),
        P::Phase::Append(1),
        P::Phase::Commit,
        P::Phase::View,
    ] {
        let root = setup();
        let before = fs::read(root.path().join("GROUNDING.yaml")).unwrap();
        let prepared = prepare(root.path());
        let error = P::publish(
            root.path(),
            &prepared,
            |_| Ok(()),
            |now| {
                if now == phase {
                    Err(Error("crash".into()))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert_eq!(error.0, "crash");
        assert!(P::capture(root.path()).is_err());
        let committed = matches!(phase, P::Phase::Commit | P::Phase::View);
        assert_eq!(
            P::recover(root.path(), |_| Ok(())).unwrap(),
            if committed {
                "committed"
            } else {
                "rolled_back"
            }
        );
        let view = P::capture(root.path()).unwrap().unwrap();
        if committed {
            assert_ne!(view, before);
        } else {
            assert_eq!(view, before);
            assert!(!stream(root.path(), "a").exists());
            assert!(!stream(root.path(), "b").exists());
        }
        P::publish(root.path(), &prepared, |_| Ok(()), |_| Ok(())).unwrap();
        let a = fs::read(stream(root.path(), "a")).unwrap();
        P::publish(root.path(), &prepared, |_| Ok(()), |_| Ok(())).unwrap();
        assert_eq!(fs::read(stream(root.path(), "a")).unwrap(), a);
    }
}
#[test]
fn partial_append_is_rolled_back_but_foreign_tail_preserves_all_files() {
    let root = setup();
    let prepared = prepare(root.path());
    P::publish(
        root.path(),
        &prepared,
        |_| Ok(()),
        |phase| {
            if phase == P::Phase::Append(1) {
                Err(Error("crash".into()))
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err();
    let path = stream(root.path(), "a");
    let raw = fs::read(&path).unwrap();
    fs::write(&path, &raw[..raw.len() / 2]).unwrap();
    let foreign = stream(root.path(), "b");
    let mut other = fs::read(&foreign).unwrap();
    other.extend(b"foreign");
    fs::write(&foreign, &other).unwrap();
    assert!(P::recover(root.path(), |_| Ok(())).is_err());
    assert_eq!(fs::read(&path).unwrap(), raw[..raw.len() / 2]);
    assert_eq!(fs::read(&foreign).unwrap(), other);
    other.truncate(other.len() - 7);
    fs::write(&foreign, other).unwrap();
    assert_eq!(P::recover(root.path(), |_| Ok(())).unwrap(), "rolled_back");
    assert!(!path.exists());
}
#[test]
fn rollback_preserves_preexisting_committed_frames() {
    let root = setup();
    let first = prepare(root.path());
    P::publish(root.path(), &first, |_| Ok(()), |_| Ok(())).unwrap();
    let path = stream(root.path(), "a");
    let original = fs::read(&path).unwrap();
    let base = frames("a").1;
    let next = C::Event::create("a", "next", vec![base.id().into()], Some(&base), None).unwrap();
    let second = P::prepare(
        root.path(),
        "transaction-2",
        P::bind_view(b"known: {}\n", "transaction-2").unwrap(),
        BTreeMap::from([("a".into(), next.encode().unwrap())]),
    )
    .unwrap();
    P::publish(
        root.path(),
        &second,
        |_| Ok(()),
        |phase| {
            if phase == P::Phase::Append(0) {
                Err(Error("crash".into()))
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err();
    assert_eq!(P::recover(root.path(), |_| Ok(())).unwrap(), "rolled_back");
    assert_eq!(fs::read(path).unwrap(), original);
    P::capture(root.path()).unwrap();
}
#[test]
fn stale_views_verifier_changes_and_rejected_admission_publish_nothing() {
    let root = setup();
    let prepared = prepare(root.path());
    assert!(
        P::publish(
            root.path(),
            &prepared,
            |_| Err(Error("rejected".into())),
            |_| Ok(())
        )
        .is_err()
    );
    assert!(!root.path().join(".kpopper/history").exists());
    assert!(
        P::publish(
            root.path(),
            &prepared,
            |_| {
                fs::write(root.path().join("GROUNDING.yaml"), b"changed").unwrap();
                Ok(())
            },
            |_| Ok(())
        )
        .is_err()
    );
    assert!(!root.path().join(".kpopper/history").exists());
}
#[test]
fn full_capture_rejects_unrelated_corruption_and_orphan_frames() {
    let root = setup();
    let prepared = prepare(root.path());
    P::publish(root.path(), &prepared, |_| Ok(()), |_| Ok(())).unwrap();
    let path = stream(root.path(), "b");
    let raw = fs::read(&path).unwrap();
    let mut damaged = raw.clone();
    damaged[50] ^= 1;
    fs::write(&path, damaged).unwrap();
    assert!(P::capture(root.path()).is_err());
    fs::write(&path, raw).unwrap();
    fs::write(stream(root.path(), "orphan"), frames("orphan").0).unwrap();
    assert!(P::capture(root.path()).is_err());
}
#[test]
fn experimental_authority_refuses_existing_formats_and_old_capture_refuses_it() {
    let root = setup();
    let marker = root.path().join(".kpopper/history.yaml");
    assert!(
        kpop_native::history_capture::capture(&root.path().join("GROUNDING.yaml"), None, None)
            .is_err()
    );
    fs::write(
        &marker,
        b"version: 1\nprofile: history/v1\nauthority: history\nrecord_id: old\ngeneration: 1\n",
    )
    .unwrap();
    assert!(P::prepare(root.path(), "x", vec![], BTreeMap::new()).is_err());
    assert!(!root.path().join(".kpopper/history").exists());
}
#[test]
fn changed_marker_and_recovery_verifier_edits_refuse() {
    let root = setup();
    let prepared = prepare(root.path());
    P::publish(
        root.path(),
        &prepared,
        |_| Ok(()),
        |p| {
            if p == P::Phase::Commit {
                Err(Error("crash".into()))
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err();
    assert!(
        P::recover(root.path(), |_| {
            fs::write(root.path().join("GROUNDING.yaml"), b"foreign").unwrap();
            Ok(())
        })
        .is_err()
    );
    assert_eq!(
        fs::read(root.path().join("GROUNDING.yaml")).unwrap(),
        b"foreign"
    );
    assert!(
        root.path()
            .join(".kpopper/.history-node-publication.json")
            .exists()
    );
}

#[test]
fn recovery_refuses_valid_unrelated_commits_added_during_verification() {
    let other = setup();
    let extra = P::prepare(
        other.path(),
        "unrelated",
        P::bind_view(b"known: {unrelated: true}", "unrelated").unwrap(),
        BTreeMap::from([("c".into(), frames("c").0)]),
    )
    .unwrap();
    P::publish(other.path(), &extra, |_| Ok(()), |_| Ok(())).unwrap();
    let extra_manifest =
        fs::read(other.path().join(".kpopper/history-commits/unrelated.json")).unwrap();
    let extra_stream = fs::read(stream(other.path(), "c")).unwrap();
    let root = setup();
    let prepared = prepare(root.path());
    P::publish(
        root.path(),
        &prepared,
        |_| Ok(()),
        |phase| {
            if phase == P::Phase::Commit {
                Err(Error("crash".into()))
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err();
    let view = fs::read(root.path().join("GROUNDING.yaml")).unwrap();
    let error = P::recover(root.path(), |_| {
        fs::write(
            root.path().join(".kpopper/history-commits/unrelated.json"),
            &extra_manifest,
        )
        .unwrap();
        fs::write(stream(root.path(), "c"), &extra_stream).unwrap();
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.0, "node_publication_verifier_changed_inventory");
    assert_eq!(fs::read(root.path().join("GROUNDING.yaml")).unwrap(), view);
    assert!(
        root.path()
            .join(".kpopper/.history-node-publication.json")
            .exists()
    );
}

#[test]
fn separately_prepared_writers_cannot_publish_over_a_new_frontier() {
    let root = setup();
    let first = prepare(root.path());
    let second = P::prepare(
        root.path(),
        "second",
        P::bind_view(b"known: {other: true}", "second").unwrap(),
        BTreeMap::from([("c".into(), frames("c").0)]),
    )
    .unwrap();
    P::publish(root.path(), &first, |_| Ok(()), |_| Ok(())).unwrap();
    let view = P::capture(root.path()).unwrap();
    assert_eq!(
        P::publish(root.path(), &second, |_| Ok(()), |_| Ok(()))
            .unwrap_err()
            .0,
        "node_publication_stale_frontier"
    );
    assert_eq!(P::capture(root.path()).unwrap(), view);
    assert!(!stream(root.path(), "c").exists());
}

#[test]
fn recovery_checks_unrelated_corruption_before_changing_any_tail() {
    let root = setup();
    let first = prepare(root.path());
    P::publish(root.path(), &first, |_| Ok(()), |_| Ok(())).unwrap();
    let base = frames("a").1;
    let event = C::Event::create("a", "later", vec![base.id().into()], Some(&base), None).unwrap();
    let second = P::prepare(
        root.path(),
        "second",
        P::bind_view(b"known: {second: true}", "second").unwrap(),
        BTreeMap::from([("a".into(), event.encode().unwrap())]),
    )
    .unwrap();
    P::publish(
        root.path(),
        &second,
        |_| Ok(()),
        |p| {
            if p == P::Phase::Append(0) {
                Err(Error("crash".into()))
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err();
    let a = fs::read(stream(root.path(), "a")).unwrap();
    let b = stream(root.path(), "b");
    let mut raw = fs::read(&b).unwrap();
    raw[70] ^= 1;
    fs::write(&b, raw).unwrap();
    assert!(P::recover(root.path(), |_| Ok(())).is_err());
    assert_eq!(fs::read(stream(root.path(), "a")).unwrap(), a);
}

fn lazy_document(
    original: Option<&kpop_native::history_node_current::Original>,
    body: V,
) -> Vec<u8> {
    let originals = original
        .map(|o| BTreeMap::from([("a".into(), o.encode().unwrap())]))
        .unwrap_or_default();
    let document = V::Map(BTreeMap::from([
        ("known".into(), V::Map(BTreeMap::from([("a".into(), body)]))),
        (
            "meta".into(),
            V::Map(BTreeMap::from([(
                "node_history".into(),
                V::Map(BTreeMap::from([
                    (
                        "version".into(),
                        V::Integer(kpop_native::value::Integer::new("1").unwrap()),
                    ),
                    ("originals".into(), V::Map(originals)),
                ])),
            )])),
        ),
    ]));
    kpop_native::history_yaml::encode_document(&document).unwrap()
}
#[test]
fn lazy_creation_has_no_history_file_and_first_change_materializes_original_atomically() {
    for crash in [
        P::Phase::Journal,
        P::Phase::Append(0),
        P::Phase::Commit,
        P::Phase::View,
    ] {
        let root = setup();
        let body = V::Text("original body".into());
        let original = kpop_native::history_node_current::Original::create(
            "a",
            "known",
            "birth",
            body.clone(),
            BTreeMap::new(),
        )
        .unwrap();
        let created = P::bind_view(&lazy_document(Some(&original), body.clone()), "birth").unwrap();
        let first = P::prepare(root.path(), "birth", created.clone(), BTreeMap::new()).unwrap();
        P::publish(root.path(), &first, |_| Ok(()), |_| Ok(())).unwrap();
        assert_eq!(P::capture(root.path()).unwrap(), Some(created.clone()));
        assert!(!stream(root.path(), "a").exists());
        let initial = original.restore(body).unwrap();
        let first_version = initial.reconstruct(None).unwrap();
        let changed = V::Text("changed body".into());
        let next = C::Event::create(
            "a",
            "change",
            vec![initial.id().into()],
            Some(&first_version),
            Some(original.state(changed.clone())),
        )
        .unwrap();
        let after = P::bind_view(&lazy_document(None, changed), "change").unwrap();
        let second = P::prepare(
            root.path(),
            "change",
            after.clone(),
            BTreeMap::from([(
                "a".into(),
                [initial.encode().unwrap(), next.encode().unwrap()].concat(),
            )]),
        )
        .unwrap();
        P::publish(
            root.path(),
            &second,
            |_| Ok(()),
            |p| {
                if p == crash {
                    Err(Error("crash".into()))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        let committed = matches!(crash, P::Phase::Commit | P::Phase::View);
        P::recover(root.path(), |_| Ok(())).unwrap();
        if committed {
            assert_eq!(P::capture(root.path()).unwrap(), Some(after));
            let versions =
                C::decode_stream(&fs::read(stream(root.path(), "a")).unwrap(), "a").unwrap();
            assert_eq!(versions[initial.id()].state(), first_version.state());
            assert_eq!(versions.len(), 2);
        } else {
            assert_eq!(P::capture(root.path()).unwrap(), Some(created));
            assert!(!stream(root.path(), "a").exists());
        }
    }
}
#[test]
fn unchanged_original_is_not_repeated_in_later_manifest() {
    let root = setup();
    let body = V::Text("original".into());
    let original = kpop_native::history_node_current::Original::create(
        "a",
        "known",
        "birth",
        body.clone(),
        BTreeMap::new(),
    )
    .unwrap();
    let view = P::bind_view(&lazy_document(Some(&original), body), "birth").unwrap();
    let first = P::prepare(root.path(), "birth", view.clone(), BTreeMap::new()).unwrap();
    P::publish(root.path(), &first, |_| Ok(()), |_| Ok(())).unwrap();
    let second = P::prepare(
        root.path(),
        "context-only",
        P::bind_view(&view, "context-only").unwrap(),
        BTreeMap::new(),
    )
    .unwrap();
    P::publish(root.path(), &second, |_| Ok(()), |_| Ok(())).unwrap();
    let m: serde_json::Value = serde_json::from_slice(
        &fs::read(
            root.path()
                .join(".kpopper/history-commits/context-only.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(m["touched"], serde_json::json!([]));
    P::capture(root.path()).unwrap();
}

#[test]
fn deleting_the_whole_manifest_closure_is_not_an_empty_initial_record() {
    let root = setup();
    let prepared = prepare(root.path());
    P::publish(root.path(), &prepared, |_| Ok(()), |_| Ok(())).unwrap();
    fs::remove_dir_all(root.path().join(".kpopper/history-commits")).unwrap();
    fs::remove_dir_all(root.path().join(".kpopper/history")).unwrap();
    assert_eq!(
        P::capture(root.path()).unwrap_err().0,
        "node_publication_missing_manifest"
    );
}
#[test]
fn portable_export_reconstructs_without_original_source_or_history_paths() {
    let root = setup();
    let prepared = prepare(root.path());
    P::publish(root.path(), &prepared, |_| Ok(()), |_| Ok(())).unwrap();
    let expected = P::capture(root.path()).unwrap();
    let bundle = P::export(root.path()).unwrap();
    let raw = bundle.encode().unwrap();
    drop(root);
    let copy = P::Bundle::decode(&raw).unwrap().reconstruct().unwrap();
    assert_eq!(P::capture(copy.path()).unwrap(), expected);
    assert_eq!(P::export(copy.path()).unwrap().encode().unwrap(), raw);
    let mut bad = raw;
    let i = bad.len() / 2;
    bad[i] ^= 1;
    assert!(P::Bundle::decode(&bad).is_err());
}

#[test]
fn recovery_admission_receives_the_exact_retained_operation() {
    let root = setup();
    let prepared = prepare(root.path());
    let before = prepared.before_view().unwrap();
    let after = prepared.after_view().unwrap();
    let frames = prepared.frames().unwrap();
    P::publish(
        root.path(),
        &prepared,
        |p| {
            assert_eq!(p.operation(), "transaction-1");
            Ok(())
        },
        |phase| {
            if phase == P::Phase::Commit {
                Err(Error("crash".into()))
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err();
    P::recover(root.path(), |replay| {
        assert_eq!(replay.operation(), "transaction-1");
        assert_eq!(replay.before_view()?, before);
        assert_eq!(replay.after_view()?, after);
        assert_eq!(replay.frames()?, frames);
        Ok(())
    })
    .unwrap();
}

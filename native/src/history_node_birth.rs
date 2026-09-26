//! Direct first authorship in compact history. A marker without a view or
//! manifests reserves record identity; only the first committed operation is a record.
use crate::{
    Result,
    history_authoring::{n, obj, s, strings},
    history_contract::*,
    history_node_capture::Capture,
    history_node_publication as P,
    history_transaction::Layout,
    history_transaction_fs as F,
    history_view::{list, map_mut, truth},
    history_yaml as Y,
    project_modes::WriteRoute,
    reasoning_authoring::{self as A, World},
    reasoning_runtime::{OperationalBounds, Runtime},
    require,
    value::TypedValue as V,
};
use std::{fs, path::Path};

const MARKER: &str = ".kpopper/history.yaml";
const NODE_JOURNAL: &str = ".kpopper/.history-node-publication.json";

fn empty_directory(root: &Path, relative: &str) -> Result<()> {
    let path = F::target(root, relative)?;
    if !path.exists() {
        return Ok(());
    }
    let mut pending = vec![path];
    let mut visits = 0;
    while let Some(path) = pending.pop() {
        let meta = fs::symlink_metadata(&path)?;
        require(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "existing_history_evidence",
        )?;
        for item in fs::read_dir(path)? {
            visits += 1;
            require(visits <= 100_000, "history_limit")?;
            pending.push(item?.path());
        }
    }
    Ok(())
}

fn environment(root: &Path) -> Result<()> {
    for entry in ["GROUNDING.yaml", "PROVENANCE.yaml"] {
        let layout = Layout::for_entry(entry)?;
        for relative in [
            layout.entry.as_str(),
            layout.replaced.as_str(),
            layout.journal.as_str(),
            &format!("{}.history", layout.journal),
        ] {
            require(
                F::read(&F::target(root, relative)?)?.is_none(),
                "existing_record_evidence",
            )?;
        }
        if entry != "GROUNDING.yaml" {
            require(
                F::read(&F::target(root, &layout.authority)?)?.is_none(),
                "existing_record_evidence",
            )?;
        }
        for relative in [
            &layout.objects,
            &layout.commits,
            &layout.cancellations,
            &layout.retained,
        ] {
            empty_directory(root, relative)?;
        }
        let hypotheses = F::target(root, &layout.hypotheses)?;
        if hypotheses.is_dir() {
            for member in fs::read_dir(hypotheses)? {
                let name = member?.file_name();
                let name = name.to_str().ok_or_else(|| error("invalid_path"))?;
                require(
                    name.starts_with('.') || !name.ends_with(".yaml") && !name.ends_with(".yml"),
                    "existing_hypothesis_requires_record",
                )?;
            }
        } else {
            require(!hypotheses.exists(), "invalid_history_path")?;
        }
    }
    require(
        F::read(&F::target(root, NODE_JOURNAL)?)?.is_none(),
        "node_publication_recovery_required",
    )?;
    for relative in [
        "evidence/legacy",
        "evidence/migration",
        "evidence/bootstrap",
        "evidence/reports",
        "evidence/view-edits",
        ".kpopper/evidence/domain",
    ] {
        empty_directory(root, relative)?;
    }
    Ok(())
}

fn first_action(action: &V, runtime: Option<&Runtime>) -> Result<V> {
    let mut action = action.clone();
    let a = map_mut(&mut action)?;
    require(
        string_is(field(a, "kind")?, "add") && !a.get("section").is_some_and(truth),
        "unsupported_history_bootstrap_action",
    )?;
    require(
        a.get("profile")
            .is_none_or(|v| *v == V::Null || string_is(v, "core/v1")),
        "history_profile_migration_required",
    )?;
    if !a.get("as_of").is_some_and(truth) {
        a.insert(
            "as_of".into(),
            s(&chrono::Local::now().date_naive().to_string()),
        );
    }
    let doc = A::declare_document(&obj([("meta", obj([("updated", V::Null)]))]))?;
    let mut world = World::new(&doc, None, runtime, OperationalBounds::default())?;
    let (normalized, _) = world.normalize(&action, None)?;
    let refusals = world.validate(&normalized)?;
    require(
        refusals.is_empty(),
        &format!("refused - {}", refusals.join("\n          ")),
    )?;
    let a = map(&normalized)?;
    let body = field(a, "body")?;
    let deps = map(body)
        .ok()
        .and_then(|b| b.get(text(&world.fields()["deps"]).unwrap()));
    if let Some(deps) = deps {
        let deps = list(deps).map_err(|_| error("invalid_bootstrap_dependencies"))?;
        for dep in deps {
            text(dep).map_err(|_| error("invalid_bootstrap_dependencies"))?;
        }
        require(
            deps.is_empty()
                || a.get("hypothesis").is_some_and(|v| *v != V::Null)
                    && !A::blocked_text(body).is_empty(),
            "unresolved_history_subject",
        )?;
    }
    Ok(action)
}

pub(crate) fn create(
    route: &WriteRoute,
    original: &[std::path::PathBuf],
    action: &V,
    runtime: Option<&Runtime>,
    by: V,
) -> Result<(V, String)> {
    create_with_probe(route, original, action, runtime, by, &mut |_| Ok(()))
}

fn create_with_probe(
    route: &WriteRoute,
    original: &[std::path::PathBuf],
    action: &V,
    runtime: Option<&Runtime>,
    by: V,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<(V, String)> {
    require(route.paths().len() == 1, "choose one logical record entry")?;
    let entry = &route.paths()[0];
    require(
        entry.file_name().is_some_and(|n| n == "GROUNDING.yaml"),
        "node_history_entry_unsupported",
    )?;
    let root = entry.parent().ok_or_else(|| error("invalid_path"))?;
    let _lock = F::DirectoryGuard::acquire(root, true)?;
    route.verify()?;
    let action = first_action(action, runtime)?;
    environment(root)?;
    let marker_path = F::target(root, MARKER)?;
    let existing = F::read(&marker_path)?;
    let marker = if let Some(raw) = &existing {
        P::validate_authority(&Y::decode_document(raw)?)?;
        require(Capture::read(root)?.is_unborn(), "existing_record_evidence")?;
        raw.clone()
    } else {
        Y::encode_document(&obj([
            ("version", n("3")),
            ("profile", s("node-history/v1")),
            ("authority", s("history")),
            ("record_id", s(&crate::public_history::fresh_id("record")?)),
            ("generation", n("1")),
            ("requires", strings(vec!["node-history/v1".into()])),
        ]))?
    };
    let attributes_path = F::target(root, ".gitattributes")?;
    let old_attributes = F::read(&attributes_path)?;
    let mut attributes = old_attributes.clone().unwrap_or_default();
    if !attributes.ends_with(P::GIT_ATTRIBUTES.as_bytes()) {
        if !attributes.is_empty() && !attributes.ends_with(b"\n") {
            attributes.push(b'\n');
        }
        attributes.extend_from_slice(P::GIT_ATTRIBUTES.as_bytes());
        F::replace(&attributes_path, Some(&attributes))?;
    }
    if existing.is_none() {
        fs::create_dir_all(F::target(root, ".kpopper")?)?;
        F::sync(root)?;
        F::publish_immutable(root, MARKER, &marker)?;
    }
    // Interruption here leaves only reserved identity; readers still see no record.
    probe("marker")?;
    let result =
        crate::public_node_history::write_inner(route, original, &action, probe, runtime, by);
    if !entry.exists() && F::read(&F::target(root, NODE_JOURNAL)?)?.is_none() {
        environment(root)?;
        if existing.is_none() && F::read(&marker_path)?.as_deref() == Some(marker.as_slice()) {
            F::remove(&marker_path)?;
        }
        if F::read(&attributes_path)?.as_deref() == Some(attributes.as_slice()) {
            F::replace(&attributes_path, old_attributes.as_deref())?;
        }
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn action(named: bool) -> V {
        let mut action = obj([
            ("kind", s("add")),
            ("id", s("p.first")),
            ("body", obj([("v", n("1"))])),
            ("as_of", s("2026-09-25")),
        ]);
        if named {
            map_mut(&mut action)
                .unwrap()
                .insert("hypothesis".into(), s("alternative"));
        }
        action
    }

    #[test]
    fn compact_first_authorship_preserves_bootstrap_corpus_bodies_and_dispositions() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/history-bootstrap.json")).unwrap();
        for case in cases.as_array().unwrap() {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let original = vec![root.join("GROUNDING.yaml")];
            let route = WriteRoute::capture(&original, &root).unwrap();
            let runtime = crate::history_authoring::tests::runtime(&root.join("runtime"));
            let action = V::from_tagged(&case["action"]).unwrap();
            let output = V::from_tagged(&case["output"]).unwrap();
            let subject = text(&map(&action).unwrap()["id"]).unwrap();
            let expected = list(&map(&output).unwrap()["files"])
                .unwrap()
                .iter()
                .filter_map(|file| {
                    let file = map(file).unwrap();
                    if !string_is(&file["role"], "history_object") {
                        return None;
                    }
                    let raw = crate::history_transaction::unblob(&file["after"])
                        .unwrap()
                        .unwrap();
                    let object = Y::decode_document(&raw).unwrap();
                    let fields = map(&object).unwrap();
                    (string_is(&fields["subject"], subject) && !string_is(&fields["kind"], "act"))
                        .then_some(object)
                })
                .next()
                .unwrap();
            create(&route, &original, &action, Some(&runtime), s("writer"))
                .unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
            let capture = Capture::read(&root).unwrap();
            let actual = capture
                .history
                .objects()
                .iter()
                .find_map(|(id, object)| {
                    let fields = map(object).unwrap();
                    (string_is(&fields["subject"], subject) && !string_is(&fields["kind"], "act"))
                        .then(|| capture.object(subject, id).unwrap())
                })
                .unwrap();
            assert_eq!(
                map(&actual).unwrap()["body"],
                map(&expected).unwrap()["body"],
                "{}",
                case["name"]
            );
            assert_eq!(
                map(&map(&actual).unwrap()["authored"]).unwrap()["collection"],
                map(&map(&expected).unwrap()["authored"]).unwrap()["collection"]
            );
            assert_eq!(map(&actual).unwrap()["by"], s("writer"));
            assert!(
                !map(&map(&actual).unwrap()["authored"])
                    .unwrap()
                    .contains_key("locator")
            );
            let proposed = map(&action)
                .unwrap()
                .get("hypothesis")
                .is_some_and(|v| *v != V::Null);
            let state = &map(&map(capture.state()).unwrap()["subjects"]).unwrap()[subject];
            assert!(string_is(
                &map(state).unwrap()["acceptance"],
                if proposed { "proposed" } else { "accepted" }
            ));
        }
    }

    #[test]
    fn interrupted_first_publications_recover_without_an_empty_published_record() {
        for named in [false, true] {
            for phase in ["marker", "journal", "append", "committed", "view"] {
                let temp = tempfile::tempdir().unwrap();
                let root = temp.path().canonicalize().unwrap();
                let original = vec![root.join("GROUNDING.yaml")];
                let route = WriteRoute::capture(&original, &root).unwrap();
                let runtime = crate::history_authoring::tests::runtime(&root.join("runtime"));
                let mut reached = false;
                let result = create_with_probe(
                    &route,
                    &original,
                    &action(named),
                    Some(&runtime),
                    s("writer"),
                    &mut |point| {
                        if point == phase {
                            reached = true;
                            Err(error("interrupted"))
                        } else {
                            Ok(())
                        }
                    },
                );
                if !reached {
                    assert_eq!(phase, "append", "unreached {phase}: {result:?}");
                    assert!(result.is_ok(), "{result:?}");
                    continue;
                }
                assert!(result.is_err(), "{phase}");
                let marker = fs::read(root.join(MARKER)).unwrap();
                if phase == "marker" {
                    assert!(!original[0].exists());
                    assert!(Capture::read(&root).unwrap().is_unborn());
                } else {
                    // An interrupted journal is never accepted as a live reading.
                    assert!(Capture::read(&root).is_err());
                    crate::public_node_history::recover(&route, &original, false, Some(&runtime))
                        .unwrap_or_else(|e| panic!("{named}/{phase}: {e}"));
                }
                if !original[0].exists() {
                    create(
                        &route,
                        &original,
                        &action(named),
                        Some(&runtime),
                        s("writer"),
                    )
                    .unwrap_or_else(|e| panic!("retry {named}/{phase}: {e}"));
                }
                assert_eq!(fs::read(root.join(MARKER)).unwrap(), marker);
                let current = Capture::read(&root).unwrap();
                assert!(!current.is_unborn());
                assert_eq!(current.snapshot.transactions.len(), 1);
                assert_eq!(
                    fs::read(root.join(crate::history_node_checkpoint::PATH)).unwrap(),
                    fs::read(&original[0]).unwrap(),
                );
                assert!(current.history.objects().values().any(|object| {
                    map(object).is_ok_and(|m| m.get("by") == Some(&s("writer")))
                }));
            }
        }
    }

    #[test]
    fn first_recovery_preserves_a_foreign_view_and_existing_attributes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let original = vec![root.join("GROUNDING.yaml")];
        let route = WriteRoute::capture(&original, &root).unwrap();
        let runtime = crate::history_authoring::tests::runtime(&root.join("runtime"));
        fs::write(root.join(".gitattributes"), "user.txt text\n").unwrap();
        assert!(
            create_with_probe(
                &route,
                &original,
                &action(false),
                Some(&runtime),
                s("writer"),
                &mut |point| {
                    if point == "journal" {
                        Err(error("interrupted"))
                    } else {
                        Ok(())
                    }
                }
            )
            .is_err()
        );
        let foreign = b"known: {p.foreign: {v: 9}}\n";
        fs::write(&original[0], foreign).unwrap();
        assert!(
            crate::public_node_history::recover(&route, &original, false, Some(&runtime)).is_err()
        );
        assert_eq!(fs::read(&original[0]).unwrap(), foreign);
        assert!(
            fs::read_to_string(root.join(".gitattributes"))
                .unwrap()
                .starts_with("user.txt text\n")
        );
        assert!(root.join(NODE_JOURNAL).exists());
    }
}

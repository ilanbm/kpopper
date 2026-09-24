//! Public edited-file proposals require an exact caller-retained accepted view.
use crate::{
    Result,
    history_authoring::{obj, s, strings},
    history_contract::*,
    history_node_edits as E, history_node_publication as P, history_node_writer as W,
    history_transaction_fs as F, history_yaml as Y,
    project_modes::WriteRoute,
    public_node_history as Public,
    reasoning_runtime::Runtime,
    recording_privacy as Privacy, require,
    value::TypedValue as V,
};
use std::path::{Path, PathBuf};
fn guard(route: &WriteRoute, original: &[PathBuf]) -> Result<V> {
    Ok(obj([
        ("kind", s("public-node-edits/v1")),
        ("routing", crate::direct_history::routing(route, original)?),
    ]))
}
fn check(
    route: &WriteRoute,
    original: &[PathBuf],
    prepared: &P::Prepared,
    runtime: Option<&Runtime>,
) -> Result<()> {
    Public::scope(route)?;
    require(
        prepared.guard()?.as_ref() == Some(&guard(route, original)?),
        "node_history_route_changed",
    )?;
    let root = route.paths()[0].parent().unwrap();
    let (before, after, action, objects) = Public::candidate(root, prepared)?;
    require(
        string_is(field(map(&action)?, "kind")?, "view-edit-proposals"),
        "node_edit_evidence_mismatch",
    )?;
    let raw = prepared
        .before_view()?
        .ok_or_else(|| error("node_edit_evidence_mismatch"))?;
    let edited = Y::decode_document(&raw)?;
    // The entire raw file becomes immutable evidence, so unrelated private entries matter too.
    for document in [before.document(), after.document(), &edited, &objects] {
        require(
            !Privacy::private_marker(document),
            "private_proposal_requires_draft",
        )?;
    }
    crate::history_sources::capture(root, "GROUNDING.yaml", &edited)?;
    W::verify(root, prepared, runtime)?;
    route.verify()
}
pub(crate) fn verify_recovery(
    route: &WriteRoute,
    original: &[PathBuf],
    prepared: &P::Prepared,
    runtime_override: Option<&Runtime>,
) -> Result<()> {
    let root = route.paths()[0].parent().unwrap();
    let (before, _, _, _) = Public::candidate(root, prepared)?;
    let loaded;
    let runtime = if let Some(runtime) = runtime_override {
        Some(runtime)
    } else {
        loaded = crate::public_workspace::runtime_for_document(before.document())?;
        loaded.as_ref()
    };
    check(route, original, prepared, runtime)
}
pub(crate) fn run(
    original: &[PathBuf],
    cwd: &Path,
    baseline: &Path,
    subjects: Option<&[String]>,
    because: &str,
    by: Option<&str>,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<V> {
    let route = WriteRoute::capture(original, cwd)?;
    Public::scope(&route)?;
    let root = route.paths()[0].parent().unwrap();
    let _lock = F::DirectoryGuard::acquire(root, true)?;
    let canonical =
        F::read(&cwd.join(baseline))?.ok_or_else(|| error("node_edit_baseline_required"))?;
    let capture = E::capture(root, &canonical)?;
    let raw = E::edited(root)?;
    let selected = E::selection(&capture, &raw, subjects)?;
    let action = obj([
        ("kind", s("view-edit-proposals")),
        ("subjects", strings(selected.clone())),
        ("because", s(because)),
    ]);
    let edited = Y::decode_document(&raw)?;
    if Privacy::private_marker(capture.document()) || Privacy::private_marker(&edited) {
        return Privacy::draft(
            route.project(),
            &action,
            &edited,
            "private edited-view evidence",
        );
    }
    let runtime = crate::public_workspace::runtime_for_document(capture.document())?;
    let mut options = crate::direct_history::options("view-edit", by.map(s).unwrap_or(V::Null))?;
    options.strict = true;
    let prepared = W::prepare_edits(
        root,
        &canonical,
        because,
        subjects,
        &options,
        runtime.as_ref(),
    )?
    .with_guard(&guard(&route, original)?)?;
    P::publish(
        root,
        &prepared,
        |p| check(&route, original, p, runtime.as_ref()),
        |phase| {
            route.verify()?;
            probe(match phase {
                P::Phase::Journal => "journal",
                P::Phase::Append(_) => "append",
                P::Phase::Evidence(_) => "evidence",
                P::Phase::Commit => "committed",
                P::Phase::View => "view",
            })?;
            // Recheck the full semantic/source/privacy boundary at every publication transition.
            check(&route, original, &prepared, runtime.as_ref())
        },
    )?;
    crate::session_activity::published(
        root,
        &[crate::history_transaction::FileImage {
            path: "GROUNDING.yaml".into(),
            role: "record".into(),
            before: prepared.before_view()?,
            after: Some(prepared.after_view()?),
        }],
        Some(&selected.iter().cloned().collect()),
    );
    Ok(obj([
        ("state", s("proposed")),
        ("operation", s(prepared.operation())),
        ("subjects", strings(selected)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    fn fixture() -> (tempfile::TempDir, Vec<PathBuf>, Vec<u8>) {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join(".kpopper")).unwrap();
        fs::write(root.path().join(".kpopper/history.yaml"), "version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: fixture\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
        let original = vec![root.path().join("GROUNDING.yaml")];
        fs::write(&original[0], "meta:\n  purpose: Fixture\nschema:\n  deps: rests_on\n  snapshot: seen\n  predicate: wrong_if\nknown: {}\n").unwrap();
        let route = WriteRoute::capture(&original, root.path()).unwrap();
        Public::write(
            &route,
            &original,
            &V::from_json(&json!({"kind":"add","id":"p.a","body":{"v":1}})).unwrap(),
            &mut |_| Ok(()),
        )
        .unwrap();
        let canonical = fs::read(&original[0]).unwrap();
        fs::write(root.path().join("saved.yaml"), &canonical).unwrap();
        let mut doc = Y::decode_document(&canonical).unwrap().to_json().unwrap();
        doc["known"]["p.a"]["v"] = json!(2);
        let raw = Y::encode_document(&V::from_json(&doc).unwrap()).unwrap();
        fs::write(&original[0], &raw).unwrap();
        (root, original, raw)
    }
    #[test]
    fn public_edits_recover_exact_images_and_refuse_changed_routing() {
        for phase in ["journal", "append", "evidence", "committed", "view"] {
            let (root, original, edited) = fixture();
            assert!(
                run(
                    &original,
                    root.path(),
                    Path::new("saved.yaml"),
                    None,
                    "human edit",
                    None,
                    &mut |at| if at == phase {
                        Err(error("crash"))
                    } else {
                        Ok(())
                    }
                )
                .is_err()
            );
            let route = WriteRoute::capture(&original, root.path()).unwrap();
            let config = route.project().config_path.clone();
            let mut changed = route.config().clone();
            crate::history_view::map_mut(&mut changed)
                .unwrap()
                .insert("mode".into(), s("advanced"));
            drop(route);
            fs::write(
                &config,
                serde_json::to_vec(&changed.to_json().unwrap()).unwrap(),
            )
            .unwrap();
            assert!(crate::direct_history::recover(&original, root.path(), false).is_err());
            fs::remove_file(&config).unwrap();
            // Recovery uses the journal's verified baseline, not an external retained file.
            fs::remove_file(root.path().join("saved.yaml")).unwrap();
            let result = crate::direct_history::recover(&original, root.path(), false).unwrap();
            let committed = ["committed", "view"].contains(&phase);
            assert!(string_is(
                &map(&result).unwrap()["state"],
                if committed {
                    "committed"
                } else {
                    "rolled_back"
                }
            ));
            if committed {
                crate::history_node_capture::Capture::read(root.path()).unwrap();
            } else {
                assert_eq!(fs::read(&original[0]).unwrap(), edited);
            }
        }
    }
    #[test]
    fn public_edits_recovery_checks_private_raw_evidence() {
        let (root, original, _) = fixture();
        let raw = fs::read(&original[0]).unwrap();
        let mut doc = Y::decode_document(&raw).unwrap().to_json().unwrap();
        doc["known"]["p.a"]["private"] = json!(true);
        fs::write(
            &original[0],
            Y::encode_document(&V::from_json(&doc).unwrap()).unwrap(),
        )
        .unwrap();
        let route = WriteRoute::capture(&original, root.path()).unwrap();
        let mut op = crate::direct_history::options("edit", V::Null).unwrap();
        op.strict = true;
        let p = W::prepare_edits(
            root.path(),
            &fs::read(root.path().join("saved.yaml")).unwrap(),
            "human edit",
            None,
            &op,
            None,
        )
        .unwrap()
        .with_guard(&guard(&route, &original).unwrap())
        .unwrap();
        assert!(
            W::publish(root.path(), &p, None, |at| if at == P::Phase::Journal {
                Err(error("crash"))
            } else {
                Ok(())
            })
            .is_err()
        );
        drop(route);
        assert!(
            crate::direct_history::recover(&original, root.path(), false)
                .unwrap_err()
                .to_string()
                .contains("private_proposal")
        );
    }
}

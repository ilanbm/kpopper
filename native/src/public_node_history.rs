//! Guarded public entry for explicitly marked node-history records.
use crate::{
    Result,
    history_authoring::{obj, s},
    history_contract::*,
    history_node_capture::Capture,
    history_node_publication as P, history_node_writer as W,
    history_view::list,
    project_modes::WriteRoute,
    recording_privacy as Privacy, require,
    value::TypedValue as V,
};
use std::path::{Path, PathBuf};
fn scope(route: &WriteRoute) -> Result<()> {
    require(
        route.paths().len() == 1
            && !route.pending_required()?
            && string_is(&map(route.config())?["mode"], "simple"),
        "node_history_pending_unsupported",
    )?;
    require(
        P::selected(&route.paths()[0])?,
        "node_publication_authority_required",
    )?;
    route.verify()
}
fn guard(route: &WriteRoute, original: &[PathBuf]) -> Result<V> {
    Ok(obj([
        ("kind", s("public-node-history/v1")),
        ("routing", crate::direct_history::routing(route, original)?),
    ]))
}
fn verify_guard(route: &WriteRoute, original: &[PathBuf], prepared: &P::Prepared) -> Result<()> {
    scope(route)?;
    require(
        prepared.guard()?.as_ref() == Some(&guard(route, original)?),
        "node_history_route_changed",
    )
}
fn candidate(root: &Path, prepared: &P::Prepared) -> Result<(Capture, Capture, V, V)> {
    let (before, after) = prepared.snapshots(root)?;
    let before = Capture::from_snapshot(before)?;
    let after = Capture::from_snapshot(after)?;
    let receipt = W::receipt(&after.snapshot, prepared.operation())?;
    let action = field(
        map(field(map(&map(&receipt)?["before"])?, "authoring")?)?,
        "action",
    )?
    .clone();
    let ids = list(field(
        map(field(map(&map(&receipt)?["after"])?, "authoring")?)?,
        "objects",
    )?)?;
    let objects = V::List(
        ids.iter()
            .map(|id| {
                let id = text(id)?;
                let object = after
                    .history
                    .objects()
                    .get(id)
                    .ok_or_else(|| error("missing_object"))?;
                after.object(text(&map(object)?["subject"])?, id)
            })
            .collect::<Result<Vec<_>>>()?,
    );
    for document in [before.document(), after.document()] {
        crate::history_sources::capture(root, "GROUNDING.yaml", document)?;
    }
    Ok((before, after, action, objects))
}
pub(crate) fn write(
    route: &WriteRoute,
    original: &[PathBuf],
    action: &V,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<(V, String)> {
    scope(route)?;
    let root = route.paths()[0].parent().unwrap();
    let before = Capture::read(root)?;
    require(
        !map(action)?
            .get("hypothesis")
            .is_some_and(crate::history_view::truth),
        "node_history_hypotheses_unsupported",
    )?;
    // Preserve the source inventory and all imported-pointer checks before authoring.
    let source = crate::source_capture::capture_source(
        route.paths(),
        &route.project().root,
        crate::source_capture::ReadMode::Frozen,
        None,
    )?;
    if let Some(draft) = Privacy::selected_draft(route.project(), action, before.document())? {
        return Ok((draft, String::new()));
    }
    let runtime = crate::public_workspace::runtime_for_document(before.document())?;
    let prepared = W::prepare(
        root,
        action,
        &crate::direct_history::options("write", V::Null)?,
        runtime.as_ref(),
    )?
    .with_guard(&guard(route, original)?)?;
    let (_, after, _, objects) = candidate(root, &prepared)?;
    if let Some(draft) = Privacy::candidate_draft(route.project(), action, after.document())? {
        return Ok((draft, String::new()));
    }
    if Privacy::private_marker(&objects) {
        return Ok((
            Privacy::draft(
                route.project(),
                action,
                &obj([("history", objects)]),
                "private historical proposal",
            )?,
            String::new(),
        ));
    }
    P::publish(
        root,
        &prepared,
        |p| {
            verify_guard(route, original, p)?;
            source.verify()?;
            W::verify(root, p, runtime.as_ref())?;
            candidate(root, p)?;
            source.verify()?;
            route.verify()
        },
        |phase| {
            route.verify()?;
            probe(match phase {
                P::Phase::Journal => "journal",
                P::Phase::Append(_) => "append",
                P::Phase::Commit => "committed",
                P::Phase::View => "view",
            })?;
            route.verify()
        },
    )?;
    let id = text(field(map(action)?, "id")?)?.to_owned();
    crate::session_activity::published(
        root,
        &[crate::history_transaction::FileImage {
            path: "GROUNDING.yaml".into(),
            role: "record".into(),
            before: prepared.before_view()?,
            after: Some(prepared.after_view()?),
        }],
        Some(&std::collections::BTreeSet::from([id])),
    );
    let notice = if string_is(&map(action)?["kind"], "add")
        && !map(action)?.get("amend").is_some_and(|v| *v != V::Null)
    {
        crate::direct_history::nearest_existing(
            before.document(),
            &Map::new(),
            action,
            runtime.as_ref(),
        )
    } else {
        String::new()
    };
    Ok((
        obj([
            ("state", s("committed")),
            ("operation", s(prepared.operation())),
        ]),
        notice,
    ))
}
pub(crate) fn recover(
    route: &WriteRoute,
    original: &[PathBuf],
    before: bool,
    runtime_override: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<V> {
    scope(route)?;
    require(
        !before,
        "node_history_recovery_uses_durable_commit_decision",
    )?;
    let root = route.paths()[0].parent().unwrap();
    let mut operation = String::new();
    let state = P::recover(root, |p| {
        verify_guard(route, original, p)?;
        let (before, after, action, objects) = candidate(root, p)?;
        require(
            !Privacy::selection_is_private(&action, before.document(), false)?
                && !Privacy::selection_is_private(&action, after.document(), true)?,
            "private_proposal_requires_draft",
        )?;
        require(
            !Privacy::private_marker(&objects),
            "private_proposal_requires_draft",
        )?;
        let loaded;
        let runtime = if let Some(runtime) = runtime_override {
            Some(runtime)
        } else {
            loaded = crate::public_workspace::runtime_for_document(after.document())?;
            loaded.as_ref()
        };
        W::verify(root, p, runtime)?;
        operation = p.operation().into();
        route.verify()
    })?;
    route.verify()?;
    Ok(obj([("state", s(state)), ("operation", s(&operation))]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn v(j: serde_json::Value) -> V {
        V::from_json(&j).unwrap()
    }
    fn setup() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".kpopper")).unwrap();
        std::fs::write(root.path().join(".kpopper/history.yaml"),"version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: fixture\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
        std::fs::write(root.path().join("GROUNDING.yaml"),"meta:\n  purpose: Fixture\nschema:\n  deps: rests_on\n  snapshot: seen\n  predicate: wrong_if\nknown: {}\n").unwrap();
        root
    }
    #[test]
    fn public_recovery_revalidates_routing_before_mutation_and_uses_durable_decision() {
        for phase in ["journal", "committed", "view"] {
            let root = setup();
            let original = vec![root.path().join("GROUNDING.yaml")];
            let route = WriteRoute::capture(&original, root.path()).unwrap();
            let config_path = route.project().config_path.clone();
            let mut config = route.config().clone();
            let V::Map(m) = &mut config else { panic!() };
            m.insert("mode".into(), s("advanced"));
            let result = write(
                &route,
                &original,
                &v(json!({"kind":"add","id":"p.a","body":{"v":1}})),
                &mut |at| {
                    if at == phase {
                        Err(error("crash"))
                    } else {
                        Ok(())
                    }
                },
            );
            assert!(result.is_err());
            drop(route);
            let view = std::fs::read(&original[0]).unwrap();
            let journal = root.path().join(".kpopper/.history-node-publication.json");
            let journal_before = std::fs::read(&journal).unwrap();
            std::fs::write(
                &config_path,
                serde_json::to_vec(&config.to_json().unwrap()).unwrap(),
            )
            .unwrap();
            assert!(crate::direct_history::recover(&original, root.path(), false).is_err());
            assert_eq!(std::fs::read(&original[0]).unwrap(), view);
            assert_eq!(std::fs::read(&journal).unwrap(), journal_before);
            std::fs::remove_file(&config_path).unwrap();
            let result = crate::direct_history::recover(&original, root.path(), false).unwrap();
            assert_eq!(
                map(&result).unwrap()["state"],
                s(if phase == "journal" {
                    "rolled_back"
                } else {
                    "committed"
                })
            );
            Capture::read(root.path()).unwrap();
        }
    }
    #[test]
    fn recovery_refuses_a_private_record_even_with_valid_semantic_and_routing_hashes() {
        let root = setup();
        let original = vec![root.path().join("GROUNDING.yaml")];
        let mut doc =
            crate::history_yaml::decode_document(&std::fs::read(&original[0]).unwrap()).unwrap();
        let V::Map(d) = &mut doc else { panic!() };
        let V::Map(meta) = d.get_mut("meta").unwrap() else {
            panic!()
        };
        meta.insert("private".into(), V::Bool(true));
        std::fs::write(
            &original[0],
            crate::history_yaml::encode_document(&doc).unwrap(),
        )
        .unwrap();
        let route = WriteRoute::capture(&original, root.path()).unwrap();
        let p = W::prepare(
            root.path(),
            &v(json!({"kind":"add","id":"p.a","body":{"v":1}})),
            &crate::direct_history::options("write", V::Null).unwrap(),
            None,
        )
        .unwrap()
        .with_guard(&guard(&route, &original).unwrap())
        .unwrap();
        assert!(
            P::publish(
                root.path(),
                &p,
                |p| W::verify(root.path(), p, None),
                |phase| if phase == P::Phase::Commit {
                    Err(error("crash"))
                } else {
                    Ok(())
                }
            )
            .is_err()
        );
        drop(route);
        let before = std::fs::read(&original[0]).unwrap();
        assert!(
            crate::direct_history::recover(&original, root.path(), false)
                .unwrap_err()
                .0
                .contains("private")
        );
        assert_eq!(std::fs::read(&original[0]).unwrap(), before);
        assert!(
            root.path()
                .join(".kpopper/.history-node-publication.json")
                .exists()
        );
    }
}

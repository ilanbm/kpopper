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
pub(crate) fn scope(route: &WriteRoute) -> Result<()> {
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
pub(crate) fn guard(route: &WriteRoute, original: &[PathBuf]) -> Result<V> {
    Ok(obj([
        ("kind", s("public-node-history/v1")),
        ("routing", crate::direct_history::routing(route, original)?),
    ]))
}
pub(crate) fn verify_guard(
    route: &WriteRoute,
    original: &[PathBuf],
    prepared: &P::Prepared,
) -> Result<()> {
    scope(route)?;
    require(
        prepared.guard()?.as_ref() == Some(&guard(route, original)?),
        "node_history_route_changed",
    )
}
pub(crate) fn candidate(root: &Path, prepared: &P::Prepared) -> Result<(Capture, Capture, V, V)> {
    let (before, after) = prepared.snapshots(root)?;
    let before = Capture::from_snapshot(before)?;
    let after = Capture::from_snapshot(after)?;
    let receipt = W::receipt(&after.snapshot, prepared.operation())?;
    let action = W::action(&receipt)?;
    let after_receipt = map(&map(&receipt)?["after"])?;
    let key = if after_receipt.contains_key("identity_authoring") {
        "identity_authoring"
    } else if after_receipt.contains_key("hypothesis_authoring") {
        "hypothesis_authoring"
    } else {
        "authoring"
    };
    let ids = list(field(map(field(after_receipt, key)?)?, "objects")?)?;
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
fn private_targets(capture: &Capture, action: &V) -> Result<bool> {
    let action = map(action)?;
    if !action.get("kind").is_some_and(|v| {
        ["accept", "correct", "refute", "propose", "retire"]
            .iter()
            .any(|k| string_is(v, k))
    }) {
        return Ok(false);
    }
    let over = list(field(action, "over")?)?;
    for id in std::iter::once(field(action, "of")?).chain(over) {
        if let Some(object) = capture.history.objects().get(text(id)?) {
            if Privacy::private_marker(object) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
pub(crate) fn write(
    route: &WriteRoute,
    original: &[PathBuf],
    action: &V,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<(V, String)> {
    write_with_runtime(route, original, action, probe, None)
}
fn privacy_action(action: &V) -> Result<V> {
    if map(action)?
        .get("kind")
        .is_some_and(|v| string_is(v, "hypothesis"))
    {
        return privacy_action(field(map(action)?, "action")?);
    }
    let a = map(action)?;
    if ["same", "distinct"]
        .iter()
        .any(|k| string_is(&a["kind"], k))
    {
        let mut selected = a.clone();
        selected.insert(
            "ids".into(),
            V::List(vec![field(a, "a")?.clone(), field(a, "b")?.clone()]),
        );
        Ok(V::Map(selected))
    } else {
        Ok(action.clone())
    }
}
// Identity can change either root in every named world. Named edits select their
// own layer; checking only current would miss private dependencies of proposals.
fn named_selection_private(capture: &Capture, selection: &V, candidate: bool) -> Result<bool> {
    let a = map(selection)?;
    let identity = a
        .get("kind")
        .is_some_and(|v| ["same", "distinct"].iter().any(|k| string_is(v, k)));
    let name = a
        .get("hypothesis")
        .filter(|v| **v != V::Null)
        .map(text)
        .transpose()?;
    if !identity && name.is_none() {
        return Ok(false);
    }
    let context = crate::history_node_hypothesis::context(capture)?;
    for (group_name, group) in map(&context.groups)? {
        if !identity && name != Some(group_name.as_str()) {
            continue;
        }
        if crate::recording_privacy::private_marker(field(map(group)?, "head")?) {
            return Ok(true);
        }
        let world = crate::history_hypothesis_authoring::layer(
            &context.base,
            &context.groups,
            std::slice::from_ref(group_name),
        )?;
        if Privacy::selection_is_private(selection, &world, candidate)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn write_with_runtime(
    route: &WriteRoute,
    original: &[PathBuf],
    action: &V,
    probe: &mut dyn FnMut(&str) -> Result<()>,
    runtime_override: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<(V, String)> {
    scope(route)?;
    let root = route.paths()[0].parent().unwrap();
    let before = Capture::read(root)?;
    if private_targets(&before, action)? {
        return Ok((
            Privacy::draft(
                route.project(),
                action,
                &obj([]),
                "private historical claim",
            )?,
            String::new(),
        ));
    }
    // Preserve the source inventory and all imported-pointer checks before authoring.
    let source = crate::source_capture::capture_source(
        route.paths(),
        &route.project().root,
        crate::source_capture::ReadMode::Frozen,
        None,
    )?;
    let selection = privacy_action(action)?;
    if let Some(draft) = Privacy::selected_draft(route.project(), &selection, before.document())? {
        return Ok((draft, String::new()));
    }
    if named_selection_private(&before, &selection, false)? {
        return Ok((
            Privacy::draft(
                route.project(),
                action,
                &obj([]),
                "private named hypothesis closure",
            )?,
            String::new(),
        ));
    }
    let loaded;
    let runtime = if let Some(runtime) = runtime_override {
        Some(runtime)
    } else {
        loaded = crate::public_workspace::runtime_for_document(before.document())?;
        loaded.as_ref()
    };
    let request = if let Some(name) = map(action)?.get("hypothesis").filter(|v| **v != V::Null) {
        obj([
            ("kind", s("hypothesis")),
            ("name", name.clone()),
            ("action", action.clone()),
        ])
    } else {
        action.clone()
    };
    let prepared = W::prepare(
        root,
        &request,
        &crate::direct_history::options("write", V::Null)?,
        runtime,
    )?
    .with_guard(&guard(route, original)?)?;
    let (_, after, _, objects) = candidate(root, &prepared)?;
    if let Some(draft) = Privacy::candidate_draft(route.project(), &selection, after.document())? {
        return Ok((draft, String::new()));
    }
    if named_selection_private(&after, &selection, true)? || Privacy::private_marker(&objects) {
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
            W::verify(root, p, runtime)?;
            candidate(root, p)?;
            source.verify()?;
            route.verify()
        },
        |phase| {
            route.verify()?;
            W::verify_sources(root, &prepared)?;
            probe(match phase {
                P::Phase::Journal => "journal",
                P::Phase::Append(_) => "append",
                P::Phase::Evidence(_) => "evidence",
                P::Phase::Import(_) => "import",
                P::Phase::Commit => "committed",
                P::Phase::View => "view",
            })?;
            W::verify_sources(root, &prepared)?;
            route.verify()
        },
    )?;
    let ids = if let Some(ids) = map(&selection)?.get("ids") {
        list(ids)?
            .iter()
            .map(|v| text(v).map(str::to_owned))
            .collect::<Result<_>>()?
    } else {
        std::collections::BTreeSet::from([text(field(map(action)?, "id")?)?.to_owned()])
    };
    crate::session_activity::published(
        root,
        &[crate::history_transaction::FileImage {
            path: "GROUNDING.yaml".into(),
            role: "record".into(),
            before: prepared.before_view()?,
            after: Some(prepared.after_view()?),
        }],
        Some(&ids),
    );
    let notice = if string_is(&map(action)?["kind"], "add")
        && !map(action)?.get("amend").is_some_and(|v| *v != V::Null)
    {
        crate::direct_history::nearest_existing(before.document(), &Map::new(), action, runtime)
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
        if p.guard()?
            .as_ref()
            .and_then(|v| map(v).ok())
            .and_then(|v| v.get("kind"))
            .is_some_and(|kind| string_is(kind, "public-node-edits/v1"))
        {
            crate::public_node_edits::verify_recovery(route, original, p, runtime_override)?;
            operation = p.operation().into();
            return Ok(());
        }
        if p.guard()?
            .as_ref()
            .and_then(|v| map(v).ok())
            .and_then(|v| v.get("kind"))
            .is_some_and(|kind| string_is(kind, "public-node-report/v1"))
        {
            crate::public_update::node::verify_recovery(route, original, p, runtime_override)?;
            operation = p.operation().into();
            return Ok(());
        }
        verify_guard(route, original, p)?;
        let (before, after, action, objects) = candidate(root, p)?;
        let folded = if string_is(field(map(&action)?, "kind")?, "hypothesis") {
            map(field(map(&action)?, "action")?)?
                .get("kind")
                .is_some_and(|v| ["fold", "refute"].iter().any(|k| string_is(v, k)))
        } else {
            false
        };
        if folded {
            let context = crate::history_node_hypothesis::context(&before)?;
            let names = list(field(map(field(map(&action)?, "action")?)?, "names")?)?
                .iter()
                .map(|v| text(v).map(str::to_owned))
                .collect::<Result<Vec<_>>>()?;
            let receipt = W::receipt(&after.snapshot, p.operation())?;
            let source = field(W::intent(&receipt)?, "by")?;
            let source = if *source == V::Null {
                None
            } else {
                Some(text(source)?)
            };
            require(
                crate::public_consolidation::private_selection(&context, &names, source)?.is_none(),
                "private_proposal_requires_draft",
            )?;
        } else {
            let selection = privacy_action(&action)?;
            require(
                !Privacy::selection_is_private(&selection, before.document(), false)?
                    && !Privacy::selection_is_private(&selection, after.document(), true)?
                    && !named_selection_private(&before, &selection, false)?
                    && !named_selection_private(&after, &selection, true)?
                    && !private_targets(&before, &action)?,
                "private_proposal_requires_draft",
            )?;
        }
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
    #[test]
    fn recovery_refuses_private_historical_target_absent_from_current() {
        let root = setup();
        let original = vec![root.path().join("GROUNDING.yaml")];
        let opts = crate::direct_history::options("write", V::Null).unwrap();
        let p = W::prepare(
            root.path(),
            &v(json!({"kind":"add","id":"p.a","body":{"v":1,"private":true}})),
            &opts,
            None,
        )
        .unwrap();
        W::publish(root.path(), &p, None, |_| Ok(())).unwrap();
        let c = Capture::read(root.path()).unwrap();
        let id = map(&map(&map(c.state()).unwrap()["subjects"]).unwrap()["p.a"]).unwrap()["head"]
            .clone();
        let request = |kind: &str| {
            v(
                json!({"kind":kind,"id":"p.a","of":id.to_json().unwrap(),"over":[],"because":"recorded evidence"}),
            )
        };
        let opts = crate::direct_history::options("act", V::Null).unwrap();
        let p = W::prepare(root.path(), &request("retire"), &opts, None).unwrap();
        W::publish(root.path(), &p, None, |_| Ok(())).unwrap();
        let c = Capture::read(root.path()).unwrap();
        assert!(!Privacy::private_marker(c.document()));
        assert!(private_targets(&c, &request("refute")).unwrap());
        let route = WriteRoute::capture(&original, root.path()).unwrap();
        let opts = crate::direct_history::options("act", V::Null).unwrap();
        let p = W::prepare(root.path(), &request("refute"), &opts, None)
            .unwrap()
            .with_guard(&guard(&route, &original).unwrap())
            .unwrap();
        assert!(
            P::publish(
                root.path(),
                &p,
                |p| W::verify(root.path(), p, None),
                |phase| if phase == P::Phase::Journal {
                    Err(error("crash"))
                } else {
                    Ok(())
                }
            )
            .is_err()
        );
        drop(route);
        let current = std::fs::read(&original[0]).unwrap();
        assert!(
            crate::direct_history::recover(&original, root.path(), false)
                .unwrap_err()
                .0
                .contains("private")
        );
        assert_eq!(std::fs::read(&original[0]).unwrap(), current);
    }
}

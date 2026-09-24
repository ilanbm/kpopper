//! Public pinned branch adoption for explicitly selected node-history records.
use crate::{
    Result,
    history_authoring::{obj, s},
    history_contract::*,
    history_node_adoption as A,
    history_node_branch_source::Observation,
    history_node_capture::Capture,
    history_node_publication as P, history_node_writer as W,
    project_modes::WriteRoute,
    public_consolidation::{CommandOutput, Options},
    recording_privacy as Privacy, require,
    value::TypedValue as V,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
const GUARD: &str = "public-node-branch/v1";
fn scope(route: &WriteRoute) -> Result<()> {
    // Explicit branch adoption writes the selected branch world in Advanced mode,
    // just as legacy adoption does; ordinary pending-write routing is unchanged.
    require(
        route.paths().len() == 1 && P::selected(&route.paths()[0])?,
        "node_publication_authority_required",
    )?;
    route.verify()
}
fn guard(route: &WriteRoute, original: &[PathBuf]) -> Result<V> {
    Ok(obj([
        ("kind", s(GUARD)),
        ("routing", crate::direct_history::routing(route, original)?),
    ]))
}
fn private(capture: &Capture) -> bool {
    Privacy::private_marker(capture.document())
        || capture
            .history
            .objects()
            .values()
            .any(Privacy::private_marker)
        || capture.snapshot.legacy.values().any(|legacy| {
            let captured = &legacy.captured;
            Privacy::private_marker(&captured.document)
                || captured.objects.values().any(Privacy::private_marker)
                || captured.inactive_generations.values().any(|generation| {
                    Privacy::private_marker(&generation.authority)
                        || generation.objects.values().any(Privacy::private_marker)
                        || generation
                            .cancellation
                            .as_ref()
                            .is_some_and(Privacy::private_marker)
                })
        })
        || capture.snapshot.raw_evidence.iter().any(|(path, raw)| {
            if path.starts_with("evidence/legacy/") && path.ends_with(".zip")
                || path.starts_with("evidence/bootstrap/") && path.ends_with(".zip")
            {
                return archive_private(raw);
            }
            if path.starts_with(".kpopper/hypotheses/")
                && (path.ends_with(".yaml") || path.ends_with(".yml"))
            {
                return yaml_private(raw);
            }
            false
        })
}
fn yaml_private(raw: &[u8]) -> bool {
    crate::history_yaml::decode_source_value(raw)
        .map(|value| Privacy::private_marker(&value.typed()))
        .unwrap_or(true)
}
fn json_private(raw: &[u8]) -> bool {
    serde_json::from_slice(raw)
        .ok()
        .and_then(|json| V::from_tagged(&json).or_else(|_| V::from_json(&json)).ok())
        .is_none_or(|value| Privacy::private_marker(&value))
}
fn archive_private(raw: &[u8]) -> bool {
    crate::history_node_archive::Archive::decode(raw)
        .map(|archive| {
            Privacy::private_marker(archive.metadata())
                || archive.files().iter().any(|(path, raw)| {
                    if path.ends_with(".yaml") || path.ends_with(".yml") {
                        yaml_private(raw)
                    } else if path.ends_with(".json") {
                        json_private(raw)
                    } else {
                        false
                    }
                })
        })
        .unwrap_or(true)
}

#[cfg(test)]
mod privacy_tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn archived_inactive_source_and_physical_markers_are_private() {
        let archive = crate::history_node_archive::Archive::new(
            BTreeMap::from([
                (
                    "GROUNDING.yaml".into(),
                    b"known: {p.a: {value: 1}}\n".to_vec(),
                ),
                (
                    ".kpopper/replaced.yaml".into(),
                    b"p.a: [{private: true}]\n".to_vec(),
                ),
            ]),
            V::Map(BTreeMap::new()),
        )
        .unwrap();
        assert!(archive_private(&archive.encode().unwrap()));
        assert!(json_private(br#"{"private":true}"#));
        assert!(yaml_private(b"hypothesis: {privacy: private}\nknown: {}\n"));
        assert!(!yaml_private(
            b"hypothesis: {privacy: project}\nknown: {}\n"
        ));
    }

    #[test]
    fn branch_privacy_sees_bootstrap_source_hidden_from_current_view() {
        let base = tempfile::tempdir().unwrap();
        let source = base.path().join("source");
        std::fs::create_dir_all(source.join(".kpopper")).unwrap();
        std::fs::write(source.join("GROUNDING.yaml"), b"known: {p.a: {value: 1}}\n").unwrap();
        std::fs::write(
            source.join(".kpopper/replaced.yaml"),
            b"p.a: [{private: true}]\n",
        )
        .unwrap();
        let copied = base.path().join("copy");
        crate::history_node_bootstrap::Plan::prepare(&source.join("GROUNDING.yaml"))
            .unwrap()
            .publish(&copied)
            .unwrap();
        let capture = Capture::read(&copied).unwrap();
        assert!(!Privacy::private_marker(capture.document()));
        assert!(private(&capture));
    }
}
fn clean(route: &WriteRoute, root: &Path) -> Result<()> {
    let files = P::export(root)?.files()?;
    let paths = files
        .keys()
        .map(|p| {
            crate::project_modes::resolved(&root.join(p))?
                .strip_prefix(&route.project().root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .map_err(|_| error("branch_target_outside_checkout"))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut args = vec![
        "--literal-pathspecs",
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--ignored=matching",
        "--",
    ];
    args.extend(paths.iter().map(String::as_str));
    let status = crate::public_readers::branch_read::git(&route.project().root, &args, false)?
        .ok_or_else(|| error("branch_git_unavailable"))?;
    require(
        status.is_empty(),
        "branch_target_uncommitted: commit the target record and its history before adopting a branch",
    )
}
fn output(prefix: &str, value: &V) -> Result<CommandOutput> {
    let raw = crate::public_ordinary_readers::python_safe_dump(
        &crate::history_yaml::OrdinaryValue::from_typed(value),
    )?;
    Ok(CommandOutput {
        stdout: format!(
            "{prefix}\n{}",
            String::from_utf8(raw).map_err(|_| error("invalid_branch_output"))?
        ),
        stderr: String::new(),
        code: 0,
    })
}
pub(crate) fn verify_recovery(
    route: &WriteRoute,
    original: &[PathBuf],
    p: &P::Prepared,
) -> Result<()> {
    scope(route)?;
    require(
        p.guard()?.as_ref() == Some(&guard(route, original)?),
        "node_history_route_changed",
    )?;
    let context = p
        .context()?
        .ok_or_else(|| error("node_transaction_missing_context"))?;
    require(
        crate::history_node_branch::is_union(&context)?
            && map(&if crate::history_node_transaction::is_context(&context) {
                crate::history_node_transaction::validate(&context)?
            } else {
                W::context(&context)?
            }["options"])?
            .contains_key("adoption"),
        "node_branch_admission_required",
    )?;
    let root = route.paths()[0].parent().unwrap();
    W::verify(root, p, None)?;
    let (before, after) = p.snapshots(root)?;
    for capture in [
        Capture::from_snapshot(before)?,
        Capture::from_snapshot(after)?,
    ] {
        require(!private(&capture), "private_proposal_requires_draft")?;
        crate::history_sources::capture(root, "GROUNDING.yaml", capture.document())?;
    }
    route.verify()
}
pub(crate) fn run(
    options: &Options,
    route: &WriteRoute,
    original: &[PathBuf],
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<CommandOutput> {
    scope(route)?;
    require(
        options.refute.is_none()
            && options.names.is_empty()
            && options.take.is_empty()
            && options.drops.is_empty(),
        "history_branch_choices_required: --from cannot mix named hypotheses, --take or --drop; use --choose",
    )?;
    let root = route.paths()[0].parent().unwrap();
    if !options.dry_run {
        clean(route, root)?;
    }
    require(
        (1..=16).contains(&options.from_refs.len()),
        "invalid_branch_source_set",
    )?;
    let target = Capture::read(root)?;
    let mut observations = Vec::<Observation>::new();
    let mut total = 0usize;
    for reference in &options.from_refs {
        let (_, repo, entry, oid, _) = crate::public_readers::branch_read::context(
            route.paths(),
            &route.project().root,
            reference,
        )?;
        let observation = crate::history_branch_git::capture_node(&repo, &oid, &entry)?;
        total = total.saturating_add(
            observation
                .bundle
                .files()?
                .values()
                .map(Vec::len)
                .sum::<usize>(),
        );
        require(
            total <= crate::history_branch::MAX_BYTES,
            "branch_capture_limit",
        )?;
        observations.push(observation);
    }
    let preview = A::preview(root, &observations)?;
    let revision = field(
        map(&preview)?,
        if observations.len() == 1 {
            "source_revision"
        } else {
            "source_set_revision"
        },
    )?;
    if let Some(expected) = &options.source_revision {
        require(string_is(revision, expected), "branch_source_changed")?;
    }
    if options.dry_run {
        probe("preview")?;
        target.verify_current(root)?;
        route.verify()?;
        return output("BRANCH ADOPTION PREVIEW — no target acceptance", &preview);
    }
    let by = options
        .by
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| error("--by ACTOR is required for explicit history branch adoption"))?;
    let mut choices = BTreeMap::new();
    for choice in &options.choices {
        let (subject, chosen) = choice
            .rsplit_once('=')
            .ok_or_else(|| error("--choose takes SUBJECT=VERSION"))?;
        require(
            !subject.trim().is_empty()
                && crate::history_paths::object_id(chosen)
                && choices.insert(subject.into(), s(chosen)).is_none(),
            "invalid_adoption_choice",
        )?;
    }
    let action = obj([
        ("kind", s("branch-adopt")),
        ("sources", map(&preview)?["sources"].clone()),
        ("choices", V::Map(choices.clone())),
        ("by", s(by)),
    ]);
    let sources = observations
        .iter()
        .map(Observation::capture)
        .collect::<Result<Vec<_>>>()?;
    if private(&target) || sources.iter().any(private) {
        let document = obj([
            ("target", target.document().clone()),
            (
                "source_history",
                V::List(
                    sources
                        .iter()
                        .map(|c| V::Map(c.history.objects().clone()))
                        .collect(),
                ),
            ),
        ]);
        return output(
            "BRANCH ADOPTION RETAINED PRIVATELY",
            &Privacy::draft(
                route.project(),
                &action,
                &document,
                "private branch history closure",
            )?,
        );
    }
    let source = crate::source_capture::capture_source(
        route.paths(),
        &route.project().root,
        crate::source_capture::ReadMode::Frozen,
        None,
    )?;
    let options = A::Options {
        operation: crate::public_history::fresh_id("branch-adopt")?,
        recorded_at: chrono::Utc::now().to_rfc3339(),
        by: by.into(),
    };
    let prepared = A::prepare(root, &observations, &V::Map(choices), &options)?
        .with_guard(&guard(route, original)?)?;
    probe("prepared")?;
    target.verify_current(root)?;
    source.verify()?;
    P::publish(
        root,
        &prepared,
        |p| {
            source.verify()?;
            verify_recovery(route, original, p)?;
            source.verify()
        },
        |phase| {
            route.verify()?;
            probe(match phase {
                P::Phase::Journal => "journal",
                P::Phase::Import(_) => "import",
                P::Phase::Append(_) => "append",
                P::Phase::Evidence(_) => "evidence",
                P::Phase::Commit => "committed",
                P::Phase::View => "view",
            })?;
            route.verify()
        },
    )?;
    output(
        "BRANCH ADOPTED",
        &obj([
            ("state", s("adopted")),
            ("operation", s(prepared.operation())),
            (
                if observations.len() == 1 {
                    "source_revision"
                } else {
                    "source_set_revision"
                },
                revision.clone(),
            ),
        ]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn v(j: serde_json::Value) -> V {
        V::from_json(&j).unwrap()
    }
    fn git(root: &Path, args: &[&str]) -> String {
        let o = std::process::Command::new("git")
            .args([
                "-C",
                root.to_str().unwrap(),
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "-c",
                "commit.gpgSign=false",
                "-c",
                "core.autocrlf=true",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8(o.stdout).unwrap().trim().into()
    }
    fn write(root: &Path, op: &str, action: V) {
        let options = crate::history_authoring::Options {
            operation: op.into(),
            recorded_at: "2026-09-24T12:00:00Z".into(),
            recording_day: "2026-09-24".into(),
            by: s("fixture"),
            strict: false,
            paths: crate::history_paths::Scheme::Hashed,
            receipt_version: None,
        };
        let p = W::prepare(root, &action, &options, None).unwrap();
        W::publish(root, &p, None, |_| Ok(())).unwrap();
    }
    fn commit(root: &Path) {
        git(root, &["add", "-f", "."]);
        git(root, &["commit", "-m", "Fixture"]);
    }
    fn fixture() -> (tempfile::TempDir, Options) {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(".gitattributes"), P::GIT_ATTRIBUTES).unwrap();
        std::fs::create_dir(root.path().join(".kpopper")).unwrap();
        std::fs::write(root.path().join(".kpopper/history.yaml"),"version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: fixture\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
        std::fs::write(root.path().join("GROUNDING.yaml"),"meta: {purpose: Fixture}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown: {}\n").unwrap();
        write(
            root.path(),
            "seed",
            v(json!({"kind":"add","id":"p.a","body":{"v":1}})),
        );
        git(root.path(), &["init"]);
        commit(root.path());
        git(root.path(), &["branch", "-M", "target"]);
        git(root.path(), &["switch", "-c", "source"]);
        write(
            root.path(),
            "right",
            v(json!({"kind":"set","id":"p.a","value":3})),
        );
        commit(root.path());
        let c = Capture::read(root.path()).unwrap();
        let chosen =
            text(&map(&map(&map(c.state()).unwrap()["subjects"]).unwrap()["p.a"]).unwrap()["head"])
                .unwrap();
        let options = Options {
            from_refs: vec!["source".into()],
            by: Some("fixture".into()),
            choices: vec![format!("p.a={chosen}")],
            ..Default::default()
        };
        git(root.path(), &["switch", "target"]);
        write(
            root.path(),
            "left",
            v(json!({"kind":"set","id":"p.a","value":2})),
        );
        commit(root.path());
        (root, options)
    }
    #[test]
    fn public_branch_recovery_rechecks_policy_and_uses_retained_source() {
        for stop in ["journal", "committed"] {
            let (root, options) = fixture();
            let original = vec![root.path().join("GROUNDING.yaml")];
            let route = WriteRoute::capture(&original, root.path()).unwrap();
            let config = route.project().config_path.clone();
            let old = std::fs::read(&config).ok();
            let mut moved = route.config().to_json().unwrap();
            moved["generation"] = json!(99);
            let error = run(&options, &route, &original, &mut |phase| {
                if phase == stop {
                    Err(error("stop"))
                } else {
                    Ok(())
                }
            })
            .err()
            .unwrap();
            assert!(error.0.contains("stop"), "{error}");
            drop(route);
            git(root.path(), &["branch", "-D", "source"]);
            std::fs::create_dir_all(config.parent().unwrap()).unwrap();
            std::fs::write(&config, serde_json::to_vec(&moved).unwrap()).unwrap();
            let journal = root.path().join(".kpopper/.history-node-publication.json");
            let held = std::fs::read(&journal).unwrap();
            let before = std::fs::read(root.path().join("GROUNDING.yaml")).unwrap();
            let route = WriteRoute::capture(&original, root.path()).unwrap();
            assert!(crate::public_node_history::recover(&route, &original, false, None).is_err());
            drop(route);
            assert_eq!(std::fs::read(&journal).unwrap(), held);
            assert_eq!(
                std::fs::read(root.path().join("GROUNDING.yaml")).unwrap(),
                before
            );
            match old {
                Some(raw) => std::fs::write(&config, raw).unwrap(),
                None => std::fs::remove_file(&config).unwrap(),
            }
            let route = WriteRoute::capture(&original, root.path()).unwrap();
            let result =
                crate::public_node_history::recover(&route, &original, false, None).unwrap();
            assert!(string_is(
                &map(&result).unwrap()["state"],
                if stop == "journal" {
                    "rolled_back"
                } else {
                    "committed"
                }
            ));
            let c = Capture::read(root.path()).unwrap();
            assert_eq!(
                map(&map(c.document()).unwrap()["known"]).unwrap()["p.a"]
                    .to_json()
                    .unwrap()["v"],
                if stop == "journal" { 2 } else { 3 }
            );
        }
    }
}

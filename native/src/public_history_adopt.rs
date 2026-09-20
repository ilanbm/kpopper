//! Public pending-contribution adoption with a retained, source-free recovery journal.
use crate::{
    Result,
    history_authoring::{n, obj, s},
    history_contract::*,
    history_contribution_adoption::{self as A, Artifact},
    history_emit as E,
    history_store::Store,
    history_transaction::{self as T, PreparedMutation},
    history_transaction_fs as F,
    history_view::map_mut,
    history_yaml as Y,
    pending_state::Ledger,
    project_modes::{self, WriteRoute},
    require,
    value::TypedValue as V,
};
use std::path::{Path, PathBuf};
pub(crate) const KIND: &str = "history-adoption/v1";

pub fn choices(items: &[String]) -> Result<V> {
    let mut choices = Map::new();
    for item in items {
        let (subject, version) = item
            .split_once('=')
            .ok_or_else(|| error("each --choose is SUBJECT=VERSION"))?;
        require(
            !choices.contains_key(subject),
            "a subject may be chosen only once",
        )?;
        choices.insert(subject.into(), s(version));
    }
    Ok(V::Map(choices))
}
fn routing(route: &WriteRoute, original: &[PathBuf]) -> Result<V> {
    let paths = |paths: &[PathBuf]| -> Result<V> {
        Ok(V::List(
            paths
                .iter()
                .map(|p| {
                    project_modes::resolved(p)?
                        .to_str()
                        .map(s)
                        .ok_or_else(|| error("nonportable_project_path"))
                })
                .collect::<Result<_>>()?,
        ))
    };
    Ok(obj([
        ("paths", paths(original)?),
        ("destination", paths(route.paths())?),
        ("policy", route.config().clone()),
    ]))
}
fn envelope(mutation: &PreparedMutation, artifact: &Artifact, routing: V) -> Result<V> {
    let mut value = obj([
        ("version", n("1")),
        ("kind", s(KIND)),
        ("routing", routing),
        ("mutation", T::blob(Some(&mutation.to_bytes()?))),
        ("artifact", artifact.retained()),
    ]);
    let digest = value.digest()?;
    map_mut(&mut value)?.insert("digest".into(), s(&digest));
    Ok(value)
}
fn decode(raw: &[u8]) -> Result<(PreparedMutation, Artifact, V)> {
    let value = Y::decode_document(raw)?;
    let m = schema(
        &value,
        &[
            "version", "kind", "routing", "mutation", "artifact", "digest",
        ],
        &[],
    )?;
    require(
        is_int(&m["version"], "1") && string_is(&m["kind"], KIND),
        "invalid_history_journal",
    )?;
    let mutation = PreparedMutation::from_bytes(
        &T::unblob(&m["mutation"])?.ok_or_else(|| error("invalid_history_journal"))?,
    )?;
    let artifact = Artifact::decode(&m["artifact"])?;
    require(
        value == envelope(&mutation, &artifact, m["routing"].clone())?,
        "invalid_history_journal",
    )?;
    Ok((mutation, artifact, m["routing"].clone()))
}
fn journal(store: &Store) -> String {
    format!("{}.history", store.layout.journal)
}
fn retained(error: crate::Error, path: &Path) -> crate::Error {
    crate::Error(format!(
        "{}: recovery journal retained at {}. Preserve conflicting edits and inspect the recorded transaction before retrying recovery; do not discard the journal.",
        error.0,
        path.display()
    ))
}
fn finish(store: &Store, mutation: &PreparedMutation, path: &Path, raw: &[u8]) -> Result<()> {
    for file in mutation.files() {
        require(
            F::read(&F::target(&store.root, &file.path)?)? == file.after,
            "concurrent_edit",
        )?;
    }
    require(F::read(path)?.as_deref() == Some(raw), "concurrent_edit")?;
    F::remove(path)
}
fn cancelled(store: &Store, mutation: &PreparedMutation) -> Result<()> {
    for file in mutation.files() {
        let current = F::read(&F::target(&store.root, &file.path)?)?;
        let immutable = ["history_object", "history_evidence"].contains(&file.role.as_str());
        require(
            current == file.before || immutable && current == file.after,
            "concurrent_edit",
        )?;
    }
    Ok(())
}
fn verify_ledger(route: &WriteRoute, head: &Option<String>) -> Result<()> {
    route.verify()?;
    require(
        crate::pending_state::resolve(&route.project().root, crate::pending_state::REF)? == *head,
        "pending_changed",
    )
}
pub fn run(
    original: &[PathBuf],
    cwd: &Path,
    revision: &str,
    choices: &V,
    by: Option<&str>,
    preview: bool,
) -> Result<V> {
    run_with_probe(original, cwd, revision, choices, by, preview, &mut |_| {
        Ok(())
    })
}
fn run_with_probe(
    original: &[PathBuf],
    cwd: &Path,
    revision: &str,
    choices: &V,
    by: Option<&str>,
    preview: bool,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<V> {
    if !preview {
        require(
            by.is_some_and(|s| !s.is_empty()),
            "adoption requires an explicit --by actor",
        )?;
    }
    let route = WriteRoute::capture(original, cwd)?;
    require(route.paths().len() == 1, "choose one logical record entry")?;
    let store = Store::new(&route.paths()[0])?;
    let _lock = F::DirectoryGuard::acquire(&store.root, !preview)?;
    let ledger = Ledger::capture(route.project())?;
    let bundle = ledger.bundles.get(revision).ok_or_else(|| {
        error(if preview {
            "unknown contribution revision"
        } else {
            "unknown_contribution"
        })
    })?;
    let artifact = Artifact::from_contribution(bundle)?;
    let capture = store.capture()?;
    if preview {
        let result = A::preview(&capture, &artifact)?;
        probe("preview")?;
        capture.verify_current()?;
        verify_ledger(&route, &ledger.head)?;
        return Ok(result);
    }
    let relative = journal(&store);
    let path = F::target(&store.root, &relative)?;
    require(F::read(&path)?.is_none(), "recovery_required")?;
    let options = A::Options {
        operation: format!("adopt-{}", uuid::Uuid::new_v4().simple()),
        recorded_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, false),
        by: by.unwrap().into(),
    };
    let mutation = A::prepare(&capture, &artifact, choices, &options)?;
    probe("prepared")?;
    verify_ledger(&route, &ledger.head)?;
    capture.verify_current()?;
    let raw = E::encode_document(&envelope(&mutation, &artifact, routing(&route, original)?)?)?;
    let parent = relative
        .rsplit_once('/')
        .ok_or_else(|| error("invalid_history_journal"))?
        .0;
    F::publish_immutable(&store.root, &format!("{parent}/.gitignore"), b"*\n")?;
    F::publish_immutable(&store.root, &relative, &raw)?;
    probe("journal")?;
    A::commit(&store, &mutation, &artifact, &mut |_| {
        probe("verified")?;
        verify_ledger(&route, &ledger.head)?;
        require(F::read(&path)?.as_ref() == Some(&raw), "concurrent_edit")
    })
    .map_err(|e| retained(e, &path))?;
    probe("committed")?;
    route.verify()?;
    finish(&store, &mutation, &path, &raw).map_err(|e| retained(e, &path))?;
    Ok(obj([
        ("state", s("adopted")),
        ("revision", s(revision)),
        ("operation", s(&options.operation)),
    ]))
}
/// Called with the policy and target-directory guards already held by recovery.
/// The retained artifact is the source of truth; changed/deleted pending refs do
/// not discard an already prepared and explicitly authorized adoption.
pub(crate) fn recover(
    store: &Store,
    route: &WriteRoute,
    original: &[PathBuf],
    path: &Path,
    raw: &[u8],
    before: bool,
) -> Result<V> {
    let (mutation, artifact, recorded_route) = decode(raw)?;
    require(
        routing(route, original)? == recorded_route,
        "history_routing_changed",
    )?;
    route.verify()?;
    if before {
        require(
            !store
                .capture()?
                .commits
                .contains_key(text(&map(&mutation.to_data())?["operation"])?),
            "history_already_committed",
        )?;
        cancelled(store, &mutation)?;
        A::verify(&store.capture()?, &mutation, &artifact)?;
        route.verify()?;
        cancelled(store, &mutation)?;
        require(F::read(path)?.as_deref() == Some(raw), "concurrent_edit")?;
        F::remove(path)?;
    } else {
        A::commit(store, &mutation, &artifact, &mut |_| {
            route.verify()?;
            require(F::read(path)?.as_deref() == Some(raw), "concurrent_edit")
        })
        .map_err(|e| retained(e, path))?;
        route.verify()?;
        finish(store, &mutation, path, raw).map_err(|e| retained(e, path))?;
    }
    let data = mutation.to_data();
    let data = map(&data)?;
    Ok(obj([
        ("state", s(if before { "restored" } else { "recovered" })),
        ("operation", data["operation"].clone()),
        ("mutation_digest", data["digest"].clone()),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_contribution_adoption::tests as fixture;
    use std::fs;
    fn selected() -> serde_json::Value {
        fixture::cases()["cases"][0].clone()
    }
    fn revision(case: &serde_json::Value) -> String {
        text(&map(&V::from_tagged(&case["bundle"]).unwrap()).unwrap()["revision"])
            .unwrap()
            .into()
    }
    fn images(store: &Store) -> crate::history_authority::Files {
        let capture = store.capture().unwrap();
        let mut files = crate::history_authority::Files::new();
        files.insert(store.layout.entry.clone(), capture.entry_bytes);
        files.insert(store.layout.authority.clone(), capture.authority_bytes);
        for (p, b) in capture.storage_bytes {
            files.insert(format!("{}/{p}", store.layout.objects), b);
        }
        for (op, b) in capture.commits {
            files.insert(format!("{}/{op}.yaml", store.layout.commits), b);
        }
        files
    }
    #[test]
    fn public_preview_choices_and_refusals_preserve_records_and_pending() {
        let case = selected();
        let dir = tempfile::tempdir().unwrap();
        let store = fixture::install(dir.path(), &case, true);
        let original = vec![store.entry.clone()];
        let rev = revision(&case);
        let choices = V::from_tagged(&case["choices"]).unwrap();
        let before = images(&store);
        let pending = fixture::git(dir.path(), &["rev-parse", crate::pending_state::REF]);
        assert_eq!(
            run(&original, dir.path(), &rev, &choices, None, true).unwrap(),
            V::from_tagged(&case["preview"]).unwrap()
        );
        assert_eq!(
            run(&original, dir.path(), &rev, &choices, None, false)
                .unwrap_err()
                .0,
            "adoption requires an explicit --by actor"
        );
        assert_eq!(
            run(
                &original,
                dir.path(),
                &rev,
                &V::Map(Map::new()),
                Some("adopter"),
                false
            )
            .unwrap_err()
            .0,
            "adoption_choice_required"
        );
        assert_eq!(
            run(
                &original,
                dir.path(),
                "unknown",
                &choices,
                Some("adopter"),
                false
            )
            .unwrap_err()
            .0,
            "unknown_contribution"
        );
        assert_eq!(
            run(&original, dir.path(), "unknown", &choices, None, true)
                .unwrap_err()
                .0,
            "unknown contribution revision"
        );
        assert_eq!(images(&store), before);
        assert_eq!(
            fixture::git(dir.path(), &["rev-parse", crate::pending_state::REF]),
            pending
        );
        assert!(!dir.path().join(journal(&store)).exists());
        assert!(self::choices(&["p.x".into()]).is_err());
        assert!(self::choices(&["p.x=a".into(), "p.x=b".into()]).is_err());
        assert_eq!(
            self::choices(&["p.x=a=b".into()]).unwrap(),
            obj([("p.x", s("a=b"))])
        );
    }
    #[test]
    fn source_policy_and_target_races_fail_before_publication() {
        for stage in ["prepared", "verified"] {
            for race in ["ledger", "policy", "target", "journal"] {
                if stage == "prepared" && race == "journal" {
                    continue;
                }
                let case = selected();
                let dir = tempfile::tempdir().unwrap();
                let store = fixture::install(dir.path(), &case, true);
                let original = vec![store.entry.clone()];
                let rev = revision(&case);
                let choices = V::from_tagged(&case["choices"]).unwrap();
                let before = images(&store);
                let mut expected = before.clone();
                let result = run_with_probe(
                    &original,
                    dir.path(),
                    &rev,
                    &choices,
                    Some("adopter"),
                    false,
                    &mut |at| {
                        if at == stage {
                            match race {
                                "ledger" => {
                                    fixture::git(
                                        dir.path(),
                                        &["update-ref", "-d", crate::pending_state::REF],
                                    );
                                }
                                "policy" => {
                                    let p = project_modes::Project::open(dir.path())?;
                                    fs::write(&p.config_path,b"{\"version\":1,\"mode\":\"simple\",\"generation\":2,\"record\":\"GROUNDING.yaml\",\"publication\":null}")?;
                                }
                                "target" => {
                                    let mut raw = fs::read(&store.entry)?;
                                    raw.extend(b"# concurrent human edit\n");
                                    fs::write(&store.entry, &raw)?;
                                    expected.insert(store.layout.entry.clone(), raw);
                                }
                                "journal" => {
                                    fs::write(
                                        dir.path().join(journal(&store)),
                                        b"changed journal",
                                    )?;
                                }
                                _ => unreachable!(),
                            }
                        }
                        Ok(())
                    },
                );
                assert!(result.is_err(), "{stage}/{race}");
                assert_eq!(images(&store), expected, "{stage}/{race}");
                assert_eq!(
                    dir.path().join(journal(&store)).exists(),
                    stage == "verified"
                );
            }
        }
    }
    #[test]
    fn all_partial_file_images_recover_or_cancel_without_source_ref() {
        let case = selected();
        let (_, _, choices, _) = fixture::inputs(&case);
        let rev = revision(&case);
        // Objects first, manifest next, rendered record last: include every prefix.
        let mut count = None;
        for before in [false, true] {
            let mut prefix = 0;
            loop {
                let dir = tempfile::tempdir().unwrap();
                let store = fixture::install(dir.path(), &case, true);
                let original = vec![store.entry.clone()];
                let initial = images(&store);
                let result = run_with_probe(
                    &original,
                    dir.path(),
                    &rev,
                    &choices,
                    Some("adopter"),
                    false,
                    &mut |stage| require(stage != "journal", "interrupted"),
                );
                assert_eq!(result.unwrap_err().0, "interrupted");
                let path = dir.path().join(journal(&store));
                let raw = fs::read(&path).unwrap();
                let (mutation, _, _) = decode(&raw).unwrap();
                let mut files = mutation.files().iter().collect::<Vec<_>>();
                files.sort_by_key(|f| match f.role.as_str() {
                    "history_object" => 0,
                    "history_commit" => 1,
                    "record" => 2,
                    _ => 3,
                });
                count.get_or_insert(files.len());
                for file in files.iter().take(prefix) {
                    fixture::write(
                        dir.path(),
                        &[(file.path.clone(), file.after.clone().unwrap())].into(),
                    );
                }
                fixture::git(dir.path(), &["update-ref", "-d", crate::pending_state::REF]);
                let recovered = crate::direct_history::recover(&original, dir.path(), before);
                let has_commit = files
                    .iter()
                    .take(prefix)
                    .any(|f| f.role == "history_commit");
                if before && has_commit {
                    assert!(
                        recovered
                            .unwrap_err()
                            .0
                            .starts_with("history_already_committed")
                    );
                    assert_eq!(fs::read(&path).unwrap(), raw);
                } else {
                    let result = recovered.unwrap();
                    assert_eq!(
                        map(&result).unwrap()["state"],
                        s(if before { "restored" } else { "recovered" })
                    );
                    assert!(!path.exists());
                    if before {
                        for (p, b) in &initial {
                            assert_eq!(fs::read(dir.path().join(p)).unwrap(), *b);
                        }
                    } else {
                        for file in mutation.files() {
                            assert_eq!(
                                fs::read(dir.path().join(&file.path)).unwrap(),
                                file.after.clone().unwrap()
                            );
                        }
                    }
                    // Replay is idempotent even after the original journal is restored.
                    if !before {
                        fs::write(&path, &raw).unwrap();
                        crate::direct_history::recover(&original, dir.path(), false).unwrap();
                        assert!(!path.exists());
                    }
                }
                if prefix == count.unwrap() {
                    break;
                }
                prefix += 1;
            }
        }
    }
    #[test]
    fn recovery_refuses_policy_and_retained_artifact_forgery_and_keeps_journal() {
        for race in ["policy", "artifact", "record"] {
            let case = selected();
            let dir = tempfile::tempdir().unwrap();
            let store = fixture::install(dir.path(), &case, true);
            let original = vec![store.entry.clone()];
            let rev = revision(&case);
            let choices = V::from_tagged(&case["choices"]).unwrap();
            run_with_probe(
                &original,
                dir.path(),
                &rev,
                &choices,
                Some("adopter"),
                false,
                &mut |at| require(at != "journal", "interrupted"),
            )
            .unwrap_err();
            let path = dir.path().join(journal(&store));
            match race {
                "policy" => {
                    let p = project_modes::Project::open(dir.path()).unwrap();
                    fs::write(p.config_path,b"{\"version\":1,\"mode\":\"simple\",\"generation\":2,\"record\":\"GROUNDING.yaml\",\"publication\":null}").unwrap();
                }
                "artifact" => {
                    let raw = fs::read(&path).unwrap();
                    let (mutation, mut artifact, route) = decode(&raw).unwrap();
                    let p = artifact
                        .files
                        .keys()
                        .find(|p| p.starts_with("objects/"))
                        .unwrap()
                        .clone();
                    artifact.files.get_mut(&p).unwrap().extend(b"tampered");
                    fs::write(
                        &path,
                        E::encode_document(&envelope(&mutation, &artifact, route).unwrap())
                            .unwrap(),
                    )
                    .unwrap();
                }
                "record" => {
                    let mut raw = fs::read(&store.entry).unwrap();
                    raw.extend(b"# preserved human edit\n");
                    fs::write(&store.entry, raw).unwrap();
                }
                _ => unreachable!(),
            }
            let raw = fs::read(&path).unwrap();
            let before = images(&store);
            assert!(
                crate::direct_history::recover(&original, dir.path(), false).is_err(),
                "{race}"
            );
            assert_eq!(fs::read(&path).unwrap(), raw);
            assert_eq!(images(&store), before);
        }
    }
}

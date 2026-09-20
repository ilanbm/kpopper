//! Routed public history acts retain their exact intent before publishing objects.
//! Recovery replays that intent with fresh policy and semantic verification.
use crate::{
    Result, history_adapter,
    history_authoring::{self as A, n, obj, s},
    history_contract::*,
    history_edits as E, history_emit, history_hypothesis_authoring as HA, history_identity as I,
    history_paths::Scheme,
    history_store::Store,
    history_transaction::{self as T, PreparedMutation},
    history_transaction_fs as F,
    history_view::map_mut,
    history_yaml,
    project_modes::WriteRoute,
    public_history::fresh_id,
    public_workspace,
    reasoning_runtime::Runtime,
    recording_privacy as Privacy, require,
    value::TypedValue as V,
};
use std::path::{Path, PathBuf};

fn routing(route: &WriteRoute, original: &[PathBuf]) -> Result<V> {
    let paths = |paths: &[PathBuf]| -> Result<V> {
        Ok(V::List(
            paths
                .iter()
                .map(|p| {
                    let path = crate::project_modes::resolved(p)?;
                    path.to_str()
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
fn envelope(mutation: &PreparedMutation, route: V) -> Result<V> {
    let mut value = obj([
        ("version", n("1")),
        ("kind", s("direct-history/v1")),
        ("routing", route),
        ("mutation", T::blob(Some(&mutation.to_bytes()?))),
    ]);
    let digest = value.digest()?;
    map_mut(&mut value)?.insert("digest".into(), s(&digest));
    Ok(value)
}
fn decode(raw: &[u8]) -> Result<(PreparedMutation, V)> {
    let value = history_yaml::decode_document(raw)?;
    let fields = schema(
        &value,
        &["version", "kind", "routing", "mutation", "digest"],
        &[],
    )?;
    require(
        is_int(&fields["version"], "1") && string_is(&fields["kind"], "direct-history/v1"),
        "invalid_history_journal",
    )?;
    let bytes = T::unblob(&fields["mutation"])?.ok_or_else(|| error("invalid_history_journal"))?;
    let mutation = PreparedMutation::from_bytes(&bytes)?;
    require(
        value == envelope(&mutation, fields["routing"].clone())?,
        "invalid_history_journal",
    )?;
    Ok((mutation, fields["routing"].clone()))
}
fn options(prefix: &str, by: V) -> Result<A::Options> {
    let now = chrono::Utc::now();
    let day = chrono::Local::now().date_naive();
    Ok(A::Options {
        operation: fresh_id(prefix)?,
        recorded_at: now.to_rfc3339(),
        recording_day: day.to_string(),
        by,
        strict: true,
        paths: Scheme::Hashed,
        receipt_version: None,
    })
}
enum ReceiptFamily {
    Authoring,
    Edit,
    Hypothesis,
    Identity,
    Branch,
}
type RecoveryProbe<'a> = Option<&'a mut dyn FnMut(&str) -> Result<()>>;

fn verify_report_route(
    route: &WriteRoute,
    expected: &PreparedMutation,
) -> Result<()> {
    let data = expected.to_data();
    let receipt = map(&map(&data)?["receipt"])?;
    let before = map(&receipt["before"])?;
    let authoring = map(before.get("authoring").ok_or_else(|| error("invalid report journal"))?)?;
    let context = map(authoring.get("context").ok_or_else(|| error("invalid report journal"))?)?;
    require(context.get("policy") == Some(route.config()),
        "report_preparation_stale: project policy changed")?;
    let routing = crate::source_capture::routing_observation(
        route.paths(), &route.project().root,
    )?;
    require(context.get("routing") == Some(&routing),
        "report_preparation_stale: project routing changed")
}
fn receipt_family(mutation: &PreparedMutation) -> Result<ReceiptFamily> {
    let data = mutation.to_data();
    let receipt = map(&map(&data)?["receipt"])?;
    let before = map(&receipt["before"])?;
    let after = map(&receipt["after"])?;
    Ok(if after.contains_key("history_branch_adoption") {
        ReceiptFamily::Branch
    } else if before.contains_key("identity_authoring") {
        ReceiptFamily::Identity
    } else if before.contains_key("history_edit") {
        ReceiptFamily::Edit
    } else if before.contains_key("hypothesis_authoring") {
        ReceiptFamily::Hypothesis
    } else {
        ReceiptFamily::Authoring
    })
}
fn verify_mutation(
    store: &Store,
    mutation: &PreparedMutation,
    runtime: Option<&Runtime>,
) -> Result<()> {
    match receipt_family(mutation)? {
        ReceiptFamily::Edit => E::verify_prepared(store, mutation, runtime),
        ReceiptFamily::Hypothesis => HA::verify_prepared(store, mutation, runtime),
        ReceiptFamily::Identity => I::verify_prepared(store, mutation, runtime),
        ReceiptFamily::Branch => crate::history_branch_adoption::verify(
            &store.capture()?, mutation, &crate::history_branch_adoption::live_evidence(store, mutation)?),
        ReceiptFamily::Authoring => A::verify_prepared(store, mutation, runtime),
    }
}
fn commit(
    store: &Store,
    mutation: &PreparedMutation,
    runtime: Option<&Runtime>,
    verify: F::Verify<'_>,
) -> Result<V> {
    match receipt_family(mutation)? {
        ReceiptFamily::Edit => E::commit(store, mutation, runtime, verify),
        ReceiptFamily::Hypothesis => HA::commit(store, mutation, runtime, verify),
        ReceiptFamily::Identity => I::commit(store, mutation, runtime, verify),
        ReceiptFamily::Branch => crate::history_branch_adoption::commit(store, mutation, verify),
        ReceiptFamily::Authoring => A::commit(store, mutation, runtime, verify),
    }
}
pub(crate) fn journal(store: &Store) -> String {
    format!("{}.history", store.layout.journal)
}
fn retained_journal(error: crate::Error, journal: &Path) -> crate::Error {
    crate::Error(format!(
        "{}: recovery journal retained at {}. Preserve conflicting edits and inspect the recorded transaction before retrying recovery; do not discard the journal.",
        error.0,
        journal.display()
    ))
}
fn finish(store: &Store, mutation: &PreparedMutation, path: &Path, raw: &[u8]) -> Result<()> {
    // No retained guard is released until all mutable and immutable after-images match.
    for file in mutation.files() {
        require(
            F::read(&F::target(&store.root, &file.path)?)? == file.after,
            "concurrent_edit",
        )
        .map_err(|e| retained_journal(e, path))?;
    }
    require(F::read(path)?.as_deref() == Some(raw), "concurrent_edit")?;
    F::remove(path)
}
fn cancelled_images(store: &Store, mutation: &PreparedMutation) -> Result<()> {
    for file in mutation.files() {
        let current = F::read(&F::target(&store.root, &file.path)?)?;
        // Interrupted object publication is retained as immutable evidence. It
        // never permits a later editable view to pass as the restored before image.
        let immutable = [
            "history_object",
            "history_commit",
            "history_evidence",
            "history_retained",
        ]
        .contains(&file.role.as_str());
        require(
            current == file.before || immutable && current == file.after,
            "concurrent_edit",
        )?;
    }
    Ok(())
}
fn publish(
    store: &Store,
    mutation: &PreparedMutation,
    route: &WriteRoute,
    original: &[PathBuf],
    runtime: Option<&Runtime>,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<V> {
    let routing = routing(route, original)?;
    let raw = history_emit::encode_document(&envelope(mutation, routing.clone())?)?;
    let relative = journal(store);
    let path = F::target(&store.root, &relative)?;
    require(F::read(&path)?.is_none(), "recovery_required")?;
    route.verify()?;
    let parent = relative
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .ok_or_else(|| error("invalid_history_journal"))?;
    F::publish_immutable(&store.root, &format!("{parent}/.gitignore"), b"*\n")?;
    F::publish_immutable(&store.root, &relative, &raw)?;
    probe("journal")?;
    let result = commit(store, mutation, runtime, &mut |_| {
        route.verify()?;
        require(F::read(&path)?.as_ref() == Some(&raw), "concurrent_edit")
    })
    .map_err(|e| retained_journal(e, &path))?;
    probe("committed")?;
    route.verify()?;
    finish(store, mutation, &path, &raw)?;
    Ok(result)
}

/// Publish a mutation prepared by another public history adapter through the
/// same retained journal used by every direct-history write. The caller owns
/// the route and directory guards across preparation and this call.
pub(crate) fn publish_prepared(
    store: &Store,
    mutation: &PreparedMutation,
    route: &WriteRoute,
    original: &[PathBuf],
    runtime: Option<&Runtime>,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<V> {
    publish(store, mutation, route, original, runtime, probe)
}

pub fn act(original: &[PathBuf], cwd: &Path, action: &V) -> Result<V> {
    act_with_probe(original, cwd, action, &mut |_| Ok(()))
}
fn act_with_probe(
    original: &[PathBuf],
    cwd: &Path,
    action: &V,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<V> {
    let a = schema(action, &["kind", "id", "of", "over", "because"], &[])?;
    require(
        ["accept", "refute", "correct", "propose", "retire"].contains(&text(&a["kind"])?),
        "invalid_history_act",
    )?;
    let route = WriteRoute::capture(original, cwd)?;
    require(route.paths().len() == 1, "choose one logical record entry")?;
    let store = Store::new(&route.paths()[0])?;
    let _lock = F::DirectoryGuard::acquire(&store.root, true)?;
    require(
        F::read(&F::target(&store.root, &journal(&store))?)?.is_none(),
        "recovery_required",
    )?;
    let captured = store.capture()?;
    if let Some(target) = captured.objects.get(text(&a["of"])?)
        && Privacy::private_marker(target)
    {
        return Privacy::draft(
            route.project(),
            action,
            &obj([("target", target.clone())]),
            "private historical claim",
        );
    }
    let mut document = history_adapter::from_store_capture(&captured)?
        .document()
        .clone();
    if let Some(meta) = map_mut(&mut document)?.get_mut("meta") {
        map_mut(meta)?.remove("history");
    }
    if let Some(draft) = Privacy::selected_draft(route.project(), action, &document)? {
        return Ok(draft);
    }
    let runtime = public_workspace::runtime_for_document(&document)?;
    let mutation = A::prepare_act(
        &store,
        &captured,
        action,
        &options("act", V::Null)?,
        runtime.as_ref(),
    )?;
    let authored = V::List(
        mutation
            .files()
            .iter()
            .filter(|f| f.role == "history_object")
            .map(|f| history_yaml::decode_document(f.after.as_ref().unwrap()))
            .collect::<Result<_>>()?,
    );
    if Privacy::private_marker(&authored) {
        return Privacy::draft(
            route.project(),
            action,
            &obj([("history", authored)]),
            "private historical proposal",
        );
    }
    publish(&store, &mutation, &route, original, runtime.as_ref(), probe)?;
    crate::session_activity::published(
        &store.root,
        mutation.files(),
        Some(&std::collections::BTreeSet::from([
            text(&a["id"])?.to_owned()
        ])),
    );
    Ok(obj([
        ("state", s("committed")),
        ("act", a["kind"].clone()),
        ("subject", a["id"].clone()),
        ("of", a["of"].clone()),
    ]))
}

pub fn write(original: &[PathBuf], cwd: &Path, action: &V) -> Result<V> {
    write_with_probe(original, cwd, action, &mut |_| Ok(()))
}
fn write_with_probe(
    original: &[PathBuf],
    cwd: &Path,
    action: &V,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<V> {
    let a = map(action)?;
    let kind = text(field(a, "kind")?)?;
    require(
        ["add", "set", "review"].contains(&kind),
        "unsupported_history_action",
    )?;
    let route = WriteRoute::capture(original, cwd)?;
    require(route.paths().len() == 1, "choose one logical record entry")?;
    let store = Store::new(&route.paths()[0])?;
    let _lock = F::DirectoryGuard::acquire(&store.root, true)?;
    require(
        F::read(&F::target(&store.root, &journal(&store))?)?.is_none(),
        "recovery_required",
    )?;
    let captured = store.capture()?;
    let write_options = options("write", V::Null)?;
    let hypothesis = a
        .get("hypothesis")
        .filter(|v| **v != V::Null)
        .map(text)
        .transpose()?;
    let mut document = if let Some(name) = hypothesis {
        let context = HA::capture(&store, &captured, &write_options)?;
        if map(&context.groups)?.contains_key(name) {
            HA::layer(&context.base, &context.groups, &[name.to_owned()])?
        } else {
            context.base
        }
    } else {
        history_adapter::from_store_capture(&captured)?
            .document()
            .clone()
    };
    if let Some(meta) = map_mut(&mut document)?.get_mut("meta") {
        map_mut(meta)?.remove("history");
    }
    if let Some(draft) = Privacy::selected_draft(route.project(), action, &document)? {
        return Ok(draft);
    }
    let id = text(field(a, "id")?)?;
    let existing = crate::reasoning_snapshot::entries(&document)?;
    let mut candidate = document.clone();
    if kind == "add" {
        for collection in crate::reasoning_fields::collections(&document)?.keys() {
            map_mut(map_mut(&mut candidate)?.get_mut(collection).unwrap())?.remove(id);
        }
        let collection = a
            .get("into")
            .and_then(|v| text(v).ok())
            .or_else(|| existing.get(id).map(|(c, _)| c.as_str()))
            .unwrap_or("known");
        map_mut(
            map_mut(&mut candidate)?
                .entry(collection.into())
                .or_insert_with(crate::history_authoring::empty),
        )?
        .insert(id.into(), field(a, "body")?.clone());
    } else if kind == "set"
        && let Some((collection, body)) = existing.get(id)
        && matches!(body, V::Map(_))
    {
        let mut body = body.clone();
        let b = map_mut(&mut body)?;
        let key = if b.contains_key("v") { "v" } else { "quoted" };
        b.insert(key.into(), field(a, "value")?.clone());
        if let Some(source) = a.get("source").filter(|v| **v != V::Null) {
            b.insert("from".into(), source.clone());
        }
        map_mut(map_mut(&mut candidate)?.get_mut(collection).unwrap())?.insert(id.into(), body);
    }
    if let Some(draft) = Privacy::candidate_draft(route.project(), action, &candidate)? {
        return Ok(draft);
    }
    let runtime = public_workspace::runtime_for_document(&document)?;
    let mutation = if let Some(name) = hypothesis {
        HA::prepare(
            &store,
            &captured,
            name,
            action,
            None,
            &write_options,
            runtime.as_ref(),
        )?
    } else {
        A::prepare(&store, &captured, action, &write_options, runtime.as_ref())?
    };
    let authored = V::List(
        mutation
            .files()
            .iter()
            .filter(|f| f.role == "history_object")
            .map(|f| history_yaml::decode_document(f.after.as_ref().unwrap()))
            .collect::<Result<_>>()?,
    );
    if Privacy::private_marker(&authored) {
        return Privacy::draft(
            route.project(),
            action,
            &obj([("history", authored)]),
            "private historical proposal",
        );
    }
    publish(&store, &mutation, &route, original, runtime.as_ref(), probe)?;
    crate::session_activity::published(
        &store.root,
        mutation.files(),
        Some(&std::collections::BTreeSet::from([id.to_owned()])),
    );
    Ok(obj([
        ("state", s("committed")),
        ("operation", map(&mutation.to_data())?["operation"].clone()),
    ]))
}

pub fn proposals(
    original: &[PathBuf],
    cwd: &Path,
    subjects: Option<&[String]>,
    because: &str,
    by: Option<&str>,
) -> Result<V> {
    let route = WriteRoute::capture(original, cwd)?;
    require(route.paths().len() == 1, "choose one logical record entry")?;
    let store = Store::new(&route.paths()[0])?;
    let _lock = F::DirectoryGuard::acquire(&store.root, true)?;
    require(
        F::read(&F::target(&store.root, &journal(&store))?)?.is_none(),
        "recovery_required",
    )?;
    let captured = store.capture_reconciliation(true)?;
    let runtime = public_workspace::runtime_for_document(&captured.document)?;
    let mutation = E::prepare_proposals(
        &store,
        because,
        subjects,
        &options("view-edit", by.map(s).unwrap_or(V::Null))?,
        runtime.as_ref(),
    )?;
    for file in mutation
        .files()
        .iter()
        .filter(|f| f.role == "history_object")
    {
        require(
            !Privacy::private_marker(&history_yaml::decode_document(
                file.after.as_ref().unwrap(),
            )?),
            "private_proposal_requires_draft",
        )?;
    }
    publish(
        &store,
        &mutation,
        &route,
        original,
        runtime.as_ref(),
        &mut |_| Ok(()),
    )?;
    Ok(obj([
        ("state", s("proposed")),
        ("operation", map(&mutation.to_data())?["operation"].clone()),
        (
            "subjects",
            subjects
                .map(|s| V::List(s.iter().map(|s| V::Text(s.clone())).collect()))
                .unwrap_or(V::Null),
        ),
    ]))
}

pub fn recover(original: &[PathBuf], cwd: &Path, before: bool) -> Result<V> {
    recover_with_runtime(original, cwd, before, None)
}
pub(crate) fn recover_with_runtime(
    original: &[PathBuf],
    cwd: &Path,
    before: bool,
    runtime_override: Option<&Runtime>,
) -> Result<V> {
    recover_inner(original, cwd, before, runtime_override, None, None)
}

/// Finish only the exact history mutation retained by an outer report journal.
/// An already-finished operation is accepted only when every published image
/// still matches; a different pending journal is never recovered.
pub(crate) fn recover_expected(
    original: &[PathBuf],
    cwd: &Path,
    runtime_override: Option<&Runtime>,
    expected: &PreparedMutation,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<V> {
    recover_inner(
        original,
        cwd,
        false,
        runtime_override,
        Some(expected),
        Some(probe),
    )
}

fn recover_inner(
    original: &[PathBuf],
    cwd: &Path,
    before: bool,
    runtime_override: Option<&Runtime>,
    expected: Option<&PreparedMutation>,
    resume_probe: RecoveryProbe<'_>,
) -> Result<V> {
    let route = WriteRoute::capture(original, cwd)?;
    require(route.paths().len() == 1, "choose one logical record entry")?;
    if let Some(expected) = expected {
        verify_report_route(&route, expected)?;
    }
    let store = Store::new(&route.paths()[0])?;
    let _lock = F::DirectoryGuard::acquire(&store.root, true)?;
    if let Some(raw) = F::read(&F::target(&store.root, &store.layout.journal)?)? {
        require(expected.is_none(), "history report recovery journal mismatch")?;
        let mutation = PreparedMutation::from_bytes(&raw)?;
        let data = mutation.to_data();
        let baseline = map(&map(&data)?["baseline"])?;
        if baseline
            .get("bootstrap")
            .and_then(|b| map(b).ok())
            .and_then(|b| b.get("kind"))
            .is_some_and(|k| string_is(k, crate::history_bootstrap::KIND))
        {
            let runtime = public_workspace::core_runtime()?;
            let mutation = crate::history_bootstrap::recover(
                &store.entry,
                route.config(),
                runtime.as_ref(),
                if before {
                    F::Direction::Before
                } else {
                    F::Direction::After
                },
                &mut |_| route.verify(),
            )?;
            let data = mutation.to_data();
            return Ok(obj([
                ("state", s(if before { "restored" } else { "recovered" })),
                ("operation", map(&data)?["operation"].clone()),
                ("mutation_digest", map(&data)?["digest"].clone()),
            ]));
        }
        require(
            map(&data)?.get("transition").is_none(),
            "transition_recovery_required: history activation requires the original managed-deployment guard",
        )?;
        return Err(error("unsupported public recovery journal"));
    }
    let path = F::target(&store.root, &journal(&store))?;
    let Some(raw) = F::read(&path)? else {
        let expected = expected.ok_or_else(|| error("no_recovery_pending"))?;
        let data = expected.to_data();
        let operation = text(&map(&data)?["operation"])?;
        let capture = store.capture()
            .map_err(|e| error(&format!("report_preparation_stale: {e}")))?;
        let manifest = expected.files().iter().find(|file| file.role == "history_commit")
            .and_then(|file| file.after.as_ref()).ok_or_else(|| error("invalid_history_journal"))?;
        let mut completed = capture.commits.get(operation) == Some(manifest);
        if completed {
            for file in expected.files() {
                if F::read(&F::target(&store.root, &file.path)?)? != file.after {
                    completed = false;
                    break;
                }
            }
        }
        if !completed {
            for file in expected.files() {
                require(
                    F::read(&F::target(&store.root, &file.path)?)? == file.before,
                    "report_preparation_stale: history report preparation changed before publication",
                )?;
            }
            let loaded_runtime;
            let runtime = if let Some(runtime) = runtime_override {
                Some(runtime)
            } else {
                loaded_runtime = if string_is(
                    &map(&map(&expected.to_data())?["receipt"])?["profile"],
                    "core/v1",
                ) {
                    public_workspace::core_runtime()?
                } else {
                    public_workspace::runtime()?
                };
                loaded_runtime.as_ref()
            };
            verify_mutation(&store, expected, runtime)
                .map_err(|e| error(&format!("report_preparation_stale: {e}")))?;
            for file in expected.files().iter().filter(|file| file.role == "history_object") {
                require(
                    !Privacy::private_marker(&history_yaml::decode_document(
                        file.after.as_ref().ok_or_else(|| error("invalid_history_journal"))?,
                    )?),
                    "report_preparation_stale: private_proposal_requires_draft",
                )?;
            }
            let mut noop = |_: &str| Ok(());
            let probe = match resume_probe {
                Some(probe) => probe,
                None => &mut noop,
            };
            publish(&store, expected, &route, original, runtime, probe)?;
        }
        return Ok(obj([
            ("state", s("recovered")),
            ("operation", s(operation)),
            ("mutation_digest", map(&data)?["digest"].clone()),
        ]));
    };
    if map(&history_yaml::decode_document(&raw)?)?
        .get("kind")
        .is_some_and(|kind| string_is(kind, crate::public_history_adopt::KIND))
    {
        require(expected.is_none(), "history report recovery journal mismatch")?;
        return crate::public_history_adopt::recover(&store, &route, original, &path, &raw, before);
    }
    let (mutation, retained) = decode(&raw)?;
    if let Some(expected) = expected {
        require(mutation.to_bytes()? == expected.to_bytes()?,
            "history report recovery journal mismatch")?;
    }
    require(
        routing(&route, original)? == retained,
        "history_routing_changed",
    )?;
    let loaded_runtime;
    let runtime = if let Some(runtime) = runtime_override {
        Some(runtime)
    } else {
        loaded_runtime = if string_is(
            &map(&map(&mutation.to_data())?["receipt"])?["profile"],
            "core/v1",
        ) {
            public_workspace::core_runtime()?
        } else {
            public_workspace::runtime()?
        };
        loaded_runtime.as_ref()
    };
    route.verify()?;
    if before {
        require(
            !store
                .capture()?
                .commits
                .contains_key(text(&map(&mutation.to_data())?["operation"])?),
            "history_already_committed: committed evidence requires an explicit new act",
        )?;
        cancelled_images(&store, &mutation)?;
        verify_mutation(&store, &mutation, runtime)?;
        route.verify()?;
        cancelled_images(&store, &mutation)?;
        require(F::read(&path)?.as_ref() == Some(&raw), "concurrent_edit")?;
        F::remove(&path)?;
    } else {
        commit(&store, &mutation, runtime, &mut |_| {
            route.verify()?;
            require(F::read(&path)?.as_ref() == Some(&raw), "concurrent_edit")
        })
        .map_err(|e| retained_journal(e, &path))?;
        route.verify()?;
        finish(&store, &mutation, &path, &raw)?;
    }
    let data = mutation.to_data();
    Ok(obj([
        ("state", s(if before { "restored" } else { "recovered" })),
        ("operation", map(&data)?["operation"].clone()),
        ("mutation_digest", map(&data)?["digest"].clone()),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    fn fixture(root: &Path) -> (PathBuf, V) {
        let source = root.join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("GROUNDING.yaml"), "known: {p.x: {v: 1}}\n").unwrap();
        let plan = crate::history_migration::Plan::prepare(
            &source.join("GROUNDING.yaml"),
            &source,
            crate::history_migration::Options {
                operation: "import-fixture".into(),
                recorded_at: "2026-09-19T12:00:00+00:00".into(),
                record_id: Some("fixture".into()),
                read_mode: crate::source_capture::ReadMode::Frozen,
                route: false,
                as_of: None,
            },
            None,
        )
        .unwrap();
        let copy = root.join("copy");
        plan.publish(&copy).unwrap();
        let entry = copy.join("GROUNDING.yaml");
        let captured = Store::new(&entry).unwrap().capture().unwrap();
        let head =
            map(&map(&map(&captured.state).unwrap()["subjects"]).unwrap()["p.x"]).unwrap()["head"]
                .clone();
        (
            entry,
            obj([
                ("kind", s("retire")),
                ("id", s("p.x")),
                ("of", head),
                ("over", V::List(vec![])),
                ("because", s("explicit test disposition")),
            ]),
        )
    }
    fn core_fixture(root: &Path, runtime: &Runtime) -> PathBuf {
        let entry = root.join("GROUNDING.yaml");
        let action = obj([
            ("kind", s("add")),
            ("id", s("p.base")),
            ("body", obj([("v", n("1"))])),
            ("as_of", s("2026-09-19")),
            ("why", V::Null),
            ("into", V::Null),
            ("hypothesis", V::Null),
            ("source", V::Null),
            ("at", V::Null),
        ]);
        let policy = crate::project_modes::Project::open(root)
            .unwrap()
            .config()
            .unwrap();
        let mutation = crate::history_bootstrap::prepare(
            &entry,
            &action,
            &policy,
            &crate::history_bootstrap::BootstrapOptions {
                operation: "first-fixture".into(),
                recorded_at: "2026-09-19T12:00:00+00:00".into(),
                recording_day: "2026-09-19".into(),
                record_id: "fixture".into(),
                by: V::Null,
            },
            Some(runtime),
        )
        .unwrap();
        crate::history_bootstrap::publish(&entry, &mutation, &policy, Some(runtime), &mut |_| {
            Ok(())
        })
        .unwrap();
        entry
    }
    #[test]
    fn interruptions_replay_exact_bytes_and_rollback_cannot_erase_a_commit() {
        for stage in ["journal", "committed"] {
            for before in [true, false] {
                let temp = tempfile::tempdir().unwrap();
                let (entry, action) = fixture(temp.path());
                let cwd = entry.parent().unwrap();
                let original = fs::read(&entry).unwrap();
                let stopped =
                    act_with_probe(std::slice::from_ref(&entry), cwd, &action, &mut |at| {
                        require(at != stage, "interrupted")
                    });
                assert_eq!(stopped.unwrap_err().0, "interrupted");
                let store = Store::new(&entry).unwrap();
                let journal = cwd.join(journal(&store));
                assert!(journal.is_file());
                assert_eq!(
                    act(std::slice::from_ref(&entry), cwd, &action)
                        .unwrap_err()
                        .0,
                    "recovery_required"
                );
                let bytes = fs::read(&journal).unwrap();
                let (mutation, _) = decode(&bytes).unwrap();
                let recovered = recover(std::slice::from_ref(&entry), cwd, before);
                if stage == "committed" && before {
                    assert_eq!(
                        recovered.unwrap_err().0,
                        "history_already_committed: committed evidence requires an explicit new act"
                    );
                    assert_eq!(fs::read(&journal).unwrap(), bytes);
                    recover(std::slice::from_ref(&entry), cwd, false).unwrap();
                } else {
                    recovered.unwrap();
                }
                assert!(!journal.exists());
                if stage == "journal" && before {
                    assert_eq!(fs::read(&entry).unwrap(), original);
                } else {
                    for file in mutation.files() {
                        assert_eq!(F::read(&cwd.join(&file.path)).unwrap(), file.after);
                    }
                }
            }
        }
    }
    #[test]
    fn altered_routing_or_journal_refuses_recovery_without_discarding_it() {
        for corrupt in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let (entry, action) = fixture(temp.path());
            let cwd = entry.parent().unwrap();
            act_with_probe(std::slice::from_ref(&entry), cwd, &action, &mut |_| {
                Err(error("interrupted"))
            })
            .unwrap_err();
            let path = cwd.join(journal(&Store::new(&entry).unwrap()));
            let (mutation, mut retained) = decode(&fs::read(&path).unwrap()).unwrap();
            map_mut(&mut retained)
                .unwrap()
                .insert("paths".into(), V::List(vec![s("/unrelated")]));
            let mut value = envelope(&mutation, retained).unwrap();
            if corrupt {
                map_mut(&mut value)
                    .unwrap()
                    .insert("digest".into(), s(&"0".repeat(64)));
            }
            fs::write(&path, history_emit::encode_document(&value).unwrap()).unwrap();
            let old = fs::read(&path).unwrap();
            let err = recover(std::slice::from_ref(&entry), cwd, false).unwrap_err();
            assert_eq!(
                err.0,
                if corrupt {
                    "invalid_history_journal"
                } else {
                    "history_routing_changed"
                }
            );
            assert_eq!(fs::read(path).unwrap(), old);
        }
    }

    #[test]
    fn named_hypothesis_journals_recover_with_their_receipt_family() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = Runtime::open(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/reasoning/native")
                .join(format!(
                    "{}.kpopper-runtime",
                    crate::reasoning_runtime::target_name().unwrap()
                )),
            cache.path(),
            crate::reasoning_runtime::OperationalBounds::default(),
        )
        .unwrap();
        for stage in ["journal", "committed"] {
            for before in [true, false] {
                let temp = tempfile::tempdir().unwrap();
                let entry = core_fixture(temp.path(), &runtime);
                let cwd = entry.parent().unwrap();
                let original = fs::read(&entry).unwrap();
                let action = obj([
                    ("kind", s("add")),
                    ("id", s("p.named")),
                    ("body", obj([("v", n("2"))])),
                    ("as_of", V::Null),
                    ("why", V::Null),
                    ("into", V::Null),
                    ("hypothesis", s("alpha")),
                    ("source", V::Null),
                    ("at", V::Null),
                ]);
                let route = WriteRoute::capture(std::slice::from_ref(&entry), cwd).unwrap();
                let store = Store::new(&entry).unwrap();
                let captured = store.capture().unwrap();
                let write_options = options("write", V::Null).unwrap();
                let mutation = HA::prepare(
                    &store,
                    &captured,
                    "alpha",
                    &action,
                    None,
                    &write_options,
                    Some(&runtime),
                )
                .unwrap();
                let stopped = publish(
                    &store,
                    &mutation,
                    &route,
                    std::slice::from_ref(&entry),
                    Some(&runtime),
                    &mut |at| require(at != stage, "interrupted"),
                );
                drop(route);
                assert_eq!(stopped.unwrap_err().0, "interrupted");
                let store = Store::new(&entry).unwrap();
                let path = cwd.join(journal(&store));
                let bytes = fs::read(&path).unwrap();
                let (mutation, _) = decode(&bytes).unwrap();
                assert!(matches!(
                    receipt_family(&mutation).unwrap(),
                    ReceiptFamily::Hypothesis
                ));
                let recovered =
                    recover_with_runtime(std::slice::from_ref(&entry), cwd, before, Some(&runtime));
                if stage == "committed" && before {
                    assert_eq!(
                        recovered.unwrap_err().0,
                        "history_already_committed: committed evidence requires an explicit new act"
                    );
                    recover_with_runtime(std::slice::from_ref(&entry), cwd, false, Some(&runtime))
                        .unwrap();
                } else {
                    recovered.unwrap();
                }
                assert!(!path.exists());
                if stage == "journal" && before {
                    assert_eq!(fs::read(&entry).unwrap(), original);
                } else {
                    for file in mutation.files() {
                        assert_eq!(F::read(&cwd.join(&file.path)).unwrap(), file.after);
                    }
                }
            }
        }
    }

    #[test]
    fn terminal_corruption_keeps_the_exact_recovery_guard() {
        for role in ["record", "history_object", "history_commit"] {
            let temp = tempfile::tempdir().unwrap();
            let (entry, action) = fixture(temp.path());
            let cwd = entry.parent().unwrap();
            let store = Store::new(&entry).unwrap();
            let path = cwd.join(journal(&store));
            let mut expected = Vec::new();
            let result = act_with_probe(std::slice::from_ref(&entry), cwd, &action, &mut |stage| {
                if stage == "committed" {
                    expected = fs::read(&path).unwrap();
                    let (mutation, _) = decode(&expected).unwrap();
                    let image = mutation.files().iter().find(|f| f.role == role).unwrap();
                    fs::write(cwd.join(&image.path), "corrupt").unwrap();
                }
                Ok(())
            });
            let error = result.unwrap_err().0;
            assert!(error.starts_with("concurrent_edit:"));
            assert!(error.contains(path.to_str().unwrap()));
            assert_eq!(fs::read(&path).unwrap(), expected);
            assert!(recover(std::slice::from_ref(&entry), cwd, false).is_err());
            assert_eq!(fs::read(&path).unwrap(), expected);
        }
    }

    #[test]
    fn rollback_preserves_guard_after_a_post_journal_record_edit() {
        let temp = tempfile::tempdir().unwrap();
        let (entry, action) = fixture(temp.path());
        let cwd = entry.parent().unwrap();
        act_with_probe(std::slice::from_ref(&entry), cwd, &action, &mut |_| {
            Err(error("interrupted"))
        })
        .unwrap_err();
        let path = cwd.join(journal(&Store::new(&entry).unwrap()));
        let retained = fs::read(&path).unwrap();
        let edited = fs::read_to_string(&entry).unwrap().replace("v: 1", "v: 7");
        fs::write(&entry, &edited).unwrap();
        assert_eq!(
            recover(std::slice::from_ref(&entry), cwd, true)
                .unwrap_err()
                .0,
            "concurrent_edit"
        );
        assert_eq!(fs::read(&path).unwrap(), retained);
        assert_eq!(fs::read_to_string(&entry).unwrap(), edited);
    }
}

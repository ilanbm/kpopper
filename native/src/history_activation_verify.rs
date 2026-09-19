//! Independent retained-byte replay and live source witnesses for authority changes.
use crate::{
    Result,
    history_activation::{self as L, after, at, base, file, layout},
    history_adapter as Adapter,
    history_authoring::{empty, obj, s, strings},
    history_authority as A,
    history_cancellation::CancellationPlan,
    history_capture as H,
    history_contract::*,
    history_migration as M, history_migration_copy as Copy, history_migration_source as MS,
    history_reduce as Reduce, history_transaction as T, history_transaction_fs as FS,
    history_view::{self as View, list, map_mut},
    history_yaml as Y,
    identity::{sha256, typed_object_identity},
    reasoning_runtime::Runtime,
    reasoning_snapshot::{self as S, Snapshot},
    require,
    source_capture::{self as Source, ReadMode},
    source_inventory::{self as SI, name},
    value::TypedValue as V,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

pub(crate) fn pending(ledger: &V) -> Result<()> {
    let bundles = map(at(ledger, "bundles")?)?;
    let mut ids = BTreeSet::new();
    let mut revisions = BTreeSet::new();
    for event in list(at(ledger, "events")?)? {
        let id = text(at(event, "event_id")?)?;
        let revision = text(at(event, "revision")?)?;
        require(
            !id.is_empty() && ids.insert(id) && bundles.contains_key(revision),
            "invalid_transition_pending",
        )?;
        revisions.insert(revision);
    }
    require(
        revisions
            .iter()
            .copied()
            .eq(bundles.keys().map(String::as_str)),
        "invalid_transition_pending",
    )?;
    for (revision, bundle) in bundles {
        require(
            at(bundle, "revision")? == &s(revision),
            "invalid_transition_pending",
        )?;
        crate::pending_bundle::validate(
            &obj([
                ("revision", s(revision)),
                ("manifest", at(bundle, "manifest")?.clone()),
            ]),
            &S::history_bytes(at(bundle, "files")?)?,
        )?;
    }
    Ok(())
}
pub(crate) fn tree(root: &Path, relative: &str) -> Result<V> {
    let dir = FS::target(root, relative)?;
    if !dir.exists() {
        return Ok(empty());
    }
    require(dir.is_dir(), "invalid_transition_tree")?;
    let mut pending = vec![dir];
    let mut files = Map::new();
    let mut visits = 0;
    while let Some(dir) = pending.pop() {
        for child in fs::read_dir(dir)? {
            let child = child?;
            visits += 1;
            require(visits <= MAX_OBJECTS, "history_limit")?;
            let path = child.path();
            let meta = child.file_type()?;
            require(!meta.is_symlink(), "invalid_transition_tree")?;
            if meta.is_dir() {
                pending.push(path);
            } else {
                require(meta.is_file(), "invalid_transition_tree")?;
                let relative =
                    MS::posix(path.strip_prefix(root).map_err(|_| error("invalid_path"))?)?;
                let raw = FS::read(&path)?.ok_or_else(|| error("transition_history_changed"))?;
                files.insert(relative, s(&sha256(&raw)));
            }
        }
    }
    Ok(V::Map(files))
}
pub(crate) fn trees(entry: &Path) -> Result<V> {
    let l = layout(entry)?;
    let root = entry.parent().unwrap();
    Ok(V::Map(
        [l.objects, l.commits, l.cancellations, M::ARTIFACTS.into()]
            .iter()
            .map(|p| Ok((p.clone(), tree(root, p)?)))
            .collect::<Result<Map>>()?,
    ))
}
pub(crate) fn read_witnesses(
    entry: &Path,
    m: &T::PreparedMutation,
    cancellation: Option<&CancellationPlan>,
) -> Result<()> {
    let root = entry.parent().unwrap();
    let b = base(m)?;
    let touched = m
        .files()
        .iter()
        .map(|i| (root.join(&i.path), i))
        .collect::<BTreeMap<_, _>>();
    let trees = map(at(&b, "managed_trees")?)?;
    for (relative, before) in trees {
        let before = map(before)?;
        let prefix = format!("{relative}/");
        let mut optional = Map::new();
        for item in m.files() {
            if item.path.starts_with(&prefix)
                && ["history_object", "history_commit", "history_retained"]
                    .contains(&item.role.as_str())
            {
                optional.insert(item.path.clone(), s(&sha256(after(item)?)));
            }
        }
        if let Some(plan) = cancellation
            && plan.receipt_path.starts_with(&prefix)
        {
            optional.insert(plan.receipt_path.clone(), s(&sha256(&plan.receipt_bytes)));
        }
        let current = tree(root, relative)?;
        let current = map(&current)?;
        require(
            before.iter().all(|(p, d)| current.get(p) == Some(d))
                && current
                    .iter()
                    .all(|(p, d)| before.contains_key(p) || optional.get(p) == Some(d)),
            "transition_history_changed",
        )?;
    }
    for (relative, expected) in map(at(&b, "source_files")?)? {
        let path = FS::target(root, relative)?;
        let raw = FS::read(&path)?;
        if let Some(item) = touched.get(&path) {
            require(
                raw == item.before
                    || raw == item.after
                    || cancellation.is_some_and(|c| {
                        item.role == "history_authority" && raw.as_ref() == Some(&c.marker_bytes)
                    }),
                "transition_source_changed",
            )?;
        } else {
            require(
                raw.is_some_and(|b| &s(&sha256(&b)) == expected),
                "transition_source_changed",
            )?;
        }
    }
    let tree_roots = trees.keys().map(|p| root.join(p)).collect::<Vec<_>>();
    for event in list(at(&b, "reads")?)? {
        let kind = text(at(event, "kind")?)?;
        let path = Path::new(text(at(event, "path")?)?);
        require(path.is_absolute(), "invalid_transition_observation")?;
        if touched.contains_key(path)
            || tree_roots.iter().any(|root| {
                path.starts_with(root) || SI::escaped(root).is_ok_and(|p| path.starts_with(p))
            })
        {
            continue;
        }
        let actual = match kind {
            "bytes" => FS::read(path)?.map(|b| s(&sha256(&b))).unwrap_or(V::Null),
            "glob" => strings(
                SI::glob(path)?
                    .iter()
                    .map(|p| name(p).map(str::to_owned))
                    .collect::<Result<Vec<_>>>()?,
            ),
            "directory" => {
                MS::no_link(path)?;
                V::Bool(path.is_dir())
            }
            "exists" => V::Bool(path.exists()),
            _ => return Err(error("unsupported_transition_observation")),
        };
        require(&actual == at(event, "value")?, "transition_source_changed")?;
    }
    Ok(())
}
fn retained(entry: &Path, m: &T::PreparedMutation, path: &str) -> Result<Vec<u8>> {
    if let Some(item) = m.files().iter().find(|i| i.path == path) {
        return Ok(after(item)?.to_vec());
    }
    FS::read(&FS::target(entry.parent().unwrap(), path)?)?
        .ok_or_else(|| error("transition_original_mismatch"))
}
pub(crate) fn candidate(
    entry: &Path,
    m: &T::PreparedMutation,
    runtime: Option<&Runtime>,
) -> Result<()> {
    L::isolated(|| candidate_isolated(entry, m, runtime))
}
fn candidate_isolated(
    entry: &Path,
    m: &T::PreparedMutation,
    runtime: Option<&Runtime>,
) -> Result<()> {
    let data = m.to_data();
    let b = base(m)?;
    let activation = at(&b, "activation")?;
    let artifacts = map(activation)?
        .get("artifact_root")
        .map(text)
        .transpose()?
        .unwrap_or(M::ARTIFACTS);
    let record = file(m, "record")?;
    if !string_is(at(&b, "direction")?, "activate") {
        require(
            string_is(at(&b, "direction")?, "deactivate"),
            "invalid_authority_transition",
        )?;
        let original = Snapshot::from_json(
            &T::unblob(at(&b, "original_snapshot")?)?
                .ok_or_else(|| error("transition_original_mismatch"))?,
        )?;
        require(
            record.after == T::unblob(at(&b, "original_entry")?)?,
            "transition_original_mismatch",
        )?;
        let path = format!(
            "{}/{}.yaml",
            layout(entry)?.commits,
            text(at(activation, "operation")?)?
        );
        let raw = FS::read(&FS::target(entry.parent().unwrap(), &path)?)?
            .ok_or_else(|| error("transition_commit_mismatch"))?;
        require(
            at(activation, "first_commit_sha256")? == &s(&sha256(&raw)),
            "transition_commit_mismatch",
        )?;
        let first = Y::decode_document(&raw)?;
        A::validate_commit(&first)?;
        let before = at(at(&first, "receipt")?, "before")?;
        require(
            at(before, "snapshot_id")? == &s(original.snapshot_id())
                && at(at(before, "source")?, &layout(entry)?.entry)? == &s(&sha256(after(record)?)),
            "transition_original_mismatch",
        )?;
        return Ok(());
    }
    let marker = at(at(&data, "transition")?, "after")?;
    let commit = file(m, "history_commit")?;
    let commits = BTreeMap::from([(
        text(at(&data, "operation")?)?.to_owned(),
        after(commit)?.to_vec(),
    )]);
    let mut objects = A::ObjectBytes::new();
    for item in m.files().iter().filter(|i| i.role == "history_object") {
        let o = Y::decode_document(after(item)?)?;
        validate_object(&o)?;
        let key = (
            text(at(&o, "subject")?)?.to_owned(),
            text(at(&o, "id")?)?.to_owned(),
        );
        require(
            objects.insert(key, after(item)?.to_vec()).is_none(),
            "transition_import_mismatch",
        )?;
    }
    let selected = A::committed_objects(marker, &commits, &objects)?;
    let mut capture = M::empty_capture(entry.parent().unwrap(), &layout(entry)?.entry, marker)?;
    capture.entry_bytes = after(record)?.to_vec();
    capture.document = Y::decode_document(after(record)?)?;
    capture.state = Reduce::reduce(&selected, None, None)?;
    capture.baseline = H::baseline(marker, &commits, &capture.state)?;
    capture.commits = commits;
    capture.object_bytes = objects;
    capture.objects = selected;
    require(
        View::render(
            &capture,
            &capture.objects,
            &capture.object_bytes,
            &capture.commits,
        )? == after(record)?,
        "transition_view_mismatch",
    )?;
    let adapted = Adapter::from_store_capture(&capture)?;
    let original_bytes = retained(entry, m, &format!("{artifacts}/original.json"))?;
    let original = Snapshot::from_json(&original_bytes)?;
    require(
        M::entry_identity(adapted.document())?
            == M::entry_identity(at(&original.to_data(), "document")?)?,
        "transition_meaning_mismatch",
    )?;
    require(
        at(activation, "first_commit_sha256")? == &s(&sha256(after(commit)?)),
        "transition_commit_mismatch",
    )?;
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let sources = map(at(&b, "source_files")?)?;
    let storage = map(at(&b, "source_storage")?)?;
    require(
        sources.keys().eq(storage.keys()),
        "transition_original_mismatch",
    )?;
    for (relative, expected) in sources {
        let item = m.files().iter().find(|i| &i.path == relative);
        let raw = if let Some(item) = item {
            item.before.clone()
        } else {
            FS::read(&FS::target(entry.parent().unwrap(), relative)?)?
        }
        .ok_or_else(|| error("transition_source_changed"))?;
        require(&s(&sha256(&raw)) == expected, "transition_source_changed")?;
        let stored = &storage[relative];
        require(
            at(stored, "sha256")? == expected,
            "transition_original_mismatch",
        )?;
        require(
            retained(entry, m, text(at(stored, "path")?)?)? == raw,
            "transition_original_mismatch",
        )?;
        let path = FS::target(root, relative)?;
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, raw)?;
    }
    let replay = M::Plan::prepare(
        &root.join(&layout(entry)?.entry),
        root,
        M::Options {
            operation: text(at(activation, "operation")?)?.into(),
            recorded_at: text(at(activation, "recorded_at")?)?.into(),
            record_id: Some(text(at(activation, "record_id")?)?.into()),
            read_mode: ReadMode::Frozen,
            route: false,
            as_of: None,
        },
        runtime,
    )?;
    require(
        at(replay.manifest(), "complete")? == &V::Bool(true),
        "transition_import_mismatch",
    )?;
    let replay_layout = layout(entry)?;
    let replay_commit = Y::decode_document(
        replay
            .files()
            .get(&format!(
                "{}/{}.yaml",
                replay_layout.commits,
                text(at(activation, "operation")?)?
            ))
            .ok_or_else(|| error("transition_import_mismatch"))?,
    )?;
    let replay_objects = replay
        .files()
        .iter()
        .filter(|(p, _)| p.starts_with(&format!("{}/", replay_layout.objects)))
        .map(|(_, raw)| {
            let o = Y::decode_document(raw)?;
            Ok((text(at(&o, "id")?)?.to_owned(), o))
        })
        .collect::<Result<Map>>()?;
    // The replay may retain inactive generations. Only its new import's selected
    // object set participates in equivalence, as in the original publication.
    let selected_ids = list(at(&replay_commit, "objects")?)?
        .iter()
        .map(|o| text(at(o, "id")?).map(str::to_owned))
        .collect::<Result<BTreeSet<_>>>()?;
    let replay_objects = replay_objects
        .into_iter()
        .filter(|(id, _)| selected_ids.contains(id))
        .collect::<Map>();
    require(
        typed_object_identity(&V::Map(replay_objects))?
            == typed_object_identity(&V::Map(capture.objects.clone()))?,
        "transition_import_mismatch",
    )?;
    let mut template = at(&replay_commit, "view_template")?.clone();
    let meta = map_mut(&mut template)?
        .get_mut("meta")
        .ok_or_else(|| error("transition_template_mismatch"))?;
    let import = map_mut(meta)?
        .get_mut("history_import")
        .ok_or_else(|| error("transition_template_mismatch"))?;
    let members = map_mut(import)?
        .get_mut("members")
        .ok_or_else(|| error("transition_template_mismatch"))?;
    let V::List(members) = members else {
        return Err(error("transition_template_mismatch"));
    };
    for member in members.iter_mut() {
        if at(member, "path")? == &s(&format!("{artifacts}/original.json")) {
            map_mut(member)?.insert("sha256".into(), s(&sha256(&original_bytes)));
        }
    }
    let first = Y::decode_document(after(commit)?)?;
    let before = at(at(&first, "receipt")?, "before")?;
    if let Some(expected) = map(before)?.get("originals_storage") {
        require(
            expected == at(&b, "source_storage")?
                && expected == at(replay.manifest(), "originals")?,
            "transition_original_mismatch",
        )?;
    }
    if let Some(observation) = map(&b)?.get("live_observation") {
        let live_bytes = retained(entry, m, &format!("{artifacts}/live.json"))?;
        require(
            at(observation, "sha256")? == &s(&sha256(&live_bytes))
                && map(before)?.get("observation") == Some(observation),
            "transition_observation_mismatch",
        )?;
        let live = Snapshot::from_json(&live_bytes)?;
        Copy::validate_pending(&live)?;
        require(
            at(observation, "snapshot_id")? == &s(live.snapshot_id())
                && M::entry_identity(at(&live.to_data(), "document")?)?
                    == M::entry_identity(at(&original.to_data(), "document")?)?,
            "transition_observation_mismatch",
        )?;
        let live_data = live.to_data();
        let pending = at(at(&live_data, "context")?, "pending")?;
        let ledger = at(at(at(&b, "project")?, "pending")?, "ledger")?;
        for key in ["ref", "events", "bundles"] {
            require(
                at(pending, key)? == at(ledger, key)?,
                "transition_observation_mismatch",
            )?;
        }
        let candidate =
            Snapshot::from_json(&retained(entry, m, &format!("{artifacts}/candidate.json"))?)?;
        Copy::validate_pending(&candidate)?;
        require(
            typed_object_identity(at(&candidate.to_data(), "document")?)?
                == typed_object_identity(adapted.document())?,
            "transition_observation_mismatch",
        )?;
        same_live_context(&candidate, &live)?;
        members.push(obj([
            ("path", s(&format!("{artifacts}/live.json"))),
            ("sha256", at(observation, "sha256")?.clone()),
            ("role", s("retained_original")),
        ]));
        members.sort_by_key(|m| text(at(m, "path").unwrap()).unwrap().to_owned());
    } else {
        require(
            !map(before)?.contains_key("observation"),
            "transition_observation_mismatch",
        )?;
    }
    require(
        typed_object_identity(&template)? == typed_object_identity(at(&first, "view_template")?)?,
        "transition_template_mismatch",
    )?;
    require(
        M::entry_identity(at(&replay.original().to_data(), "document")?)?
            == M::entry_identity(at(&original.to_data(), "document")?)?,
        "transition_meaning_mismatch",
    )?;
    require(
        typed_object_identity(at(&replay.original().to_data(), "hypotheses")?)?
            == typed_object_identity(at(&original.to_data(), "hypotheses")?)?,
        "transition_hypothesis_mismatch",
    )
}
pub(crate) fn same_live_context(actual: &Snapshot, expected: &Snapshot) -> Result<()> {
    let left = actual.to_data();
    let right = expected.to_data();
    for key in ["pending", "target", "project", "history_contributions"] {
        require(
            map(at(&left, "context")?)?.get(key) == map(at(&right, "context")?)?.get(key),
            "transition_live_context_mismatch",
        )?;
    }
    require(
        M::hypothesis_identity(at(&left, "hypotheses")?)?
            == M::hypothesis_identity(at(&right, "hypotheses")?)?,
        "transition_live_context_mismatch",
    )
}
pub(crate) fn live_result(
    entry: &Path,
    m: &T::PreparedMutation,
    runtime: Option<&Runtime>,
) -> Result<()> {
    let b = base(m)?;
    let observation = map(&b)?.get("live_observation");
    let actual = Source::capture_source_with_runtime(
        &[entry.into()],
        entry.parent().unwrap(),
        if observation.is_some() {
            ReadMode::Live
        } else {
            ReadMode::Frozen
        },
        None,
        runtime,
    )?;
    if let Some(observation) = observation {
        let raw = FS::read(&FS::target(
            entry.parent().unwrap(),
            text(at(observation, "path")?)?,
        )?)?
        .ok_or_else(|| error("transition_observation_mismatch"))?;
        require(
            at(observation, "sha256")? == &s(&sha256(&raw)),
            "transition_observation_mismatch",
        )?;
        let expected = Snapshot::from_json(&raw)?;
        Copy::validate_pending(&expected)?;
        require(
            at(observation, "snapshot_id")? == &s(expected.snapshot_id()),
            "transition_observation_mismatch",
        )?;
        same_live_context(actual.snapshot(), &expected)?;
        require(
            M::entry_identity(&actual.document())?
                == M::entry_identity(at(&expected.to_data(), "document")?)?,
            "transition_meaning_mismatch",
        )?;
    }
    Ok(())
}

/// The general source witnesses accept either image during interrupted replay.
/// Finalization additionally requires the intended terminal bytes after the last
/// external probe, while the recovery journal still excludes readers.
pub(crate) fn terminal_images(
    entry: &Path,
    mutation: &T::PreparedMutation,
    direction: FS::Direction,
) -> Result<()> {
    for item in mutation.files() {
        let immutable =
            ["history_object", "history_commit", "history_retained"].contains(&item.role.as_str());
        if direction == FS::Direction::Before && immutable {
            continue;
        }
        let intended = if direction == FS::Direction::After {
            &item.after
        } else {
            &item.before
        };
        require(
            &FS::read(&FS::target(entry.parent().unwrap(), &item.path)?)? == intended,
            "transition_terminal_image_mismatch",
        )?;
    }
    Ok(())
}

//! Named proposal authoring keeps physical sources, group authority and acceptance separate.
use crate::{
    Result, history_adapter as D,
    history_authoring::{self as A, Options},
    history_authoring_audit::ReplayAudit,
    history_authority as Authority,
    history_capture::{self as H, Capture},
    history_contract::*,
    history_hypotheses as HH, history_paths as HP, history_preparation as Preparation,
    history_reduce,
    history_store::Store,
    history_transaction::{self as T, PreparedMutation},
    history_transaction_fs as FS,
    history_view::{self as View, list, map_mut, truth},
    history_yaml as Y,
    identity::sha256,
    reasoning_authoring::{self as W, World},
    reasoning_fields as F,
    reasoning_runtime::{OperationalBounds, Runtime},
    reasoning_snapshot::{CaptureOptions, Snapshot, entries},
    require,
    value::TypedValue as V,
};
use A::{empty, n, obj, s, strings};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(crate) struct Context {
    pub base: V,
    pub groups: V,
    pub index: V,
    pub physical: BTreeMap<String, String>,
}

/// Every physical filename remains authority evidence, including unreadable YAML.
pub(crate) fn physical_files(store: &Store) -> Result<BTreeMap<String, Vec<u8>>> {
    let directory = FS::target(&store.root, &store.layout.hypotheses)?;
    if !directory.is_dir() {
        return Ok(BTreeMap::new());
    }
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| error("invalid_path"))?;
        if name.starts_with('.') || !(name.ends_with(".yaml") || name.ends_with(".yml")) {
            continue;
        }
        let relative = format!("{}/{}", store.layout.hypotheses, name);
        let path = FS::target(&store.root, &relative)?;
        require(
            path.is_file() && path.metadata()?.len() <= 16 * 1024 * 1024,
            "history_limit",
        )?;
        let raw = FS::read(&path)?.ok_or_else(|| error("concurrent_hypothesis_edit"))?;
        total = total.saturating_add(raw.len());
        require(total <= 16 * 1024 * 1024, "history_limit")?;
        files.insert(relative, raw);
    }
    Ok(files)
}
pub(crate) fn physical_evidence(store: &Store) -> Result<V> {
    Ok(V::Map(
        physical_files(store)?
            .into_iter()
            .map(|(p, raw)| (p, s(&sha256(&raw))))
            .collect(),
    ))
}
pub(crate) fn active_physical(store: &Store, document: &V) -> Result<BTreeMap<String, String>> {
    let files = physical_files(store)?;
    let mut active = files
        .keys()
        .map(|path| {
            (
                Path::new(path)
                    .file_stem()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned(),
                path.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mapping = map(document)?
        .get("meta")
        .map(map)
        .transpose()?
        .and_then(|m| m.get("history_hypothesis_import"))
        .filter(|v| **v != V::Null);
    if let Some(mapping) = mapping {
        let mapping = schema(mapping, &["version", "physical"], &[])?;
        require(
            is_int(&mapping["version"], "1"),
            "invalid_hypothesis_import_mapping",
        )?;
        let mut names = vec![];
        let mut paths = BTreeSet::new();
        for item in list(&mapping["physical"])? {
            let item = schema(item, &["name", "path", "sha256"], &[])?;
            let name = text(&item["name"])?;
            hypothesis_name(&s(name))?;
            let path = text(&item["path"])?;
            let target = Path::new(path);
            require(
                target.parent() == Some(Path::new(&store.layout.hypotheses))
                    && target.file_stem().and_then(|v| v.to_str()) == Some(name)
                    && target
                        .extension()
                        .and_then(|v| v.to_str())
                        .is_some_and(|v| ["yaml", "yml"].contains(&v)),
                "invalid_hypothesis_import_path",
            )?;
            FS::target(&store.root, path)?;
            let hash = text(&item["sha256"])?;
            require(
                hash.len() == 64 && HP::object_id(hash),
                "invalid_identifier",
            )?;
            names.push(name);
            require(paths.insert(path), "duplicate_hypothesis_import")?;
            require(
                active.get(name).is_some_and(|v| v == path),
                "missing_imported_hypothesis",
            )?;
            require(sha256(&files[path]) == hash, "imported_hypothesis_changed")?;
            active.remove(name);
        }
        require(
            names.windows(2).all(|p| p[0] < p[1]),
            "duplicate_hypothesis_import",
        )?;
    }
    Ok(active)
}
pub(crate) fn capture(store: &Store, captured: &Capture, options: &Options) -> Result<Context> {
    A::guards(store, captured, options)?;
    let adapted = D::from_store_capture(captured)?;
    let (groups, index) = HH::layers(adapted.projection(), adapted.document())?;
    let physical = active_physical(store, adapted.document())?;
    require(
        !map(&groups)?.keys().any(|name| physical.contains_key(name)),
        "hypothesis_authority_collision",
    )?;
    Ok(Context {
        base: A::document(captured)?,
        groups,
        index,
        physical,
    })
}
pub(crate) fn guard_names(
    names: &[String],
    groups: &V,
    physical: &BTreeMap<String, String>,
    required: bool,
) -> Result<()> {
    require(
        !names.is_empty() && names.iter().collect::<BTreeSet<_>>().len() == names.len(),
        "invalid_hypothesis_names",
    )?;
    for name in names {
        hypothesis_name(&s(name))?;
        require(
            !physical.contains_key(name),
            "hypothesis_authority_collision",
        )?;
        require(
            !required || map(groups)?.contains_key(name),
            "missing_history_hypothesis",
        )?;
    }
    Ok(())
}
pub(crate) fn layer(base: &V, groups: &V, names: &[String]) -> Result<V> {
    let mut doc = base.clone();
    for name in names {
        let group = map(field(map(groups)?, name)?)?;
        require(
            !group.get("error").is_some_and(truth),
            "unresolved_history_hypothesis",
        )?;
        for (collection, members) in F::collections(field(group, "doc")?)? {
            for id in members.keys() {
                for (other, values) in map_mut(&mut doc)? {
                    if other != &collection
                        && let V::Map(values) = values
                    {
                        values.remove(id);
                    }
                }
            }
            let target = map_mut(&mut doc)?.entry(collection).or_insert_with(empty);
            if !matches!(target, V::Map(_)) {
                *target = empty();
            }
            map_mut(target)?.extend(members);
        }
    }
    Ok(doc)
}
pub(crate) fn world<'a>(
    doc: &V,
    groups: Option<&V>,
    runtime: Option<&'a Runtime>,
) -> Result<World<'a>> {
    require(
        string_is(&map(&F::capabilities(doc, None)?)?["profile"], "core/v1"),
        "ordinary_history_authoring_unsupported",
    )?;
    let snapshot = Snapshot::from_data(
        doc,
        CaptureOptions {
            hypotheses: groups.cloned(),
            ..Default::default()
        },
    )?;
    World::new(doc, Some(&snapshot), runtime, OperationalBounds::default())
}
pub(crate) fn pins(
    capture: &Capture,
    index: &V,
    name: &str,
    deps: &[V],
    blocked: bool,
) -> Result<(V, V)> {
    let mut pins = Map::new();
    let mut gaps = Map::new();
    let groups = map(&map(index)?["groups"])?;
    let group = groups.get(name).map(map).transpose()?;
    for dependency in deps {
        let dep = text(dependency)?;
        if let Some(versions) = group.and_then(|g| g.get(dep)).filter(|v| truth(v)) {
            let versions = list(versions)?;
            let meanings = versions
                .iter()
                .map(|v| {
                    history_reduce::claim_meaning(map(capture
                        .objects
                        .get(text(v)?)
                        .ok_or_else(|| error("incomplete_closure"))?)?)
                })
                .collect::<Result<BTreeSet<_>>>()?;
            require(meanings.len() == 1, "unresolved_history_hypothesis")?;
            pins.insert(
                dep.into(),
                versions
                    .iter()
                    .min_by_key(|v| text(v).unwrap())
                    .unwrap()
                    .clone(),
            );
        } else if map(&map(&capture.state)?["subjects"])?.contains_key(dep) {
            pins.insert(dep.into(), map(A::head(capture, dep)?)?["id"].clone());
        } else {
            require(blocked, "unresolved_history_subject")?;
            gaps.insert(dep.into(), s("unavailable"));
        }
    }
    Ok((V::Map(pins), V::Map(gaps)))
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn act(
    capture: &Capture,
    target: &V,
    kind: &str,
    because: &str,
    over: V,
    extra: &[String],
    read: Option<V>,
    options: &Options,
) -> Result<V> {
    let target = map(target)?;
    let subject = text(&target["subject"])?;
    let mut saw = A::saw(capture, subject)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    saw.extend(extra.iter().cloned());
    let mut body = obj([
        ("act", s(kind)),
        ("of", target["id"].clone()),
        ("over", over),
        ("because", s(because)),
    ]);
    if let Some(read) = read {
        map_mut(&mut body)?.insert("read".into(), read);
    }
    A::make_object(
        subject,
        "act",
        body,
        strings(saw),
        None,
        empty(),
        empty(),
        options,
    )
}
#[allow(clippy::too_many_arguments)]
fn mutation(
    store: &Store,
    captured: &Capture,
    objects: &[V],
    mut intent: V,
    before: &V,
    after: &V,
    groups_before: Option<&V>,
    groups_after: Option<&V>,
    options: &Options,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
) -> Result<PreparedMutation> {
    let cap = F::capabilities(before, None)?;
    let archive = A::archive(store)?;
    let physical = physical_evidence(store)?;
    // Imported originals also keep their names: a new edit cannot appropriate them.
    let all_physical = physical_files(store)?
        .keys()
        .map(|p| {
            (
                Path::new(p).file_stem().unwrap().to_str().unwrap().into(),
                p.clone(),
            )
        })
        .collect();
    let im = map(&intent)?;
    let names = if string_is(&im["kind"], "edit") {
        vec![text(&im["name"])?.into()]
    } else {
        list(&im["names"])?
            .iter()
            .map(|v| text(v).map(str::to_owned))
            .collect::<Result<_>>()?
    };
    guard_names(&names, &empty(), &all_physical, false)?;
    map_mut(&mut intent)?.extend([
        ("archive".into(), archive.clone()),
        ("physical".into(), physical.clone()),
        ("baseline".into(), captured.baseline.clone()),
    ]);
    let mut before = A::evidence(before, &mut world(before, groups_before, runtime)?, audit)?;
    map_mut(&mut before)?.insert("hypothesis_authoring".into(), intent);
    let mut after = A::evidence(after, &mut world(after, groups_after, runtime)?, audit)?;
    let ids = objects
        .iter()
        .map(|o| text(&map(o)?["id"]).map(str::to_owned))
        .collect::<Result<BTreeSet<_>>>()?;
    map_mut(&mut after)?.insert(
        "hypothesis_authoring".into(),
        obj([("objects", strings(ids))]),
    );
    let receipt = T::semantic_receipt(text(&map(&cap)?["profile"])?, &cap, &before, &after)?;
    let mut template = View::template(&captured.commits)?;
    for object in objects {
        let object = map(object)?;
        if !string_is(&object["kind"], "act") {
            map_mut(&mut template)?
                .entry(text(&map(&object["authored"])?["collection"])?.into())
                .or_insert_with(empty);
        }
    }
    let prepared = Preparation::prepare_commit(
        captured,
        &options.operation,
        objects,
        &template,
        &receipt,
        options.requires().as_ref(),
    )?;
    require(A::archive(store)? == archive, "concurrent_archive_edit")?;
    require(
        physical_evidence(store)? == physical,
        "concurrent_hypothesis_edit",
    )?;
    Ok(prepared)
}

pub fn prepare(
    store: &Store,
    captured: &Capture,
    name: &str,
    action: &V,
    head: Option<&V>,
    options: &Options,
    runtime: Option<&Runtime>,
) -> Result<PreparedMutation> {
    prepare_inner(store, captured, name, action, head, options, runtime, None)
}
#[allow(clippy::too_many_arguments)]
fn prepare_inner(
    store: &Store,
    captured: &Capture,
    name: &str,
    action: &V,
    head: Option<&V>,
    options: &Options,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
) -> Result<PreparedMutation> {
    let Context {
        base,
        mut groups,
        index,
        physical,
    } = capture(store, captured, options)?;
    require(options.strict, "explicit_root_disposition_required")?;
    guard_names(&[name.into()], &groups, &physical, false)?;
    let mut action = action.clone();
    let a = map_mut(&mut action)?;
    let kind = text(field(a, "kind")?)?.to_owned();
    require(
        ["add", "set", "review"].contains(&kind.as_str()) && !a.get("section").is_some_and(truth),
        "invalid_hypothesis_action",
    )?;
    require(
        a.get("hypothesis")
            .is_none_or(|v| *v == V::Null || string_is(v, name)),
        "hypothesis_name_mismatch",
    )?;
    a.insert("hypothesis".into(), s(name));
    if !a.get("as_of").is_some_and(truth) {
        require(!options.recording_day.is_empty(), "missing_recording_time")?;
        a.insert("as_of".into(), s(&options.recording_day));
    }
    let head = if let Some(group) = map(&groups)?.get(name) {
        let group = map(group)?;
        require(
            !group.get("error").is_some_and(truth),
            "unresolved_history_hypothesis",
        )?;
        require(
            head.filter(|v| **v != V::Null)
                .is_none_or(|v| v.digest().ok() == group["head"].digest().ok()),
            "hypothesis_head_change_requires_group_edit",
        )?;
        group["head"].clone()
    } else {
        let head = head
            .filter(|v| **v != V::Null)
            .cloned()
            .unwrap_or_else(|| obj([("born", a["as_of"].clone())]));
        map(&head).map_err(|_| error("invalid_history_hypothesis"))?;
        map_mut(&mut groups)?.insert(
            name.into(),
            obj([
                ("name", s(name)),
                ("kind", s(HH::KIND)),
                ("doc", empty()),
                ("head", head.clone()),
                ("ids", V::List(vec![])),
                ("raw", empty()),
                ("error", V::Null),
            ]),
        );
        head
    };
    let before = layer(&base, &groups, &[name.into()])?;
    let profile = map(&F::capabilities(&before, None)?)?["profile"].clone();
    require(
        map(&map(&groups)?[name])?
            .get("profile")
            .is_none_or(|v| *v == V::Null || *v == profile),
        "history_hypothesis_profile_migration_required",
    )?;
    require(
        map(&action)?
            .get("profile")
            .is_none_or(|v| *v == V::Null || *v == profile),
        "history_profile_migration_required",
    )?;
    let mut prior_world = world(&before, Some(&groups), runtime)?;
    let (normalized, _) = prior_world.normalize(&action, None)?;
    let normalized_map = map(&normalized)?;
    let subject = text(field(normalized_map, "id")?)?;
    let mut validation_groups = groups.clone();
    if kind == "add" {
        // Python's validation-only ids set removes this member, while the actual
        // document and before/after evidence retain the complete named world.
        let group = map_mut(map_mut(&mut validation_groups)?.get_mut(name).unwrap())?;
        if list(&group["ids"])?.contains(&s(subject)) {
            for values in map_mut(group.get_mut("doc").unwrap())?.values_mut() {
                if let V::Map(values) = values {
                    values.remove(subject);
                }
            }
        }
    }
    let refusals = world(&before, Some(&validation_groups), runtime)?.validate(&normalized)?;
    require(
        refusals.is_empty(),
        &format!("refused - {}", refusals.join("; ")),
    )?;
    let version = options.receipt_version.unwrap_or(2);
    require([1, 2].contains(&version), "invalid_hypothesis_receipt")?;
    let previous = map(&map(&index)?["groups"])?
        .get(name)
        .map(map)
        .transpose()?
        .and_then(|g| g.get(subject))
        .map(list)
        .transpose()?
        .unwrap_or(&[]);
    let fields = prior_world.fields().clone();
    let deps_field = text(&fields["deps"])?;
    let mut after = before.clone();
    let mut objects = vec![];
    let why = normalized_map
        .get("why")
        .filter(|v| truth(v))
        .map(text)
        .transpose()?;
    if kind == "review" {
        require(!previous.is_empty(), "review_requires_group_proposal")?;
        let target = map(&captured.objects[text(&previous[0])?])?;
        require(string_is(&target["kind"], "judgment"), "invalid_review")?;
        require(
            normalized_map.get("_record_scope").is_none_or(|v| {
                v.digest().ok()
                    == map(&target["body"])
                        .ok()
                        .and_then(|b| b.get("scope"))
                        .unwrap_or(&V::Null)
                        .digest()
                        .ok()
            }),
            "history_review_scope_change_requires_claim",
        )?;
        let deps = list(field(map(&entries(&before)?[subject].1)?, deps_field)?)?.to_vec();
        let (pins, gaps) = pins(
            captured,
            &index,
            name,
            &deps,
            !W::blocked_text(&target["body"]).is_empty(),
        )?;
        require(!truth(&gaps), "unavailable_review_pin")?;
        for version in previous {
            objects.push(act(
                captured,
                &captured.objects[text(version)?],
                "review",
                why.unwrap_or("explicit hypothesis review"),
                V::List(vec![]),
                &[],
                Some(pins.clone()),
                options,
            )?);
        }
    } else {
        let candidate = prior_world.candidate(&normalized)?;
        let known = entries(candidate.document())?;
        let (collection, mut body) = known
            .get(subject)
            .ok_or_else(|| error("unresolved_history_subject"))?
            .clone();
        let mut authored = if kind == "set" {
            let target = if previous.is_empty() {
                A::head(captured, subject)?
            } else {
                &captured.objects[text(&previous[0])?]
            };
            map(target)?["authored"].clone()
        } else {
            let mut authored = obj([
                ("collection", s(&collection)),
                ("profile", profile),
                ("fields", V::Map(fields.clone())),
            ]);
            let group_versions = map(&map(&index)?["groups"])?
                .get(name)
                .map(map)
                .transpose()?;
            let has_headers = group_versions
                .into_iter()
                .flat_map(|g| g.values())
                .map(list)
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .any(|v| {
                    text(v)
                        .ok()
                        .and_then(|id| captured.objects.get(id))
                        .and_then(|o| map(o).ok())
                        .and_then(|o| o.get("authored"))
                        .and_then(|a| map(a).ok())
                        .and_then(|a| a.get("locator"))
                        .and_then(|l| map(l).ok())
                        .is_some_and(|l| l.contains_key("document_headers"))
                });
            if has_headers {
                let headers = map(&map(&map(&groups)?[name])?["doc"])?
                    .iter()
                    .filter(|(k, _)| ["meta", "schema", "record", "also"].contains(&k.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                map_mut(&mut authored)?.insert(
                    "locator".into(),
                    obj([("document_headers", V::Map(headers))]),
                );
            }
            authored
        };
        let judgment = map(&body).is_ok_and(|b| b.contains_key(deps_field));
        let deps = map(&body)
            .ok()
            .and_then(|b| b.get(deps_field))
            .map(list)
            .transpose()?
            .unwrap_or(&[])
            .to_vec();
        if version == 2 && judgment {
            let mut seen = Map::new();
            for dep in &deps {
                let dep = text(dep)?;
                if prior_world.raw().contains_key(dep) {
                    seen.insert(dep.into(), prior_world.history(dep)?);
                }
            }
            map_mut(&mut body)?.insert(text(&fields["snapshot"])?.into(), V::Map(seen));
        }
        map_mut(&mut authored)?.insert(
            "hypothesis".into(),
            obj([
                ("version", n("1")),
                ("name", s(name)),
                ("head", head.clone()),
            ]),
        );
        let (pins, gaps) = pins(
            captured,
            &index,
            name,
            &deps,
            !W::blocked_text(&body).is_empty(),
        )?;
        let claim = A::make_object(
            subject,
            if judgment { "judgment" } else { "reading" },
            body.clone(),
            strings(A::saw(captured, subject)?),
            Some(authored),
            pins,
            gaps,
            options,
        )?;
        let id = text(&map(&claim)?["id"])?.to_owned();
        objects.push(claim.clone());
        objects.push(act(
            captured,
            &claim,
            "propose",
            why.unwrap_or("explicit named hypothesis"),
            V::List(vec![]),
            std::slice::from_ref(&id),
            None,
            options,
        )?);
        for version in previous {
            objects.push(act(
                captured,
                &captured.objects[text(version)?],
                "retire",
                &format!("superseded within hypothesis {name}"),
                V::List(vec![]),
                std::slice::from_ref(&id),
                None,
                options,
            )?);
        }
        map_mut(map_mut(&mut after)?.entry(collection).or_insert_with(empty))?
            .insert(subject.into(), body);
    }
    let intent = obj([
        ("version", n(&version.to_string())),
        ("kind", s("edit")),
        ("name", s(name)),
        ("head", head),
        ("action", action),
        ("operation", s(&options.operation)),
        ("recorded_at", s(&options.recorded_at)),
        ("by", options.by.clone()),
    ]);
    mutation(
        store,
        captured,
        &objects,
        intent,
        &before,
        &after,
        Some(&groups),
        Some(&groups),
        options,
        runtime,
        audit,
    )
}

pub fn prepare_refute(
    store: &Store,
    captured: &Capture,
    names: &[String],
    because: &str,
    options: &Options,
    runtime: Option<&Runtime>,
) -> Result<PreparedMutation> {
    refute_inner(store, captured, names, because, options, runtime, None)
}
#[allow(clippy::too_many_arguments)]
fn refute_inner(
    store: &Store,
    captured: &Capture,
    names: &[String],
    because: &str,
    options: &Options,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
) -> Result<PreparedMutation> {
    let context = capture(store, captured, options)?;
    require(options.strict, "explicit_root_disposition_required")?;
    guard_names(names, &context.groups, &context.physical, true)?;
    require(!because.trim().is_empty(), "act_reason_required")?;
    let mut names = names.to_vec();
    names.sort();
    let mut chosen = BTreeMap::new();
    for name in &names {
        for (subject, versions) in map(&map(&map(&context.index)?["groups"])?[name])? {
            require(
                chosen.insert(subject.clone(), versions.clone()).is_none(),
                "overlapping_hypothesis_selection",
            )?;
        }
    }
    let mut objects = vec![];
    for versions in chosen.values() {
        for version in list(versions)? {
            objects.push(act(
                captured,
                &captured.objects[text(version)?],
                "refute",
                because,
                V::List(vec![]),
                &[],
                None,
                options,
            )?);
        }
    }
    let intent = obj([
        ("version", n("1")),
        ("kind", s("refute")),
        ("names", strings(names)),
        ("because", s(because)),
        ("take", V::List(vec![])),
        ("drops", empty()),
        ("operation", s(&options.operation)),
        ("recorded_at", s(&options.recorded_at)),
        ("by", options.by.clone()),
    ]);
    mutation(
        store,
        captured,
        &objects,
        intent,
        &context.base,
        &context.base,
        None,
        None,
        options,
        runtime,
        audit,
    )
}

/// Reconstruct only validated causal parents, never the incoming operation or siblings.
pub(crate) fn replay_context(
    store: &Store,
    mutation: &PreparedMutation,
    key: &str,
) -> Result<(Capture, Options, V, ReplayAudit)> {
    let live = store.capture()?;
    let data = mutation.to_data();
    let receipt = &map(&data)?["receipt"];
    let intent = field(map(&map(receipt)?["before"])?, key)?.clone();
    let intent_map = map(&intent)?;
    require(
        A::archive(store)? == *field(intent_map, "archive")?,
        "concurrent_archive_edit",
    )?;
    require(
        physical_evidence(store)? == *field(intent_map, "physical")?,
        "concurrent_hypothesis_edit",
    )?;
    let manifest = mutation
        .files()
        .iter()
        .find(|f| f.role == "history_commit")
        .and_then(|f| f.after.as_deref())
        .ok_or_else(|| error("invalid_mutation"))?;
    let manifest = Y::decode_document(manifest)?;
    Authority::validate_commit(&manifest)?;
    let manifest = map(&manifest)?;
    let mut commits = Authority::Files::new();
    let mut todo = map(&manifest["parents"])?
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    while let Some(operation) = todo.pop() {
        if commits.contains_key(&operation) {
            continue;
        }
        let raw = live
            .commits
            .get(&operation)
            .ok_or_else(|| error("incomplete_closure"))?;
        let parent = Y::decode_document(raw)?;
        todo.extend(map(&map(&parent)?["parents"])?.keys().cloned());
        commits.insert(operation, raw.clone());
    }
    let mut capture = live.clone();
    capture.objects = Authority::committed_objects(&live.marker, &commits, &live.object_bytes)?;
    capture.commits = commits;
    let raw = live
        .object_bytes
        .iter()
        .filter(|((_, id), _)| capture.objects.contains_key(id))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    capture.state =
        history_reduce::reduce_bytes(&raw, Some(map(&map(&live.state)?["rules"])?), None)?;
    capture.baseline = H::baseline(&live.marker, &capture.commits, &capture.state)?;
    capture.entry_bytes = mutation
        .files()
        .iter()
        .find(|f| f.role == "record")
        .and_then(|f| f.before.clone())
        .ok_or_else(|| error("invalid_mutation"))?;
    capture.document = Y::decode_document(&capture.entry_bytes)?;
    let audit = ReplayAudit::from_parents(receipt, &capture.commits)?;
    let requires = manifest
        .get("requires")
        .map(list)
        .transpose()?
        .unwrap_or(&[]);
    let options = Options {
        operation: text(&map(&data)?["operation"])?.into(),
        recorded_at: text(field(intent_map, "recorded_at")?)?.into(),
        recording_day: String::new(),
        by: field(intent_map, "by")?.clone(),
        strict: requires.contains(&s(Authority::ROOT_DISPOSITION)),
        paths: if requires.contains(&s(HP::CAPABILITY)) {
            HP::Scheme::Hashed
        } else {
            HP::Scheme::Legacy
        },
        receipt_version: None,
    };
    Ok((capture, options, intent, audit))
}
pub fn verify_prepared(
    store: &Store,
    mutation: &PreparedMutation,
    runtime: Option<&Runtime>,
) -> Result<()> {
    let (capture, mut options, intent, audit) =
        replay_context(store, mutation, "hypothesis_authoring")?;
    let intent = map(&intent)?;
    let version = field(intent, "version")?;
    require(
        is_int(version, "1") || is_int(version, "2"),
        "invalid_hypothesis_receipt",
    )?;
    options.receipt_version = Some(if is_int(version, "2") { 2 } else { 1 });
    let expected = match text(field(intent, "kind")?)? {
        "edit" => prepare_inner(
            store,
            &capture,
            text(field(intent, "name")?)?,
            field(intent, "action")?,
            Some(field(intent, "head")?),
            &options,
            runtime,
            Some(&audit),
        )?,
        "refute" => {
            let names = list(field(intent, "names")?)?
                .iter()
                .map(|v| text(v).map(str::to_owned))
                .collect::<Result<Vec<_>>>()?;
            refute_inner(
                store,
                &capture,
                &names,
                text(field(intent, "because")?)?,
                &options,
                runtime,
                Some(&audit),
            )?
        }
        _ => return Err(error("invalid_hypothesis_receipt")),
    };
    require(
        expected.to_bytes()? == mutation.to_bytes()?,
        "hypothesis_receipt_mismatch",
    )
}
pub fn commit(
    store: &Store,
    mutation: &PreparedMutation,
    runtime: Option<&Runtime>,
    verify: FS::Verify<'_>,
) -> Result<V> {
    store.commit(mutation, &mut |data| {
        verify_prepared(store, mutation, runtime)?;
        verify(data)?;
        let intent = map(field(
            map(&map(&map(data)?["receipt"])?["before"])?,
            "hypothesis_authoring",
        )?)?;
        require(
            A::archive(store)? == *field(intent, "archive")?,
            "concurrent_archive_edit",
        )?;
        require(
            physical_evidence(store)? == *field(intent, "physical")?,
            "concurrent_hypothesis_edit",
        )?;
        crate::history_sources::capture(
            &store.root,
            &store.layout.entry,
            &store.capture()?.document,
        )?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as J;
    fn runtime(cache: &Path) -> Runtime {
        Runtime::open(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/reasoning/native")
                .join(format!(
                    "{}.zip",
                    crate::reasoning_runtime::target_name().unwrap()
                )),
            cache,
            OperationalBounds::default(),
        )
        .unwrap()
    }
    fn write(root: &Path, case: &J) {
        for (path, raw) in case["files"].as_object().unwrap() {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, raw.as_str().unwrap()).unwrap();
        }
    }
    fn options(version: u8) -> Options {
        Options {
            operation: "hypothesis-1".into(),
            recorded_at: "2026-09-19T09:30:00+00:00".into(),
            recording_day: "2026-09-19".into(),
            by: s("writer"),
            strict: true,
            paths: HP::Scheme::Hashed,
            receipt_version: Some(version),
        }
    }
    fn case(name: &str) -> J {
        let data: J = serde_json::from_str(include_str!(
            "../tests/fixtures/history-hypothesis-candidate.json"
        ))
        .unwrap();
        data["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap()
            .clone()
    }
    #[test]
    fn named_replay_preserves_base_and_originals_through_review_refutation_and_retry() {
        let case = case("new-judgment");
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &case);
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
        let before = store.capture().unwrap();
        let original = before.object_bytes.clone();
        let action = V::from_tagged(&case["action"]).unwrap();
        let mut options = options(2);
        let mutation = prepare(
            &store,
            &before,
            "trial",
            &action,
            None,
            &options,
            Some(&runtime),
        )
        .unwrap();
        verify_prepared(&store, &mutation, Some(&runtime)).unwrap();
        assert!(verify_prepared(&store, &mutation, None).is_err());
        commit(&store, &mutation, Some(&runtime), &mut |_| Ok(())).unwrap();
        commit(&store, &mutation, Some(&runtime), &mut |_| Ok(())).unwrap();
        let accepted = store.capture().unwrap();
        assert!(
            !entries(&A::document(&accepted).unwrap())
                .unwrap()
                .contains_key("d.ready")
        );
        assert_eq!(
            entries(&A::document(&accepted).unwrap()).unwrap()["p.input"].1,
            entries(&A::document(&before).unwrap()).unwrap()["p.input"].1
        );
        options.operation = "review-1".into();
        let review = prepare(
            &store,
            &accepted,
            "trial",
            &obj([("kind", s("review")), ("id", s("d.ready"))]),
            None,
            &options,
            Some(&runtime),
        )
        .unwrap();
        commit(&store, &review, Some(&runtime), &mut |_| Ok(())).unwrap();
        commit(&store, &review, Some(&runtime), &mut |_| Ok(())).unwrap();
        options.operation = "refute-1".into();
        let refute = prepare_refute(
            &store,
            &store.capture().unwrap(),
            &["trial".into()],
            "tested and refuted",
            &options,
            Some(&runtime),
        )
        .unwrap();
        commit(&store, &refute, Some(&runtime), &mut |_| Ok(())).unwrap();
        for operation in [&mutation, &review, &refute] {
            verify_prepared(&store, operation, Some(&runtime)).unwrap();
        }
        commit(&store, &refute, Some(&runtime), &mut |_| Ok(())).unwrap();
        let after = store.capture().unwrap();
        assert_eq!(after.commits.len(), 4);
        for (key, raw) in original {
            assert_eq!(after.object_bytes[&key], raw);
        }
        let adapted = D::from_store_capture(&after).unwrap();
        assert!(
            map(&HH::layers(adapted.projection(), adapted.document())
                .unwrap()
                .0)
            .unwrap()
            .is_empty()
        );
        std::fs::write(
            &store.entry,
            refute
                .files()
                .iter()
                .find(|f| f.role == "record")
                .unwrap()
                .before
                .as_ref()
                .unwrap(),
        )
        .unwrap();
        commit(&store, &refute, Some(&runtime), &mut |_| Ok(())).unwrap();
        assert_eq!(store.capture().unwrap().entry_bytes, after.entry_bytes);
    }
    #[test]
    fn named_commit_rechecks_physical_and_archive_after_caller_verification() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        for physical in [true, false] {
            let case = case("new");
            let root = tempfile::tempdir().unwrap();
            write(root.path(), &case);
            let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
            let before = store.capture().unwrap();
            let mutation = prepare(
                &store,
                &before,
                "trial",
                &V::from_tagged(&case["action"]).unwrap(),
                None,
                &options(2),
                Some(&runtime),
            )
            .unwrap();
            let result = commit(&store, &mutation, Some(&runtime), &mut |_| {
                let path = if physical {
                    store.root.join(&store.layout.hypotheses).join("other.yaml")
                } else {
                    store.root.join(&store.layout.replaced)
                };
                std::fs::create_dir_all(path.parent().unwrap())?;
                std::fs::write(path, b"p:\n  p.changed: {v: 1}\n")?;
                Ok(())
            });
            assert_eq!(
                result.unwrap_err().0,
                if physical {
                    "concurrent_hypothesis_edit"
                } else {
                    "concurrent_archive_edit"
                }
            );
            assert_eq!(store.capture().unwrap().commits.len(), 1);
        }
    }
    #[test]
    fn physical_import_map_requires_exact_filename_membership_and_bytes() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
        let path = format!("{}/trial.yaml", store.layout.hypotheses);
        std::fs::create_dir_all(root.path().join(&store.layout.hypotheses)).unwrap();
        let raw = b"p:\n  p.input: {v: 1}\n";
        std::fs::write(root.path().join(&path), raw).unwrap();
        let item = obj([
            ("name", s("trial")),
            ("path", s(&path)),
            ("sha256", s(&sha256(raw))),
        ]);
        let doc = |items| {
            obj([(
                "meta",
                obj([(
                    "history_hypothesis_import",
                    obj([("version", n("1")), ("physical", V::List(items))]),
                )]),
            )])
        };
        assert_eq!(active_physical(&store, &empty()).unwrap().len(), 1);
        assert!(
            active_physical(&store, &doc(vec![item.clone()]))
                .unwrap()
                .is_empty()
        );
        assert!(active_physical(&store, &doc(vec![item.clone(), item.clone()])).is_err());
        for (key, value) in [
            ("name", s("other")),
            ("path", s("../trial.yaml")),
            ("sha256", s(&"a".repeat(64))),
        ] {
            let mut bad = item.clone();
            map_mut(&mut bad).unwrap().insert(key.into(), value);
            assert!(active_physical(&store, &doc(vec![bad])).is_err());
        }
        std::fs::write(
            root.path().join(&store.layout.hypotheses).join("trial.yml"),
            raw,
        )
        .unwrap();
        assert_eq!(
            active_physical(&store, &doc(vec![item])).unwrap_err().0,
            "missing_imported_hypothesis"
        );
    }
    #[test]
    fn rehashed_named_receipt_cannot_replace_semantic_replay() {
        let case = case("new");
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &case);
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
        let mutation = prepare(
            &store,
            &store.capture().unwrap(),
            "trial",
            &V::from_tagged(&case["action"]).unwrap(),
            None,
            &options(2),
            Some(&runtime),
        )
        .unwrap();
        let data = mutation.to_data();
        let data = map(&data).unwrap();
        let old = map(&data["receipt"]).unwrap();
        let mut after = old["after"].clone();
        map_mut(&mut after)
            .unwrap()
            .insert("forged".into(), V::Bool(true));
        let receipt =
            T::semantic_receipt("core/v1", &old["capabilities"], &old["before"], &after).unwrap();
        let mut files = mutation.files().to_vec();
        for file in &mut files {
            if file.role == "history_commit" {
                let mut manifest = Y::decode_document(file.after.as_ref().unwrap()).unwrap();
                map_mut(&mut manifest)
                    .unwrap()
                    .insert("receipt".into(), receipt.clone());
                file.after = Some(crate::history_emit::encode_document(&manifest).unwrap());
            }
        }
        let forged = PreparedMutation::prepare(
            "hypothesis-1",
            &data["authority"],
            &data["baseline"],
            files,
            &receipt,
            "GROUNDING.yaml",
            None,
        )
        .unwrap();
        assert_eq!(
            verify_prepared(&store, &forged, Some(&runtime))
                .unwrap_err()
                .0,
            "hypothesis_receipt_mismatch"
        );
    }
    #[test]
    fn named_actions_match_pinned_and_candidate_python_envelopes() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let mut failures = vec![];
        for raw in [
            include_str!("../tests/fixtures/history-hypothesis.json"),
            include_str!("../tests/fixtures/history-hypothesis-candidate.json"),
            include_str!("../tests/fixtures/history-hypothesis-retained.json"),
        ] {
            let data: J = serde_json::from_str(raw).unwrap();
            for case in data["cases"].as_array().unwrap() {
                let root = tempfile::tempdir().unwrap();
                write(root.path(), case);
                let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
                let capture = store.capture().unwrap();
                let action = V::from_tagged(&case["action"]).unwrap();
                let head = V::from_tagged(&case["head"]).unwrap();
                let audit = case
                    .get("receipt")
                    .map(|r| ReplayAudit::oracle(&V::from_tagged(r).unwrap()).unwrap());
                let options = options(case["version"].as_u64().unwrap() as u8);
                let result = if map(&action)
                    .unwrap()
                    .get("kind")
                    .is_some_and(|v| string_is(v, "_refute"))
                {
                    let a = map(&action).unwrap();
                    let names = list(&a["names"])
                        .unwrap()
                        .iter()
                        .map(|v| text(v).unwrap().into())
                        .collect::<Vec<_>>();
                    refute_inner(
                        &store,
                        &capture,
                        &names,
                        a.get("because")
                            .map(|v| text(v).unwrap())
                            .unwrap_or("explicit test refutation"),
                        &options,
                        Some(&runtime),
                        audit.as_ref(),
                    )
                } else {
                    prepare_inner(
                        &store,
                        &capture,
                        case["group"].as_str().unwrap(),
                        &action,
                        Some(&head),
                        &options,
                        Some(&runtime),
                        audit.as_ref(),
                    )
                };
                let label = format!("{}-{}", case["version"], case["name"].as_str().unwrap());
                match (case.get("output"), result) {
                    (Some(expected), Ok(actual)) => {
                        let actual = actual.to_bytes().unwrap();
                        if actual != expected.as_str().unwrap().as_bytes() {
                            std::fs::write(
                                std::env::temp_dir()
                                    .join(format!("hypothesis-{label}-actual.json")),
                                actual,
                            )
                            .unwrap();
                            std::fs::write(
                                std::env::temp_dir()
                                    .join(format!("hypothesis-{label}-expected.json")),
                                expected.as_str().unwrap(),
                            )
                            .unwrap();
                            failures.push(format!("{label}: byte mismatch"));
                        }
                    }
                    (Some(_), Err(e)) => failures.push(format!("{label}: {e}")),
                    // b639cdb crashed on scalar hypotheses. The fixed 1.7 reader
                    // supports scalars even with receipt1; both fixed versions
                    // above compare whole accepted bytes against that reader.
                    (None, Ok(_))
                        if case["name"] == "scalar"
                            && data["oracle"] == "b639cdbab1e3c624ad52708350166d222821aec1" => {}
                    (None, Ok(_)) => failures.push(format!("{label}: unexpectedly accepted")),
                    (None, Err(_)) => {}
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}

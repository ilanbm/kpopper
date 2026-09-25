//! Source-free replay of complete committed history observations. Decoding these
//! bytes never opens a source path and does not create a writable store handle.
use crate::{
    Result, history_adapter,
    history_authority::{self as A, Files},
    history_capture::{self as H, Capture, Layout},
    history_contract::*,
    history_paths,
    history_projection::CapturedHistory,
    history_reduce,
    history_store::Store,
    history_view::{self as W, list},
    history_yaml as Y,
    identity::sha256,
    require,
    value::{Integer, TypedValue as V},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

fn s(value: &str) -> V {
    V::Text(value.into())
}
pub(crate) fn files_valid(files: &Files) -> Result<()> {
    require(files.len() <= 2 * MAX_OBJECTS + 2, "history_limit")?;
    let mut size = 0;
    for (path, bytes) in files {
        A::relative_path(path)?;
        size += bytes.len();
        require(
            bytes.len() <= 16 * 1024 * 1024 && size <= 64 * 1024 * 1024,
            "history_limit",
        )?;
        for (index, _) in path.match_indices('/') {
            require(
                !files.contains_key(&path[..index]),
                "history_bundle_path_collision",
            )?;
        }
    }
    Ok(())
}
pub(crate) fn capture(files: &Files, rules: Option<&V>) -> Result<Capture> {
    files_valid(files)?;
    let authority_bytes = files
        .get("authority.yaml")
        .ok_or_else(|| error("history_bundle_membership"))?;
    let entry_bytes = files
        .get("entry.yaml")
        .ok_or_else(|| error("history_bundle_membership"))?;
    let marker = Y::decode_document(authority_bytes)?;
    A::validate_authority(&marker)?;
    let document = Y::decode_document(entry_bytes)?;
    let mut commits = Files::new();
    let mut storage = Files::new();
    let mut cancellations = Files::new();
    for (path, bytes) in files {
        let parts = path.split('/').collect::<Vec<_>>();
        match parts.as_slice() {
            ["commits", name] if name.ends_with(".yaml") => {
                let operation = name.strip_suffix(".yaml").unwrap();
                token(&s(operation))?;
                commits.insert(operation.into(), bytes.clone());
            }
            ["cancellations", name] if name.ends_with(".yaml") => {
                cancellations.insert((*name).into(), bytes.clone());
            }
            ["objects", directory, name] if name.ends_with(".yaml") => {
                let path = format!("{directory}/{name}");
                history_paths::parse_object_path(&path)?;
                storage.insert(path, bytes.clone());
            }
            ["authority.yaml" | "entry.yaml"] => {}
            _ => return Err(error("invalid_history_bundle_path")),
        }
    }
    let (object_bytes, object_paths) = A::objects_from_storage(&commits, &storage)?;
    let mut generations =
        A::committed_generations(&marker, &commits, &object_bytes, &cancellations)?;
    require(
        storage.keys().collect::<BTreeSet<_>>() == object_paths.values().collect::<BTreeSet<_>>(),
        "history_bundle_membership",
    )?;
    let V::Integer(generation) = &map(&marker)?["generation"] else {
        return Err(error("invalid_authority"));
    };
    let current = generations.remove(generation.as_str());
    let (commits, objects) = current.map(|g| (g.commits, g.objects)).unwrap_or_default();
    let all = objects
        .iter()
        .chain(generations.values().flat_map(|g| g.objects.iter()))
        .map(|(id, o)| Ok((text(&map(o)?["subject"])?.to_owned(), id.clone())))
        .collect::<Result<BTreeSet<_>>>()?;
    require(
        object_bytes.keys().cloned().collect::<BTreeSet<_>>() == all,
        "history_bundle_membership",
    )?;
    let selected_bytes = object_bytes
        .iter()
        .filter(|((_, id), _)| objects.contains_key(id))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let state = history_reduce::reduce_bytes(&selected_bytes, rules.map(map).transpose()?, None)?;
    let baseline = H::baseline(&marker, &commits, &state)?;
    let actual = map(&document)?
        .get("meta")
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("history"))
        .ok_or_else(|| error("baseline_mismatch"))?;
    A::bind_authority(&marker, actual)?;
    let captured = Capture {
        // Private placeholder paths are never read or returned by this module.
        root: PathBuf::new(),
        layout: Layout::for_entry("entry.yaml")?,
        entry_bytes: entry_bytes.clone(),
        document,
        view_alternatives: Vec::new(),
        authority_bytes: authority_bytes.clone(),
        marker,
        commits,
        object_bytes,
        objects,
        state,
        baseline,
        inactive_generations: generations,
        cancellation_bytes: cancellations,
        storage_bytes: storage,
        object_paths,
        inventory: BTreeMap::new(),
    };
    let rendered = W::render_document(
        &captured,
        &captured.objects,
        &captured.object_bytes,
        &captured.commits,
    )?;
    require(
        rendered.digest()? == captured.document.digest()? || Store::known_view(&captured)?,
        "unresolved_view_edit",
    )?;
    let actual = map(&map(&captured.document)?["meta"])?;
    let actual = map(&actual["history"])?;
    for (role, act) in [("heads", false), ("open_acts", true)] {
        for (subject, versions) in map(&actual[role])? {
            for version in list(versions)? {
                let obj = captured
                    .objects
                    .get(text(version)?)
                    .ok_or_else(|| error("incomplete_view_baseline"))?;
                let obj = map(obj)?;
                require(
                    string_is(&obj["subject"], subject) && string_is(&obj["kind"], "act") == act,
                    "incomplete_view_baseline",
                )?;
            }
        }
    }
    Ok(captured)
}

/// Validates complete portable target/member evidence independently of sharing.
/// `files` are already decoded checksummed bytes from the Snapshot transport.
pub fn validate_observation(evidence: &V, files: &Files) -> Result<CapturedHistory> {
    Ok(validate_observation_capture(evidence, files)?.1)
}
pub(crate) fn validate_observation_capture(
    evidence: &V,
    files: &Files,
) -> Result<(Capture, CapturedHistory)> {
    let e = schema(
        evidence,
        &[
            "version",
            "files",
            "sha256",
            "rules",
            "baseline",
            "projection",
        ],
        &["inactive_generations"],
    )?;
    require(
        is_int(&e["version"], "1") || is_int(&e["version"], "2"),
        "unsupported_history_observation",
    )?;
    files_valid(files)?;
    let hashes = map(&e["sha256"])?;
    require(
        files.keys().collect::<Vec<_>>() == hashes.keys().collect::<Vec<_>>()
            && files.contains_key("entry.yaml")
            && files.contains_key("authority.yaml"),
        "history_bundle_membership",
    )?;
    for (path, bytes) in files {
        require(hashes[path] == s(&sha256(bytes)), "history_bundle_checksum")?;
    }
    let captured = capture(files, Some(&e["rules"]))?;
    let retained = V::Map(
        captured
            .inactive_generations
            .iter()
            .map(|(generation, g)| (generation.clone(), s(&g.digest)))
            .collect(),
    );
    require(
        if is_int(&e["version"], "2") {
            !captured.inactive_generations.is_empty()
                && e.get("inactive_generations") == Some(&retained)
        } else {
            captured.inactive_generations.is_empty() && !e.contains_key("inactive_generations")
        },
        "history_generation_capability_required",
    )?;
    require(
        captured.baseline.digest()? == e["baseline"].digest()?,
        "baseline_mismatch",
    )?;
    let adapted = history_adapter::from_store_capture(&captured)?;
    require(
        adapted.projection().digest()? == e["projection"].digest()?,
        "projection_mismatch",
    )?;
    Ok((captured, adapted))
}

const CAPABILITY: &str = "history-closure/v1";
const GENERATIONS: &str = "history-generations/v1";
const CANCELLATIONS: &str = "generation-cancellation/v1";
const SUBSET: &str = "history-subset/v1";
const PREFIX: &str = "history-closure/";
pub(crate) fn privacy(value: &V) -> Result<()> {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            V::Map(m) => {
                require(
                    m.get("private").is_none_or(|v| *v == V::Bool(false))
                        && m.get("shareability")
                            .is_none_or(|v| string_is(v, "project"))
                        && ["privacy", "visibility"].iter().all(|k| {
                            m.get(*k).is_none_or(|v| {
                                !["private", "unclear", "unknown", "personal"]
                                    .iter()
                                    .any(|s| string_is(v, s))
                            })
                        }),
                    "private_history_evidence",
                )?;
                pending.extend(m.values());
            }
            V::List(v) => pending.extend(v),
            _ => {}
        }
    }
    Ok(())
}
pub(crate) fn validate_artifact(manifest: &V, revision: &V, files: &Files) -> Result<Capture> {
    Y::validate_value(manifest, 16 * 1024 * 1024)?;
    let m = schema(
        manifest,
        &[
            "version",
            "requires",
            "roots",
            "scope",
            "shareability",
            "rules",
            "baseline",
            "files",
        ],
        &["inactive_generations"],
    )?;
    let version = if is_int(&m["version"], "1") {
        1
    } else if is_int(&m["version"], "2") {
        2
    } else if is_int(&m["version"], "3") {
        3
    } else {
        0
    };
    let required = list(&m["requires"])?;
    let declared = required
        .iter()
        .filter(|v| !string_is(v, history_paths::CAPABILITY))
        .cloned()
        .collect::<Vec<_>>();
    require(
        version != 0
            && [
                vec![s(CAPABILITY)],
                vec![s(CAPABILITY), s(SUBSET)],
                vec![s(CAPABILITY), s(GENERATIONS)],
                vec![s(CAPABILITY), s(GENERATIONS), s(CANCELLATIONS)],
            ]
            .contains(&declared),
        "unsupported_history_bundle",
    )?;
    require(
        *revision == s(&manifest.digest()?),
        "history_bundle_identity",
    )?;
    files_valid(files)?;
    let hashes = map(&m["files"])?;
    require(
        files.keys().collect::<Vec<_>>() == hashes.keys().collect::<Vec<_>>()
            && files.contains_key("entry.yaml")
            && files.contains_key("authority.yaml"),
        "history_bundle_membership",
    )?;
    for (path, raw) in files {
        require(hashes[path] == s(&sha256(raw)), "history_bundle_checksum")?;
        privacy(&Y::decode_document(raw)?)?;
    }
    let captured = capture(files, Some(&m["rules"]))?;
    let meta = map(&map(&captured.document)?["meta"])?;
    require(
        version == 2 || !meta.contains_key("history_subset"),
        "subset_capability_required",
    )?;
    require(
        map(&captured.state)?["rules"].digest()? == m["rules"].digest()?,
        "rules_mismatch",
    )?;
    require(
        captured.baseline.digest()? == m["baseline"].digest()?,
        "baseline_mismatch",
    )?;
    let mut required = vec![s(CAPABILITY)];
    if version == 2 {
        required.push(s(SUBSET));
    }
    if version == 3 {
        required.push(s(GENERATIONS));
    }
    if !captured.cancellation_bytes.is_empty() {
        required = vec![s(CAPABILITY), s(GENERATIONS), s(CANCELLATIONS)];
    }
    if captured.storage_bytes.keys().any(|p| {
        history_paths::parse_object_path(p).is_ok_and(|v| v.0 == history_paths::Scheme::Hashed)
    }) {
        required.push(s(history_paths::CAPABILITY));
    }
    require(
        m["requires"] == V::List(required),
        "history_generation_capability_required",
    )?;
    let retained = V::Map(
        captured
            .inactive_generations
            .iter()
            .map(|(generation, g)| (generation.clone(), s(&g.digest)))
            .collect(),
    );
    require(
        if version == 3 {
            !captured.inactive_generations.is_empty()
                && m.get("inactive_generations") == Some(&retained)
        } else {
            captured.inactive_generations.is_empty() && !m.contains_key("inactive_generations")
        },
        "history_generation_capability_required",
    )?;
    if version == 2 {
        validate_subset(&captured, m)?;
    }
    require(
        string_is(&m["shareability"], "project"),
        "history_not_shareable",
    )?;
    let roots = list(&m["roots"])?;
    let roots = roots
        .iter()
        .map(|v| text(v).map(str::to_owned))
        .collect::<Result<Vec<_>>>()?;
    require(
        !roots.is_empty() && roots.windows(2).all(|w| w[0] < w[1]),
        "invalid_history_roots",
    )?;
    for root in &roots {
        history_paths::subject(root)?;
    }
    let scope = map(&m["scope"])?;
    require(
        scope.get("kind").is_some_and(|v| {
            ["project", "external", "code"]
                .iter()
                .any(|s| string_is(v, s))
        }) && scope
            .get("environment")
            .and_then(|v| text(v).ok())
            .is_some_and(|s| {
                !s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
                    .is_empty()
            }),
        "invalid_history_scope",
    )?;
    if scope.get("kind").is_some_and(|v| string_is(v, "code")) {
        require(
            scope
                .get("commit")
                .and_then(|v| text(v).ok())
                .is_some_and(history_paths::object_id),
            "invalid_history_scope",
        )?;
    }
    privacy(&m["scope"])?;
    let subjects = captured
        .objects
        .values()
        .chain(
            captured
                .inactive_generations
                .values()
                .filter(|_| version != 2)
                .flat_map(|g| g.objects.values()),
        )
        .map(|v| text(&map(v)?["subject"]).map(str::to_owned))
        .collect::<Result<BTreeSet<_>>>()?;
    require(
        roots.iter().all(|r| subjects.contains(r)),
        "invalid_history_roots",
    )?;
    require(
        version == 2 || roots.iter().cloned().collect::<BTreeSet<_>>() == subjects,
        "history_export_requires_full_authorization",
    )?;
    let document = history_adapter::from_store_capture(&captured)?;
    let entries = crate::reasoning_snapshot::entries(document.document())?;
    for (id, (_, body)) in &entries {
        if version == 2 && !roots.contains(id) {
            continue;
        }
        let body = map(body).map_err(|_| error("history_scope_mismatch"))?;
        require(
            body.get("scope").unwrap_or(&V::Null).digest()? == m["scope"].digest()?,
            "history_scope_mismatch",
        )?;
    }
    require(
        version != 2 || roots.iter().all(|r| entries.contains_key(r)),
        "unavailable_subset_root",
    )?;
    Ok(captured)
}

fn locator_disclosures(captured: &Capture, origin: &Map) -> Result<()> {
    locator_disclosures_in(
        &captured.objects,
        text(&origin["source_entry"])?,
        &origin["disclosed_locators"],
    )
}
/// Every non-entry locator carried by the selected objects is explicitly disclosed,
/// and every disclosure is used. Shared by legacy subsets and compact closures.
pub(crate) fn locator_disclosures_in(objects: &Map, entry: &str, disclosed: &V) -> Result<()> {
    A::relative_path(entry)?;
    let mut allowed = BTreeSet::new();
    for item in list(disclosed)? {
        let item = schema(item, &["path", "sha256"], &[])?;
        let path = text(&item["path"])?;
        A::relative_path(path)?;
        require(
            item["sha256"] == V::Null
                || text(&item["sha256"])
                    .is_ok_and(|s| s.len() == 64 && history_paths::object_id(s)),
            "invalid_locator_disclosures",
        )?;
        allowed.insert((path.to_owned(), item["sha256"].digest()?));
    }
    let mut used = BTreeSet::new();
    let mut pending = Vec::new();
    for value in objects.values() {
        let obj = map(value)?;
        if let Some(locator) = obj
            .get("authored")
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get("locator"))
        {
            pending.push(locator);
        }
        if let Some(at) = obj.get("at") {
            pending.push(at);
        }
    }
    while let Some(value) = pending.pop() {
        match value {
            V::Map(m) => {
                if let Some(path) = m.get("path") {
                    let path = text(path)?;
                    A::relative_path(path)?;
                    if path != entry {
                        let key = (
                            path.to_owned(),
                            m.get("sha256").unwrap_or(&V::Null).digest()?,
                        );
                        require(allowed.contains(&key), "undisclosed_history_locator")?;
                        used.insert(key);
                    }
                }
                pending.extend(m.values());
            }
            V::List(v) => pending.extend(v),
            _ => {}
        }
    }
    require(used == allowed, "unused_locator_disclosure")
}
pub(crate) fn subset_subjects(captured: &Capture, roots: &V) -> Result<BTreeSet<String>> {
    let document = history_adapter::from_store_capture(captured)?;
    subset_subjects_in(&captured.objects, document.document(), roots)
}
/// The complete subject dependency closure of `roots` over full semantic objects.
/// `document` supplies only collection membership for collection-scope rules.
pub(crate) fn subset_subjects_in(
    objects: &Map,
    document: &V,
    roots: &V,
) -> Result<BTreeSet<String>> {
    use std::sync::LazyLock;
    static ID: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+").unwrap());
    static REF: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"\{\{[\s\x1c-\x1f]*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)[\s\x1c-\x1f]*\}\}",
        )
        .unwrap()
    });
    let mut by_subject: BTreeMap<String, Vec<&V>> = BTreeMap::new();
    for value in objects.values() {
        by_subject
            .entry(text(&map(value)?["subject"])?.into())
            .or_default()
            .push(value);
    }
    let mut pending = list(roots)?
        .iter()
        .map(|v| text(v).map(str::to_owned))
        .collect::<Result<Vec<_>>>()?;
    require(!pending.is_empty(), "invalid_history_roots")?;
    let mut selected = BTreeSet::new();
    while let Some(subject) = pending.pop() {
        if !selected.insert(subject.clone()) {
            continue;
        }
        let claims = by_subject
            .get(&subject)
            .ok_or_else(|| error("incomplete_subset_dependency"))?;
        let mut deps = BTreeSet::new();
        for value in claims {
            deps.extend(references(value)?.into_iter().map(|r| r.subject));
            let obj = map(value)?;
            let V::Map(body) = &obj["body"] else { continue };
            if string_is(&obj["kind"], "act") {
                continue;
            }
            let fields = map(&map(&obj["authored"])?["fields"])?;
            let field = |role: &str, fallback: &str| {
                fields
                    .get(role)
                    .and_then(|v| text(v).ok())
                    .unwrap_or(fallback)
                    .to_owned()
            };
            if let Some(declared) = body.get(&field("deps", "rests_on")) {
                match declared {
                    V::Map(m) => deps.extend(m.keys().cloned()),
                    V::List(v) => deps.extend(
                        v.iter()
                            .map(|v| text(v).map(str::to_owned))
                            .collect::<Result<Vec<_>>>()?,
                    ),
                    _ => return Err(error("invalid_subset_dependencies")),
                }
            }
            if let Some(seen) = body.get(&field("snapshot", "seen")) {
                deps.extend(
                    map(seen)
                        .map_err(|_| error("invalid_subset_dependencies"))?
                        .keys()
                        .cloned(),
                );
            }
            if let Some(gaps) = obj.get("pin_gaps") {
                deps.extend(map(gaps)?.keys().cloned());
            }
            let sources = match body.get("from") {
                Some(V::Text(s)) => vec![s.as_str()],
                Some(V::List(v)) => v.iter().filter_map(|v| text(v).ok()).collect(),
                _ => Vec::new(),
            };
            deps.extend(
                sources
                    .into_iter()
                    .filter(|s| by_subject.contains_key(*s))
                    .map(str::to_owned),
            );
            let mut strings = Vec::new();
            let mut walk = vec![&obj["body"]];
            while let Some(value) = walk.pop() {
                match value {
                    V::Text(s) => strings.push(s.as_str()),
                    V::Map(m) => {
                        strings.extend(m.keys().map(String::as_str));
                        walk.extend(m.values());
                    }
                    V::List(v) => walk.extend(v),
                    _ => {}
                }
            }
            for source in strings {
                deps.extend(REF.captures_iter(source).map(|c| c[1].to_owned()));
            }
            let mut mentions = BTreeSet::new();
            for value in body.values() {
                match value {
                    V::Text(s) => mentions.extend(ID.find_iter(s).map(|m| m.as_str().to_owned())),
                    V::List(v) => {
                        for s in v.iter().filter_map(|v| text(v).ok()) {
                            mentions.extend(ID.find_iter(s).map(|m| m.as_str().to_owned()));
                        }
                    }
                    V::Map(m) => {
                        let keys = m.keys().map(String::as_str).collect::<Vec<_>>();
                        if [
                            vec!["ref"],
                            vec!["num"],
                            vec!["text"],
                            vec!["bool"],
                            vec!["args", "op"],
                            vec!["expr"],
                        ]
                        .contains(&keys)
                        {
                            mentions.extend(crate::reasoning_language::legacy_references(value));
                        } else {
                            for s in m.keys() {
                                mentions.extend(ID.find_iter(s).map(|m| m.as_str().to_owned()));
                            }
                        }
                    }
                    _ => {}
                }
            }
            deps.extend(mentions.into_iter().filter(|s| by_subject.contains_key(s)));
            for field in ["rule".to_owned(), field("predicate", "wrong_if")] {
                if let Some(value @ V::Map(_)) = body.get(&field) {
                    deps.extend(crate::reasoning_language::references(
                        &crate::reasoning_language::lower(value)?,
                    ));
                }
            }
            if let Some(definition) = body.get("collection_scope").filter(|v| **v != V::Null) {
                let collection = map(definition)
                    .ok()
                    .and_then(|m| m.get("collection"))
                    .and_then(|v| text(v).ok())
                    .ok_or_else(|| error("invalid_subset_scope"))?;
                if let Some(members) = map(document)?.get(collection) {
                    deps.extend(map(members)?.keys().cloned());
                }
            }
        }
        require(
            deps.iter()
                .all(|s| !crate::reasoning_fields::BUILTINS.contains(&s.as_str())),
            "unsupported_subset_dependency",
        )?;
        pending.extend(deps.difference(&selected).cloned());
    }
    Ok(selected)
}
fn validate_subset(captured: &Capture, manifest: &Map) -> Result<()> {
    require(captured.commits.len() == 1, "invalid_subset_commits")?;
    let commit = Y::decode_document(captured.commits.values().next().unwrap())?;
    let commit = map(&commit)?;
    crate::history_transaction::validate_receipt(&commit["receipt"])?;
    let origin = map(&map(&captured.document)?["meta"])?
        .get("history_subset")
        .ok_or_else(|| error("invalid_subset_origin"))?;
    crate::history_projection::validate_subset_origin(origin)?;
    let o = map(origin)?;
    require(
        commit["operation"] == o["operation"]
            && map(&commit["parents"])?.is_empty()
            && map(&commit["receipt"])?["after"].digest()?
                == V::Map(Map::from([("history_subset".into(), origin.clone())])).digest()?,
        "invalid_subset_receipt",
    )?;
    let subjects = map(&o["subjects"])?;
    require(
        o["roots"] == manifest["roots"]
            && subjects.keys().collect::<Vec<_>>()
                == map(&map(&captured.state)?["subjects"])?
                    .keys()
                    .collect::<Vec<_>>(),
        "invalid_subset_coverage",
    )?;
    locator_disclosures(captured, o)?;
    require(
        subset_subjects(captured, &o["roots"])? == subjects.keys().cloned().collect(),
        "invalid_subset_coverage",
    )?;
    for (subject, evidence) in subjects {
        let evidence = map(evidence)?;
        let inventory = V::Map(
            captured
                .objects
                .iter()
                .filter(|(_, v)| map(v).is_ok_and(|m| string_is(&m["subject"], subject)))
                .map(|(id, _)| {
                    (
                        id.clone(),
                        s(&sha256(
                            &captured.object_bytes[&(subject.clone(), id.clone())],
                        )),
                    )
                })
                .collect(),
        );
        require(
            evidence["objects_digest"] == s(&inventory.digest()?)
                && evidence["reduction_digest"]
                    == s(&map(&map(&captured.state)?["subjects"])?[subject].digest()?),
            "invalid_subset_receipt",
        )?;
    }
    let adapted = history_adapter::from_store_capture(captured)?;
    let projection = map(adapted.projection())?;
    require(
        string_is(&map(&projection["coverage"])?["scope"], "selected")
            && projection.get("origin") == Some(origin),
        "invalid_subset_coverage",
    )
}

/// Replay the full v3 contribution manifest and all its historical file evidence.
pub fn validate_contribution(bundle: &V, files: &Files) -> Result<CapturedHistory> {
    let b = map(bundle)?;
    let m = map(field(b, "manifest")?)?;
    let revision = field(b, "revision")?;
    require(
        is_int(field(m, "version")?, "3") && *revision == s(&field(b, "manifest")?.digest()?),
        "invalid_history_contribution",
    )?;
    files_valid(files)?;
    let hashes = map(field(m, "evidence")?)?;
    require(
        files.keys().collect::<Vec<_>>() == hashes.keys().collect::<Vec<_>>(),
        "invalid_history_contribution",
    )?;
    for (path, raw) in files {
        require(
            hashes[path] == s(&sha256(raw)),
            "invalid_history_contribution",
        )?;
    }
    privacy(field(m, "document")?)?;
    privacy(field(m, "scope")?)?;
    let binding = map(field(m, "history")?)?;
    let history_manifest = field(binding, "manifest")?;
    let hm = map(history_manifest)?;
    let history_files = map(field(hm, "files")?)?
        .keys()
        .filter_map(|path| {
            files
                .get(&format!("{PREFIX}{path}"))
                .map(|raw| (path.clone(), raw.clone()))
        })
        .collect::<Files>();
    let captured = validate_artifact(
        history_manifest,
        field(binding, "revision")?,
        &history_files,
    )?;
    let adapted = history_adapter::from_store_capture(&captured)?;
    require(
        field(m, "roots")? == field(hm, "roots")?
            && field(m, "scope")?.digest()? == field(hm, "scope")?.digest()?,
        "invalid_history_contribution",
    )?;
    require(
        field(m, "document")?.digest()? == adapted.document().digest()?,
        "invalid_history_contribution",
    )?;
    let cap = crate::reasoning_capabilities::history_document_capabilities(adapted.document())?;
    require(
        field(m, "reasoning")? == &cap,
        "invalid_history_contribution",
    )?;
    let mut evidence = files.clone();
    for (path, raw) in &history_files {
        require(
            evidence.remove(&format!("{PREFIX}{path}")) == Some(raw.clone()),
            "invalid_history_contribution",
        )?;
    }
    require(
        !evidence.keys().any(|p| p.starts_with(PREFIX)),
        "invalid_history_contribution",
    )?;
    let mut required = BTreeSet::new();
    for raw in history_files.values() {
        let value = Y::decode_document(raw)?;
        let mut pending = vec![&value];
        while let Some(value) = pending.pop() {
            match value {
                V::Map(m) => {
                    if let Some(file) = m.get("file") {
                        let name = text(file)?;
                        A::relative_path(name)?;
                        required.insert(name.to_owned());
                    }
                    pending.extend(m.values());
                }
                V::List(v) => pending.extend(v),
                _ => {}
            }
        }
    }
    let meta = map(&map(adapted.document())?["meta"])?;
    if let Some(imported) = meta.get("history_import").filter(|v| **v != V::Null) {
        let imported = schema(
            imported,
            &["version", "operation", "recorded_at", "members"],
            &[],
        )?;
        require(
            is_int(&imported["version"], "1"),
            "unsupported_history_import",
        )?;
        let mut seen = BTreeSet::new();
        for item in list(&imported["members"])? {
            let item = schema(item, &["path", "sha256", "role"], &[])?;
            let name = text(&item["path"])?;
            A::relative_path(name)?;
            require(
                seen.insert(name)
                    && ["retained_original", "replaced"]
                        .iter()
                        .any(|s| string_is(&item["role"], s)),
                "invalid_history_import",
            )?;
            required.insert(name.to_owned());
            require(
                evidence
                    .get(name)
                    .is_some_and(|raw| item["sha256"] == s(&sha256(raw))),
                "retained_history_mismatch",
            )?;
        }
    }
    require(
        evidence.keys().cloned().collect::<BTreeSet<_>>() == required,
        "invalid_history_contribution",
    )?;
    let expected = V::Map(Map::from([
        ("version".into(), V::Integer(Integer::new("3")?)),
        ("requires".into(), hm["requires"].clone()),
        ("roots".into(), hm["roots"].clone()),
        ("document".into(), adapted.document().clone()),
        ("scope".into(), m["scope"].clone()),
        ("reasoning".into(), cap),
        (
            "history".into(),
            V::Map(Map::from([
                ("revision".into(), binding["revision"].clone()),
                ("manifest".into(), history_manifest.clone()),
            ])),
        ),
        ("evidence".into(), V::Map(hashes.clone())),
    ]));
    require(
        *revision == s(&expected.digest()?),
        "invalid_history_contribution",
    )?;
    Ok(adapted)
}

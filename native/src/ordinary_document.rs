//! Complete ordinary record loading under the same participant read guards; finite callers use the source_document façade.
use crate::{
    Result,
    history_contract::{self as C, error},
    history_transaction as T, history_transaction_fs as F, history_yaml as Y,
    ordinary_fields as reasoning_fields,
    ordinary_source::{Key as K, Source as S},
    ordinary_value::{Map, Scalar, Value as V, map, string_is, text, truth},
    require,
    source_inventory::{Inventory, absolute, name},
    value::TypedValue as CV,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub(crate) struct Document {
    pub source: S,
    pub hypotheses: V,
    pub origins: BTreeMap<String, BTreeMap<String, PathBuf>>,
    pub members: Vec<PathBuf>,
    pub history: Option<crate::history_capture::Capture>,
    pub history_projection: Option<CV>,
    pub history_view: Option<CV>,
    pub overlay: Option<crate::source_overlay::Overlay<V>>,
}
fn parse(inventory: &mut Inventory, path: &Path) -> Result<S> {
    parse_bytes(&inventory.read(path)?)
}
fn parse_bytes(raw: &[u8]) -> Result<S> {
    let value = Y::decode_full_ordinary_source_value(raw)?;
    Ok(if truth(&value.projected()) {
        value
    } else {
        S::Map(vec![])
    })
}
fn pointers(value: &S, path: &Path) -> Vec<PathBuf> {
    let mut result = vec![];
    for key in ["record", "also"] {
        let values = match value.get(key) {
            Some(v @ S::Scalar(Scalar::Finite(CV::Text(_)))) => vec![v],
            Some(S::List(a)) => a.iter().collect(),
            Some(S::Map(a)) => a.iter().map(|(_, v)| v).collect(),
            _ => vec![],
        };
        for v in values {
            if let S::Scalar(Scalar::Finite(CV::Text(v))) = v
                && (v.ends_with(".yaml") || v.ends_with(".yml"))
            {
                result.push(path.parent().unwrap().join(v));
            }
        }
    }
    result
}
fn expanded(
    paths: &[PathBuf],
    inventory: &mut Inventory,
    literal_existing: bool,
) -> Result<Vec<PathBuf>> {
    let mut out = vec![];
    for path in paths {
        let pattern = if literal_existing && path.exists() {
            crate::source_inventory::escaped(path)?
        } else {
            path.clone()
        };
        let found = inventory.glob(&pattern)?;
        if found.is_empty() {
            out.push(absolute(path)?);
        } else {
            out.extend(found);
        }
    }
    Ok(out)
}
pub(crate) fn members(entries: &[PathBuf], inventory: &mut Inventory) -> Result<Vec<PathBuf>> {
    let mut out = vec![];
    let mut seen = BTreeSet::new();
    let mut pending: Vec<_> = entries.iter().rev().cloned().collect();
    while let Some(path) = pending.pop() {
        let path = absolute(&path)?;
        if seen.contains(&path) || !inventory.exists(&path)? {
            continue;
        }
        seen.insert(path.clone());
        require(seen.len() <= 100_000, "source_limit")?;
        out.push(path.clone());
        pending.extend(pointers(&parse(inventory, &path)?, &path).into_iter().rev());
    }
    Ok(out)
}
fn update(target: &mut Vec<(K, S)>, key: K, value: S) {
    if let Some((_, old)) = target
        .iter_mut()
        .find(|(candidate, _)| candidate.python_eq(&key))
    {
        *old = value;
    } else {
        target.push((key, value));
    }
}

fn validate_structure(value: &S) -> Result<()> {
    let S::Map(top) = value else {
        return Ok(());
    };
    require(
        top.iter().all(|(key, _)| key.text().is_some()),
        "invalid_ordinary_structural_key",
    )?;
    let projected = value.projected();
    let collections = reasoning_fields::collections(&projected)?;
    for section in collections.keys() {
        let Some(S::Map(members)) = value.get(section) else {
            continue;
        };
        require(
            members.iter().all(|(key, _)| key.text().is_some()),
            "invalid_ordinary_structural_key",
        )?;
    }
    Ok(())
}
pub(crate) fn merge(
    target: &mut S,
    incoming: &S,
    path: &Path,
    origins: &mut BTreeMap<String, BTreeMap<String, PathBuf>>,
) -> Result<()> {
    let (S::Map(target), S::Map(incoming)) = (target, incoming) else {
        return Err(error("invalid_schema"));
    };
    for (key, value) in incoming {
        let key_text = key
            .text()
            .ok_or_else(|| error("invalid_ordinary_structural_key"))?;
        if let S::Map(m) = value {
            if !matches!(
                target.iter().find(|(k, _)| k.python_eq(key)),
                Some((_, S::Map(_)))
            ) {
                origins.remove(key_text);
            }
            origins.entry(key_text.to_owned()).or_default().extend(
                m.iter()
                    .filter_map(|(k, _)| k.text().map(|k| (k.to_owned(), path.to_owned()))),
            );
        } else {
            origins.remove(key_text);
        }
        if let Some((_, S::Map(old))) = target.iter_mut().find(|(k, _)| k.python_eq(key))
            && let S::Map(new) = value
        {
            for (field, item) in new {
                if key_text == "meta"
                    && field.text() == Some("prefixes")
                    && let S::Map(add) = item
                    && let Some((_, S::Map(existing))) =
                        old.iter_mut().find(|(k, _)| k.python_eq(field))
                {
                    for (k, v) in add {
                        update(existing, k.clone(), v.clone());
                    }
                    continue;
                }
                update(old, field.clone(), item.clone());
            }
        } else {
            update(target, key.clone(), value.clone());
        }
    }
    Ok(())
}
fn marker(path: &Path, inventory: &mut Inventory) -> Result<Option<V>> {
    let layout = T::Layout::for_entry(name(Path::new(
        path.file_name().ok_or_else(|| error("invalid_path"))?,
    ))?)?;
    let path = path.parent().unwrap().join(layout.authority);
    if !inventory.exists(&path)? {
        return Ok(None);
    }
    let raw = inventory.read(&path)?;
    require(raw.len() <= 1024 * 1024, "history_limit")?;
    let marker = Y::decode_document(&raw)?;
    crate::history_authority::validate_authority(&marker)?;
    Ok(Some(V::from_typed(&marker)))
}
fn physical(directory: &Path, inventory: &mut Inventory) -> Result<V> {
    let mut out = Map::new();
    if !inventory.directory(directory)? {
        return Ok(V::Map(out));
    }
    let pattern = crate::source_inventory::escaped(directory)?;
    let mut paths = inventory.glob(&pattern.join("*.yaml"))?;
    paths.extend(inventory.glob(&pattern.join("*.yml"))?);
    paths.sort();
    for path in paths {
        let key = name(
            path.file_stem()
                .map(Path::new)
                .ok_or_else(|| error("invalid_path"))?,
        )?;
        let mut head = V::Map(Map::new());
        let mut doc = V::Map(Map::new());
        let raw = inventory.read(&path)?;
        let err = (|| -> Result<()> {
            let source = parse_bytes(&raw)?;
            validate_structure(&source)?;
            let value = source.projected();
            let mut body = map(&value)
                .map_err(|_| error("not a mapping of collections"))?
                .clone();
            let h = body.remove("hypothesis").unwrap_or(V::Null);
            if h != V::Null {
                map(&h).map_err(|_| {
                    error("hypothesis: must be a mapping - claim, wrong_if, born, folds")
                })?;
                head = h;
            }
            doc = V::Map(body);
            Ok(())
        })()
        .err();
        if err.as_ref().is_some_and(|e| {
            ["source_limit", "snapshot_changed", "history_limit"].contains(&e.0.as_str())
        }) {
            return Err(err.unwrap());
        }
        out.insert(
            key.into(),
            V::Map(Map::from([
                ("name".into(), V::Text(key.into())),
                ("path".into(), V::Text(name(&path)?.into())),
                ("head".into(), head),
                ("doc".into(), doc),
                (
                    "error".into(),
                    err.and_then(|e| {
                        crate::ordinary_yaml_diagnostic::hypothesis_error(&raw, Some(&e))
                            .map(V::Text)
                    })
                    .unwrap_or(V::Null),
                ),
            ])),
        );
    }
    Ok(V::Map(out))
}
pub(crate) fn load(
    paths: &[PathBuf],
    inventory: &mut Inventory,
    allow_missing: bool,
) -> Result<Document> {
    let entries = expanded(paths, inventory, true)?;
    require(!entries.is_empty(), "invalid_snapshot")?;
    let active = entries
        .iter()
        .map(|p| {
            marker(p, inventory)
                .map(|m| m.is_some_and(|m| string_is(&map(&m).unwrap()["authority"], "history")))
        })
        .collect::<Result<Vec<_>>>()?;
    let has_history = active.iter().any(|v| *v);
    require(
        !has_history || entries.len() == 1,
        "history_composite_capture_unsupported",
    )?;
    // Explicit history selection escapes existing names. The ordinary reader's
    // caller patterns retain its separate glob semantics.
    let entries = if has_history {
        entries
    } else {
        expanded(paths, inventory, false)?
    };
    let first = &entries[0];
    let layout = T::Layout::for_entry(name(Path::new(first.file_name().unwrap()))?)?;
    let hypdir = first.parent().unwrap().join(layout.hypotheses);
    let discovered = if has_history {
        vec![first.clone()]
    } else {
        members(&entries, inventory)?
    };
    let guarded = entries
        .iter()
        .chain(&discovered)
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut roots = guarded
        .iter()
        .filter_map(|p| p.parent())
        .filter(|p| p.is_dir())
        .map(|p| p.canonicalize())
        .collect::<std::io::Result<BTreeSet<_>>>()?;
    if hypdir.is_dir() {
        roots.insert(hypdir.canonicalize()?);
    }
    let _guards = roots
        .iter()
        .map(|p| F::DirectoryGuard::acquire(p, false))
        .collect::<Result<Vec<_>>>()?;
    let guarded = guarded.into_iter().collect::<Vec<_>>();
    F::check_member_journals(&guarded)?;
    require(
        has_history || members(&entries, inventory)? == discovered,
        "concurrent_edit",
    )?;
    let mut document = Document {
        source: S::Map(vec![]),
        hypotheses: V::Map(Map::new()),
        origins: BTreeMap::new(),
        members: discovered,
        history: None,
        history_projection: None,
        history_view: None,
        overlay: None,
    };
    if has_history {
        let store = crate::history_store::Store::new(first)?;
        let capture = store.capture()?;
        let current = capture.document == Y::decode_document(&store.render(&capture)?)?;
        require(
            current || crate::history_store::Store::known_view(&capture)?,
            "unresolved_view_edit",
        )?;
        let adapted = crate::history_adapter::from_store_capture(&capture)?;
        let retained = crate::history_sources::capture(
            &store.root,
            name(Path::new(first.file_name().unwrap()))?,
            adapted.document(),
        )?;
        for (path, raw) in retained.files() {
            inventory.exists(&store.root.join(path))?;
            inventory.retain(&store.root.join(path), raw.clone())?;
        }
        inventory.retain(first, capture.entry_bytes.clone())?;
        inventory.retain(
            &store.root.join(&capture.layout.authority),
            capture.authority_bytes.clone(),
        )?;
        for (path, raw) in &capture.cancellation_bytes {
            inventory.retain(
                &store.root.join(&capture.layout.cancellations).join(path),
                raw.clone(),
            )?;
        }
        for (path, raw) in &capture.storage_bytes {
            inventory.retain(
                &store.root.join(&capture.layout.objects).join(path),
                raw.clone(),
            )?;
        }
        for (path, raw) in &capture.commits {
            inventory.retain(
                &store
                    .root
                    .join(&capture.layout.commits)
                    .join(format!("{path}.yaml")),
                raw.clone(),
            )?;
        }
        // Include inactive generation manifests retained by the same disk capture.
        for generation in capture.inactive_generations.values() {
            for (path, raw) in &generation.commits {
                inventory.retain(
                    &store
                        .root
                        .join(&capture.layout.commits)
                        .join(format!("{path}.yaml")),
                    raw.clone(),
                )?;
            }
        }
        document.source = S::from_typed(adapted.document());
        // Active history projections belong to their authoritative entry for
        // local session attribution. Origins remain outside portable Snapshots.
        document.origins = crate::reasoning_fields::collections(adapted.document())?
            .into_iter()
            .map(|(section, members)| {
                (
                    section,
                    members
                        .into_keys()
                        .map(|id| (id, first.to_owned()))
                        .collect(),
                )
            })
            .collect();
        document.history_projection = Some(adapted.projection().clone());
        document.history_view = Some(
            V::Map(Map::from([
                (
                    "status".into(),
                    V::Text(
                        if current {
                            "current"
                        } else {
                            "stale_generated"
                        }
                        .into(),
                    ),
                ),
                (
                    "baseline_digest".into(),
                    V::Text(crate::reasoning_snapshot::digest(
                        &C::map(&C::map(&capture.document)?["meta"])?["history"],
                    )?),
                ),
            ]))
            .try_typed()?,
        );
        document.history = Some(capture);
    } else {
        let mut seen = BTreeSet::new();
        for entry in &entries {
            let mut pending = vec![entry.clone()];
            while let Some(path) = pending.pop() {
                let path = absolute(&path)?;
                if !inventory.exists(&path)? {
                    require(allow_missing, "missing_record")?;
                    let S::Map(source) = &mut document.source else {
                        unreachable!()
                    };
                    update(source, K::text_key("meta"), S::Map(vec![]));
                    continue;
                }
                let value = parse(inventory, &path)?;
                validate_structure(&value)?;
                let typed = value.projected();
                let declared = map(&typed)?
                    .get("meta")
                    .and_then(|m| map(m).ok())
                    .is_some_and(|m| m.contains_key("history"));
                let authority = marker(&path, inventory)?;
                require(
                    !declared || authority.is_some(),
                    "missing_history_authority",
                )?;
                require(
                    !authority
                        .as_ref()
                        .is_some_and(|m| string_is(&map(m).unwrap()["authority"], "history")),
                    "history_reader_unsupported",
                )?;
                require(!declared, "authority_mismatch")?;
                reasoning_fields::capabilities(&typed, None)?;
                seen.insert(path.clone());
                require(seen.len() <= 100_000, "source_limit")?;
                merge(&mut document.source, &value, &path, &mut document.origins)?;
                let mut follow = vec![];
                for pointer in pointers(&value, &path) {
                    let pointer = absolute(&pointer)?;
                    if !seen.contains(&pointer) && inventory.exists(&pointer)? {
                        follow.push(pointer);
                    }
                }
                // A sibling may point back to another queued sibling. Check again
                // when scheduling below by processing a single depth-first frame.
                pending.extend(follow.into_iter().rev());
                while pending
                    .last()
                    .is_some_and(|p| absolute(p).ok().is_some_and(|p| seen.contains(&p)))
                {
                    pending.pop();
                }
            }
        }
    }
    document.hypotheses = physical(&hypdir, inventory)?;
    if let Some(capture) = &document.history {
        let typed_doc = document.source.strict_typed()?;
        let doc = V::from_typed(&typed_doc);
        if let Some(imported) = map(&doc)?
            .get("meta")
            .and_then(|m| map(m).ok())
            .and_then(|m| m.get("history_hypothesis_import"))
            .filter(|v| **v != V::Null)
        {
            crate::history_hypothesis_import::validate_mapping(
                &imported.try_typed()?,
                name(Path::new(first.file_name().unwrap()))?,
            )?;
            let V::Map(hypotheses) = &mut document.hypotheses else {
                unreachable!()
            };
            for item in crate::ordinary_value::list(&map(imported)?["physical"])? {
                let item = map(item)?;
                let key = text(&item["name"])?;
                let path = F::target(&capture.root, text(&item["path"])?)?;
                let hyp = hypotheses
                    .get(key)
                    .ok_or_else(|| error("missing_imported_hypothesis"))?;
                require(
                    Path::new(text(&map(hyp)?["path"])?).canonicalize()? == path,
                    "missing_imported_hypothesis",
                )?;
                require(
                    crate::identity::sha256(&inventory.read(&path)?) == text(&item["sha256"])?,
                    "imported_hypothesis_changed",
                )?;
                hypotheses.remove(key);
            }
        }
        let (named, _) = crate::history_hypotheses::layers(
            document.history_projection.as_ref().unwrap(),
            &typed_doc,
        )?;
        let named = V::from_typed(&named);
        let V::Map(physical) = &mut document.hypotheses else {
            unreachable!()
        };
        for (key, hyp) in map(&named)? {
            require(
                !physical.contains_key(key),
                "hypothesis_authority_collision",
            )?;
            physical.insert(key.clone(), hyp.clone());
        }
        for hyp in physical.values() {
            let hyp = map(hyp)?;
            let body = hyp
                .get("doc")
                .or_else(|| hyp.get("document"))
                .ok_or_else(|| error("invalid_snapshot"))?;
            reasoning_fields::capabilities(body, None)?;
            require(
                !map(body)?
                    .get("meta")
                    .and_then(|m| map(m).ok())
                    .is_some_and(|m| m.contains_key("history")),
                "history_hypothesis_unsupported",
            )?;
        }
    }
    F::check_member_journals(&guarded)?;
    Ok(document)
}

impl Document {
    pub(crate) fn try_finite(self) -> Result<crate::source_document::Document> {
        // Canonical consumers discard overlay display metadata, but must still
        // refuse an overlay snapshot outside their finite value domain.
        self.overlay
            .map(|overlay| overlay.try_finite())
            .transpose()?;
        Ok(crate::source_document::Document {
            source: self.source.try_finite()?,
            hypotheses: self.hypotheses.finite_projection()?,
            origins: self.origins,
            members: self.members,
            history: self.history,
        })
    }
}

//! Named ordinary proposals use the same admission and exact-image transaction.
use super::*;
use crate::history_view::truth;

fn document_of(hypothesis: &V) -> Result<V> {
    let h = map(hypothesis)?;
    Ok(h.get("document")
        .or_else(|| h.get("doc"))
        .cloned()
        .unwrap_or_else(|| V::Map(Map::new())))
}
pub(super) fn normalize_groups(value: &V) -> Result<Map> {
    map(value)?
        .iter()
        .map(|(name, h)| {
            let mut body = map(h)?.clone();
            body.insert("document".into(), document_of(h)?);
            Ok((name.clone(), V::Map(body)))
        })
        .collect()
}
fn layered(base: &V, groups: &Map, name: &str) -> Result<V> {
    crate::history_hypothesis_authoring::layer(base, &V::Map(groups.clone()), &[name.into()])
}
fn parsed_hypothesis(raw: &[u8]) -> Result<(V, V)> {
    let source = crate::history_yaml::decode_ordinary_source_value(raw)?;
    let mut doc = map(&source.projected())?.clone();
    let head = doc
        .remove("hypothesis")
        .unwrap_or_else(|| V::Map(Map::new()));
    map(&head)?;
    Ok((head, V::Map(doc)))
}
fn carry(
    document: &source_document::Document,
    inventory: &Inventory,
    id: &str,
) -> Result<(String, Vec<String>)> {
    for path in &document.members {
        let text = std::str::from_utf8(
            inventory
                .files
                .get(path)
                .ok_or_else(|| error("snapshot_changed"))?,
        )
        .map_err(|_| error("invalid_history_yaml"))?;
        let lines = text.split('\n').map(str::to_owned).collect::<Vec<_>>();
        if let Some((collection, member)) = locate(&lines, id) {
            let mut block = lines[member.start..member.end].to_vec();
            while block.last().is_some_and(|line| line.trim().is_empty()) {
                block.pop();
            }
            return Ok((collection.name, block));
        }
    }
    Err(error(&format!(
        "refused - no file of the record holds {id}"
    )))
}
fn insert_block(
    lines: &mut Vec<String>,
    collection: &str,
    id: &str,
    block: Vec<String>,
) -> Result<String> {
    ensure_collection(lines, collection);
    let group = collections(lines)
        .into_iter()
        .find(|c| c.name == collection)
        .ok_or_else(|| error("invalid_hypothesis_collection"))?;
    let entries = members(lines, &group);
    let target_indent = entries.first().map_or(2, |m| m.indent);
    let old_indent = block.first().map_or(0, |s| indent(s));
    let block = block
        .into_iter()
        .map(|line| {
            if line.trim().is_empty() {
                line
            } else {
                format!(
                    "{}{}",
                    " ".repeat(
                        indent(&line)
                            .saturating_add(target_indent)
                            .saturating_sub(old_indent)
                    ),
                    line.trim_start_matches(' ')
                )
            }
        })
        .collect::<Vec<_>>();
    if entries.is_empty() {
        lines.splice(group.start + 1..group.start + 1, block);
        return Ok(format!("{id} into {collection}, its first entry"));
    }
    let best = entries
        .iter()
        .map(|m| common_prefix(&m.name, id))
        .max()
        .unwrap_or(0);
    let neighbors = entries
        .iter()
        .filter(|m| best == 0 || common_prefix(&m.name, id) == best)
        .collect::<Vec<_>>();
    let (anchor, before) = neighbors
        .iter()
        .find(|m| m.name.as_str() > id)
        .map(|m| (*m, true))
        .unwrap_or((*neighbors.last().unwrap(), false));
    let at = if before { anchor.start } else { anchor.end };
    lines.splice(at..at, block);
    Ok(format!(
        "{id} into {collection}, {} {}",
        if before { "before" } else { "after" },
        anchor.name
    ))
}
fn snapshot_order(
    reader: &Reader<'_>,
    body: &Map,
    seen: &Map,
    base: &crate::history_yaml::OrdinaryValue,
    hyp: &crate::history_yaml::OrdinaryValue,
) -> Result<Source> {
    let mut source = base.clone();
    if let (
        crate::history_yaml::OrdinaryValue::Map(base),
        crate::history_yaml::OrdinaryValue::Map(hyp),
    ) = (&mut source, hyp)
    {
        // The snapshot lens only looks up entry bodies; the hypothesis must win.
        let mut layers = hyp
            .iter()
            .filter(|(key, _)| key.text() != Some("hypothesis"))
            .cloned()
            .collect::<Vec<_>>();
        layers.append(base);
        *base = layers;
    }
    ordered_snapshot(reader, body, seen, &source)
}
fn hypothesis_members(root: &Path, directory: &Path, inventory: &mut Inventory) -> Result<Map> {
    let mut result = Map::new();
    if !inventory.directory(directory)? {
        return Ok(result);
    }
    let pattern = crate::source_inventory::escaped(directory)?;
    let mut paths = inventory.glob(&pattern.join("*.yaml"))?;
    paths.extend(inventory.glob(&pattern.join("*.yml"))?);
    for path in paths {
        result.insert(
            relative(root, &path)?,
            s(&crate::identity::sha256(&inventory.read(&path)?)),
        );
    }
    Ok(result)
}

pub(super) fn prepare(
    action: &V,
    route: &WriteRoute,
    source_body: Option<&Source>,
) -> Result<Preparation> {
    prepare_mode(action, route, source_body, false)
}

/// An Advanced project's local ordinary record keeps its named proposals
/// beside it, as a Simple one does; only the Simple-mode routing gate differs.
pub(super) fn prepare_advanced_local(
    action: &V,
    route: &WriteRoute,
    source_body: Option<&Source>,
) -> Result<Preparation> {
    prepare_mode(action, route, source_body, true)
}

fn prepare_mode(
    action: &V,
    route: &WriteRoute,
    source_body: Option<&Source>,
    advanced_local: bool,
) -> Result<Preparation> {
    let input = map(action)?;
    let group = text(field(input, "hypothesis")?)?.to_owned();
    hypothesis_name(&s(&group))?;
    require(
        string_is(&map(route.config())?["mode"], "simple")
            || advanced_local && route.pending_required()?,
        "legacy_authoring_requires_simple_project",
    )?;
    let entry = route
        .paths()
        .first()
        .ok_or_else(|| error("missing_record_path"))?;
    let mut inventory = Inventory::default();
    let document = source_document::load(route.paths(), &mut inventory, false)?;
    require(
        document.history.is_none(),
        "history_direct_writer_unsupported: use the history writer",
    )?;
    let base = projected_document(&document.source);
    let runtime = crate::public_workspace::runtime_for_document(&base)?;
    let mut groups = normalize_groups(&document.hypotheses)?;
    let directory = entry
        .parent()
        .unwrap()
        .join(entry_layout(entry)?.hypotheses);
    let fresh = !groups.contains_key(&group);
    let target = if let Some(h) = groups.get(&group) {
        let h = map(h)?;
        require(
            !h.get("error").is_some_and(truth),
            &format!(
                "refused - hypothesis {group} could not be read: {}",
                h.get("error").map(display).transpose()?.unwrap_or_default()
            ),
        )?;
        PathBuf::from(text(field(h, "path")?)?)
    } else {
        directory.join(format!("{group}.yaml"))
    };
    let before = if inventory.exists(&target)? {
        Some(inventory.read(&target)?)
    } else {
        None
    };
    if fresh {
        groups.insert(
            group.clone(),
            object([
                ("document", V::Map(Map::new())),
                ("doc", V::Map(Map::new())),
                ("head", V::Map(Map::new())),
                ("error", V::Null),
                ("path", s(name(&target)?)),
            ]),
        );
    }
    let original_hyp = document_of(&groups[&group])?;
    let under = layered(&base, &groups, &group)?;
    let mut reader =
        Reader::new(&under, runtime.as_ref())?.with_layers(groups.clone(), BTreeSet::new())?;
    reader.for_action(action)?;
    let (normalized, notes) = reader.normalize(action)?;
    let notice = nearest_notice(&reader, &normalized, &document, &inventory)?;
    let mut action = map(&normalized)?.clone();
    let stamp = action_stamp(&action);
    let kind = text(field(&action, "kind")?)?.to_owned();
    let id = text(field(&action, "id")?)?.to_owned();
    let mut dated = action.clone();
    if dated.get("as_of").is_none_or(|v| *v == V::Null) {
        dated.insert("as_of".into(), s(&stamp));
    }
    let mut refusals = reader.validate(&V::Map(dated))?;
    refusals.extend(notice.refusals);
    require(
        refusals.is_empty(),
        &format!("refused - {}", refusals.join("\n          ")),
    )?;
    let original_entries = crate::reasoning_snapshot::entries(&original_hyp)?;
    let deps = text(&reader.fields()["deps"])?;
    let snapshot = reader.snapshot_field()?;
    if kind == "review" {
        require(
            original_entries.contains_key(&id),
            &format!(
                "refused - {id} is not in hypothesis {group}{}",
                if reader
                    .raw()
                    .get(&id)
                    .is_some_and(|b| map(b).is_ok_and(|m| m.contains_key(deps)))
                {
                    " - review it in the base, or propose it here with add"
                } else {
                    ""
                }
            ),
        )?;
    }
    let mut candidate_action = action.clone();
    candidate_action.insert("as_of".into(), s(&stamp));
    // A declared scope routed this write into the proposal; a named set writes
    // only the value, its day, reason and citation.
    candidate_action.remove("_record_scope");
    let candidate = if kind == "review" {
        let body = map(&reader.raw()[&id])?;
        let seen = dependency_snapshot(&reader, body)?;
        update_review_candidate(
            &under,
            &original_entries[&id].0,
            &id,
            snapshot,
            &seen,
            body.contains_key("reviewed").then_some(stamp.as_str()),
        )?
    } else {
        reader.candidate(&V::Map(candidate_action))?
    };
    if let Some(draft) =
        Privacy::candidate_draft(route.project(), &V::Map(action.clone()), &candidate)?
    {
        return Ok(Preparation::Draft {
            output: format!("{}\n", crate::public_core_readers::json_value(&draft)?),
            inventory,
        });
    }
    if kind == "set"
        && original_entries.contains_key(&id)
        && action.get("as_of").is_none_or(|v| *v == V::Null)
        && action.get("source").is_none_or(|v| *v == V::Null)
        && reader.value(&id)? != V::Null
        && same_legacy(&reader.value(&id)?, field(&action, "value")?)
    {
        return Ok(Preparation::Draft {
            output: format!(
                "{id} is already {} in hypothesis {group}; nothing written\n",
                scalar(field(&action, "value")?, Style::Bare)?
            ),
            inventory,
        });
    }
    let initial = before
        .as_ref()
        .map(|b| String::from_utf8(b.clone()))
        .transpose()
        .map_err(|_| error("invalid_history_yaml"))?
        .unwrap_or_else(|| format!("hypothesis: {{born: \"{stamp}\"}}\n"));
    let hyp_source = crate::history_yaml::decode_ordinary_source_value(initial.as_bytes())?;
    let mut lines = initial.split('\n').map(str::to_owned).collect::<Vec<_>>();
    let mut output = notice.text.lines().map(str::to_owned).collect::<Vec<_>>();
    output.extend(notes.clone());
    let collection;
    match kind.as_str() {
        "set" => {
            if !original_entries.contains_key(&id) {
                let (col, block) = carry(&document, &inventory, &id)?;
                output.push(format!(
                    "carry {} of hypothesis {group}",
                    insert_block(&mut lines, &col, &id, block)?
                ));
            }
            collection = locate(&lines, &id)
                .ok_or_else(|| error("missing_hypothesis_entry"))?
                .0
                .name;
            let old = set_entry(
                &mut lines,
                &id,
                field(&action, "value")?,
                &stamp,
                action.get("why").and_then(|v| text(v).ok()),
                action.get("source").and_then(|v| text(v).ok()),
                action.get("at").and_then(|v| text(v).ok()),
            )?;
            output.push(format!(
                "set {id} in hypothesis {group}: {old} -> {} (as of {stamp})",
                scalar(field(&action, "value")?, Style::Bare)?
            ));
        }
        "add" => {
            let mut body = field(&action, "body")?.clone();
            let mut ordered = preserve_order(&body, source_body);
            if let Some(scope) = map(&body).ok().and_then(|m| m.get("scope"))
                && let Source::Map(fields) = &mut ordered
                && let Some((_, value)) = fields.iter_mut().find(|(key, _)| key == "scope")
            {
                // Only a routed scope is missing from the authored body, and
                // routing declares it as a mapping; an authored scope is kept.
                *value = match source_body.and_then(|body| body.get("scope")) {
                    Some(written) => written.clone(),
                    None => ordered_scope(scope)?,
                };
            }
            if let Ok(m) = map(&body)
                && m.contains_key(deps)
            {
                let seen = dependency_snapshot(&reader, m)?;
                let order = snapshot_order(&reader, m, &seen, &document.source, &hyp_source)?;
                map_mut(&mut body)?.insert(snapshot.into(), V::Map(seen));
                if let Source::Map(m) = &mut ordered {
                    if let Some((_, value)) = m.iter_mut().find(|(key, _)| key == snapshot) {
                        *value = order;
                    } else {
                        m.push((snapshot.into(), order));
                    }
                }
            }
            action.insert("body".into(), body);
            collection = collection_for(&reader, &action)?;
            ensure_collection(&mut lines, &collection);
            output.push(format!(
                "add {} of hypothesis {group}",
                insert_entry(&mut lines, &collection, &id, &ordered)?
            ));
        }
        "review" => {
            collection = original_entries[&id].0.clone();
            let body = map(&reader.raw()[&id])?;
            let old = body
                .get(snapshot)
                .and_then(|v| map(v).ok())
                .cloned()
                .unwrap_or_default();
            let seen = dependency_snapshot(&reader, body)?;
            let order = snapshot_order(&reader, body, &seen, &document.source, &hyp_source)?;
            let changed = old.len() != seen.len()
                || old
                    .iter()
                    .any(|(k, v)| seen.get(k).is_none_or(|n| !same_legacy(v, n)));
            let (_, member) =
                locate(&lines, &id).ok_or_else(|| error("missing_hypothesis_entry"))?;
            if changed {
                replace_field_ordered(&mut lines, &member, snapshot, &order)?;
            }
            let (_, member) = locate(&lines, &id).unwrap();
            if inline(&lines[member.start]).starts_with('{') {
                if body.contains_key("reviewed") {
                    in_braces(&mut lines, &member, "reviewed", "", None, Some(&stamp))?;
                }
            } else if field_span(&lines, &member, "reviewed").is_some() {
                replace_date_field(&mut lines, &member, "reviewed", &stamp)?;
            }
            output.push(format!(
                "review {id} in hypothesis {group}: {} ({stamp})",
                if changed {
                    "seen rewritten from what the record holds under it"
                } else {
                    "what it saw is what the record holds under it"
                }
            ));
            for dependency in body[deps].clone().into_list()? {
                let d = text(&dependency)?;
                match (old.get(d), seen.get(d)) {
                    (Some(a), Some(b)) if !same_legacy(a, b) => {
                        let (a, b) = crate::public_ordinary_readers::apart(a, b, 40);
                        output.push(format!("  {d}: {a} -> {b}"));
                    }
                    (None, Some(b)) => output.push(format!(
                        "  {d}: {} (never checked against it before)",
                        crate::public_ordinary_readers::short(b, 40)
                    )),
                    _ => {}
                }
            }
        }
        _ => return Err(error("unsupported_named_action")),
    }
    let after = lines.join("\n").into_bytes();
    let (head, after_doc) = parsed_hypothesis(&after)?;
    let expected_head = if fresh {
        object([("born", s(&stamp))])
    } else {
        map(&groups[&group])?["head"].clone()
    };
    require(
        authored_equal(&head, &expected_head),
        "the write broke the hypothesis and was undone: head changed",
    )?;
    verify_untouched_document(&original_hyp, &after_doc, &collection, &id)?;
    let after_entries = crate::reasoning_snapshot::entries(&after_doc)?;
    require(
        after_entries.contains_key(&id),
        "the write broke the hypothesis and was undone: entry missing",
    )?;
    let expected_body = if kind == "add" {
        field(&action, "body")?.clone()
    } else {
        crate::reasoning_snapshot::entries(&candidate)?[&id]
            .1
            .clone()
    };
    require(
        authored_equal(&after_entries[&id].1, &expected_body),
        "the write broke the hypothesis and was undone: entry changed meaning",
    )?;
    let h = map_mut(groups.get_mut(&group).unwrap())?;
    h.insert("document".into(), after_doc.clone());
    h.insert("doc".into(), after_doc);
    let after_under = layered(&base, &groups, &group)?;
    let after_reader = Reader::new(&after_under, runtime.as_ref())?
        .with_layers(groups.clone(), BTreeSet::new())?;
    if kind == "set" {
        require(
            same_legacy(&after_reader.value(&id)?, field(&action, "value")?),
            "the write broke the hypothesis and was undone: value changed",
        )?;
    }
    let projected = crate::public_ordinary_readers::Projection::new(
        &after_under,
        &groups,
        &Map::new(),
        vec![],
        runtime.as_ref(),
    )?;
    output.extend(crate::ordinary_write_report::render(
        &kind,
        &id,
        &projected.base,
        Some(&group),
        &crate::ordinary_write_report::Ancillary::default(),
    )?);
    let judgments = after_entries
        .values()
        .filter(|(_, b)| map(b).is_ok_and(|m| m.contains_key(deps)))
        .count();
    let entries = after_entries.len() - judgments;
    output.push(String::new());
    output.push(format!(
        "the base is untouched; {group} holds {entries} entr{} and {judgments} judgment{}{}",
        if entries == 1 { "y" } else { "ies" },
        if judgments == 1 { "" } else { "s" },
        if map(&head)?
            .get("folds")
            .is_some_and(|v| string_is(v, "never"))
        {
            ", and never folds"
        } else {
            ""
        }
    ));
    let root = common_root(entry, &document.members)?;
    let entry_relative = relative(&root, entry)?;
    let mut hypotheses = hypothesis_members(&root, &directory, &mut inventory)?;
    hypotheses
        .entry(relative(&root, &target)?)
        .or_insert(V::Null);
    let record_members = document
        .members
        .iter()
        .map(|p| {
            Ok((
                relative(&root, p)?,
                s(&crate::identity::sha256(
                    inventory
                        .files
                        .get(p)
                        .ok_or_else(|| error("snapshot_changed"))?,
                )),
            ))
        })
        .collect::<Result<Map>>()?;
    let baseline = object([
        ("kind", s("direct/v1")),
        ("transaction_root", s(name(&absolute(&root)?)?)),
        ("record_members", V::Map(record_members)),
        ("policy", route.config().clone()),
        ("hypothesis_members", V::Map(hypotheses)),
        ("hypotheses_directory", s(&relative(&root, &directory)?)),
    ]);
    let capabilities = crate::reasoning_fields::capabilities(&base, None)?;
    let evidence = object([(
        "source_sha256",
        s(&crate::identity::sha256(&base.canonical_bytes()?)),
    )]);
    let receipt = T::semantic_receipt(
        text(&map(&capabilities)?["profile"])?,
        &capabilities,
        &evidence,
        &evidence,
    )?;
    let mutation = PreparedMutation::prepare(
        &crate::public_history::fresh_id("direct")?,
        &authority(entry)?,
        &baseline,
        vec![FileImage {
            path: relative(&root, &target)?,
            role: "hypothesis".into(),
            before,
            after: Some(after),
        }],
        &receipt,
        &entry_relative,
        None,
    )?;
    Ok(Preparation::Mutation(Prepared {
        inventory,
        mutation,
        output: format!("{}\n", output.join("\n")),
        root,
        journal: entry_layout(Path::new(&entry_relative))?.journal,
        subject: id,
        diagnostics: notes,
        page: None,
    }))
}

pub(super) fn verify_recovery(
    route: &WriteRoute,
    root: &Path,
    mutation: &PreparedMutation,
    baseline: &Map,
) -> Result<()> {
    let Some(expected) = baseline.get("hypothesis_members") else {
        return Ok(());
    };
    require(
        baseline.get("policy") == Some(route.config()),
        "project_route_changed",
    )?;
    let expected = map(expected)?;
    let directory = root.join(text(field(baseline, "hypotheses_directory")?)?);
    let mut inventory = Inventory::default();
    let actual = hypothesis_members(root, &directory, &mut inventory)?;
    require(
        actual.keys().all(|p| expected.contains_key(p)),
        "concurrent_hypothesis_edit",
    )?;
    for (path, digest) in expected {
        if let Some(image) = mutation.files().iter().find(|i| i.path == *path) {
            let current = F::read(&root.join(path))?;
            require(
                current == image.before || current == image.after,
                "concurrent_hypothesis_edit",
            )?;
        } else {
            require(
                actual.get(path) == Some(digest),
                "concurrent_hypothesis_edit",
            )?;
        }
    }
    inventory.verify()
}

// A newly named proposal may need its own directory. Verify the untouched source
// closure before creating it, then retain that exact directory observation for the
// locked publication check. Hypothesis membership is checked again separately.
pub(super) fn prepare_directories(prepared: &Prepared) -> Result<Inventory> {
    let mut inventory = prepared.inventory.clone();
    let data = prepared.mutation.to_data();
    let baseline = map(field(map(&data)?, "baseline")?)?;
    let Some(directory) = baseline.get("hypotheses_directory") else {
        return Ok(inventory);
    };
    prepared.inventory.verify()?;
    let checked_directory = F::target(&prepared.root, text(directory)?)?;
    let directory = absolute(&prepared.root.join(text(directory)?))?;
    for image in prepared.mutation.files() {
        require(
            F::target(&prepared.root, &image.path)?.parent() == Some(checked_directory.as_path()),
            "invalid_hypothesis_directory",
        )?;
    }
    let key = ("directory".into(), absolute(&directory)?);
    if inventory.events.get(&key) == Some(&crate::source_inventory::Observation::Directory(false)) {
        // Refuse a concurrent creation; only our planned directory may change.
        require(!directory.exists(), "snapshot_changed")?;
        std::fs::create_dir_all(&directory)?;
        require(
            !std::fs::symlink_metadata(&directory)?
                .file_type()
                .is_symlink(),
            "snapshot_changed",
        )?;
        inventory
            .events
            .insert(key, crate::source_inventory::Observation::Directory(true));
    }
    inventory.verify()?;
    Ok(inventory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    const BASE: &str = "known:\n  p.x: {v: 1, of: 2026-09-01, note: preserve}\njudgments:\n  d.keep: {verdict: keep, rests_on: [p.x], seen: {p.x: 1}, wrong_if: p.x > 9}\n";
    fn fixture(legacy: bool, existing: bool) -> (tempfile::TempDir, PathBuf, WriteRoute, Prepared) {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join(if legacy {
            "PROVENANCE.yaml"
        } else {
            "GROUNDING.yaml"
        });
        fs::write(&entry, BASE).unwrap();
        if existing {
            let hyp = entry
                .parent()
                .unwrap()
                .join(entry_layout(&entry).unwrap().hypotheses)
                .join("alpha.yaml");
            fs::create_dir_all(hyp.parent().unwrap()).unwrap();
            fs::write(hyp, "hypothesis: {born: 2026-09-01, claim: preserve}\nknown:\n  p.x: {v: 3, of: 2026-09-01, note: preserve}\n").unwrap();
        }
        let route = WriteRoute::capture(std::slice::from_ref(&entry), temp.path()).unwrap();
        let action = object([
            ("kind", s("set")),
            ("id", s("p.x")),
            ("value", n("4")),
            ("as_of", s("2026-09-19")),
            ("hypothesis", s("alpha")),
            ("why", V::Null),
            ("source", V::Null),
            ("at", V::Null),
        ]);
        let Preparation::Mutation(prepared) = prepare(&action, &route, None).unwrap() else {
            panic!("expected mutation")
        };
        (temp, entry, route, prepared)
    }
    fn interrupt(prepared: &Prepared, route: &WriteRoute) {
        let inventory = prepare_directories(prepared).unwrap();
        let mut verify = |_: &V| verify_prepared(prepared, route, &inventory);
        let mut stop = |_: &V| Err(error("retained_after_write"));
        assert_eq!(
            F::publish_legacy(
                &prepared.root,
                &prepared.journal,
                &prepared.mutation,
                &mut verify,
                Some(&mut stop)
            )
            .unwrap_err()
            .0,
            "retained_after_write"
        );
    }
    #[test]
    fn named_images_recover_and_rollback_with_untouched_base() {
        for legacy in [false, true] {
            for existing in [false, true] {
                for rollback in [false, true] {
                    for old_image in [false, true] {
                        let (temp, entry, route, prepared) = fixture(legacy, existing);
                        interrupt(&prepared, &route);
                        let image = &prepared.mutation.files()[0];
                        let target = prepared.root.join(&image.path);
                        if old_image {
                            if let Some(before) = &image.before {
                                fs::write(&target, before).unwrap();
                            } else {
                                fs::remove_file(&target).unwrap();
                            }
                        }
                        drop(route);
                        super::super::recover(std::slice::from_ref(&entry), temp.path(), rollback)
                            .unwrap();
                        assert_eq!(
                            F::read(&target).unwrap(),
                            if rollback {
                                image.before.clone()
                            } else {
                                image.after.clone()
                            }
                        );
                        assert_eq!(fs::read_to_string(&entry).unwrap(), BASE);
                        assert!(!prepared.root.join(&prepared.journal).exists());
                    }
                }
            }
        }
    }
    #[test]
    fn named_publication_refuses_changed_inputs_without_overwriting_them() {
        for change in ["record", "hypothesis", "sibling", "policy"] {
            let (temp, entry, route, prepared) = fixture(false, true);
            let target = prepared.root.join(&prepared.mutation.files()[0].path);
            let before = fs::read(&target).unwrap();
            let changed = match change {
                "record" => entry.clone(),
                "hypothesis" => target.clone(),
                "sibling" => target.parent().unwrap().join("other.yaml"),
                _ => route.project().config_path.clone(),
            };
            fs::create_dir_all(changed.parent().unwrap()).unwrap();
            let bytes = if change == "policy" {
                br#"{"mode":"advanced"}"#.to_vec()
            } else {
                b"known: {p.user: {v: preserve}}\n".to_vec()
            };
            fs::write(&changed, &bytes).unwrap();
            assert!(publish(prepared, &route).is_err(), "{change}");
            assert_eq!(fs::read(&changed).unwrap(), bytes);
            if change != "hypothesis" {
                assert_eq!(fs::read(&target).unwrap(), before);
            }
            let _ = temp;
        }
    }
    #[test]
    fn recovery_retains_journal_when_policy_or_hypothesis_membership_changes() {
        for change in ["record", "hypothesis", "sibling", "policy"] {
            let (temp, entry, route, prepared) = fixture(false, true);
            interrupt(&prepared, &route);
            let journal = prepared.root.join(&prepared.journal);
            let journal_before = fs::read(&journal).unwrap();
            let target = prepared.root.join(&prepared.mutation.files()[0].path);
            let changed = match change {
                "record" => entry.clone(),
                "hypothesis" => target.clone(),
                "sibling" => target.parent().unwrap().join("other.yaml"),
                _ => route.project().config_path.clone(),
            };
            fs::create_dir_all(changed.parent().unwrap()).unwrap();
            let bytes = if change == "policy" {
                br#"{"mode":"advanced"}"#.to_vec()
            } else {
                b"known: {p.user: {v: preserve}}\n".to_vec()
            };
            fs::write(&changed, &bytes).unwrap();
            drop(route);
            assert!(
                super::super::recover(std::slice::from_ref(&entry), temp.path(), false).is_err(),
                "{change}"
            );
            assert_eq!(fs::read(&changed).unwrap(), bytes);
            assert_eq!(fs::read(&journal).unwrap(), journal_before);
        }
    }
}

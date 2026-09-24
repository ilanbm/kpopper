//! Ordinary identity edits operate on captured text and preserve source layout.
use crate::{
    Result,
    history_authoring::{obj, s},
    history_contract::{Map, error, map, text},
    history_identity_rewrite::{expression, tokens},
    history_transaction::{self as T, FileImage, PreparedMutation},
    history_transaction_fs as F,
    history_yaml::{OrdinaryValue as O, SourceValue as Source},
    project_modes::WriteRoute,
    public_ordinary_readers::Projection,
    reasoning_runtime::Runtime,
    require,
    source_inventory::{Inventory, absolute, name},
    value::TypedValue as V,
};
use libyaml_safer::{Event, EventData as E, Parser, ScalarStyle};
use regex::Regex;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::LazyLock,
};

static TOP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*):(?: |$)").unwrap());
static MEMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^( +)([A-Za-z_][A-Za-z0-9_.]*):(?: |$)").unwrap());
static FLOW: &str = r#"("(?:[^"\\]|\\.)*"|'(?:[^']|'')*'|[^,}\n]+)"#;

/// Use the public record check for both migration admission and its report.
/// Optional page coverage belongs to the Hub, not the record's declared count.
fn record_check(source: &V, hypotheses: &Map, runtime: Option<&Runtime>) -> Result<String> {
    use crate::ordinary_value::{Map as OrdinaryMap, Value};
    let hypotheses = hypotheses
        .iter()
        .map(|(name, value)| (name.clone(), Value::from_typed(value)))
        .collect();
    crate::ordinary_views::Projection::new(
        &Value::from_typed(source),
        &hypotheses,
        &OrdinaryMap::new(),
        vec![],
        runtime,
    )?
    .check(None)
    .map(|(text, _)| text)
}

struct Node {
    start: usize,
    end: usize,
    tag: Option<String>,
    kind: Kind,
}

fn raw_of(document: &V) -> Result<Map> {
    Ok(crate::reasoning_fields::collections(document)?
        .into_values()
        .flatten()
        .collect())
}
fn ordered(source: &O) -> Result<Source> {
    Ok(match source {
        O::Scalar(v) => Source::Scalar(v.clone()),
        O::List(a) => Source::List(a.iter().map(ordered).collect::<Result<_>>()?),
        O::Map(m) => Source::Map(
            m.iter()
                .map(|(k, v)| {
                    Ok((
                        k.text()
                            .ok_or_else(|| {
                                error(
                                    "invalid_history_value: snapshot mappings require string keys",
                                )
                            })?
                            .to_owned(),
                        ordered(v)?,
                    ))
                })
                .collect::<Result<_>>()?,
        ),
    })
}
fn source_body<'a>(source: &'a O, id: &str) -> Option<&'a O> {
    if let O::Map(collections) = source {
        collections.iter().find_map(|(_, entries)| entries.get(id))
    } else {
        None
    }
}
fn retirement(
    worlds: &[(Option<String>, Map)],
    sources: &super::ordinary_sameness::Sources<'_>,
    live: &BTreeSet<String>,
    deps: &str,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if deps == "also" {
        return out;
    }
    let idish = Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)*$").unwrap();
    for (name, raw) in worlds {
        let source = name
            .as_ref()
            .and_then(|n| sources.hypotheses.get(n).copied())
            .or_else(|| if name.is_none() { sources.base } else { None });
        let mut ids = vec![];
        if let Some(O::Map(collections)) = source {
            for (_, entries) in collections {
                if let O::Map(entries) = entries {
                    for (id, _) in entries {
                        if let Some(id) = id.text().filter(|id| raw.contains_key(*id))
                            && !ids.contains(&id)
                        {
                            ids.push(id);
                        }
                    }
                }
            }
        }
        for id in raw.keys() {
            if !ids.contains(&id.as_str()) {
                ids.push(id);
            }
        }
        for id in ids {
            let aliases = match get(&raw[id], "also") {
                V::Text(v) => vec![v.as_str()],
                V::List(a) => a.iter().filter_map(|v| text(v).ok()).collect(),
                _ => vec![],
            };
            for alias in aliases {
                if idish.is_match(alias) && !live.contains(alias) && alias != id {
                    out.entry(alias.into()).or_insert_with(|| id.into());
                }
            }
        }
    }
    out
}
fn declared_distinct(worlds: &[(Option<String>, Map)], a: &str, b: &str) -> bool {
    worlds.iter().any(|(_, raw)| {
        raw.iter().any(|(id, body)| {
            (id == a && ids_value(get(body, "distinct_from")).contains(&b.to_owned()))
                || (id == b && ids_value(get(body, "distinct_from")).contains(&a.to_owned()))
        })
    })
}
fn relative(root: &Path, path: &Path) -> Result<String> {
    let path = absolute(path)?;
    let root = absolute(root)?;
    let value = name(
        path.strip_prefix(&root)
            .map_err(|_| error("invalid_path"))?,
    )?
    .replace('\\', "/");
    crate::history_authority::relative_path(&value)?;
    Ok(value)
}
fn notice_path(path: &Path, base: &Path) -> String {
    let from = base.components().collect::<Vec<_>>();
    let to = path.components().collect::<Vec<_>>();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for part in &to[common..] {
        out.push(part.as_os_str());
    }
    out.to_string_lossy().replace('\\', "/")
}
fn layout(entry: &Path) -> Result<T::Layout> {
    T::Layout::for_entry(
        entry
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or_else(|| error("invalid_path"))?,
    )
}
fn authority(entry: &Path) -> Result<V> {
    let path = entry.parent().unwrap().join(layout(entry)?.authority);
    if let Some(bytes) = F::read(&path)? {
        let value = crate::history_yaml::decode_document(&bytes)?;
        crate::history_authority::validate_authority(&value)?;
        require(
            get(&value, "authority") == &s("legacy"),
            "project_route_changed",
        )?;
        Ok(value)
    } else {
        let id = format!(
            "legacy-{}",
            &crate::identity::sha256(name(&absolute(entry)?)?.as_bytes())[..32]
        );
        crate::history_authority::authority(
            &id,
            "legacy",
            &crate::history_authoring::n("0"),
            Map::new(),
        )
    }
}
fn rebuild(
    members: &[PathBuf],
    hypotheses: &Map,
    texts: &BTreeMap<PathBuf, String>,
) -> Result<(O, Map)> {
    let mut source = O::Map(vec![]);
    let mut origins = BTreeMap::new();
    for path in members {
        let value = crate::history_yaml::decode_ordinary_source_value(texts[path].as_bytes())?;
        let value = if crate::history_view::truth(&value.projected()) {
            value
        } else {
            O::Map(vec![])
        };
        crate::source_document::merge(&mut source, &value, path, &mut origins)?;
    }
    let mut hypotheses = hypotheses.clone();
    for hyp in hypotheses.values_mut() {
        if crate::history_view::truth(get(hyp, "error")) {
            continue;
        }
        let path = PathBuf::from(text(get(hyp, "path"))?);
        let mut doc = map(&crate::history_yaml::decode_ordinary_source_value(
            texts[&path].as_bytes(),
        )?
        .projected())?
        .clone();
        let head = doc.remove("hypothesis").unwrap_or(V::Null);
        let V::Map(hyp) = hyp else {
            return Err(error("invalid_snapshot"));
        };
        hyp.insert("doc".into(), V::Map(doc.clone()));
        hyp.insert("document".into(), V::Map(doc));
        hyp.insert("head".into(), head);
        hyp.remove("ids");
        hyp.remove("raw");
    }
    Ok((source, hypotheses))
}
fn selected_privacy(source: &V, hypotheses: &Map, a: &str, b: &str) -> Result<V> {
    let mut selected = Map::new();
    for doc in std::iter::once(source).chain(
        hypotheses
            .values()
            .filter(|h| !crate::history_view::truth(get(h, "error")))
            .map(|h| get(h, "doc")),
    ) {
        for (collection, members) in crate::reasoning_fields::collections(doc)? {
            let value = selected
                .entry(collection)
                .or_insert_with(|| V::Map(Map::new()));
            let V::Map(value) = value else { unreachable!() };
            value.extend(members);
        }
    }
    let selected = V::Map(selected);
    let entries = crate::reasoning_snapshot::entries(&selected)?;
    if entries.contains_key(a) && entries.contains_key(b) {
        crate::pending_bundle::closure(&selected, &[a.into(), b.into()])
    } else {
        Ok(selected)
    }
}
/// The public caller holds the entry lock. Every candidate remains in memory until all checks pass.
pub(super) fn run(
    request: &super::Request,
    route: &WriteRoute,
    runtime_override: Option<&Runtime>,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<String> {
    if let crate::history_identity::Action::Distinct { because } = &request.action {
        require(
            !because.contains('\n'),
            "the why is one line: a second line would be a line of the record",
        )?;
    }
    let entry = &route.paths()[0];
    let local = layout(entry)?;
    require(
        F::read(&entry.parent().unwrap().join(&local.journal))?.is_none(),
        "recovery_required",
    )?;
    let mut inventory = Inventory::default();
    let document = crate::source_document::load(route.paths(), &mut inventory, false)?;
    let source = document.source.projected();
    crate::source_capture::require_ordinary(
        &crate::ordinary_value::Value::from_typed(&source),
        &crate::ordinary_value::Value::from_typed(&document.hypotheses),
    )?;
    let hypotheses = map(&document.hypotheses)?;
    let retained = selected_privacy(&source, hypotheses, &request.a, &request.b)?;
    if crate::recording_privacy::private_marker(&retained) {
        let draft = crate::recording_privacy::draft(
            route.project(),
            &super::action(request),
            &retained,
            "private identity change retained for review",
        )?;
        return Err(error(&format!(
            "private draft retained at {}",
            text(get(&draft, "path"))?
        )));
    }
    let capabilities = crate::reasoning_fields::capabilities(&source, None)?;
    let loaded_runtime;
    let runtime = if runtime_override.is_some() {
        runtime_override
    } else {
        loaded_runtime = crate::public_workspace::runtime_for_document(&source)?;
        loaded_runtime.as_ref()
    };
    let before = Projection::new(&source, hypotheses, &Map::new(), vec![], runtime)
        .map_err(|e| crate::ordinary_fields::in_record_order(&document.source, e))?;
    let mut side = crate::ordinary_write_report::Ancillary::capture(entry, &mut inventory)?;
    let brief_path = entry.parent().unwrap().join(&local.view);
    let has_brief = side.inputs.get(&brief_path).is_some_and(Option::is_some);
    let mut texts = BTreeMap::new();
    let mut order = document.members.clone();
    let mut worlds = vec![(None, raw_of(&source)?)];
    let mut hyp_sources = BTreeMap::new();
    for (name, h) in hypotheses {
        if crate::history_view::truth(get(h, "error")) {
            continue;
        }
        let path = PathBuf::from(text(get(h, "path"))?);
        order.push(path.clone());
        hyp_sources.insert(
            name.clone(),
            crate::history_yaml::decode_ordinary_source_value(
                inventory
                    .files
                    .get(&path)
                    .ok_or_else(|| error("snapshot_changed"))?,
            )?,
        );
        worlds.push((Some(name.clone()), raw_of(get(h, "doc"))?));
    }
    if has_brief {
        order.push(brief_path.clone());
    }
    for path in &order {
        texts.insert(
            path.clone(),
            std::str::from_utf8(
                inventory
                    .files
                    .get(path)
                    .ok_or_else(|| error("snapshot_changed"))?,
            )
            .map_err(|_| error("invalid_utf8"))?
            .to_owned(),
        );
    }
    let originals = texts.clone();
    let sources = super::ordinary_sameness::Sources {
        base: Some(&document.source),
        hypotheses: hyp_sources.iter().map(|(k, v)| (k.clone(), v)).collect(),
    };
    let live = worlds
        .iter()
        .flat_map(|(_, raw)| raw.keys().cloned())
        .collect::<BTreeSet<_>>();
    let fields = before.base.reader.fields();
    let deps = text(&fields["deps"])?;
    // A record whose judgments carry no snapshot or predicate yet has no such
    // field to rewrite.
    let snapshot = text(&fields["snapshot"]).unwrap_or("");
    let predicate = text(&fields["predicate"]).unwrap_or("");
    let retired = retirement(&worlds, &sources, &live, deps);
    let same = matches!(request.action, crate::history_identity::Action::Same { .. });
    require(
        request.a != request.b,
        &if same {
            format!(
                "refused - {} and {} are one id already",
                request.a, request.b
            )
        } else {
            format!(
                "refused - {} and {} are one id: distinct needs two",
                request.a, request.b
            )
        },
    )?;
    let (survivor, obsolete) = match &request.action {
        crate::history_identity::Action::Same { keep } => match keep.as_deref() {
            None | Some("a") => (&request.a, &request.b),
            Some(v) if v == request.a => (&request.a, &request.b),
            Some("b") => (&request.b, &request.a),
            Some(v) if v == request.b => (&request.b, &request.a),
            _ => {
                return Err(error(&format!(
                    "refused - --keep names the id that survives: {} or {}",
                    request.a, request.b
                )));
            }
        },
        _ => (&request.a, &request.b),
    };
    for id in [survivor, obsolete] {
        require(
            !crate::reasoning_fields::BUILTINS.contains(&id.as_str()),
            &if same {
                format!("refused - {id} is counted by the reader, never written")
            } else {
                format!(
                    "refused - {id} is counted by the reader; it is nothing to be distinct from"
                )
            },
        )?;
        require(
            live.contains(id),
            &format!(
                "refused - {id} is not an entry of the record or of any hypothesis beside it{}",
                retired.get(id).map_or(String::new(), |into| format!(
                    " - it was retired into {into}"
                ))
            ),
        )?;
    }
    let stamp = request
        .as_of
        .clone()
        .unwrap_or_else(|| chrono::Local::now().date_naive().to_string());
    let body_of = |id: &str| worlds.iter().find_map(|(_, raw)| raw.get(id)).unwrap();
    let mut output = vec![];

    let report_hypothesis = if same {
        require(
            !declared_distinct(&worlds, survivor, obsolete),
            &format!(
                "refused - {} and {} were declared distinct; remove the distinct_from that says so before saying otherwise",
                request.a, request.b
            ),
        )?;
        for (name, h) in hypotheses {
            require(
                !crate::history_view::truth(get(h, "error")),
                &format!(
                    "refused - hypothesis {name} could not be read ({}), so the migration could not reach what it holds",
                    crate::source_text::ordinary_python_str(get(h, "error"))
                ),
            )?;
        }
        let kinds = |id: &str| {
            worlds
                .iter()
                .filter_map(|(_, raw)| raw.get(id))
                .map(|b| shaped(b, deps))
                .collect::<Vec<_>>()
        };
        let (left, right) = (kinds(survivor), kinds(obsolete));
        let (lj, rj) = (left.iter().any(|v| *v), right.iter().any(|v| *v));
        require(
            lj == left.iter().all(|v| *v) && rj == right.iter().all(|v| *v) && lj == rj,
            &format!(
                "refused - {survivor} is {} and {obsolete} {}: one subject cannot be both",
                if lj { "a judgment" } else { "an entry" },
                if rj { "a judgment" } else { "an entry" }
            ),
        )?;
        for id in [survivor, obsolete] {
            require(
                matches!(body_of(id), V::Map(_)),
                &format!(
                    "refused - {id} is a line, not an entry with fields - an open question is closed by answering it, not folded into another"
                ),
            )?;
        }
        let mentioned = mentions(
            obsolete,
            &worlds,
            fields,
            has_brief.then(|| side.brief.projected()).as_ref(),
            &sources,
        );
        let before_check = record_check(&source, hypotheses, runtime)?;
        let mut aliases = BTreeMap::new();
        let mut notes = vec![];
        let base_survivor = source_body(&document.source, survivor)
            .map(ordered)
            .transpose()?;
        for (name, raw) in &worlds {
            let world_source = name
                .as_ref()
                .and_then(|n| hyp_sources.get(n))
                .unwrap_or(&document.source);
            let world = if let Some(name) = name {
                before.layers.get(name).ok_or_else(|| {
                    let prefix = format!("! hypothesis {name} cannot be read over the base: ");
                    let why = before
                        .unread
                        .iter()
                        .find_map(|v| v.strip_prefix(&prefix))
                        .unwrap_or("unknown layer error");
                    error(&format!(
                        "refused - hypothesis {name} cannot be read over the base: {why}"
                    ))
                })?
            } else {
                &before.base
            };
            let mut merged = None;
            let mut identities = FieldIdentity::new();
            if raw.contains_key(obsolete) {
                let sb = source_body(world_source, survivor)
                    .map(ordered)
                    .transpose()?
                    .or_else(|| base_survivor.clone())
                    .unwrap_or(Source::Map(vec![]));
                let rb = ordered(
                    source_body(world_source, obsolete)
                        .ok_or_else(|| error("identity_missing_body"))?,
                )?;
                let mut admission = |id: &str, existing: &V, new: &V, day: Option<&str>| {
                    let decision = crate::reasoning_authoring_guards::may_supersede(
                        &world.reader,
                        id,
                        existing,
                        new,
                        day.or(Some(&stamp)),
                        None,
                        false,
                    )?;
                    Ok((decision.allowed, decision.reason))
                };
                let (mut body, why, origins) = Merge {
                    survivor,
                    retired: obsolete,
                    reader: &world.reader,
                    as_of: request.as_of.as_deref(),
                }
                .bodies_with_origin(&sb, &rb, &mut admission)?;
                if let Source::Map(fields) = &body {
                    for (field, value) in fields {
                        let nested = match value {
                            Source::List(a) => a.iter().any(|v| !matches!(v, Source::Scalar(_))),
                            Source::Map(m) => {
                                m.iter().any(|(_, v)| !matches!(v, Source::Scalar(_)))
                            }
                            _ => false,
                        };
                        let Some(from_retired) = origins.get(field).filter(|_| nested) else {
                            continue;
                        };
                        let origin_id = if *from_retired { obsolete } else { survivor };
                        let path = if let Some(name) = name
                            .as_ref()
                            .filter(|_| source_body(world_source, origin_id).is_some())
                        {
                            PathBuf::from(text(get(&hypotheses[name], "path"))?)
                        } else {
                            document
                                .origins
                                .values()
                                .find_map(|m| m.get(origin_id))
                                .cloned()
                                .ok_or_else(|| error("identity_source_changed"))?
                        };
                        identities.insert(
                            field.clone(),
                            field_identity(&originals[&path], origin_id, field, value)?,
                        );
                    }
                }
                notes.extend(why.into_iter().map(|v| {
                    name.as_ref()
                        .map_or(v.clone(), |n| format!("{v} (in hypothesis {n})"))
                }));
                let Source::Map(fields) = &mut body else {
                    unreachable!()
                };
                let at = fields.iter().position(|(k, _)| k == "also").unwrap();
                let (_, also) = fields.remove(at);
                let V::List(also) = also.typed() else {
                    unreachable!()
                };
                aliases.insert(
                    name.clone(),
                    also.iter()
                        .map(|v| text(v).map(str::to_owned))
                        .collect::<Result<Vec<_>>>()?,
                );
                merged = Some(body);
            } else if let Some(body) = raw.get(survivor).filter(|v| matches!(v, V::Map(_))) {
                let mut also = match get(body, "also") {
                    V::Text(v) => vec![v.clone()],
                    V::List(a) => a
                        .iter()
                        .map(|v| text(v).map(str::to_owned))
                        .collect::<Result<Vec<_>>>()?,
                    _ => vec![],
                };
                if !also.contains(obsolete) {
                    also.push(obsolete.clone());
                }
                aliases.insert(name.clone(), also);
            }
            let files = if let Some(name) = name {
                vec![PathBuf::from(text(get(&hypotheses[name], "path"))?)]
            } else {
                document.members.clone()
            };
            for path in files {
                let changed = same_file_with_identity(
                    &texts[&path],
                    survivor,
                    obsolete,
                    raw,
                    merged.as_ref(),
                    deps,
                    snapshot,
                    &identities,
                )?;
                texts.insert(path, changed);
            }
        }
        if has_brief {
            texts.insert(
                brief_path.clone(),
                brief_file(&texts[&brief_path], survivor, obsolete)?,
            );
        }
        let mut counts = vec![];
        for path in &order {
            let (changed, count) =
                rewrite_text(&texts[path], obsolete, survivor, predicate, snapshot)?;
            if count > 0 {
                counts.push((path.clone(), count));
            }
            texts.insert(path.clone(), dedupe_text(&changed, survivor));
        }
        for (name, _) in &worlds {
            if let Some(also) = aliases.get(name) {
                let files = if let Some(name) = name {
                    vec![PathBuf::from(text(get(&hypotheses[name], "path"))?)]
                } else {
                    document.members.clone()
                };
                for path in files {
                    texts.insert(path.clone(), append_also(&texts[&path], survivor, also)?);
                }
            }
        }
        for path in &document.members {
            let mut lines = texts[path]
                .split('\n')
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if bump_updated(&mut lines, &stamp)? {
                texts.insert(path.clone(), lines.join("\n"));
                break;
            }
        }
        let (after_source, after_hypotheses) = rebuild(&document.members, hypotheses, &texts)?;
        let after_check = record_check(&after_source.projected(), &after_hypotheses, runtime)
            .map_err(|e| {
                error(&format!(
                    "refused - the migration broke the record and was undone: {e}"
                ))
            })?;
        let live_after = raw_of(&after_source.projected())?
            .keys()
            .cloned()
            .chain(
                after_hypotheses
                    .values()
                    .flat_map(|h| raw_of(get(h, "doc")).unwrap_or_default().into_keys()),
            )
            .collect::<BTreeSet<_>>();
        require(
            !live_after.contains(obsolete) && live_after.contains(survivor),
            &format!(
                "refused - the migration broke the record and was undone: {}",
                if live_after.contains(obsolete) {
                    format!("{obsolete} is still held")
                } else {
                    format!("{survivor} is not held after the write")
                }
            ),
        )?;
        let previous = before_check
            .lines()
            .filter_map(|v| v.strip_prefix("FAIL "))
            .map(|v| rewrite_text(v, obsolete, survivor, "wrong_if", "seen").map(|v| v.0))
            .collect::<Result<BTreeSet<_>>>()?;
        let failures = after_check
            .lines()
            .filter_map(|v| v.strip_prefix("FAIL "))
            .filter(|v| !previous.contains(*v))
            .collect::<Vec<_>>();
        require(
            failures.is_empty(),
            &format!(
                "refused - check would fail after the migration, so nothing was changed:\n  {}",
                failures.join("\n  ")
            ),
        )?;
        let kept = matches!(
            &request.action,
            crate::history_identity::Action::Same { keep: Some(_) }
        );
        output.push(format!(
            "same {} {}: {obsolete} retired into {survivor}{}",
            request.a,
            request.b,
            if kept {
                format!(" (kept {survivor})")
            } else {
                String::new()
            }
        ));
        output.extend(notes.into_iter().map(|v| format!("  {v}")));
        if !mentioned.is_empty() {
            let order = [
                deps,
                snapshot,
                predicate,
                "rule",
                "text",
                "from",
                "distinct_from",
                "the brief",
            ];
            let keys = order.iter().copied().filter(|v| !v.is_empty()).chain(
                mentioned
                    .iter()
                    .map(|(k, _)| k.as_str())
                    .filter(|k| !order.contains(k)),
            );
            let parts = keys
                .filter_map(|kind| {
                    mentioned
                        .iter()
                        .find(|(k, _)| k == kind)
                        .map(|(_, a)| format!("{kind}: {}", a.join(", ")))
                })
                .collect::<Vec<_>>();
            output.push(format!("  rewritten - {}", parts.join(" · ")));
        }
        if !counts.is_empty() {
            output.push(format!(
                "  {}",
                counts
                    .iter()
                    .map(|(p, n)| format!(
                        "{n} mention{} in {}",
                        if *n == 1 { "" } else { "s" },
                        notice_path(p, entry.parent().unwrap())
                    ))
                    .collect::<Vec<_>>()
                    .join(" · ")
            ));
        }
        output.push(format!(
            "  check: {}",
            after_check.lines().next_back().unwrap_or("")
        ));
        None
    } else {
        let crate::history_identity::Action::Distinct { because } = &request.action else {
            unreachable!()
        };
        let why = because
            .split(whitespace)
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        require(
            !why.is_empty(),
            "refused - distinct takes the why: distinct <a> <b> \"<why>\"",
        )?;
        require(
            matches!(body_of(survivor), V::Map(_)),
            &format!(
                "refused - {survivor} is a line, not an entry with fields; distinct is written on a field"
            ),
        )?;
        if declared_distinct(&worlds, survivor, obsolete) {
            return Ok(format!(
                "{survivor} and {obsolete} are already declared distinct; nothing written\n"
            ));
        }
        let mut target = document
            .members
            .iter()
            .find(|path| {
                locate(
                    &texts[*path]
                        .split('\n')
                        .map(str::to_owned)
                        .collect::<Vec<_>>(),
                    survivor,
                )
                .is_some()
            })
            .cloned();
        let mut named = None;
        if target.is_none() {
            for (name, raw) in worlds.iter().skip(1) {
                if raw.contains_key(survivor) {
                    named = name.clone();
                    target = Some(PathBuf::from(text(get(
                        &hypotheses[name.as_ref().unwrap()],
                        "path",
                    ))?));
                    break;
                }
            }
        }
        let target = target.ok_or_else(|| error("identity_target_not_found"))?;
        let mut distinct = ids_value(get(body_of(survivor), "distinct_from"));
        distinct.push(obsolete.clone());
        let value = distinct.join(", ");
        texts.insert(
            target.clone(),
            distinct_text(
                &texts[&target],
                survivor,
                &value,
                &why,
                &stamp,
                named.is_some(),
            )?,
        );
        let (after_source, after_hypotheses) = rebuild(&document.members, hypotheses, &texts)?;
        let after_raw = if let Some(name) = &named {
            raw_of(get(&after_hypotheses[name], "doc"))?
        } else {
            raw_of(&after_source.projected())?
        };
        require(
            after_raw
                .get(survivor)
                .is_some_and(|b| ids_value(get(b, "distinct_from")).contains(obsolete)),
            &format!(
                "refused - the write broke the record and was undone: {survivor} does not carry distinct_from: {obsolete} after the write"
            ),
        )?;
        output.push(format!(
            "distinct {survivor} from {obsolete}: {why}{}",
            named
                .as_ref()
                .map_or(String::new(), |n| format!(" (in hypothesis {n})"))
        ));
        output.push(format!(
            "  the pair returns as no candidate; {survivor} carries distinct_from: {value}"
        ));
        named
    };
    let (after_source, after_hypotheses) = rebuild(&document.members, hypotheses, &texts)?;
    let after_value = after_source.projected();
    let after = Projection::new(
        &after_value,
        &after_hypotheses,
        &Map::new(),
        vec![],
        runtime,
    )?;
    if has_brief {
        side.brief =
            crate::history_yaml::decode_ordinary_source_value(texts[&brief_path].as_bytes())?;
    }
    let world = report_hypothesis
        .as_ref()
        .and_then(|n| after.layers.get(n))
        .unwrap_or(&after.base);
    output.extend(crate::ordinary_write_report::render(
        "set", survivor, world, None, &side,
    )?);
    let mut root = entry.parent().unwrap().to_path_buf();
    for path in &order {
        while !path.starts_with(&root) {
            require(root.pop(), "invalid_path")?;
        }
    }
    let mut files = vec![];
    for path in &order {
        if texts[path] == originals[path] {
            continue;
        }
        if document.members.contains(path) {
            let old =
                crate::history_yaml::decode_ordinary_source_value(originals[path].as_bytes())?
                    .projected();
            let new = crate::history_yaml::decode_ordinary_source_value(texts[path].as_bytes())?
                .projected();
            require(
                ["record", "also"]
                    .iter()
                    .all(|k| get(&old, k) == get(&new, k)),
                "direct_pointer_change: record membership needs a separate migration",
            )?;
        }
        let role = if path == entry {
            "record"
        } else if document.members.contains(path) {
            "record_member"
        } else if path == &brief_path {
            "view"
        } else {
            "hypothesis"
        };
        files.push(FileImage {
            path: relative(&root, path)?,
            role: role.into(),
            before: Some(originals[path].as_bytes().to_vec()),
            after: Some(texts[path].as_bytes().to_vec()),
        });
    }
    if files.is_empty() {
        return Ok(output.join("\n") + "\n");
    }
    let digest = |path: &Path| {
        inventory
            .files
            .get(path)
            .map(|v| s(&crate::identity::sha256(v)))
            .ok_or_else(|| error("snapshot_changed"))
    };
    let record_members = document
        .members
        .iter()
        .map(|p| Ok((relative(&root, p)?, digest(p)?)))
        .collect::<Result<Map>>()?;
    let hypothesis_directory = entry.parent().unwrap().join(&local.hypotheses);
    let hypothesis_paths = inventory
        .events
        .iter()
        .filter_map(|((kind, pattern), event)| {
            if kind == "glob"
                && pattern.parent() == Some(hypothesis_directory.as_path())
                && let crate::source_inventory::Observation::Glob(paths) = event
            {
                return Some(paths);
            }
            None
        })
        .flatten()
        .collect::<BTreeSet<_>>();
    let hypothesis_members = hypothesis_paths
        .into_iter()
        .map(|path| Ok((relative(&root, path)?, digest(path)?)))
        .collect::<Result<Map>>()?;
    let input_files = side
        .inputs
        .iter()
        .map(|(path, digest)| {
            Ok((
                relative(&root, path)?,
                digest.as_ref().map_or(V::Null, |v| s(v)),
            ))
        })
        .collect::<Result<Map>>()?;
    let baseline = obj([
        ("kind", s("direct/v1")),
        ("transaction_root", s(name(&absolute(&root)?)?)),
        ("record_members", V::Map(record_members)),
        ("hypothesis_members", V::Map(hypothesis_members)),
        (
            "hypotheses_directory",
            s(&relative(
                &root,
                &entry.parent().unwrap().join(&local.hypotheses),
            )?),
        ),
        ("input_files", V::Map(input_files)),
        ("policy", route.config().clone()),
    ]);
    document
        .source
        .strict_typed()
        .map_err(|_| error("invalid_history_value: snapshot mappings require string keys"))?;
    let receipt = T::semantic_receipt(
        text(get(&capabilities, "profile"))?,
        &capabilities,
        &obj([("document", source)]),
        &obj([("document", after_value)]),
    )?;
    let operation = crate::public_history::fresh_id("direct")?;
    let mutation = PreparedMutation::prepare(
        &operation,
        &authority(entry)?,
        &baseline,
        files,
        &receipt,
        &relative(&root, entry)?,
        None,
    )?;
    let journal = relative(&root, &entry.parent().unwrap().join(&local.journal))?;
    probe("prepared")?;
    let mut verify = |_: &V| {
        route.verify()?;
        inventory.verify()?;
        require(
            crate::legacy_authoring::local_route(entry, route)?
                == crate::legacy_authoring::AuthorityRoute::Legacy,
            "project_route_changed",
        )
    };
    let mut committed = |_: &V| probe("published");
    F::publish_legacy(
        &root,
        &journal,
        &mutation,
        &mut verify,
        Some(&mut committed),
    )?;
    crate::session_activity::published(
        &root,
        mutation.files(),
        Some(&BTreeSet::from([survivor.clone(), obsolete.clone()])),
    );
    Ok(output.join("\n") + "\n")
}
enum Kind {
    Pending,
    Scalar(String, ScalarStyle),
    List(Vec<usize>),
    Map(Vec<(usize, usize)>),
}
struct Graph<'a> {
    parser: Parser<&'a [u8]>,
    nodes: Vec<Node>,
    anchors: BTreeMap<String, usize>,
    source_len: usize,
}
impl Graph<'_> {
    fn next(&mut self) -> Result<Event> {
        self.parser
            .parse()
            .map_err(|_| error("invalid_identity_yaml"))
    }
    fn offset(&self, index: u64) -> Result<usize> {
        require(index as usize <= self.source_len, "invalid_identity_yaml")?;
        Ok(index as usize)
    }
    fn node(&mut self, event: Event, depth: usize) -> Result<usize> {
        require(depth <= 128 && self.nodes.len() < 200_000, "history_limit")?;
        if let E::Alias { anchor } = &event.data {
            return self
                .anchors
                .get(anchor)
                .copied()
                .ok_or_else(|| error("invalid_identity_yaml"));
        }
        let id = self.nodes.len();
        let start = self.offset(event.start_mark.index)?;
        let mut end = self.offset(event.end_mark.index)?;
        self.nodes.push(Node {
            start,
            end,
            tag: None,
            kind: Kind::Pending,
        });
        let anchor = match &event.data {
            E::Scalar { anchor, .. }
            | E::SequenceStart { anchor, .. }
            | E::MappingStart { anchor, .. } => anchor.clone(),
            _ => None,
        };
        if let Some(anchor) = anchor {
            require(
                self.anchors.insert(anchor, id).is_none(),
                "invalid_identity_yaml",
            )?;
        }
        let mut tag = None;
        let kind = match event.data {
            E::Scalar {
                value,
                style,
                tag: scalar_tag,
                ..
            } => {
                tag = scalar_tag;
                Kind::Scalar(value, style)
            }
            E::SequenceStart { .. } => {
                let mut values = vec![];
                loop {
                    let event = self.next()?;
                    if matches!(event.data, E::SequenceEnd) {
                        end = self.offset(event.end_mark.index)?;
                        break;
                    }
                    values.push(self.node(event, depth + 1)?);
                }
                Kind::List(values)
            }
            E::MappingStart { .. } => {
                let mut pairs = vec![];
                loop {
                    let event = self.next()?;
                    if matches!(event.data, E::MappingEnd) {
                        end = self.offset(event.end_mark.index)?;
                        break;
                    }
                    let key = self.node(event, depth + 1)?;
                    let event = self.next()?;
                    pairs.push((key, self.node(event, depth + 1)?));
                }
                Kind::Map(pairs)
            }
            _ => return Err(error("invalid_identity_yaml")),
        };
        self.nodes[id] = Node {
            start,
            end,
            kind,
            tag,
        };
        Ok(id)
    }
    fn scalar(&self, id: usize) -> Option<&str> {
        if let Kind::Scalar(value, _) = &self.nodes[id].kind {
            Some(value)
        } else {
            None
        }
    }
    fn pairs(&self, id: usize, active: &mut BTreeSet<usize>) -> Result<Vec<(usize, usize)>> {
        require(active.insert(id), "cyclic identity YAML alias")?;
        let Kind::Map(pairs) = &self.nodes[id].kind else {
            return Err(error("identity_invalid_mapping"));
        };
        let mut merged = vec![];
        let mut regular = vec![];
        for &(key, value) in pairs {
            let node = &self.nodes[key];
            let merge = node.tag.as_deref() == Some("tag:yaml.org,2002:merge")
                || (node.tag.is_none()
                    && matches!(&node.kind, Kind::Scalar(v, ScalarStyle::Plain) if v == "<<"));
            if merge {
                match &self.nodes[value].kind {
                    Kind::Map(_) => merged.extend(self.pairs(value, active)?),
                    Kind::List(items) => {
                        for &item in items.iter().rev() {
                            merged.extend(self.pairs(item, active)?);
                        }
                    }
                    _ => return Err(error("identity_invalid_mapping")),
                }
            } else {
                regular.push((key, value));
            }
        }
        active.remove(&id);
        merged.extend(regular);
        Ok(merged)
    }
    fn field(&self, id: usize, field: &str) -> Result<Option<usize>> {
        Ok(self
            .pairs(id, &mut BTreeSet::new())?
            .into_iter()
            .rev()
            .find(|(key, _)| self.scalar(*key) == Some(field))
            .map(|(_, v)| v))
    }
    fn identity(
        &self,
        id: usize,
        value: &Source,
        active: &mut BTreeSet<usize>,
    ) -> Result<crate::history_emit::OrdinaryIdentity> {
        require(active.insert(id), "cyclic identity YAML alias")?;
        let mut identity = crate::history_emit::OrdinaryIdentity {
            id: Some(id),
            children: vec![],
        };
        match value {
            Source::Scalar(v) => {
                let Kind::Scalar(raw, style) = &self.nodes[id].kind else {
                    return Err(error("identity_source_changed"));
                };
                require(
                    crate::history_yaml::ordinary_scalar(
                        raw,
                        *style,
                        self.nodes[id].tag.as_deref(),
                    )? == *v,
                    "identity_source_changed",
                )?;
                if !matches!(v, V::Date(_) | V::DateTime(_)) {
                    identity.id = None;
                }
            }
            Source::List(values) => {
                let Kind::List(nodes) = &self.nodes[id].kind else {
                    return Err(error("identity_source_changed"));
                };
                require(nodes.len() == values.len(), "identity_source_changed")?;
                for (&id, value) in nodes.iter().zip(values) {
                    identity.children.push(self.identity(id, value, active)?);
                }
            }
            Source::Map(values) => {
                let pairs = self.pairs(id, &mut BTreeSet::new())?;
                for (key, value) in values {
                    let &(key_id, value_id) = pairs
                        .iter()
                        .rev()
                        .find(|(id, _)| self.scalar(*id) == Some(key))
                        .ok_or_else(|| error("identity_source_changed"))?;
                    identity.children.push(self.identity(
                        key_id,
                        &Source::Scalar(s(key)),
                        active,
                    )?);
                    identity
                        .children
                        .push(self.identity(value_id, value, active)?);
                }
            }
        }
        active.remove(&id);
        Ok(identity)
    }
}
pub(crate) type FieldIdentity = BTreeMap<String, crate::history_emit::OrdinaryIdentity>;
pub(crate) fn source_identity(
    raw: &str,
    value: &Source,
) -> Result<crate::history_emit::OrdinaryIdentity> {
    let (graph, root) = parse(raw)?;
    graph.identity(root, value, &mut BTreeSet::new())
}
pub(crate) fn field_identity(
    raw: &str,
    id: &str,
    field: &str,
    value: &Source,
) -> Result<crate::history_emit::OrdinaryIdentity> {
    let (graph, root) = parse(raw)?;
    for (key, collection) in graph.pairs(root, &mut BTreeSet::new())? {
        if graph
            .scalar(key)
            .is_some_and(|v| ["meta", "schema", "record", "also", "hypothesis"].contains(&v))
            || !matches!(graph.nodes[collection].kind, Kind::Map(_))
        {
            continue;
        }
        if let Some(body) = graph.field(collection, id)? {
            let field = graph
                .field(body, field)?
                .ok_or_else(|| error("identity_source_changed"))?;
            return graph.identity(field, value, &mut BTreeSet::new());
        }
    }
    Err(error("identity_source_changed"))
}
fn ordinary_source(value: &Source) -> O {
    match value {
        Source::Scalar(v) => O::Scalar(v.clone()),
        Source::List(a) => O::List(a.iter().map(ordinary_source).collect()),
        Source::Map(m) => O::Map(
            m.iter()
                .map(|(k, v)| {
                    (
                        crate::history_yaml::OrdinaryKey::text_key(k),
                        ordinary_source(v),
                    )
                })
                .collect(),
        ),
    }
}
fn parse(value: &str) -> Result<(Graph<'_>, usize)> {
    require(value.len() <= 16 * 1024 * 1024, "history_limit")?;
    let mut parser = Parser::new();
    parser.set_input(value.as_bytes());
    let mut graph = Graph {
        parser,
        nodes: vec![],
        anchors: BTreeMap::new(),
        source_len: value.len(),
    };
    require(
        matches!(graph.next()?.data, E::StreamStart { .. }),
        "invalid_identity_yaml",
    )?;
    require(
        matches!(graph.next()?.data, E::DocumentStart { .. }),
        "invalid_identity_yaml",
    )?;
    let event = graph.next()?;
    let root = graph.node(event, 0)?;
    require(
        matches!(graph.next()?.data, E::DocumentEnd { .. })
            && matches!(graph.next()?.data, E::StreamEnd),
        "invalid_identity_yaml",
    )?;
    Ok((graph, root))
}
fn token_count(value: &str, old: &str, new: &str) -> (String, usize) {
    let count = (value.len() - tokens(value, old, "").len()) / old.len();
    (tokens(value, old, new), count)
}
struct Visitor<'a, 'b> {
    graph: &'a Graph<'b>,
    source: &'a str,
    old: &'a str,
    new: &'a str,
    predicate: &'a str,
    snapshot: &'a str,
    seen: BTreeSet<usize>,
    spans: Vec<(usize, usize, String, usize)>,
}
impl Visitor<'_, '_> {
    fn original(&self, id: usize) -> &str {
        let n = &self.graph.nodes[id];
        &self.source[n.start..n.end]
    }
    fn span(&mut self, id: usize, value: String, count: usize) {
        let n = &self.graph.nodes[id];
        self.spans.push((n.start, n.end, value, count));
    }
    fn protect(&mut self, id: usize) {
        self.span(id, self.original(id).into(), 0);
    }
    fn visit(&mut self, id: usize, executable: bool, snapshot: bool) -> Result<()> {
        if !self.seen.insert(id) {
            return Ok(());
        }
        match &self.graph.nodes[id].kind {
            Kind::Map(pairs)
                if executable
                    && pairs.len() == 1
                    && self.graph.scalar(pairs[0].0) == Some("expr") =>
            {
                let value = pairs[0].1;
                let original = self.graph.scalar(value).map(|v| obj([("expr", s(v))]));
                if let Some(original) = original.filter(|v| {
                    crate::reasoning_language::legacy_references(v)
                        .iter()
                        .any(|id| id == self.old)
                }) {
                    let changed = expression(&original, self.old, self.new)?;
                    let mut replacement = serde_json::to_string(text(&map(&changed)?["expr"])?)?;
                    if matches!(
                        self.graph.nodes[value].kind,
                        Kind::Scalar(_, ScalarStyle::Literal | ScalarStyle::Folded)
                    ) && self.original(value).ends_with('\n')
                    {
                        replacement.push('\n');
                    }
                    self.span(value, replacement, 1);
                } else {
                    self.protect(value);
                }
            }
            Kind::Map(pairs)
                if executable
                    && pairs.len() == 1
                    && self
                        .graph
                        .scalar(pairs[0].0)
                        .is_some_and(|v| ["text", "num", "bool"].contains(&v)) =>
            {
                self.protect(pairs[0].1)
            }
            Kind::Map(pairs) => {
                for &(key, value) in pairs {
                    if snapshot
                        && self.graph.scalar(key) == Some("computed")
                        && matches!(self.graph.nodes[value].kind, Kind::Map(_))
                    {
                        if let Kind::Map(parts) = &self.graph.nodes[value].kind {
                            for &(part, payload) in parts {
                                match self.graph.scalar(part) {
                                    Some("value") => self.protect(payload),
                                    Some("rule") => self.visit(payload, true, false)?,
                                    _ => {}
                                }
                            }
                        }
                    } else {
                        let key = self.graph.scalar(key);
                        self.visit(
                            value,
                            executable || key == Some("rule") || key == Some(self.predicate),
                            snapshot || key == Some(self.snapshot),
                        )?;
                    }
                }
            }
            Kind::List(values) => {
                for &value in values {
                    self.visit(value, executable, snapshot)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
/// Exact whole-token edits, including the reference's count of changed expression scalars.
pub(crate) fn rewrite_text(
    source: &str,
    old: &str,
    new: &str,
    predicate: &str,
    snapshot: &str,
) -> Result<(String, usize)> {
    require(!old.is_empty(), "invalid_identity_subjects")?;
    let (graph, root) = match parse(source) {
        Ok(v) => v,
        Err(e) if e.0 == "history_limit" => return Err(e),
        Err(_) => return Ok(token_count(source, old, new)),
    };
    let mut visitor = Visitor {
        graph: &graph,
        source,
        old,
        new,
        predicate,
        snapshot,
        seen: BTreeSet::new(),
        spans: vec![],
    };
    visitor.visit(root, false, false)?;
    visitor.spans.sort();
    let (mut out, mut count, mut start) = (String::new(), 0, 0);
    for (left, right, replacement, edits) in visitor.spans {
        require(
            start <= left && left <= right && right <= source.len(),
            "invalid_identity_yaml",
        )?;
        let (changed, n) = token_count(&source[start..left], old, new);
        out.push_str(&changed);
        out.push_str(&replacement);
        count += n + edits;
        start = right;
    }
    let (changed, n) = token_count(&source[start..], old, new);
    out.push_str(&changed);
    Ok((out, count + n))
}
fn scalar_spans(
    graph: &Graph<'_>,
    id: usize,
    seen: &mut BTreeSet<usize>,
    out: &mut Vec<(usize, usize)>,
) {
    if !seen.insert(id) {
        return;
    }
    let node = &graph.nodes[id];
    match &node.kind {
        Kind::Scalar(_, _) => out.push((node.start, node.end)),
        Kind::List(values) => {
            for &v in values {
                scalar_spans(graph, v, seen, out);
            }
        }
        Kind::Map(pairs) => {
            for &(k, v) in pairs {
                scalar_spans(graph, k, seen, out);
                scalar_spans(graph, v, seen, out);
            }
        }
        Kind::Pending => {}
    }
}
fn whitespace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}
fn distinct_ids(value: &str) -> Vec<&str> {
    value
        .split(|c| c == ',' || whitespace(c))
        .filter(|v| !v.is_empty())
        .collect()
}
fn string_scalar(value: &str, like: &str) -> String {
    if like.starts_with('\'') && !value.contains('\n') {
        return format!("'{}'", value.replace('\'', "''"));
    }
    let date = Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$").unwrap();
    if date.is_match(like) && date.is_match(value) {
        return value.into();
    }
    let bare = Regex::new(r"^[A-Za-z_][A-Za-z0-9_.-]*$").unwrap();
    if !like.starts_with(['\'', '"'])
        && bare.is_match(value)
        && crate::history_yaml::decode_ordinary_source_value(value.as_bytes())
            .is_ok_and(|v| v.projected() == s(value))
    {
        return value.into();
    }
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}
pub(crate) fn dedupe_text(source: &str, survivor: &str) -> String {
    let flow = if let Ok((graph, root)) = parse(source) {
        let mut protected = vec![];
        scalar_spans(&graph, root, &mut BTreeSet::new(), &mut protected);
        Regex::new(r"\[([^\[\]]*)\]")
            .unwrap()
            .replace_all(source, |m: &regex::Captures<'_>| {
                let at = m.get(0).unwrap().start();
                if protected.iter().any(|(a, b)| *a <= at && at < *b)
                    || token_count(&m[1], survivor, "").1 < 2
                {
                    return m[0].to_owned();
                }
                let mut had = false;
                let inner = m[1].replace('\n', " ");
                let mut out = vec![];
                for v in inner.split(',').map(str::trim).filter(|v| !v.is_empty()) {
                    if v == survivor {
                        if had {
                            continue;
                        }
                        had = true;
                    }
                    out.push(v);
                }
                format!("[{}]", out.join(", "))
            })
            .into_owned()
    } else {
        source.into()
    };
    Regex::new(r#"(?m)^(\s*distinct_from:\s*)(["']?)([A-Za-z0-9_.,\s]+?)(["']?)\s*$"#)
        .unwrap()
        .replace_all(&flow, |m: &regex::Captures<'_>| {
            if m[2] != m[4] {
                return m[0].into();
            }
            let mut ids = vec![];
            for id in distinct_ids(&m[3]) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            format!("{}{}", &m[1], string_scalar(&ids.join(", "), ""))
        })
        .into_owned()
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}
fn members(lines: &[String], start: usize, end: usize) -> (Option<usize>, Vec<(String, usize)>) {
    let mut ind = None;
    let mut out = vec![];
    for (i, line) in lines.iter().enumerate().take(end).skip(start + 1) {
        if line.trim().is_empty() || line.trim().starts_with('#') {
            continue;
        }
        let expected = *ind.get_or_insert(indent(line));
        if let Some(m) = MEMBER.captures(line)
            && m[1].len() == expected
        {
            out.push((m[2].into(), i));
        }
    }
    (ind, out)
}
fn block_end(lines: &[String], i: usize, ind: usize, end: usize) -> usize {
    let mut j = i + 1;
    while j < end {
        if lines[j].trim().is_empty() {
            let mut k = j;
            while k < end && lines[k].trim().is_empty() {
                k += 1;
            }
            if k < end && indent(&lines[k]) > ind {
                j = k;
                continue;
            }
            break;
        }
        if indent(&lines[j]) > ind {
            j += 1;
        } else {
            break;
        }
    }
    j
}
fn collections(lines: &[String]) -> Vec<(String, usize, usize)> {
    let starts = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| TOP.captures(l).map(|m| (m[1].to_owned(), i)))
        .collect::<Vec<_>>();
    starts
        .iter()
        .enumerate()
        .map(|(i, (n, s))| {
            (
                n.clone(),
                *s,
                starts.get(i + 1).map_or(lines.len(), |(_, s)| *s),
            )
        })
        .collect()
}
fn locate(lines: &[String], id: &str) -> Option<(String, usize, usize, usize)> {
    for (name, start, end) in collections(lines) {
        let (ind, entries) = members(lines, start, end);
        for (entry, i) in entries {
            if entry == id {
                return Some((
                    name,
                    ind.unwrap(),
                    i,
                    block_end(lines, i, ind.unwrap(), end),
                ));
            }
        }
    }
    None
}
fn inline(line: &str) -> &str {
    line.split_once(':').map_or("", |(_, v)| v.trim())
}
fn field_span(
    lines: &[String],
    start: usize,
    end: usize,
    field: &str,
) -> Option<(usize, usize, usize)> {
    let (ind, fields) = members(lines, start, end);
    fields
        .into_iter()
        .find(|(name, _)| name == field)
        .map(|(_, i)| (ind.unwrap(), i, block_end(lines, i, ind.unwrap(), end)))
}
fn replace_field(
    lines: &mut Vec<String>,
    start: usize,
    end: usize,
    field: &str,
    new: Vec<String>,
) -> usize {
    let (_, a, b) = field_span(lines, start, end, field).unwrap_or((0, end, end));
    let next = end + new.len() - (b - a);
    lines.splice(a..b, new);
    next
}
fn field_lines(field: &str, value: &Source, ind: usize) -> Result<Vec<String>> {
    field_lines_with_identity(field, value, ind, None)
}
fn field_lines_with_identity(
    field: &str,
    value: &Source,
    ind: usize,
    identity: Option<&crate::history_emit::OrdinaryIdentity>,
) -> Result<Vec<String>> {
    let nested = match value {
        Source::List(a) => a.iter().any(|v| !matches!(v, Source::Scalar(_))),
        Source::Map(m) => m.iter().any(|(_, v)| !matches!(v, Source::Scalar(_))),
        _ => false,
    };
    if let Some(identity) = identity.filter(|_| nested) {
        let source = O::Map(vec![(
            crate::history_yaml::OrdinaryKey::text_key(field),
            ordinary_source(value),
        )]);
        let tree = crate::history_emit::OrdinaryIdentity {
            id: None,
            children: vec![
                crate::history_emit::OrdinaryIdentity {
                    id: None,
                    children: vec![],
                },
                identity.clone(),
            ],
        };
        let bytes = crate::history_emit::encode_ordinary_source_identity(
            &source,
            100usize.saturating_sub(ind),
            Some(&tree),
        )?;
        return Ok(std::str::from_utf8(&bytes)
            .map_err(|_| error("invalid_utf8"))?
            .trim_end()
            .lines()
            .map(|v| format!("{}{v}", " ".repeat(ind)))
            .collect());
    }
    let lines = crate::legacy_authoring::entry_lines_ordered(
        "entry",
        &Source::Map(vec![(field.into(), value.clone())]),
        ind.saturating_sub(2),
        ind,
        false,
    )?;
    Ok(lines.into_iter().skip(1).collect())
}
pub(crate) fn entry_lines(
    id: &str,
    body: &Source,
    ind: usize,
    find: usize,
    flow: bool,
    identities: &FieldIdentity,
) -> Result<Vec<String>> {
    let Source::Map(fields) = body else {
        return crate::legacy_authoring::entry_lines_ordered(id, body, ind, find, flow);
    };
    if fields.iter().all(|(_, v)| matches!(v, Source::Scalar(_))) {
        return crate::legacy_authoring::entry_lines_ordered(id, body, ind, find, flow);
    }
    let mut out = vec![format!("{}{id}:", " ".repeat(ind))];
    for (key, value) in fields {
        out.extend(field_lines_with_identity(
            key,
            value,
            find,
            identities.get(key),
        )?);
    }
    Ok(out)
}
fn python_equal(left: &V, right: &V) -> bool {
    match (left, right) {
        (V::List(a), V::List(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| python_equal(a, b))
        }
        (V::Map(a), V::Map(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, a)| b.get(k).is_some_and(|b| python_equal(a, b)))
        }
        (V::Map(_) | V::List(_), _) | (_, V::Map(_) | V::List(_)) => false,
        _ => crate::history_yaml::ordinary_key(left.clone())
            .ok()
            .zip(crate::history_yaml::ordinary_key(right.clone()).ok())
            .is_some_and(|(a, b)| a.python_eq(&b)),
    }
}
fn set_fields(
    lines: &mut Vec<String>,
    id: &str,
    body: &Source,
    was: &Source,
    identities: &FieldIdentity,
) -> Result<()> {
    let (_, ind, start, mut end) =
        locate(lines, id).ok_or_else(|| error("identity_target_not_found"))?;
    let find = members(lines, start, end).0.unwrap_or(ind + 2);
    if inline(&lines[start]).starts_with('{') {
        let comments = lines[start + 1..end]
            .iter()
            .filter(|l| l.trim().starts_with('#'))
            .cloned()
            .collect::<Vec<_>>();
        let mut new = entry_lines(id, body, ind, find, true, identities)?;
        new.extend(comments);
        lines.splice(start..end, new);
        return Ok(());
    }
    let (Source::Map(body), Source::Map(was)) = (body, was) else {
        return Err(error("identity_invalid_body"));
    };
    for (field, _) in was {
        if field != "also"
            && !body.iter().any(|(k, _)| k == field)
            && let Some((_, a, b)) = field_span(lines, start, end, field)
        {
            lines.drain(a..b);
            end -= b - a;
        }
    }
    for (field, value) in body {
        if was
            .iter()
            .any(|(k, v)| k == field && python_equal(&v.typed(), &value.typed()))
        {
            continue;
        }
        end = replace_field(
            lines,
            start,
            end,
            field,
            field_lines_with_identity(field, value, find, identities.get(field))?,
        );
    }
    Ok(())
}
fn drop_key(
    lines: &mut Vec<String>,
    start: usize,
    end: usize,
    field: &str,
    key: &str,
) -> Result<usize> {
    let Some((_, a, b)) = field_span(lines, start, end, field) else {
        return Ok(end);
    };
    if inline(&lines[a]).starts_with('{') {
        let block = lines[a..b].join("\n");
        let re = Regex::new(&format!(r"{}\s*:\s*{FLOW}\s*,?\s*", regex::escape(key))).unwrap();
        let (mut output, mut last) = (String::new(), 0);
        for m in re.find_iter(&block) {
            if block[..m.start()]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
            {
                continue;
            }
            output.push_str(&block[last..m.start()]);
            last = m.end();
        }
        output.push_str(&block[last..]);
        let output = Regex::new(r",\s*}").unwrap().replace_all(&output, "}");
        let new = output.split('\n').map(str::to_owned).collect::<Vec<_>>();
        let next = end + new.len() - (b - a);
        lines.splice(a..b, new);
        Ok(next)
    } else if let Some((_, a, b)) = field_span(lines, a, b, key) {
        lines.drain(a..b);
        Ok(end - (b - a))
    } else {
        Ok(end)
    }
}
fn add_in(
    lines: &mut Vec<String>,
    id: &str,
    body: &Source,
    collection: &str,
    identities: &FieldIdentity,
) -> Result<()> {
    let (_, start, end) = collections(lines)
        .into_iter()
        .find(|(name, _, _)| name == collection)
        .ok_or_else(|| error(&format!("no collection {collection} in this file")))?;
    let (ind, entries) = members(lines, start, end);
    let ind = ind.unwrap_or(2);
    if entries.is_empty() {
        let new = entry_lines(id, body, ind, ind + 2, false, identities)?;
        lines.splice(start + 1..start + 1, new);
        return Ok(());
    }
    let common = |a: &str| {
        a.split('.')
            .zip(id.split('.'))
            .take_while(|(a, b)| a == b)
            .count()
    };
    let best = entries.iter().map(|(n, _)| common(n)).max().unwrap();
    let siblings = entries
        .iter()
        .filter(|(n, _)| best == 0 || common(n) == best)
        .collect::<Vec<_>>();
    let (anchor, before) = siblings
        .iter()
        .find(|(n, _)| n.as_str() > id)
        .map_or((*siblings.last().unwrap(), false), |v| (*v, true));
    let (_, at) = anchor;
    let ae = block_end(lines, *at, ind, end);
    let find = members(lines, *at, ae).0.unwrap_or(ind + 2);
    let mut new = entry_lines(
        id,
        body,
        ind,
        find,
        inline(&lines[*at]).starts_with('{'),
        identities,
    )?;
    let mut pos = if before { *at } else { ae };
    if before {
        if pos > 0 && lines[pos - 1].trim().is_empty() && pos - 1 > start {
            new.push(String::new());
        }
    } else if pos < end && lines[pos].trim().is_empty() {
        pos += 1;
        new.push(String::new());
    }
    lines.splice(pos..pos, new);
    Ok(())
}
/// Edit one physical member of a world before the whole-token rename.
#[cfg(test)]
pub(crate) fn same_file(
    source: &str,
    survivor: &str,
    retired: &str,
    raw: &crate::history_contract::Map,
    merged: Option<&Source>,
    deps: &str,
    snapshot: &str,
) -> Result<String> {
    same_file_with_identity(
        source,
        survivor,
        retired,
        raw,
        merged,
        deps,
        snapshot,
        &FieldIdentity::new(),
    )
}
#[allow(clippy::too_many_arguments)] // The source and identity accompany the same per-world edit contract.
fn same_file_with_identity(
    source: &str,
    survivor: &str,
    retired: &str,
    raw: &Map,
    merged: Option<&Source>,
    deps: &str,
    snapshot: &str,
    identities: &FieldIdentity,
) -> Result<String> {
    let mut lines = source.split('\n').map(str::to_owned).collect::<Vec<_>>();
    for (id, body) in raw {
        if id == survivor || id == retired || locate(&lines, id).is_none() {
            continue;
        }
        let Ok(body) = map(body) else {
            continue;
        };
        let both_deps = body
            .get(deps)
            .and_then(|v| if let V::List(v) = v { Some(v) } else { None })
            .filter(|v| v.contains(&s(survivor)) && v.contains(&s(retired)));
        let both_seen = body
            .get(snapshot)
            .and_then(|v| map(v).ok())
            .is_some_and(|m| m.contains_key(survivor) && m.contains_key(retired));
        if both_deps.is_none() && !both_seen {
            continue;
        }
        let (_, ind, start, mut end) = locate(&lines, id).unwrap();
        let find = members(&lines, start, end).0.unwrap_or(ind + 2);
        if let Some(values) = both_deps {
            let value = Source::from_typed(&V::List(
                values
                    .iter()
                    .filter(|v| **v != s(retired))
                    .cloned()
                    .collect(),
            ));
            end = replace_field(
                &mut lines,
                start,
                end,
                deps,
                field_lines(deps, &value, find)?,
            );
        }
        if both_seen {
            drop_key(&mut lines, start, end, snapshot, retired)?;
        }
    }
    let mut removed = None;
    if raw.contains_key(retired)
        && let Some((collection, _, start, end)) = locate(&lines, retired)
    {
        let block = lines[start..end].to_vec();
        let after = if end < lines.len()
            && lines[end].trim().is_empty()
            && start > 0
            && lines[start - 1].trim().is_empty()
        {
            end + 1
        } else {
            end
        };
        lines.drain(start..after);
        if let Some((_, start, end)) = collections(&lines)
            .into_iter()
            .find(|(name, _, _)| *name == collection)
            && members(&lines, start, end).1.is_empty()
        {
            lines.drain(start..end);
        }
        removed = Some((collection, block));
    }
    if let Some(merged) = merged {
        if locate(&lines, survivor).is_some() {
            set_fields(
                &mut lines,
                survivor,
                merged,
                &Source::from_typed(&raw[survivor]),
                identities,
            )?;
        } else if !raw.contains_key(survivor)
            && let Some((collection, _)) = &removed
        {
            add_in(&mut lines, survivor, merged, collection, identities)?;
        }
    }
    if let Some((_, block)) = removed
        && let Some((_, ind, start, end)) = locate(&lines, survivor)
    {
        let find = members(&lines, start, end).0.unwrap_or(ind + 2);
        let comments = block
            .iter()
            .filter(|l| l.trim().starts_with('#'))
            .map(|l| format!("{}{}", " ".repeat(find), l.trim()))
            .collect::<Vec<_>>();
        lines.splice(end..end, comments);
    }
    Ok(lines.join("\n"))
}
pub(crate) fn append_also(source: &str, survivor: &str, also: &[String]) -> Result<String> {
    let mut lines = source.split('\n').map(str::to_owned).collect::<Vec<_>>();
    let Some((_, ind, start, mut end)) = locate(&lines, survivor) else {
        return Ok(source.into());
    };
    if inline(&lines[start]).starts_with('{') {
        let block = lines[start..end].join("\n");
        let value = format!("also: [{}]", also.join(", "));
        let changed = if Regex::new(r"\balso:").unwrap().is_match(&block) {
            Regex::new(&format!(r"\balso:\s*(\[[^\]]*\]|{FLOW})"))
                .unwrap()
                .replace_all(&block, regex::NoExpand(&value))
                .into_owned()
        } else {
            let at = block
                .rfind('}')
                .ok_or_else(|| error("invalid_identity_yaml"))?;
            format!("{}, {value}{}", block[..at].trim_end(), &block[at..])
        };
        lines.splice(start..end, changed.split('\n').map(str::to_owned));
    } else {
        let find = members(&lines, start, end).0.unwrap_or(ind + 2);
        let new = field_lines(
            "also",
            &Source::List(also.iter().map(|v| Source::Scalar(s(v))).collect()),
            find,
        )?;
        if field_span(&lines, start, end, "also").is_some() {
            replace_field(&mut lines, start, end, "also", new);
        } else {
            while end > start + 1 && lines[end - 1].trim().starts_with('#') {
                end -= 1;
            }
            lines.splice(end..end, new);
        }
    }
    Ok(lines.join("\n"))
}
fn sections(brief: &V) -> Vec<&V> {
    let mut sections = match get(brief, "sections") {
        V::List(a) => a.iter().filter(|v| matches!(v, V::Map(_))).collect(),
        _ => vec![],
    };
    if let V::List(tabs) = get(brief, "tabs") {
        for tab in tabs {
            if let V::List(a) = get(tab, "sections") {
                sections.extend(a.iter().filter(|v| matches!(v, V::Map(_))));
            }
        }
    }
    sections
}
fn section_span(lines: &[String], title: &str) -> Option<(usize, usize)> {
    let item = Regex::new(r"^( *)- +title:\s*(.+?)\s*$").unwrap();
    for (i, line) in lines.iter().enumerate() {
        let Some(m) = item.captures(line) else {
            continue;
        };
        let got = m[2].trim();
        let got = if got.starts_with(['\'', '"']) {
            got.get(1..got.len().saturating_sub(1)).unwrap_or("")
        } else {
            got
        };
        if got != title {
            continue;
        }
        let ind = m[1].len();
        let mut j = i + 1;
        while j < lines.len() && (lines[j].trim().is_empty() || indent(&lines[j]) > ind) {
            j += 1;
        }
        while j > i + 1 && lines[j - 1].trim().is_empty() {
            j -= 1;
        }
        if field_span(lines, i, j, "text").is_some() {
            return Some((i, j));
        }
    }
    None
}
pub(crate) fn brief_file(source: &str, survivor: &str, retired: &str) -> Result<String> {
    let brief = crate::history_yaml::decode_ordinary_source_value(source.as_bytes())?.projected();
    let mut lines = source.split('\n').map(str::to_owned).collect::<Vec<_>>();
    for section in sections(&brief) {
        if map(get(section, "seen"))
            .is_ok_and(|m| m.contains_key(survivor) && m.contains_key(retired))
            && crate::history_view::truth(get(section, "text"))
            && let Some((start, end)) = section_span(
                &lines,
                &crate::source_text::ordinary_python_str(get(section, "title")),
            )
        {
            drop_key(&mut lines, start, end, "seen", retired)?;
        }
    }
    if map(get(&brief, "labels")).is_ok_and(|m| m.contains_key(survivor) && m.contains_key(retired))
        && let Some((_, start, end)) = collections(&lines)
            .into_iter()
            .find(|(name, _, _)| name == "labels")
    {
        let (ind, labels) = members(&lines, start, end);
        if let Some((_, at)) = labels.into_iter().find(|(name, _)| name == retired) {
            let end = block_end(&lines, at, ind.unwrap(), end);
            lines.drain(at..end);
        }
    }
    Ok(lines.join("\n"))
}
fn template_refs(value: &V, id: &str) -> bool {
    let V::Text(value) = value else {
        return false;
    };
    Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*\}\}")
        .unwrap()
        .captures_iter(value)
        .any(|m| &m[1] == id)
}
fn iterable_text(value: &V) -> Vec<String> {
    match value {
        V::List(a) => a
            .iter()
            .map(crate::source_text::ordinary_python_str)
            .collect(),
        V::Map(m) => m.keys().cloned().collect(),
        V::Text(s) => s.chars().map(|c| c.to_string()).collect(),
        _ => vec![],
    }
}
pub(crate) fn mentions(
    retired: &str,
    worlds: &[(Option<String>, crate::history_contract::Map)],
    fields: &crate::history_contract::Map,
    brief: Option<&V>,
    sources: &super::ordinary_sameness::Sources<'_>,
) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = vec![];
    let mut hit = |kind: &str, at: String| {
        if let Some((_, places)) = out.iter_mut().find(|(k, _)| k == kind) {
            places.push(at);
        } else {
            out.push((kind.into(), vec![at]));
        }
    };
    let field = |name: &str| {
        fields
            .get(name)
            .and_then(|v| text(v).ok())
            .unwrap_or("")
            .to_owned()
    };
    let (deps, snapshot, predicate) = (field("deps"), field("snapshot"), field("predicate"));
    for (name, raw) in worlds {
        let tag = name
            .as_ref()
            .map_or(String::new(), |name| format!(" (in hypothesis {name})"));
        for (id, body) in raw {
            if id == retired || !matches!(body, V::Map(_)) {
                continue;
            }
            if matches!(get(body, &deps), V::List(a) if a.contains(&s(retired))) {
                hit(&deps, format!("{id}{tag}"));
            }
            if map(get(body, &snapshot)).is_ok_and(|m| m.contains_key(retired)) {
                hit(&snapshot, format!("{id}{tag}"));
            }
            let references = |v: &V| {
                matches!(v, V::Map(_) if crate::reasoning_language::legacy_references(v).iter().any(|v| v == retired))
                    || matches!(v, V::Text(v) if token_count(v, retired, "").1 > 0)
            };
            if references(get(body, &predicate)) {
                hit(&predicate, format!("{id}{tag}"));
            }
            if references(&rule_of(body)) {
                hit("rule", format!("{id}{tag}"));
            }
            let source = name
                .as_ref()
                .and_then(|n| sources.hypotheses.get(n).copied())
                .or_else(|| if name.is_none() { sources.base } else { None });
            let ordered = source.and_then(|source| {
                if let crate::history_yaml::OrdinaryValue::Map(collections) = source {
                    collections.iter().find_map(|(_, v)| v.get(id))
                } else {
                    None
                }
            });
            let body_map = map(body).unwrap();
            let fields = if let Some(crate::history_yaml::OrdinaryValue::Map(fields)) = ordered {
                fields
                    .iter()
                    .filter_map(|(k, _)| k.text())
                    .collect::<Vec<_>>()
            } else {
                body_map.keys().map(String::as_str).collect()
            };
            for field in fields {
                if body_map
                    .get(field)
                    .is_some_and(|v| template_refs(v, retired))
                {
                    hit("text", format!("{id} ({field}){tag}"));
                }
            }
            if matches!(get(body, "from"), V::Text(v) if v.split(whitespace).filter(|v| !v.is_empty()).collect::<Vec<_>>().join(" ") == retired)
            {
                hit("from", format!("{id}{tag}"));
            }
            if ids_value(get(body, "distinct_from")).contains(&retired.to_owned()) {
                hit("distinct_from", format!("{id}{tag}"));
            }
        }
    }
    if let Some(brief) = brief {
        let title = |value: &V| {
            let v = get(value, "title");
            let title = if crate::history_view::truth(v) {
                crate::source_text::ordinary_python_str(v)
            } else {
                "?".into()
            };
            crate::source_text::ordinary_python_repr(&s(&title))
        };
        if let V::List(tabs) = get(brief, "tabs") {
            for tab in tabs {
                if iterable_text(get(tab, "serves")).contains(&retired.to_owned()) {
                    hit("the brief", format!("tab {} (serves)", title(tab)));
                }
            }
        }
        for section in sections(brief) {
            let mut what = vec![];
            if template_refs(get(section, "text"), retired) {
                what.push("text");
            }
            if map(get(section, "seen")).is_ok_and(|m| m.contains_key(retired)) {
                what.push("seen");
            }
            let pick = get(section, "pick");
            let picks = if let V::Text(v) = pick {
                vec![v.clone()]
            } else {
                iterable_text(pick)
            };
            if picks.contains(&retired.to_owned()) {
                what.push("pick");
            }
            if !what.is_empty() {
                hit(
                    "the brief",
                    format!("section {} ({})", title(section), what.join(", ")),
                );
            }
        }
        let groups = get(brief, "groups");
        let groups = crate::history_emit::encode_source(&Source::from_typed(groups), 80)
            .ok()
            .and_then(|v| String::from_utf8(v).ok())
            .unwrap_or_default();
        if token_count(&groups, retired, "").1 > 0
            || map(get(brief, "labels")).is_ok_and(|m| m.contains_key(retired))
        {
            hit("the brief", "groups and labels".into());
        }
    }
    out
}
const READING: &[&str] = &[
    "v", "quoted", "rule", "unit", "from", "at", "of", "read", "url", "file",
];
fn get<'a>(value: &'a V, key: &str) -> &'a V {
    map(value).ok().and_then(|m| m.get(key)).unwrap_or(&V::Null)
}
fn shaped(value: &V, deps: &str) -> bool {
    matches!(get(value, deps), V::List(a) if !a.is_empty() && a.iter().all(|v| matches!(v, V::Text(_))))
}
fn rule_of(value: &V) -> V {
    match get(value, "rule") {
        v @ V::Map(_) => v.clone(),
        V::Text(v) if !v.trim().is_empty() => s(v),
        _ => match get(value, "v") {
            V::Text(v)
                if crate::ordinary_reader::EXPR.is_match(v)
                    && crate::ordinary_reader::ID.is_match(v) =>
            {
                s(v)
            }
            _ => V::Null,
        },
    }
}
fn claim(value: &V) -> V {
    if !matches!(value, V::Map(_)) {
        return value.clone();
    }
    let v = if get(value, "v") != &V::Null {
        get(value, "v")
    } else {
        get(value, "quoted")
    };
    if v != &V::Null
        && !matches!(v, V::Text(v) if crate::ordinary_reader::EXPR.is_match(v) && crate::ordinary_reader::ID.is_match(v))
    {
        v.clone()
    } else {
        rule_of(value)
    }
}
fn verdict(value: &V) -> &V {
    if get(value, "verdict") == &V::Null {
        get(value, "title")
    } else {
        get(value, "verdict")
    }
}
fn short(value: &V) -> String {
    crate::public_ordinary_readers::short(value, 40)
}
fn add_ordered(body: &mut Vec<(String, Source)>, key: &str, value: Source) {
    if let Some((_, old)) = body.iter_mut().find(|(k, _)| k == key) {
        *old = value;
    } else {
        body.push((key.into(), value));
    }
}
fn ids_value(value: &V) -> Vec<String> {
    let value = if crate::history_view::truth(value) {
        crate::source_text::ordinary_python_str(value)
    } else {
        String::new()
    };
    distinct_ids(&value)
        .into_iter()
        .map(str::to_owned)
        .collect()
}
pub(crate) struct Merge<'a, 'r> {
    pub survivor: &'a str,
    pub retired: &'a str,
    pub reader: &'a crate::ordinary_reader::Reader<'r>,
    pub as_of: Option<&'a str>,
}
pub(crate) type Supersede<'a> =
    dyn FnMut(&str, &V, &V, Option<&str>) -> Result<(bool, String)> + 'a;
impl Merge<'_, '_> {
    /// Preserve reading provenance and source field order; admission is supplied by the shared writer.
    #[cfg(test)]
    pub(crate) fn bodies(
        &self,
        sb: &Source,
        rb: &Source,
        supersede: &mut Supersede<'_>,
    ) -> Result<(Source, Vec<String>)> {
        self.bodies_with_origin(sb, rb, supersede)
            .map(|(body, notes, _)| (body, notes))
    }
    fn bodies_with_origin(
        &self,
        sb: &Source,
        rb: &Source,
        supersede: &mut Supersede<'_>,
    ) -> Result<(Source, Vec<String>, BTreeMap<String, bool>)> {
        let (Source::Map(sm), Source::Map(rm)) = (sb, rb) else {
            return Err(error("identity_invalid_body"));
        };
        let (sv, rv) = (sb.typed(), rb.typed());
        let (survivor, retired) = (self.survivor, self.retired);
        let mut notes = vec![];
        let mut body;
        let mut root_retired = false;
        let mut reading_retired = false;
        let mut name_generated = false;
        if shaped(&sv, text(&self.reader.fields()["deps"])?) {
            let (vs, vr) = (verdict(&sv), verdict(&rv));
            if vs != &V::Null && vr != &V::Null && !crate::ordinary_reader::same_legacy(vs, vr) {
                let (ok, why) = supersede(survivor, &sv, &rv, self.as_of)?;
                if !ok {
                    let vs = crate::source_text::ordinary_python_repr(&s(
                        &crate::public_ordinary_readers::short(vs, 60),
                    ));
                    let vr = crate::source_text::ordinary_python_repr(&s(
                        &crate::public_ordinary_readers::short(vr, 60),
                    ));
                    return Err(error(&format!(
                        "refused - {survivor} concludes {vs} and {retired} {vr} - {why}: two verdicts on one subject are a contradiction, not one subject twice; keep the one that holds, or write the other through add --hypothesis, for a person to fold"
                    )));
                }
                body = rm.clone();
                root_retired = true;
                notes.push(format!("{survivor} takes {retired}'s verdict - {why}"));
            } else {
                body = sm.clone();
            }
        } else {
            let (cs, cr) = (claim(&sv), claim(&rv));
            let rules = [&sv, &rv].iter().all(|b| {
                get(b, "rule") != &V::Null
                    && get(b, "v") == &V::Null
                    && get(b, "quoted") == &V::Null
            });
            let equal = if rules && matches!(cs, V::Map(_)) && matches!(cr, V::Map(_)) {
                python_equal(&cs, &cr) || {
                    let lower = |v: &V| {
                        crate::reasoning_language::legacy_expression(v, false)
                            .or_else(|_| crate::reasoning_language::legacy_expression(v, true))
                    };
                    lower(&cs)
                        .ok()
                        .zip(lower(&cr).ok())
                        .is_some_and(|(a, b)| a == b)
                }
            } else {
                crate::ordinary_reader::same_legacy(&cs, &cr)
            };
            let ds = crate::reasoning_authoring_guards::read_on(&sv, self.reader);
            let dr = crate::reasoning_authoring_guards::read_on(&rv, self.reader);
            let mut other = sm.is_empty();
            if !sm.is_empty() && cr != V::Null && (cs == V::Null || !equal) {
                if dr.is_some() && (ds.is_none() || dr >= ds) {
                    let (ok, why) = supersede(survivor, &sv, &cr, dr.as_deref())?;
                    if !ok {
                        return Err(error(&format!(
                            "refused - {survivor} holds {} as of {} and {retired} holds {} as of {} - {why}: two readings that disagree are a contradiction, not one subject twice; set the one that is right, or open a hypothesis, then same",
                            short(&cs),
                            ds.as_deref().unwrap_or("None"),
                            short(&cr),
                            dr.as_deref().unwrap()
                        )));
                    }
                    other = true;
                    notes.push(format!(
                        "{survivor} takes {retired}'s reading: {} -> {} - {why}",
                        short(&cs),
                        short(&cr)
                    ));
                } else if ds.is_none() && dr.is_none() {
                    return Err(error(&format!(
                        "refused - {survivor} holds {} and {retired} holds {}, and nothing dates either reading: two readings that disagree are a contradiction, not one subject twice; set the one that is right, or open a hypothesis, then same",
                        short(&cs),
                        short(&cr)
                    )));
                } else {
                    notes.push(format!(
                        "{survivor} keeps its reading {} as of {}; {retired}'s {}{} is older",
                        short(&cs),
                        ds.as_deref().unwrap_or("None"),
                        short(&cr),
                        dr.as_ref()
                            .map_or(", undated".into(), |d| format!(" as of {d}"))
                    ));
                }
            } else if !sm.is_empty()
                && cs == V::Null
                && cr == V::Null
                && dr.is_some()
                && (ds.is_none() || dr > ds)
            {
                other = true;
                notes.push(format!(
                    "{survivor} takes {retired}'s reading of {}",
                    dr.as_deref().unwrap()
                ));
            }
            if sm.is_empty() {
                body = rm.clone();
                root_retired = true;
            } else if other {
                reading_retired = true;
                body = vec![];
                for (field, value) in sm {
                    if READING.contains(&field.as_str()) && field != "unit" {
                        if let Some((_, value)) = rm.iter().find(|(k, _)| k == field) {
                            body.push((field.clone(), value.clone()));
                        }
                    } else {
                        body.push((field.clone(), value.clone()));
                    }
                }
                for field in READING {
                    if !body.iter().any(|(k, _)| k == field)
                        && let Some((_, value)) = rm.iter().find(|(k, _)| k == field)
                    {
                        body.push(((*field).into(), value.clone()));
                    }
                }
                for (field, value) in rm {
                    if !body.iter().any(|(k, _)| k == field)
                        && !READING.contains(&field.as_str())
                        && !["also", "distinct_from"].contains(&field.as_str())
                    {
                        body.push((field.clone(), value.clone()));
                    }
                }
            } else {
                body = sm.clone();
                for (field, value) in rm {
                    if !body.iter().any(|(k, _)| k == field)
                        && !READING.contains(&field.as_str())
                        && !["also", "distinct_from"].contains(&field.as_str())
                    {
                        body.push((field.clone(), value.clone()));
                    }
                }
            }
        }
        if !crate::history_view::truth(get(&Source::Map(body.clone()).typed(), "name")) {
            let name = crate::reasoning_authoring::named(&rv);
            if !name.is_empty() {
                add_ordered(&mut body, "name", Source::Scalar(s(&name)));
                name_generated = true;
            }
        }
        body.retain(|(k, _)| !["also", "distinct_from"].contains(&k.as_str()));
        let mut also = vec![];
        let mut distinct = vec![];
        for src in [&sv, &rv] {
            let aliases = match get(src, "also") {
                V::Text(v) => vec![s(v)],
                V::List(a) => a.clone(),
                _ => vec![],
            };
            for alias in aliases {
                if alias != s(survivor) && !also.iter().any(|a| python_equal(a, &alias)) {
                    also.push(alias);
                }
            }
            for id in ids_value(get(src, "distinct_from")) {
                if id != survivor && id != retired && !distinct.contains(&id) {
                    distinct.push(id);
                }
            }
        }
        if !also.iter().any(|v| *v == s(retired)) {
            also.push(s(retired));
        }
        body.push(("also".into(), Source::from_typed(&V::List(also))));
        if !distinct.is_empty() {
            body.push((
                "distinct_from".into(),
                Source::Scalar(s(&distinct.join(", "))),
            ));
        }
        let origins = body
            .iter()
            .filter(|(field, _)| {
                !["also", "distinct_from"].contains(&field.as_str())
                    && !(field == "name" && name_generated)
            })
            .map(|(field, _)| {
                let from_retired = root_retired
                    || (reading_retired && READING.contains(&field.as_str()) && field != "unit")
                    || !sm.iter().any(|(key, _)| key == field);
                (field.clone(), from_retired)
            })
            .collect();
        Ok((Source::Map(body), notes, origins))
    }
}
pub(crate) fn bump_updated(lines: &mut Vec<String>, stamp: &str) -> Result<bool> {
    let re = Regex::new(r"^(\s+updated:\s*)(\S+)(\s*(?:#.*)?)$").unwrap();
    for (name, start, end) in collections(lines) {
        if name != "meta" {
            continue;
        }
        if inline(&lines[start]).starts_with('{') {
            let block = lines[start..end].join("\n");
            let (graph, root) = parse(&block)?;
            let Kind::Map(root) = &graph.nodes[root].kind else {
                return Err(error("invalid_identity_yaml"));
            };
            let metadata = root[0].1;
            let Kind::Map(pairs) = &graph.nodes[metadata].kind else {
                return Err(error("invalid_identity_yaml"));
            };
            let changed = if let Some((_, id)) = pairs
                .iter()
                .find(|(k, _)| graph.scalar(*k) == Some("updated"))
            {
                let n = &graph.nodes[*id];
                format!(
                    "{}{}{}",
                    &block[..n.start],
                    string_scalar(stamp, &block[n.start..n.end]),
                    &block[n.end..]
                )
            } else {
                let at = graph.nodes[metadata].start + 1;
                format!(
                    "{}updated: {}{}{}",
                    &block[..at],
                    string_scalar(stamp, ""),
                    if pairs.is_empty() { "" } else { ", " },
                    &block[at..]
                )
            };
            lines.splice(start..end, changed.split('\n').map(str::to_owned));
            return Ok(true);
        }
        for line in lines.iter_mut().take(end).skip(start + 1) {
            if let Some(m) = re.captures(line) {
                *line = format!("{}{}{}", &m[1], string_scalar(stamp, &m[2]), &m[3]);
                return Ok(true);
            }
        }
        let ind = members(lines, start, end).0.unwrap_or(2);
        lines.insert(start + 1, format!("{}updated: {stamp}", " ".repeat(ind)));
        return Ok(true);
    }
    Ok(false)
}
/// The ordinary `distinct` line edit. Validation and publication belong to the caller.
pub(crate) fn distinct_text(
    source: &str,
    id: &str,
    ids: &str,
    why: &str,
    stamp: &str,
    in_hypothesis: bool,
) -> Result<String> {
    let mut lines = source.split('\n').map(str::to_owned).collect::<Vec<_>>();
    let (_, ind, start, end) =
        locate(&lines, id).ok_or_else(|| error("identity_target_not_found"))?;
    let written = string_scalar(ids, "");
    let comment = format!("# distinct {stamp}: {why}");
    if inline(&lines[start]).starts_with('{') {
        let block = lines[start..end].join("\n");
        let re = Regex::new(r"\bdistinct_from:").unwrap();
        let changed = if re.is_match(&block) {
            Regex::new(&format!(r"\bdistinct_from:\s*{FLOW}"))
                .unwrap()
                .replace_all(
                    &block,
                    regex::NoExpand(&format!("distinct_from: {written}")),
                )
                .into_owned()
        } else {
            let at = block
                .rfind('}')
                .ok_or_else(|| error("invalid_identity_yaml"))?;
            format!(
                "{}, distinct_from: {}{}",
                block[..at].trim_end(),
                written,
                &block[at..]
            )
        };
        let mut new = changed.split('\n').map(str::to_owned).collect::<Vec<_>>();
        new.push(format!("{}{comment}", " ".repeat(ind + 2)));
        lines.splice(start..end, new);
    } else {
        let find = members(&lines, start, end).0.unwrap_or(ind + 2);
        let end = replace_field(
            &mut lines,
            start,
            end,
            "distinct_from",
            vec![format!("{}distinct_from: {written}", " ".repeat(find))],
        );
        lines.insert(end, format!("{}{comment}", " ".repeat(find)));
    }
    if !in_hypothesis {
        bump_updated(&mut lines, stamp)?;
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    /// `advanced` makes the directory a Git project, which is Advanced by default.
    fn ordinary_record(advanced: bool) -> (tempfile::TempDir, BTreeMap<PathBuf, Vec<u8>>) {
        let temp = tempfile::tempdir().unwrap();
        if advanced {
            assert!(
                std::process::Command::new("git")
                    .args(["init", "-q"])
                    .current_dir(temp.path())
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let files = [
            (
                "GROUNDING.yaml",
                "record: parts.yaml\nschema: {deps: rests_on, predicate: wrong_if, snapshot: seen}\nknown:\n  p.keep: {v: 1}\n",
            ),
            ("parts.yaml", "known:\n  p.old: {v: 1}\n"),
            (
                ".kpopper/hypotheses/alpha.yaml",
                "known:\n  p.old: {v: 1}\n  p.other: {v: 2}\n",
            ),
            (
                ".kpopper/view.yaml",
                "sections:\n  - title: Summary\n    text: '{{p.old}}'\n",
            ),
        ];
        let mut before = BTreeMap::new();
        for (path, text) in files {
            let path = temp.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            before.insert(path, text.as_bytes().to_vec());
        }
        (temp, before)
    }
    fn request() -> super::super::Request {
        super::super::Request {
            a: "p.keep".into(),
            b: "p.old".into(),
            action: crate::history_identity::Action::Same { keep: None },
            as_of: Some("2026-09-20".into()),
            record: Some(PathBuf::from("GROUNDING.yaml")),
        }
    }
    #[test]
    fn ordinary_identity_guards_all_captured_participants_before_publication() {
        for (advanced, changed) in [false, true].into_iter().flat_map(|advanced| {
            [
                "parts.yaml",
                ".kpopper/view.yaml",
                ".kpopper/hypotheses/alpha.yaml",
                ".kpopper/hypotheses/appeared.yaml",
            ]
            .map(|changed| (advanced, changed))
        }) {
            let (temp, before) = ordinary_record(advanced);
            let path = temp.path().join(changed);
            let mut reached = false;
            let result =
                super::super::run_with_runtime(request(), temp.path(), None, &mut |stage| {
                    if stage == "prepared" {
                        reached = true;
                        std::fs::write(&path, "known: {p.concurrent: {v: 9}}\n")?;
                    }
                    Ok(())
                });
            assert!(
                result.is_err(),
                "accepted concurrent change to {changed} (advanced: {advanced})"
            );
            assert!(
                reached,
                "failed before reaching guarded publication (advanced: {advanced}): {result:?}"
            );
            for (file, expected) in &before {
                if file != &path {
                    assert_eq!(
                        &std::fs::read(file).unwrap(),
                        expected,
                        "{}",
                        file.display()
                    );
                }
            }
            assert!(
                !temp
                    .path()
                    .join(layout(&temp.path().join("GROUNDING.yaml")).unwrap().journal)
                    .exists()
            );
        }
    }
    #[test]
    fn ordinary_identity_recovery_retains_exact_multi_file_images() {
        for (advanced, rollback) in [(false, false), (false, true), (true, false), (true, true)] {
            let (temp, before) = ordinary_record(advanced);
            let entry = temp.path().canonicalize().unwrap().join("GROUNDING.yaml");
            let result =
                super::super::run_with_runtime(request(), temp.path(), None, &mut |stage| {
                    require(stage != "published", "interrupted")
                });
            assert_eq!(result.unwrap_err().0, "interrupted");
            let journal = temp.path().join(layout(&entry).unwrap().journal);
            assert!(journal.is_file());
            let after = before
                .keys()
                .map(|p| (p.clone(), std::fs::read(p).unwrap()))
                .collect::<BTreeMap<_, _>>();
            assert_ne!(before, after);
            crate::legacy_authoring::recover(std::slice::from_ref(&entry), temp.path(), rollback)
                .unwrap();
            assert!(!journal.exists());
            for (path, original) in &before {
                assert_eq!(
                    &std::fs::read(path).unwrap(),
                    if rollback { original } else { &after[path] },
                    "{} (advanced: {advanced}, rollback: {rollback})",
                    path.display()
                );
            }
        }
    }
    #[test]
    fn ordinary_identity_recovery_refuses_changed_untouched_inputs() {
        for advanced in [false, true] {
            let (temp, _) = ordinary_record(advanced);
            let entry = temp.path().canonicalize().unwrap().join("GROUNDING.yaml");
            // This distinct edit leaves the brief untouched, but its report read it.
            let mut intent = request();
            intent.action = crate::history_identity::Action::Distinct {
                because: "different subjects".into(),
            };
            let result = super::super::run_with_runtime(intent, temp.path(), None, &mut |stage| {
                require(stage != "published", "interrupted")
            });
            assert_eq!(result.unwrap_err().0, "interrupted", "advanced: {advanced}");
            std::fs::write(temp.path().join(".kpopper/view.yaml"), "sections: []\n").unwrap();
            assert!(
                crate::legacy_authoring::recover(std::slice::from_ref(&entry), temp.path(), false)
                    .is_err()
            );
            assert!(temp.path().join(layout(&entry).unwrap().journal).is_file());
        }
    }
    #[test]
    fn captured_text_edits_match_python() {
        let cases: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/ordinary-identity-text.json"
        ))
        .unwrap();
        for case in cases.as_array().unwrap() {
            let source = case["source"].as_str().unwrap();
            let actual = match case["kind"].as_str().unwrap() {
                "rewrite" => {
                    let (out, count) = rewrite_text(
                        source,
                        "p.old",
                        "p.keep",
                        case["predicate"].as_str().unwrap_or("wrong_if"),
                        case["snapshot"].as_str().unwrap_or("seen"),
                    )
                    .unwrap();
                    serde_json::json!([out, count])
                }
                "dedupe" => serde_json::json!(dedupe_text(source, "p.keep")),
                "distinct" => serde_json::json!(
                    distinct_text(
                        source,
                        "p.keep",
                        case["ids"].as_str().unwrap(),
                        case["why"].as_str().unwrap(),
                        "2026-09-20",
                        case["hypothesis"].as_bool().unwrap()
                    )
                    .unwrap()
                ),
                "same-file" => {
                    let doc = crate::history_yaml::decode_ordinary_source_value(source.as_bytes())
                        .unwrap()
                        .projected();
                    let raw = crate::reasoning_fields::collections(&doc)
                        .unwrap()
                        .into_values()
                        .flatten()
                        .collect();
                    let merged = crate::history_yaml::decode_source_value(
                        case["merged"].as_str().unwrap().as_bytes(),
                    )
                    .unwrap();
                    let changed = same_file(
                        source,
                        "p.keep",
                        "p.old",
                        &raw,
                        Some(&merged),
                        case["deps"].as_str().unwrap(),
                        case["snapshot"].as_str().unwrap(),
                    );
                    if let Some(failure) = case["error"].as_str() {
                        assert_eq!(changed.unwrap_err().0, failure, "{}", case["name"]);
                        continue;
                    }
                    let (changed, _) = rewrite_text(
                        &changed.unwrap(),
                        "p.old",
                        "p.keep",
                        "wrong_if",
                        case["snapshot"].as_str().unwrap(),
                    )
                    .unwrap();
                    let changed = dedupe_text(&changed, "p.keep");
                    let also = case["also"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap().to_owned())
                        .collect::<Vec<_>>();
                    let changed = append_also(&changed, "p.keep", &also).unwrap();
                    let mut lines = changed.split('\n').map(str::to_owned).collect::<Vec<_>>();
                    bump_updated(&mut lines, "2026-09-20").unwrap();
                    serde_json::json!(lines.join("\n"))
                }
                "merge" => {
                    let doc = crate::history_yaml::decode_ordinary_source_value(source.as_bytes())
                        .unwrap()
                        .projected();
                    let reader = crate::ordinary_reader::Reader::new(&doc, None).unwrap();
                    let left = crate::history_yaml::decode_source_value(
                        case["left"].as_str().unwrap().as_bytes(),
                    )
                    .unwrap();
                    let right = crate::history_yaml::decode_source_value(
                        case["right"].as_str().unwrap().as_bytes(),
                    )
                    .unwrap();
                    let mut calls = vec![];
                    let mut admission = |id: &str, old: &V, new: &V, stamp: Option<&str>| {
                        calls.push(serde_json::json!([
                            id,
                            crate::ordinary_reader::json_value(old, 0).unwrap(),
                            crate::ordinary_reader::json_value(new, 0).unwrap(),
                            stamp
                        ]));
                        Ok((
                            case["allow"].as_bool().unwrap(),
                            "oracle admission".to_owned(),
                        ))
                    };
                    let result = Merge {
                        survivor: "p.keep",
                        retired: "p.old",
                        reader: &reader,
                        as_of: Some("2026-09-20"),
                    }
                    .bodies(&left, &right, &mut admission);
                    assert_eq!(serde_json::json!(calls), case["calls"], "{}", case["name"]);
                    if let Some(failure) = case["expected"]["error"].as_str() {
                        assert_eq!(result.unwrap_err().0, failure, "{}", case["name"]);
                    } else {
                        let (body, notes) = result.unwrap();
                        let expected = crate::history_yaml::decode_source_value(
                            case["expected"]["body"].as_str().unwrap().as_bytes(),
                        )
                        .unwrap();
                        assert_eq!(
                            crate::history_emit::encode_source(&body, 100).unwrap(),
                            crate::history_emit::encode_source(&expected, 100).unwrap(),
                            "{}",
                            case["name"]
                        );
                        assert_eq!(
                            serde_json::json!(notes),
                            case["expected"]["notes"],
                            "{}",
                            case["name"]
                        );
                    }
                    continue;
                }
                "brief-file" => {
                    let record = crate::history_yaml::decode_ordinary_source_value(
                        case["record"].as_str().unwrap().as_bytes(),
                    )
                    .unwrap();
                    let doc = record.projected();
                    let reader = crate::ordinary_reader::Reader::new(&doc, None).unwrap();
                    let raw = crate::reasoning_fields::collections(&doc)
                        .unwrap()
                        .into_values()
                        .flatten()
                        .collect();
                    let brief =
                        crate::history_yaml::decode_ordinary_source_value(source.as_bytes())
                            .unwrap()
                            .projected();
                    let got = mentions(
                        "p.old",
                        &[(None, raw)],
                        reader.fields(),
                        Some(&brief),
                        &super::super::ordinary_sameness::Sources {
                            base: Some(&record),
                            hypotheses: BTreeMap::new(),
                        },
                    );
                    assert_eq!(serde_json::json!(got), case["mentions"], "{}", case["name"]);
                    let changed = brief_file(source, "p.keep", "p.old").unwrap();
                    let (changed, _) =
                        rewrite_text(&changed, "p.old", "p.keep", "wrong_if", "seen").unwrap();
                    serde_json::json!(dedupe_text(&changed, "p.keep"))
                }
                _ => panic!("unknown text fixture"),
            };
            assert_eq!(actual, case["expected"], "{}", case["name"]);
        }
    }
}

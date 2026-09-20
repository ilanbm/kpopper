use super::{Options, Result, search_captures, search_corpus as C};
use crate::{
    history_contract::{Map, map, text},
    history_view::truth,
    history_yaml::OrdinaryValue as O,
    identity::sha256,
    ordinary_reader as R,
    public_ordinary_readers::{self as P, Projection, World},
    reasoning_authoring as A, reasoning_fields as F, reasoning_language as L,
    source_capture::{ReadMode, capture_source_with_runtime},
    source_inventory::{Inventory, name},
    value::TypedValue as V,
};
use C::{get, json_value};
use serde_json::{Value as J, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
fn sort_error(value: &O) -> Option<String> {
    fn kind(v: &V) -> &'static str {
        match v {
            V::Text(_) => "str",
            V::Bool(_) => "bool",
            V::Integer(_) => "int",
            V::Float(_) => "float",
            V::Null => "NoneType",
            V::Date(_) => "date",
            V::DateTime(_) => "datetime",
            _ => "object",
        }
    }
    match value {
        O::Map(fields) => {
            let mut sorted: Vec<usize> = vec![];
            for (index, (key, _)) in fields.iter().enumerate() {
                let value = key.scalar();
                let mut at = sorted.len();
                while at > 0 {
                    let other = fields[sorted[at - 1]].0.scalar();
                    let less = match (value, other) {
                        (V::Text(a), V::Text(b)) => a < b,
                        (V::Null, V::Null) => false,
                        _ => {
                            if let (Some((an, ad)), Some((bn, bd))) = (
                                crate::ordinary_assessment::number(value),
                                crate::ordinary_assessment::number(other),
                            ) {
                                an * bd < bn * ad
                            } else if kind(value) == kind(other) {
                                crate::source_text::ordinary_python_str(value)
                                    < crate::source_text::ordinary_python_str(other)
                            } else {
                                return Some(format!(
                                    "'<' not supported between instances of '{}' and '{}'",
                                    kind(value),
                                    kind(other)
                                ));
                            }
                        }
                    };
                    if !less {
                        break;
                    }
                    at -= 1;
                }
                sorted.insert(at, index);
            }
            for index in sorted {
                let (key, value) = &fields[index];
                if matches!(key.scalar(), V::Date(_) | V::DateTime(_)) {
                    return Some(format!(
                        "keys must be str, int, float, bool or None, not {}",
                        kind(key.scalar())
                    ));
                }
                if let Some(error) = sort_error(value) {
                    return Some(error);
                }
            }
            None
        }
        O::List(a) => a.iter().find_map(sort_error),
        _ => None,
    }
}
fn names(value: &V) -> Vec<String> {
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
fn rule_ids(id: &str, body: &V, ids: &BTreeSet<String>) -> Vec<String> {
    let mut refs = BTreeSet::new();
    if matches!(get(body, "rule"), V::Map(_)) {
        refs.extend(L::legacy_references(get(body, "rule")));
    } else {
        for field in ["rule", "v"] {
            if let V::Text(t) = get(body, field)
                && R::EXPR.is_match(t)
            {
                refs.extend(R::ID.find_iter(t).map(|m| m.as_str().to_owned()));
            }
        }
    }
    refs.into_iter()
        .filter(|v| v != id && ids.contains(v))
        .collect()
}
fn source_ids(id: &str, world: &World<'_>) -> Vec<String> {
    let mut found = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut pending = vec![id.to_owned()];
    let dep = text(&world.reader.fields()["deps"]).unwrap();
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(body) = world.reader.raw().get(&id) else {
            continue;
        };
        let Ok(m) = map(body) else { continue };
        if !world.judgments.contains_key(&id)
            && !["v", "quoted", "rule"].iter().any(|k| m.contains_key(*k))
            && ["file", "url", "asked", "read"]
                .iter()
                .any(|k| m.get(*k).is_some_and(truth))
        {
            found.insert(id.clone());
        }
        if let Some(V::Text(citation)) = m.get("from")
            && world.reader.ids.contains(citation)
        {
            pending.push(citation.clone());
        }
        if world.judgments.contains_key(&id) {
            pending.extend(names(get(body, dep)));
        } else {
            pending.extend(rule_ids(&id, body, &world.reader.ids));
        }
    }
    found.into_iter().collect()
}

fn layer(base: &V, hyp: &V) -> crate::Result<V> {
    let mut out = base.clone();
    for (collection, members) in F::collections(hyp)? {
        for (name, value) in crate::history_view::map_mut(&mut out)?.iter_mut() {
            if *name != collection
                && let V::Map(m) = value
            {
                for id in members.keys() {
                    m.remove(id);
                }
            }
        }
        let current = crate::history_view::map_mut(&mut out)?
            .entry(collection)
            .or_insert_with(|| V::Map(Map::new()));
        if let V::Map(m) = current {
            m.extend(members);
        } else {
            *current = V::Map(members);
        }
    }
    Ok(out)
}
pub(super) fn corpus(selected: &Path, options: &Options, cwd: &Path, mode: ReadMode) -> Result<J> {
    let record = if mode == ReadMode::Live {
        crate::project_modes::write_paths(&[selected.to_owned()], cwd)?[0].clone()
    } else {
        selected.to_owned()
    };
    let roots = C::roots(&record, &options.source_roots, cwd)?;
    let runtime = crate::public_workspace::runtime()?;
    let capture = capture_source_with_runtime(
        std::slice::from_ref(&record),
        cwd,
        mode,
        None,
        runtime.as_ref(),
    )
    .map_err(|e| {
        if e.0 == "missing_record" {
            crate::Error("no record found; open or map this workspace first".into())
        } else {
            e
        }
    })?;
    crate::require(
        record.is_file()
            || capture
                .pending_observation()
                .is_some_and(|o| o.ledger.head.is_some()),
        "no record found; open or map this workspace first",
    )?;
    let document = capture.ordinary_document();
    let base_bodies = C::source_bodies(capture.source());
    let context = capture.knowledge_status_context();
    let conflicts = map(get(&context, "conflicts"))?;
    let projection = Projection::new(
        document,
        map(capture.hypotheses())?,
        conflicts,
        vec![],
        runtime.as_ref(),
    )?;
    let mut inventory = Inventory::default();
    let mut stamps = BTreeMap::new();
    for path in capture.members() {
        if path.is_file() {
            stamps.insert(name(path)?.to_owned(), sha256(&inventory.read(path)?));
        }
    }
    for hyp in map(capture.hypotheses())?.values() {
        if get(hyp, "kind") == &V::Text("contribution".into()) {
            continue;
        }
        if let Ok(path) = text(get(hyp, "path")) {
            let path = Path::new(path);
            if path.is_file() {
                stamps.insert(name(path)?.to_owned(), sha256(&inventory.read(path)?));
            }
        }
    }
    let brief = record
        .parent()
        .unwrap()
        .join(crate::history_migration_source::adjunct(
            record.file_name().unwrap().to_str().unwrap(),
            "view",
        ));
    if inventory.exists(&brief)? && brief.is_file() {
        stamps.insert(name(&brief)?.to_owned(), sha256(&inventory.read(&brief)?));
    }
    let mut origins = BTreeMap::new();
    let mut body_files = BTreeMap::new();
    let mut identity_documents = BTreeMap::new();
    for path in capture.members() {
        if let Some(raw) = capture.files().get(path) {
            let parsed = crate::history_yaml::decode_ordinary_source_value(raw)?.projected();
            for (_, members) in F::collections(&parsed)? {
                for id in members.keys() {
                    origins.insert(id.clone(), path.parent().unwrap().to_owned());
                    body_files.insert(id.clone(), path.clone());
                }
            }
        }
    }
    for path in capture.members().iter().chain(
        map(capture.hypotheses())?
            .values()
            .filter(|h| !truth(get(h, "error")))
            .filter_map(|h| text(get(h, "path")).ok())
            .map(Path::new)
            .map(|p| p.to_path_buf())
            .collect::<Vec<_>>()
            .iter(),
    ) {
        if let Some(raw) = capture.files().get(path) {
            identity_documents.insert(
                path.clone(),
                super::search_yaml_identity::Document::new(raw)?,
            );
        }
    }
    let mut captured = if mode == ReadMode::Live {
        Some(search_captures::read(
            &record,
            cwd,
            options.state_dir.as_deref(),
        )?)
    } else {
        None
    };
    let mut rows = captured
        .as_mut()
        .map(|c| std::mem::take(&mut c.rows))
        .unwrap_or_default();
    let mut diagnostics = captured
        .as_mut()
        .map(|c| std::mem::take(&mut c.diagnostics))
        .unwrap_or_default();
    let mut worlds = vec![("record".to_owned(), None, None)];
    for (name, hyp) in map(capture.hypotheses())? {
        if truth(get(hyp, "error")) {
            diagnostics.push(
                json!({"ref":format!("hypothesis:{name}"),"reason":json_value(get(hyp,"error"))?}),
            );
        } else {
            let kind = if get(hyp, "kind") == &V::Text("contribution".into()) {
                "contribution"
            } else {
                "hypothesis"
            };
            worlds.push((format!("{kind}:{name}"), Some(name.clone()), Some(hyp)));
        }
    }
    for (scope, _hypname, hyp) in worlds {
        let no_hypotheses = Map::new();
        let contribution = hyp.is_some_and(|h| get(h, "kind") == &V::Text("contribution".into()));
        let publication = hyp
            .filter(|_| contribution)
            .map(|h| get(get(h, "head"), "publication"));
        let bundle = if let Some(publication) = publication {
            let revision = text(get(publication, "revision"))?;
            Some(
                capture
                    .pending_observation()
                    .and_then(|o| o.ledger.bundles.get(revision))
                    .ok_or_else(|| {
                        crate::Error("immutable contribution evidence is unavailable".into())
                    })?,
            )
        } else {
            None
        };
        let world_doc = if let Some(hyp) = hyp {
            let doc = get(hyp, "doc");
            if contribution {
                doc.clone()
            } else {
                layer(document, doc)?
            }
        } else {
            document.clone()
        };
        let alternate = if hyp.is_some() {
            Some(Projection::new(
                &world_doc,
                if contribution {
                    &no_hypotheses
                } else {
                    map(capture.hypotheses())?
                },
                &Map::new(),
                vec![],
                runtime.as_ref(),
            )?)
        } else {
            None
        };
        let world = alternate
            .as_ref()
            .map(|p| &p.base)
            .unwrap_or(&projection.base);
        let own = if let Some(hyp) = hyp {
            if let V::List(ids) = get(hyp, "ids") {
                names(&V::List(ids.clone()))
            } else {
                F::collections(get(hyp, "doc"))?
                    .values()
                    .flat_map(|m| m.keys().cloned())
                    .collect()
            }
        } else {
            world.reader.ids.iter().cloned().collect()
        };
        let own = own.into_iter().collect::<BTreeSet<_>>();
        let physical = if let Some(hyp) = hyp {
            if let Ok(path) = text(get(hyp, "path")) {
                capture
                    .files()
                    .get(Path::new(path))
                    .map(|raw| crate::history_yaml::decode_ordinary_source_value(raw))
                    .transpose()?
            } else {
                None
            }
        } else {
            Some(capture.source().clone())
        };
        let physical_bodies = physical.as_ref().map(C::source_bodies).unwrap_or_default();
        let compute_error = world.reader.ids.iter().find_map(|id| {
            let body = world.reader.raw().get(id)?;
            let original = physical_bodies
                .get(id)
                .copied()
                .filter(|s| s.projected() == *body)
                .or_else(|| {
                    base_bodies
                        .get(id)
                        .copied()
                        .filter(|s| s.projected() == *body)
                });
            original.and_then(sort_error)
        });
        let guarded = if compute_error.is_some() {
            Some(Projection::new(
                &world_doc,
                if contribution {
                    &no_hypotheses
                } else {
                    map(capture.hypotheses())?
                },
                &Map::new(),
                vec![],
                None,
            )?)
        } else {
            None
        };
        let world = guarded.as_ref().map(|p| &p.base).unwrap_or(world);
        for id in own {
            if F::BUILTINS.contains(&id.as_str()) {
                continue;
            }
            let Some(body) = world.reader.raw().get(&id) else {
                continue;
            };
            let base = if scope == "record" {
                origins
                    .get(&id)
                    .map(PathBuf::as_path)
                    .unwrap_or(record.parent().unwrap())
            } else {
                record.parent().unwrap()
            };
            if mode == ReadMode::Frozen && matches!(body, V::Map(_)) {
                let source = match get(body, "file") {
                    V::Text(path) => Some(C::expanded(base, Path::new(path))?),
                    _ => None,
                };
                if crate::recording_privacy::private_marker(body)
                    || source.is_some_and(|p| {
                        !p.starts_with(record.parent().unwrap()) || p == record.parent().unwrap()
                    })
                {
                    diagnostics.push(json!({"ref":format!("node:{id}"),"reason":"private or external source excluded from frozen search"}));
                    continue;
                }
            }
            let sources = source_ids(&id, world);
            let judgment = world.judgments.contains_key(&id);
            let (tag, reason) = if judgment {
                world.state(&id)?
            } else {
                ("RECORDED".into(), String::new())
            };
            let reference = if scope == "record" {
                format!("node:{id}")
            } else {
                format!("{scope}:node:{id}")
            };
            let deps = if judgment {
                names(get(body, text(&world.reader.fields()["deps"])?))
            } else {
                vec![]
            };
            let mut row = json!({"ref":reference,"id":id,"kind":if judgment{"judgment"}else{"entry"},"scope":scope,"status":if publication.is_some(){"PENDING"}else if scope=="record"{&tag}else{"HYPOTHESIS"},"name":A::named(body),"sources":sources,"dependencies":deps,"reason":reason,"rule_dependencies":if judgment{vec![]}else{rule_ids(&id,body,&world.reader.ids)},"content":if let Some(document)=if let Some(hyp)=hyp{text(get(hyp,"path")).ok().and_then(|path|identity_documents.get(Path::new(path)))}else{body_files.get(&id).and_then(|path|identity_documents.get(path))}{match document.dump(&id,body)?{Some(text)=>text,None=>C::dump(body,physical.as_ref().and_then(|s|C::source_body(s,&id)))?}}else{C::dump(body,physical.as_ref().and_then(|s|C::source_body(s,&id)))?}});
            if let Some(publication) = publication {
                row["publication"] = json_value(publication)?;
                row["assessment"] = json!(tag);
            }
            if conflicts.contains_key(&id) {
                row["conflict"] = json!(true);
            }
            if judgment {
                row["assessment"] = json!(tag);
                row["falsifier_holds"] = json!(
                    world
                        .reader
                        .predicate(&R::predicate_of(body, world.reader.fields()))?
                );
            } else if matches!(get(body, "rule"), V::Map(_)) {
                let computed = R::compute(
                    world.reader.raw(),
                    &world.reader.ids,
                    None,
                    runtime.as_ref().and_then(|r| r.ordinary_program()),
                );
                row["calculation"] = if let Some(error) = &compute_error {
                    json!({"value":null,"reason":error})
                } else {
                    computed["values"][&id].clone()
                };
                if row["calculation"].is_null() {
                    row["calculation"] = json!({"value":null,"reason":computed.get("error").cloned().unwrap_or(json!("unavailable"))});
                }
                row["formula"] = json!(P::predicate_text(get(body, "rule")));
            }
            rows.push(row);
            if !matches!(body, V::Map(_)) || !sources.contains(&id) {
                continue;
            }
            let reference = if scope == "record" {
                format!("source:{id}")
            } else {
                format!("{scope}:source:{id}")
            };
            let V::Text(filename) = get(body, "file") else {
                if truth(get(body, "url")) {
                    diagnostics.push(json!({"ref":reference,"reason":"remote source not fetched"}));
                }
                continue;
            };
            let path = if bundle.is_some() {
                if filename.starts_with("~/") {
                    C::expanded(cwd, Path::new(filename))?
                } else {
                    PathBuf::from(filename)
                }
            } else {
                C::expanded(base, Path::new(filename))?
            };
            let path_text = name(&path)?.to_owned();
            if bundle.is_none()
                && captured
                    .as_ref()
                    .is_some_and(|c| c.invalid.contains(&path_text))
            {
                diagnostics.push(
                    json!({"ref":reference,"reason":"captured source integrity check failed"}),
                );
                continue;
            }
            let content = (|| -> std::result::Result<String, String> {
                if bundle.is_none()
                    && !captured
                        .as_ref()
                        .is_some_and(|c| c.files.contains_key(&path_text))
                    && !roots.iter().any(|r| path.starts_with(r))
                {
                    return Err("outside allowed source roots; pass --source-root for an authorized directory".into());
                }
                if !C::text_suffix(&path) {
                    return Err("only local UTF-8 text sources are indexed".into());
                }
                if let Some(bundle) = bundle {
                    let data = bundle.files.get(filename).ok_or_else(|| {
                        "immutable contribution evidence is unavailable".to_owned()
                    })?;
                    return C::decode_text(data.clone()).map(|(text, _)| text);
                }
                if !path.is_file() {
                    return Err("local source file is unavailable".into());
                }
                C::read_text(&path).map(|(text, _)| text)
            })();
            let content = match content {
                Ok(content) => content,
                Err(reason) => {
                    diagnostics.push(json!({"ref":reference,"reason":reason}));
                    continue;
                }
            };
            if bundle.is_none() {
                rows.retain(|r| !(r["kind"] == "capture" && r["file"] == path_text));
            }
            let locator = if let Some(publication) = publication {
                format!(
                    "git:{}:{}/{}",
                    text(get(publication, "ledger_ref"))?,
                    text(get(publication, "revision"))?,
                    filename
                )
            } else {
                path_text.clone()
            };
            let capture_state = if bundle.is_none() {
                captured
                    .as_ref()
                    .and_then(|c| c.files.get(&path_text))
                    .cloned()
                    .unwrap_or(J::Null)
            } else {
                J::Null
            };
            let mut row = json!({"ref":reference,"id":id,"kind":"source","scope":scope,"status":if publication.is_some(){"PENDING"}else if scope=="record"{"SOURCE"}else{"HYPOTHESIS"},"name":A::named(body),"content":content,"file":locator,"sources":[id],"dependencies":[],"capture_state":capture_state});
            if let Some(publication) = publication {
                row["publication"] = json_value(publication)?;
            }
            rows.push(row);
        }
    }
    capture
        .verify()
        .map_err(|_| crate::Error("record changed during search; retry".into()))?;
    inventory
        .verify()
        .map_err(|_| crate::Error("record changed during search; retry".into()))?;
    if let Some(captured) = captured {
        captured
            .inventory
            .verify()
            .map_err(|_| crate::Error("capture state changed during search; retry".into()))?;
    }
    if mode == ReadMode::Live {
        crate::require(
            crate::project_modes::write_paths(&[selected.to_owned()], cwd)? == [record.clone()],
            "record path changed during search; retry",
        )?;
    }
    let conflicts = conflicts
        .iter()
        .map(|(id, variants)| {
            let names = if let V::List(a) = variants {
                a.iter()
                    .filter_map(|v| if let V::List(a) = v { a.first() } else { None })
                    .map(json_value)
                    .collect::<crate::Result<Vec<_>>>()?
            } else {
                vec![]
            };
            Ok((id.clone(), J::Array(names)))
        })
        .collect::<crate::Result<serde_json::Map<_, _>>>()?;
    let context = json!({"read_mode":if mode==ReadMode::Frozen{"frozen"}else{"live"},"contributions":json_value(get(&context,"contributions"))?,"conflicts":conflicts,"target_unavailable":json_value(get(&context,"target_unavailable"))?});
    let mut preimage = context.as_object().unwrap().clone();
    preimage.extend(json!({"record":record,"files":stamps,"rows":rows,"unindexed":diagnostics,"source_roots":roots}).as_object().unwrap().clone());
    let revision = sha256(crate::public_search_rank::encode(&J::Object(preimage))?.as_bytes());
    let mut result = context.as_object().unwrap().clone();
    result.extend(json!({"record":record,"revision":revision,"revision_kind":"search-corpus","record_sha256":stamps.get(name(&record)?),"rows":rows,"unindexed":diagnostics}).as_object().unwrap().clone());
    Ok(J::Object(result))
}

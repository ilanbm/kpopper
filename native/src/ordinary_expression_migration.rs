//! Checked in-place migration of ordinary executable fields.
use super::{core_expression_conversion as C, core_expression_migration as Core};
use crate::{
    Error, Result,
    history_contract::{Map, map, text},
    history_view::map_mut,
    history_yaml::{OrdinaryValue as O, SourceValue as S},
    ordinary_reader::Reader,
    project_modes as P, reasoning_fields as F, reasoning_language as L, require,
    source_capture::{ReadMode, capture_source_with_runtime},
    source_inventory::name,
    value::TypedValue as V,
};
use serde_json::{Value as J, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
fn get<'a>(v: &'a V, k: &str) -> &'a V {
    if let V::Map(m) = v {
        m.get(k).unwrap_or(&V::Null)
    } else {
        &V::Null
    }
}
fn source(value: &O) -> Result<S> {
    Ok(match value {
        O::Scalar(v) => S::Scalar(v.clone()),
        O::List(a) => S::List(a.iter().map(source).collect::<Result<_>>()?),
        O::Map(m) => S::Map(
            m.iter()
                .map(|(k, v)| {
                    Ok((
                        k.text()
                            .ok_or_else(|| Error("invalid migration field key".into()))?
                            .to_owned(),
                        source(v)?,
                    ))
                })
                .collect::<Result<_>>()?,
        ),
    })
}
fn expression_source(value: &V) -> S {
    match value {
        V::Map(m) => {
            let mut keys = m.keys().collect::<Vec<_>>();
            if m.contains_key("op") && m.contains_key("args") {
                keys.sort_by_key(|k| usize::from(k.as_str() != "op"));
            }
            S::Map(
                keys.into_iter()
                    .map(|k| (k.clone(), expression_source(&m[k])))
                    .collect(),
            )
        }
        V::List(a) => S::List(a.iter().map(expression_source).collect()),
        _ => S::Scalar(value.clone()),
    }
}
fn readable(tree: &V, predicate: bool) -> Result<V> {
    let tree = L::legacy_expression_detailed(tree, predicate)?.to_json()?;
    fn render(tree: &J) -> Result<String> {
        if let Some(id) = tree.get("ref").and_then(J::as_str) {
            let bare = super::convert(id, false, false)
                .is_ok_and(|v| v["expression"] == json!({"ref":id}));
            return Ok(if bare {
                id.into()
            } else {
                format!("ref({})", serde_json::to_string(id)?)
            });
        }
        if tree.get("op").is_none() {
            return Ok(super::display(tree));
        }
        let args = tree["args"].as_array().unwrap();
        let symbol = match tree["op"].as_str().unwrap() {
            "add" => "+",
            "sub" => "-",
            "mul" => "*",
            "div" => "/",
            "eq" => "==",
            "ne" => "!=",
            "lt" => "<",
            "le" => "<=",
            "gt" => ">",
            "ge" => ">=",
            _ => unreachable!(),
        };
        Ok(format!(
            "({} {symbol} {})",
            render(&args[0])?,
            render(&args[1])?
        ))
    }
    let mut text = render(&tree)?;
    if tree.get("op").is_some() {
        text = text[1..text.len() - 1].to_owned();
    }
    let value = C::obj([("expr", C::s(&text))]);
    require(
        L::legacy_expression_detailed(&value, predicate)?.to_json()? == tree,
        "expression cannot be represented without changing its meaning",
    )?;
    Ok(value)
}
fn failures(
    document: &V,
    brief: Option<&V>,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<Vec<String>> {
    let projection = crate::public_ordinary_readers::Projection::new(
        document,
        &Map::new(),
        &Map::new(),
        vec![],
        runtime,
    )?;
    Ok(projection
        .check(brief)?
        .0
        .lines()
        .filter_map(|s| s.strip_prefix("FAIL ").map(str::to_owned))
        .collect())
}
pub(super) fn migrate(cwd: &Path, record: &Path, apply: bool, compact: bool) -> Result<J> {
    let record = P::resolved(&cwd.join(record))?;
    let record = P::write_paths(&[record], cwd)?[0].clone();
    let project = P::project_for(std::slice::from_ref(&record), cwd)?;
    let policy = project.config()?;
    let runtime = crate::public_workspace::runtime()?;
    let captured = {
        let guard = project.lock()?;
        require(
            guard.config() == &policy,
            "project mode or record destination changed; retry migration",
        )?;
        capture_source_with_runtime(
            std::slice::from_ref(&record),
            cwd,
            ReadMode::Frozen,
            None,
            runtime.as_ref(),
        )?
    };
    require(
        captured.members() == [record.clone()],
        "multi-file and pointer records require primary review",
    )?;
    require(
        map(captured.hypotheses())?.is_empty(),
        "a record with hypothesis context requires primary review",
    )?;
    let document = captured.ordinary_document();
    let before = captured
        .files()
        .get(&record)
        .ok_or_else(|| Error("record unavailable".into()))?
        .clone();
    let reader = Reader::new(document, runtime.as_ref())?;
    let fields = reader.fields();
    let deps_field = text(&fields["deps"])?;
    let predicate_field = text(&fields["predicate"])?;
    let all = crate::reasoning_snapshot::entries(document)?;
    let ids = reader.ids.clone();
    let judgments = all
        .iter()
        .filter(|(_, (_, b))| map(b).is_ok_and(|m| m.contains_key(deps_field)))
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let mut changes = vec![];
    let mut skipped = vec![];
    let mut bodies = BTreeMap::<String, V>::new();
    let mut body_order = vec![];
    for id in Core::source_order(captured.source()) {
        let Some((_, body)) = all.get(&id) else {
            continue;
        };
        let Ok(body) = map(body) else { continue };
        let predicate = judgments.contains(&id);
        let mut fields = vec![(if predicate { predicate_field } else { "rule" }, predicate)];
        if !predicate && !body.contains_key("rule") && C::implicit(body, &ids) {
            fields.push(("v", false));
        }
        for (field, predicate) in fields {
            let value = body.get(field).unwrap_or(&V::Null);
            let old_tree = compact && matches!(value,V::Map(m)if !m.contains_key("expr"));
            if !old_tree && !matches!(value,V::Text(s)if !s.trim().is_empty()) {
                continue;
            }
            let converted = (|| -> Result<V> {
                let tree = if old_tree {
                    L::legacy_expression_detailed(value, predicate)?
                } else {
                    let source = text(value)?;
                    super::authored_expression(source, predicate)?
                };
                let refs = L::legacy_references(&tree);
                let unknown = refs
                    .iter()
                    .filter(|id| !ids.contains(*id))
                    .cloned()
                    .collect::<BTreeSet<_>>();
                require(
                    unknown.is_empty(),
                    &format!(
                        "unknown or ambiguous bare names: {}",
                        unknown.into_iter().collect::<Vec<_>>().join(", ")
                    ),
                )?;
                if predicate {
                    let deps = body
                        .get(deps_field)
                        .and_then(|v| if let V::List(a) = v { Some(a) } else { None });
                    require(
                        refs.iter()
                            .all(|id| deps.is_some_and(|d| d.contains(&C::s(id)))),
                        "predicate references are not declared in rests_on",
                    )?;
                }
                if compact {
                    readable(&tree, predicate)
                } else {
                    Ok(tree)
                }
            })();
            let converted = match converted {
                Ok(v) => v,
                Err(e) => {
                    skipped.push(json!({"id":id,"field":field,"reason":e.0}));
                    continue;
                }
            };
            let target = if field == "v" { "rule" } else { field };
            if !bodies.contains_key(&id) {
                bodies.insert(id.clone(), V::Map(body.clone()));
                body_order.push(id.clone());
            }
            let replacement = map_mut(bodies.get_mut(&id).unwrap())?;
            if target != field {
                replacement.remove(field);
            }
            replacement.insert(target.into(), converted.clone());
            changes.push(json!({"id":id,"field":field,"target":target,"before":value.to_json()?,"after":converted.to_json()?}));
        }
    }
    let mut answer = json!({"record":record,"before_sha256":crate::identity::sha256(&before),"changes":changes,"skipped":skipped,"applied":false,"problems":[],"fired":[]});
    if changes.is_empty() {
        captured.verify()?;
        return Ok(answer);
    }
    let source_text =
        std::str::from_utf8(&before).map_err(|_| Error("invalid migration UTF-8".into()))?;
    let newline = if source_text.matches("\r\n").count() > source_text.matches('\n').count() / 2 {
        "\r\n"
    } else {
        "\n"
    };
    let mut lines = source_text
        .replace("\r\n", "\n")
        .split('\n')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for id in &body_order {
        let (collection, _) = &all[id];
        let original = source(
            captured
                .source()
                .get(collection)
                .and_then(|v| v.get(id))
                .ok_or_else(|| Error("missing migration body".into()))?,
        )?;
        let S::Map(old_fields) = &original else {
            continue;
        };
        let updated = map(&bodies[id])?;
        let mut ordered = old_fields
            .iter()
            .filter(|(k, _)| updated.contains_key(k))
            .map(|(k, v)| {
                (
                    k.clone(),
                    if v.typed() == updated[k] {
                        v.clone()
                    } else {
                        expression_source(&updated[k])
                    },
                )
            })
            .collect::<Vec<_>>();
        for change in &changes {
            if change["id"] == id.as_str() {
                let target = change["target"].as_str().unwrap();
                if !ordered.iter().any(|(k, _)| k == target) {
                    ordered.push((target.into(), expression_source(&updated[target])));
                }
            }
        }
        crate::legacy_authoring::replace_fields(&mut lines, id, &S::Map(ordered), &original)?;
    }
    let after = lines.join(newline).into_bytes();
    let temp = tempfile::Builder::new()
        .prefix("kpopper-expression-migration-")
        .tempdir()?;
    let shadow = temp.path().join(record.file_name().unwrap());
    std::fs::write(&shadow, &after)?;
    let entry = record.file_name().unwrap().to_string_lossy();
    let brief_path = record
        .parent()
        .unwrap()
        .join(crate::history_migration_source::adjunct(&entry, "view"));
    let mut inventory = captured.inventory().clone();
    let brief = if inventory.exists(&brief_path)? {
        let bytes = inventory.read(&brief_path)?;
        let target = temp
            .path()
            .join(crate::history_migration_source::adjunct(&entry, "view"));
        std::fs::create_dir_all(target.parent().unwrap())?;
        std::fs::write(target, &bytes)?;
        Some(crate::history_yaml::decode_ordinary_source_value(&bytes)?.projected())
    } else {
        None
    };
    let after_capture = capture_source_with_runtime(
        std::slice::from_ref(&shadow),
        temp.path(),
        ReadMode::Frozen,
        None,
        runtime.as_ref(),
    )?;
    let after_document = after_capture.ordinary_document();
    let after_reader = Reader::new(after_document, runtime.as_ref())?;
    let baseline = failures(document, brief.as_ref(), runtime.as_ref())?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let errors = failures(after_document, brief.as_ref(), runtime.as_ref())?;
    let mut problems = vec![];
    let mut fired = BTreeSet::new();
    let mut discovered = BTreeSet::new();
    for id in &judgments {
        let before_pred = get(&all[id].1, predicate_field);
        let after_body = after_reader
            .raw()
            .get(id)
            .ok_or_else(|| Error("judgment missing after migration".into()))?;
        let after_pred = get(after_body, predicate_field);
        let old = reader.predicate(before_pred)?;
        let new = after_reader.predicate(after_pred)?;
        if old.is_some() && new != old {
            let py = |v: Option<bool>| match v {
                None => "None",
                Some(true) => "True",
                Some(false) => "False",
            };
            problems.push(format!("{id}: migration changes condition result from {} to {}; author the typed reading explicitly",py(old),py(new)));
        } else if old.is_none()
            && new == Some(true)
            && !crate::reasoning_authoring_guards::arrangement(&reader, &all[id].1)
        {
            fired.insert(id.clone());
            discovered.insert(format!(
                "{id}: wrong_if holds ({}) - broken by its own condition",
                crate::public_ordinary_readers::predicate_text(after_pred)
            ));
        }
    }
    let mut all_problems = errors
        .into_iter()
        .filter(|e| !baseline.contains(e) && !discovered.contains(e))
        .collect::<Vec<_>>();
    all_problems.extend(problems);
    answer["problems"] = json!(all_problems);
    answer["fired"] = json!(fired);
    answer["after_sha256"] = json!(crate::identity::sha256(&after));
    if apply && all_problems.is_empty() {
        let controls = V::Map(
            map(document)?
                .iter()
                .filter(|(k, _)| {
                    ["meta", "private", "privacy", "visibility", "shareability"]
                        .contains(&k.as_str())
                })
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        );
        let mut roots = body_order.iter().cloned().collect::<BTreeSet<_>>();
        for body in bodies.values() {
            for candidate in super::core_expression_conversion::references_in_body(body) {
                if all.contains_key(&candidate) {
                    roots.insert(candidate);
                }
            }
        }
        let selected =
            crate::pending_bundle::closure(document, &roots.into_iter().collect::<Vec<_>>())?;
        require(
            !crate::recording_privacy::private_marker(&controls)
                && !crate::recording_privacy::private_marker(&V::Map(bodies.clone()))
                && !crate::recording_privacy::private_marker(&selected),
            "private or unclear source permission; migration left the record unchanged",
        )?;
        let guard = project.lock()?;
        require(
            guard.config() == &policy,
            "project mode or record destination changed; retry migration",
        )?;
        captured.verify()?;
        inventory.verify()?;
        require(
            std::fs::read(&record)? == before,
            "record changed during migration; retry",
        )?;
        use crate::history_transaction as T;
        let root = record.parent().unwrap();
        let layout = T::Layout::for_entry(&entry)?;
        require(
            get(get(document, "meta"), "history") == &V::Null,
            "history_direct_writer_unsupported: use the history writer",
        )?;
        let authority = if root.join(&layout.authority).exists() {
            let authority = crate::history_yaml::decode_document(&std::fs::read(
                root.join(&layout.authority),
            )?)?;
            require(
                get(&authority, "authority") == &C::s("legacy"),
                "history_direct_writer_unsupported: use the history writer",
            )?;
            authority
        } else {
            crate::history_authority::authority(
                &format!(
                    "legacy-{}",
                    &crate::identity::sha256(name(&record)?.as_bytes())[..32]
                ),
                "legacy",
                &V::from_json(&json!(0))?,
                Map::new(),
            )?
        };
        let capabilities = F::capabilities(document, None)?;
        let baseline = C::obj([
            ("kind", C::s("direct/v1")),
            ("transaction_root", C::s(name(root)?)),
            ("record_members", C::obj([])),
        ]);
        let mut baseline = baseline;
        map_mut(&mut baseline)?.insert(
            "record_members".into(),
            V::Map(Map::from([(
                entry.to_string(),
                C::s(&crate::identity::sha256(&before)),
            )])),
        );
        let receipt = T::semantic_receipt(
            text(get(&capabilities, "profile"))?,
            &capabilities,
            &C::obj([("document", document.clone())]),
            &C::obj([("document", after_document.clone())]),
        )?;
        let mutation = T::PreparedMutation::prepare(
            &crate::public_history::fresh_id("direct")?,
            &authority,
            &baseline,
            vec![T::FileImage {
                path: entry.to_string(),
                role: "record".into(),
                before: Some(before),
                after: Some(after),
            }],
            &receipt,
            &entry,
            None,
        )?;
        crate::history_transaction_fs::publish_legacy(
            root,
            &layout.journal,
            &mutation,
            &mut |_| {
                guard.verify()?;
                captured.verify()?;
                inventory.verify()
            },
            None,
        )?;
        answer["applied"] = json!(true);
    } else {
        captured.verify()?;
        inventory.verify()?;
    }
    Ok(answer)
}

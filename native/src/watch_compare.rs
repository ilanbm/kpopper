//! Reusable captured ordinary/core union used by branch watch. No record writes.
use crate::{
    Result,
    history_authoring::{obj, s},
    history_contract::{Map, map, text},
    history_view::{list, map_mut, truth},
    ordinary_reader::Reader,
    value::TypedValue as V,
    watch_capture::Snapshot,
    watch_store::{digest_value, error},
};
use std::collections::{BTreeMap, BTreeSet};
fn get<'a>(m: &'a Map, key: &str) -> &'a V {
    m.get(key).unwrap_or(&V::Null)
}
fn claim(body: &V) -> V {
    if let Ok(m) = map(body) {
        for key in ["verdict", "v", "quoted", "rule", "title"] {
            if let Some(v) = m.get(key).filter(|v| **v != V::Null) {
                return if key == "rule" && matches!(v, V::Map(_)) {
                    crate::reasoning_language::legacy_rule(v).unwrap_or_else(|_| v.clone())
                } else {
                    v.clone()
                };
            }
        }
    }
    body.clone()
}
fn same(a: &V, b: &V) -> bool {
    let (a, b) = (claim(a), claim(b));
    if matches!(a, V::Map(_) | V::List(_)) || matches!(b, V::Map(_) | V::List(_)) {
        crate::source_clock::python_equal(&a, &b)
    } else {
        crate::ordinary_reader::same_legacy(&a, &b)
    }
}
fn same_scoped(a: &V, b: &V) -> bool {
    same(a, b)
        && map(a).ok().map(|m| get(m, "scope")).unwrap_or(&V::Null)
            == map(b).ok().map(|m| get(m, "scope")).unwrap_or(&V::Null)
}
pub(crate) fn entries(doc: &V) -> Result<BTreeMap<String, (String, V)>> {
    let mut result = BTreeMap::new();
    for (col, rows) in crate::reasoning_fields::collections(doc)? {
        if col == "meta" {
            continue;
        }
        for (id, body) in rows {
            if result.insert(id.clone(), (col.clone(), body)).is_some() {
                return Err(error(format!("ambiguous entry ID: {id}")));
            }
        }
    }
    Ok(result)
}
fn layer(base: &V, hyp: &V) -> Result<V> {
    let mut result = base.clone();
    for (id, (col, body)) in entries(hyp)? {
        for rows in map_mut(&mut result)?.values_mut() {
            if let V::Map(rows) = rows {
                rows.remove(&id);
            }
        }
        map_mut(&mut result)?
            .entry(col)
            .or_insert(V::Map(Map::new()));
        let col = entries(hyp)?[&id].0.clone();
        map_mut(map_mut(&mut result)?.get_mut(&col).unwrap())?.insert(id, body);
    }
    Ok(result)
}
fn operation(record: &V) -> Result<Option<crate::reasoning_operations::OperationDocument>> {
    let record = map(record)?;
    if let Some(core) = record.get("core").filter(|v| truth(v)) {
        let snapshot = crate::reasoning_snapshot::Snapshot::from_json(
            text(&map(core)?["snapshot"])?.as_bytes(),
        )?;
        Ok(Some(crate::reasoning_operations::OperationDocument::bind(
            &record["doc"],
            snapshot,
        )?))
    } else {
        Ok(None)
    }
}
fn world<'a>(
    op: &crate::reasoning_operations::OperationDocument,
    runtime: Option<&'a crate::reasoning_runtime::Runtime>,
) -> Result<crate::reasoning_operations::OperationWorld<'a>> {
    crate::reasoning_operations::OperationWorld::new(
        op,
        runtime,
        crate::reasoning_runtime::OperationalBounds::default(),
    )
}
pub(crate) fn check(
    doc: &V,
    op: Option<&crate::reasoning_operations::OperationDocument>,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<(Vec<String>, Vec<String>)> {
    if let Some(op) = op {
        let op = if op.document() == doc {
            op.clone()
        } else {
            op.derive(doc, &[], &[])?
        };
        return world(&op, runtime)?.check();
    }
    let mut doc = doc.clone();
    map_mut(&mut doc)?.remove("record");
    map_mut(&mut doc)?.remove("also");
    let projection = crate::public_ordinary_readers::Projection::new(
        &doc,
        &Map::new(),
        &Map::new(),
        vec![],
        runtime,
    )?;
    let (output, _) = projection.check(None)?;
    Ok((
        output
            .lines()
            .filter_map(|l| l.strip_prefix("FAIL ").map(str::to_owned))
            .collect(),
        output
            .lines()
            .filter_map(|l| l.strip_prefix("MOVED ").map(str::to_owned))
            .collect(),
    ))
}
fn uncertain(
    doc: &V,
    op: Option<&crate::reasoning_operations::OperationDocument>,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<BTreeMap<String, Vec<String>>> {
    if let Some(op) = op {
        let op = if op.document() == doc {
            op.clone()
        } else {
            op.derive(doc, &[], &[])?
        };
        let findings = crate::reasoning_operations::findings(world(&op, runtime)?.context())?;
        return map(&map(&findings)?["uncertain"])?
            .iter()
            .map(|(k, v)| {
                Ok((
                    k.clone(),
                    list(v)?
                        .iter()
                        .map(|v| text(v).map(str::to_owned))
                        .collect::<Result<Vec<_>>>()?,
                ))
            })
            .collect();
    }
    let reader = Reader::new(doc, runtime)?;
    let dep = text(&reader.fields["deps"])?;
    let mut result = BTreeMap::new();
    for (id, body) in &reader.raw {
        if !map(body).is_ok_and(|m| m.contains_key(dep)) {
            continue;
        }
        let flags = crate::ordinary_counts::flags(&reader, body)?;
        let mut reasons = flags
            .into_iter()
            .filter(|s| matches!(*s, "blocked" | "unchecked" | "broken" | "no_predicate"))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let pred = crate::ordinary_reader::predicate_of(body, &reader.fields);
        if truth(&pred) && reader.predicate(&pred)?.is_none() {
            reasons.push(format!(
                "predicate cannot currently be evaluated: {}",
                crate::public_ordinary_readers::predicate_text(&pred)
            ));
        } else if !truth(&pred) && !crate::reasoning_authoring::blocked_text(body).is_empty() {
            reasons.push("declared blocked condition".into());
        }
        if !reasons.is_empty() {
            result.insert(id.clone(), reasons);
        }
    }
    Ok(result)
}
#[derive(Default)]
pub(crate) struct Union {
    pub document: Option<V>,
    pub operation: Option<crate::reasoning_operations::OperationDocument>,
    pub contested: Vec<String>,
    pub refused: Vec<(String, String)>,
    pub falsified: Vec<String>,
    pub holes: Vec<String>,
    pub heads: Vec<(String, V)>,
}
/// This is the read-only union contract; a future consolidate adapter must still
/// enforce its additional fold/admission rules before publishing any document.
pub(crate) fn union(
    base: &V,
    hyps: &[V],
    baseline: &(Vec<String>, Vec<String>),
    operation: Option<&crate::reasoning_operations::OperationDocument>,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<Union> {
    let mut result = Union::default();
    let mut holders: BTreeMap<String, V> = BTreeMap::new();
    for h in hyps {
        for (id, (_, body)) in entries(&map(h)?["doc"])? {
            if holders.get(&id).is_some_and(|old| !same(old, &body)) {
                result.contested.push(id.clone());
            }
            holders.insert(id, body);
        }
    }
    result.contested.sort();
    result.contested.dedup();
    if !result.contested.is_empty() {
        return Ok(result);
    }
    let mut doc = base.clone();
    for h in hyps {
        doc = layer(&doc, &map(h)?["doc"])?;
    }
    let derived = operation
        .map(|op| {
            op.derive(
                &doc,
                &hyps
                    .iter()
                    .map(|h| text(&map(h).unwrap()["name"]).unwrap().to_owned())
                    .collect::<Vec<_>>(),
                hyps,
            )
        })
        .transpose()?;
    let base_world = operation.map(|op| world(op, runtime)).transpose()?;
    let next_world = derived.as_ref().map(|op| world(op, runtime)).transpose()?;
    let base_reader = if operation.is_none() {
        Some(Reader::for_followups(base, runtime)?)
    } else {
        None
    };
    let next_reader = if operation.is_none() {
        Some(Reader::for_followups(&doc, runtime)?)
    } else {
        None
    };
    let fields = base_world
        .as_ref()
        .map(|w| w.world().fields())
        .unwrap_or_else(|| base_reader.as_ref().unwrap().fields());
    let dep = text(&fields["deps"]).unwrap_or("");
    let original = entries(base)?;
    let mut held = BTreeMap::new();
    for h in hyps {
        for (id, (_, body)) in entries(&map(h)?["doc"])? {
            held.insert(id, (h, body));
        }
    }
    for (id, (hyp, new)) in held {
        let Some((_, old)) = original.get(&id) else {
            continue;
        };
        if map(old).is_ok_and(|m| m.contains_key(dep)) {
            continue;
        }
        if same(old, &new) && old == &new {
            continue;
        }
        let old_day = if let Some(w) = &base_world {
            crate::reasoning_authoring_guards::read_on(old, w.world())
        } else {
            crate::reasoning_authoring_guards::read_on(old, base_reader.as_ref().unwrap())
        };
        let day = if let Some(w) = &next_world {
            crate::reasoning_authoring_guards::read_on(&new, w.world())
        } else {
            crate::reasoning_authoring_guards::read_on(&new, next_reader.as_ref().unwrap())
        };
        let today = chrono::Local::now()
            .date_naive()
            .max(chrono::Utc::now().date_naive())
            .to_string();
        let from = map(&new).ok().map(|m| get(m, "from")).unwrap_or(&V::Null);
        let old_from = map(old).ok().map(|m| get(m, "from")).unwrap_or(&V::Null);
        let reason = if map(&new).is_ok_and(|m| m.contains_key(dep)) {
            Some("the base holds an entry under this id and the hypothesis a judgment - a subject does not change kind at the fold: set the entry, or give the judgment a new id".into())
        } else if day.as_ref().is_some_and(|day| day > &today) {
            Some(format!(
                "a reading dated {}, after today ({today}) - a day is the record's clock, and a day ahead is not read",
                day.as_ref().unwrap()
            ))
        } else if truth(from) && truth(old_from) && from != old_from && !same(old, &new) {
            Some(format!(
                "a reading from {} where the base reads from {} - one id follows one source; take it by name: consolidate {} --take {id}",
                crate::source_text::ordinary_python_str(from),
                crate::source_text::ordinary_python_str(old_from),
                text(&map(hyp)?["name"])?
            ))
        } else if day.is_none() && old_day.is_some() {
            Some(format!(
                "the reading is undated, and the base's is from {}",
                old_day.unwrap()
            ))
        } else if let Some(old_day) = old_day {
            let day = day.unwrap_or(today);
            if day == old_day {
                Some("a reading of the same day".into())
            } else if day < old_day {
                Some(format!(
                    "a reading from {day} that is older than the base's"
                ))
            } else {
                None
            }
        } else {
            None
        };
        if let Some(reason) = reason
            && !same(old, &new)
        {
            result.refused.push((id, reason));
        }
    }
    let (fail, _) = check(&doc, derived.as_ref(), runtime)?;
    let core_findings = derived
        .as_ref()
        .map(|op| {
            world(op, runtime)
                .and_then(|world| crate::reasoning_operations::findings(world.context()))
        })
        .transpose()?;
    for line in fail.into_iter().filter(|line| !baseline.0.contains(line)) {
        let id = line.split(':').next().unwrap();
        let falsified = if let Some(f) = &core_findings {
            list(&map(f)?["falsified"])?
                .iter()
                .any(|v| text(v).is_ok_and(|v| v.split(':').next() == Some(id)))
        } else {
            next_reader
                .as_ref()
                .unwrap()
                .raw
                .get(id)
                .is_some_and(|body| {
                    crate::ordinary_counts::flags(next_reader.as_ref().unwrap(), body)
                        .is_ok_and(|flags| flags.contains("falsified"))
                })
        };
        if falsified {
            result.falsified.push(line)
        } else {
            result.holes.push(line)
        }
    }
    for h in hyps {
        let h = map(h)?;
        let pred = map(&h["head"])
            .ok()
            .map(|head| get(head, "wrong_if"))
            .unwrap_or(&V::Null);
        if !truth(pred) {
            continue;
        }
        let got = if let Some(op) = &derived {
            world(op, runtime)?.condition(pred)?.0
        } else {
            next_reader.as_ref().unwrap().predicate(pred)?
        };
        if got == Some(true) {
            result
                .heads
                .push((text(&h["name"])?.to_owned(), pred.clone()));
        } else if got.is_none() {
            let raw = crate::source_text::ordinary_python_str(pred);
            let short = if raw.chars().count() > 60 {
                raw.chars().take(57).collect::<String>() + "..."
            } else {
                raw
            };
            result.holes.push(format!("{}: its wrong_if is not a comparison this reader decides ({short}) - a falsifier nothing evaluates tests nothing",text(&h["name"])?));
        }
    }
    result.document = Some(doc);
    result.operation = derived;
    Ok(result)
}
fn finding(kind: &str, id: &str, reason: &str) -> Result<V> {
    let mut result = obj([("kind", s(kind)), ("id", s(id)), ("reason", s(reason))]);
    let hash = digest_value(&result)?;
    map_mut(&mut result)?.insert("fingerprint".into(), s(&hash));
    Ok(result)
}
pub fn compare(snapshot: &Snapshot) -> Result<serde_json::Value> {
    let records = map(&snapshot.data)?;
    let sides = ["ancestor", "working", "main"];
    let history = sides
        .iter()
        .map(|side| {
            map(&records[*side])
                .unwrap()
                .get("history")
                .is_some_and(truth)
        })
        .collect::<Vec<_>>();
    let runtime = crate::public_workspace::runtime_for_document(&map(&records["main"])?["doc"])?;
    if history.iter().all(|v| *v) {
        return crate::ordinary_reader::json_value(
            &crate::history_watch::compare(
                &snapshot.data,
                runtime.as_ref(),
                crate::reasoning_runtime::OperationalBounds::default(),
            )?,
            0,
        );
    }
    let core = sides
        .iter()
        .map(|side| map(&records[*side]).unwrap().get("core").is_some_and(truth))
        .collect::<Vec<_>>();
    if history.iter().any(|v| *v) || core.iter().any(|v| *v) && !core.iter().all(|v| *v) {
        let reason = if history.iter().any(|v| *v) {
            "incompatible captured context: history evidence is missing on one branch"
        } else {
            "incompatible captured context: core/v1 evidence is missing on one branch"
        };
        return Ok(
            serde_json::json!({"state":"attention","findings":[crate::ordinary_reader::json_value(&finding("uncheckable","record",reason)?,0)?],"changed":[],"versions":snapshot["versions"],"identity":snapshot["identity"]}),
        );
    }
    let old = entries(&map(&records["ancestor"])?["doc"])?;
    let local = entries(&map(&records["working"])?["doc"])?;
    let main_doc = &map(&records["main"])?["doc"];
    let main = entries(main_doc)?;
    let mut base = main_doc.clone();
    let main_op = operation(&records["main"])?;
    let baseline = check(main_doc, main_op.as_ref(), runtime.as_ref())?;
    let mut findings = vec![];
    let mut changed = vec![];
    let mut delta = Map::new();
    if truth(&records["shared"]) {
        for (id, (col, body)) in entries(&map(&records["shared"])?["doc"])? {
            let expected = digest_value(&V::List(vec![s(&col), body.clone()]))?;
            let mut readings = vec![
                ("main".to_owned(), main.clone()),
                ("worktree".to_owned(), local.clone()),
            ];
            for h in list(&map(&records["working"])?["hypotheses"])? {
                readings.push((
                    format!("worktree hypothesis {}", text(&map(h)?["name"])?),
                    entries(&map(h)?["doc"])?,
                ));
            }
            for (label, entries) in readings {
                if let Some((col, other)) = entries.get(&id)
                    && digest_value(&V::List(vec![s(col), other.clone()]))? != expected
                {
                    findings.push(finding("collision",&id,&format!("{id}: shared and {label} entries differ; reconcile their value, provenance and scope"))?);
                }
            }
            if !main.contains_key(&id) {
                map_mut(&mut base)?
                    .entry(col.clone())
                    .or_insert(V::Map(Map::new()));
                map_mut(map_mut(&mut base)?.get_mut(&col).unwrap())?.insert(id, body);
            }
        }
        for line in check(&base, main_op.as_ref(), runtime.as_ref())?.0 {
            if !baseline.0.contains(&line) {
                findings.push(finding("shared", line.split(':').next().unwrap(), &line)?);
            }
        }
    }
    let ids = old
        .keys()
        .chain(local.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for id in ids {
        if old.get(&id) == local.get(&id) {
            continue;
        }
        changed.push(id.clone());
        if let Some(main_value) = main.get(&id) {
            if Some(main_value) != old.get(&id)
                && Some(main_value) != local.get(&id)
                && local
                    .get(&id)
                    .is_none_or(|(_, body)| !same_scoped(&main_value.1, body))
            {
                findings.push(finding(
                    "collision",
                    &id,
                    &format!("{id}: main and this worktree changed the same entry differently"),
                )?);
            }
        } else if old.contains_key(&id) && local.contains_key(&id) {
            findings.push(finding(
                "collision",
                &id,
                &format!("{id}: main removed the entry this worktree changed"),
            )?);
        }
        if let Some((col, body)) = local.get(&id) {
            delta.entry(col.clone()).or_insert(V::Map(Map::new()));
            map_mut(delta.get_mut(col).unwrap())?.insert(id, body.clone());
        } else {
            for rows in map_mut(&mut base)?.values_mut() {
                if let V::Map(rows) = rows {
                    rows.remove(&id);
                }
            }
        }
    }
    if get(map(&map(&records["working"])?["doc"])?, "schema")
        != get(map(&map(&records["ancestor"])?["doc"])?, "schema")
    {
        findings.push(finding(
            "uncheckable",
            "schema",
            "The worktree changed the record schema; automatic compatibility is incomplete",
        )?);
    }
    let mut hyps = vec![obj([
        ("name", s("working-change")),
        ("doc", V::Map(delta)),
        ("head", obj([("folds", s("never"))])),
    ])];
    let previous = list(&map(&records["ancestor"])?["hypotheses"])?
        .iter()
        .map(|h| (text(&map(h).unwrap()["name"]).unwrap().to_owned(), h))
        .collect::<BTreeMap<_, _>>();
    for h in list(&map(&records["working"])?["hypotheses"])? {
        let hmap = map(h)?;
        let name = text(&hmap["name"])?;
        let mut body = Map::new();
        if previous.get(name).copied() != Some(h) {
            let prior = previous
                .get(name)
                .map(|h| entries(&map(h).unwrap()["doc"]))
                .transpose()?
                .unwrap_or_default();
            for (id, (col, v)) in entries(&hmap["doc"])? {
                if prior.get(&id) != Some(&(col.clone(), v.clone())) {
                    body.entry(col.clone()).or_insert(V::Map(Map::new()));
                    map_mut(body.get_mut(&col).unwrap())?.insert(id, v);
                }
            }
        }
        hyps.push(obj([
            ("name", s(name)),
            ("doc", V::Map(body)),
            ("head", hmap["head"].clone()),
        ]));
    }
    let base_op = main_op
        .as_ref()
        .map(|op| op.derive(&base, &[], &[]))
        .transpose()?;
    let union = if core.iter().all(|v| *v) && changed.is_empty() && !truth(&records["shared"]) {
        Union::default()
    } else {
        match union(&base, &hyps, &baseline, base_op.as_ref(), runtime.as_ref()) {
            Ok(union) => union,
            Err(e) => {
                findings.push(finding("uncheckable", "record", &e.to_string())?);
                Union::default()
            }
        }
    };
    for (kind, lines) in [
        ("falsified", &union.falsified),
        ("uncheckable", &union.holes),
    ] {
        for line in lines {
            findings.push(finding(kind, line.split(':').next().unwrap(), line)?);
        }
    }
    for (id, why) in &union.refused {
        findings.push(finding("contested", id, &format!("{id}: {why}"))?);
    }
    for id in &union.contested {
        findings.push(finding(
            "collision",
            id,
            &format!("{id}: local hypotheses disagree"),
        )?);
    }
    for (name, pred) in &union.heads {
        if let V::Text(pred) = pred {
            findings.push(finding("falsified", name, &format!("{name}: {pred}"))?);
        } else {
            return Err(error("can only concatenate str (not \"dict\") to str"));
        }
    }
    if let Some(doc) = &union.document {
        let prior = uncertain(main_doc, main_op.as_ref(), runtime.as_ref())?;
        for (id, reasons) in uncertain(doc, union.operation.as_ref(), runtime.as_ref())? {
            if prior.get(&id) != Some(&reasons) {
                findings.push(finding(
                    "uncheckable",
                    &id,
                    &format!("{id}: {}", reasons.join("; ")),
                )?);
            }
        }
    }
    if core.iter().all(|v| *v)
        && get(map(&map(&records["working"])?["core"])?, "snapshot")
            != get(map(&map(&records["ancestor"])?["core"])?, "snapshot")
    {
        let projected = map(&map(&map(&records["working"])?["core"])?["findings"])?;
        for (kind, key) in [
            ("falsified", "falsified"),
            ("uncheckable", "holes"),
            ("uncheckable", "moved"),
        ] {
            for line in list(&projected[key])? {
                let line = text(line)?;
                findings.push(finding(kind, line.split(':').next().unwrap(), line)?);
            }
        }
    }
    let mut seen = BTreeSet::new();
    findings.retain(|finding| seen.insert(digest_value(finding).unwrap()));
    crate::ordinary_reader::json_value(
        &obj([
            (
                "state",
                s(if findings.is_empty() {
                    "clear"
                } else {
                    "attention"
                }),
            ),
            ("findings", V::List(findings)),
            ("changed", V::List(changed.iter().map(|v| s(v)).collect())),
            ("versions", records["versions"].clone()),
            ("identity", records["identity"].clone()),
        ]),
        0,
    )
}

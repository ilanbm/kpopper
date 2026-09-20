//! Local hypothesis consolidation under the ordinary reader's interpretation.
//! Preview and publication share one captured candidate; no replacement bypasses admission.
use super::{CommandOutput, Options};
use crate::{
    Result,
    history_authoring::{obj, s},
    history_contract::{Map, error, map, text},
    history_view::{map_mut, truth},
    history_yaml::{OrdinaryValue as O, SourceValue as Source},
    ordinary_reader::{self as R, Reader},
    project_modes::WriteRoute,
    public_ordinary_readers::{Projection, World, predicate_text, short},
    reasoning_authoring_guards as G,
    reasoning_runtime::Runtime,
    require,
    source_capture::{CapturedSource, ReadMode},
    source_inventory::Inventory,
    value::TypedValue as V,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
#[path = "ordinary_consolidation_edit.rs"]
mod edit;
#[path = "ordinary_consolidation_report.rs"]
mod report;

fn get<'a>(value: &'a V, key: &str) -> &'a V {
    map(value).ok().and_then(|m| m.get(key)).unwrap_or(&V::Null)
}
fn py(value: &V) -> String {
    crate::source_text::ordinary_python_str(value)
}
fn strings(value: &V) -> Vec<String> {
    match value {
        V::List(a) => a
            .iter()
            .filter_map(|v| text(v).ok().map(str::to_owned))
            .collect(),
        _ => vec![],
    }
}
fn entries(doc: &V) -> Result<Map> {
    Ok(crate::reasoning_fields::collections(doc)?
        .into_values()
        .flatten()
        .collect())
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
fn same_claim(a: &V, b: &V) -> bool {
    if matches!(a, V::Map(_) | V::List(_)) || matches!(b, V::Map(_) | V::List(_)) {
        crate::source_clock::python_equal(a, b)
    } else {
        R::same_legacy(a, b)
    }
}
fn verdict(body: &V) -> V {
    let value = get(body, "verdict");
    if value == &V::Null {
        get(body, "title").clone()
    } else {
        value.clone()
    }
}
fn shaped(body: &V, fields: &Map) -> bool {
    text(&fields["deps"]).ok().is_some_and(|field|matches!(get(body,field),V::List(a)if !a.is_empty()&&a.iter().all(|v|matches!(v,V::Text(_)))))
}
fn version_core(body: &V) -> V {
    let Ok(m) = map(body) else {
        return body.clone();
    };
    V::Map(
        m.iter()
            .filter(|(k, _)| {
                ![
                    "seen", "reviewed", "born", "replaced", "ended", "day", "same_as", "dropped",
                ]
                .contains(&k.as_str())
            })
            .map(|(k, v)| {
                (
                    k.clone(),
                    if k == "wrong_if" {
                        s(&predicate_text(v))
                    } else {
                        v.clone()
                    },
                )
            })
            .collect(),
    )
}
fn day(value: &V) -> Option<String> {
    let value = py(value);
    let value = value.trim_start();
    let date = value.get(..10)?;
    crate::value::Date::new(date).ok().map(|_| date.into())
}
fn layer(base: &V, hypothesis: &V) -> Result<V> {
    let mut out = base.clone();
    for (collection, members) in crate::reasoning_fields::collections(hypothesis)? {
        for (name, values) in map_mut(&mut out)?.iter_mut() {
            if *name != collection
                && let V::Map(values) = values
            {
                for id in members.keys() {
                    values.remove(id);
                }
            }
        }
        let target = map_mut(&mut out)?
            .entry(collection)
            .or_insert_with(|| V::Map(Map::new()));
        if !matches!(target, V::Map(_)) {
            *target = V::Map(Map::new());
        }
        map_mut(target)?.extend(members);
    }
    Ok(out)
}
fn checks(projection: &Projection<'_>, brief: Option<&V>) -> Result<(Vec<String>, Vec<String>)> {
    let output = projection.check(brief.filter(|v| **v != V::Null))?.0;
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
fn ordinary_error(document: &V, failure: crate::Error) -> crate::Error {
    if failure.0 != "ordinary_fields_unreadable" {
        return failure;
    }
    let Some(dep) = map(get(document, "schema"))
        .ok()
        .and_then(|s| s.get("deps"))
        .and_then(|v| text(v).ok())
        .filter(|s| !s.is_empty())
    else {
        return failure;
    };
    let present = entries(document)
        .unwrap_or_default()
        .values()
        .filter_map(|v| map(v).ok())
        .flat_map(|m| m.keys().cloned())
        .collect::<BTreeSet<_>>();
    if present.contains(dep) {
        return failure;
    }
    let seen = present.into_iter().collect::<Vec<_>>().join(", ");
    let shown = if seen.chars().count() < 300 {
        seen.clone()
    } else {
        seen.chars().take(300).collect::<String>() + " ..."
    };
    error(&format!(
        "schema names '{dep}' for 'deps', and nothing this reader can see carries it: no judgment would be found, and the record would pass by having nothing left to check.{}",
        if seen.is_empty() {
            String::new()
        } else {
            format!("\nFields it can see: {shown}")
        }
    ))
}
fn projection<'a>(doc: &V, hyps: &Map, runtime: Option<&'a Runtime>) -> Result<Projection<'a>> {
    Projection::new(doc, hyps, &Map::new(), vec![], runtime).map_err(|e| ordinary_error(doc, e))
}

#[derive(Clone)]
struct Hypothesis {
    name: String,
    path: Option<PathBuf>,
    doc: V,
    head: V,
    raw: Map,
    ids: BTreeSet<String>,
    source: O,
    text: String,
}
impl Hypothesis {
    fn path(&self) -> Result<&PathBuf> {
        self.path.as_ref().ok_or_else(|| {
            error("a supplied preview cannot be published without captured source files")
        })
    }
}
struct Update {
    id: String,
    hyp: usize,
    old: V,
    new: V,
    allowed: bool,
    why: String,
}
struct Reversal {
    id: String,
    hyp: usize,
    old: V,
    new: V,
    allowed: bool,
    why: String,
}
struct Union<'a> {
    runtime: Option<&'a Runtime>,
    base_doc: V,
    doc: V,
    base: Projection<'a>,
    view: Option<Projection<'a>>,
    hyps: Vec<Hypothesis>,
    arrived: Vec<(String, usize)>,
    updates: Vec<Update>,
    reversed: Vec<Reversal>,
    refused: Vec<usize>,
    sourced: BTreeSet<String>,
    untaken: Vec<usize>,
    untakeable: BTreeSet<String>,
    drops_needed: BTreeMap<String, Vec<String>>,
    contested: BTreeMap<String, Vec<usize>>,
    moved: Vec<String>,
    falsified: Vec<String>,
    holes: Vec<String>,
    head_falsified: Vec<(usize, String)>,
    new_subjects: Vec<String>,
    candidates: Vec<String>,
}
impl Union<'_> {
    fn red(&self) -> bool {
        !self.contested.is_empty()
            || !self.refused.is_empty()
            || !self.untaken.is_empty()
            || !self.falsified.is_empty()
            || !self.holes.is_empty()
            || !self.head_falsified.is_empty()
    }
    fn blocked(&self) -> bool {
        self.red() || !self.moved.is_empty()
    }
}

fn captured_projection<'a>(
    capture: &CapturedSource,
    runtime: Option<&'a Runtime>,
) -> Result<Projection<'a>> {
    let context = capture.ordinary_context();
    Projection::new(
        capture.ordinary_document(),
        map(capture.hypotheses())?,
        map(get(&context, "conflicts"))?,
        capture.reader_lines()?,
        runtime,
    )
    .map_err(|e| ordinary_error(capture.ordinary_document(), e))
}
fn read_hypotheses(
    capture: &CapturedSource,
    requested: &[String],
    runtime: Option<&Runtime>,
) -> Result<Vec<Hypothesis>> {
    let all = map(capture.hypotheses())?;
    let mut pool = BTreeMap::new();
    for (name, h) in all {
        if get(h, "kind") == &s("contribution") {
            continue;
        }
        require(
            !truth(get(h, "error")),
            &format!(
                "refused - hypothesis {name} could not be read: {}",
                py(get(h, "error"))
            ),
        )?;
        let doc = get(h, "doc");
        let merged = layer(capture.ordinary_document(), doc)?;
        Reader::new(&merged, runtime).map_err(|e| {
            error(&format!(
                "refused - hypothesis {name} cannot be read over the base: {}",
                ordinary_error(&merged, e)
                    .0
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(160)
                    .collect::<String>()
            ))
        })?;
        let path = PathBuf::from(text(get(h, "path"))?);
        let raw = capture
            .files()
            .get(&path)
            .ok_or_else(|| error("hypothesis_source_unavailable"))?;
        let source = crate::history_yaml::decode_ordinary_source_value(raw)?;
        let body = entries(doc)?;
        let ids = body.keys().cloned().collect();
        pool.insert(
            name.clone(),
            Hypothesis {
                name: name.clone(),
                path: Some(path),
                doc: doc.clone(),
                head: get(h, "head").clone(),
                raw: body,
                ids,
                source,
                text: std::str::from_utf8(raw)
                    .map_err(|_| error("invalid_utf8"))?
                    .into(),
            },
        );
    }
    let missing = requested
        .iter()
        .filter(|n| !pool.contains_key(*n))
        .cloned()
        .collect::<Vec<_>>();
    require(
        missing.is_empty(),
        &format!(
            "refused - no hypothesis named {} beside the record (there: {})",
            missing.join(", "),
            if pool.is_empty() {
                "none".into()
            } else {
                pool.keys().cloned().collect::<Vec<_>>().join(", ")
            }
        ),
    )?;
    if requested.is_empty() {
        Ok(pool.into_values().collect())
    } else {
        Ok(requested
            .iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|n| pool.remove(n).unwrap())
            .collect())
    }
}

struct UnionInput<'i, 'r> {
    doc: &'i V,
    all: &'i Map,
    hyps: Vec<Hypothesis>,
    base: Projection<'r>,
    runtime: Option<&'r Runtime>,
    stamp: &'i str,
    take: &'i [String],
    drops: &'i Map,
    brief: Option<&'i V>,
    page: Option<&'i V>,
}
fn union<'a>(input: UnionInput<'_, 'a>) -> Result<Union<'a>> {
    let UnionInput {
        doc,
        all,
        hyps,
        base,
        runtime,
        stamp,
        take,
        drops,
        brief,
        page,
    } = input;
    let mut c = Union {
        runtime,
        base_doc: doc.clone(),
        doc: doc.clone(),
        base,
        view: None,
        hyps,
        arrived: vec![],
        updates: vec![],
        reversed: vec![],
        refused: vec![],
        sourced: BTreeSet::new(),
        untaken: vec![],
        untakeable: BTreeSet::new(),
        drops_needed: BTreeMap::new(),
        contested: BTreeMap::new(),
        moved: vec![],
        falsified: vec![],
        holes: vec![],
        head_falsified: vec![],
        new_subjects: vec![],
        candidates: vec![],
    };
    let mut holders = BTreeMap::<String, Vec<usize>>::new();
    for (i, h) in c.hyps.iter().enumerate() {
        for id in &h.ids {
            holders.entry(id.clone()).or_default().push(i);
        }
    }
    for (id, hs) in &holders {
        let first = claim(&c.hyps[hs[0]].raw[id]);
        if hs
            .iter()
            .skip(1)
            .any(|i| !same_claim(&first, &claim(&c.hyps[*i].raw[id])))
        {
            c.contested.insert(id.clone(), hs.clone());
        }
    }
    if !c.contested.is_empty() {
        return Ok(c);
    }
    for h in &c.hyps {
        c.doc = layer(&c.doc, &h.doc)?;
    }
    c.view = Some(projection(&c.doc, &Map::new(), runtime)?);
    let world = &c.view.as_ref().unwrap().base;
    let base = &c.base.base;
    let meta = map(get(doc, "meta"))
        .ok()
        .map(|m| m.keys().cloned().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    for (id, hs) in &holders {
        if meta.contains(id) {
            continue;
        }
        let hi = *hs.last().unwrap();
        let h = &c.hyps[hi];
        let new = &h.raw[id];
        if !base.reader.ids.contains(id) {
            c.arrived.push((id.clone(), hi));
            continue;
        }
        let old = base.reader.raw.get(id).unwrap_or(&V::Null);
        let (old_claim, new_claim) = (claim(old), claim(new));
        let same = same_claim(&old_claim, &new_claim);
        if same && old == new {
            continue;
        }
        let (allowed, mut why);
        if base.judgments.contains_key(id) {
            if same
                && matches!((old, new), (V::Map(_), V::Map(_)))
                && version_core(old) == version_core(new)
                && !(G::arrangement(&base.reader, old) && get(old, "born") != get(new, "born"))
            {
                continue;
            }
            if !matches!(new, V::Map(_)) || !shaped(new, &base.reader.fields) {
                allowed = false;
                why="the base holds a judgment under this id and the hypothesis an entry - a subject does not change kind at the fold, and no name takes this: give it what it rests on and a verdict, or a new id".into();
                c.untakeable.insert(id.clone());
            } else if G::arrangement(&base.reader, old) && !G::arrangement(&base.reader, new) {
                allowed = false;
                why="what replaces an arrangement is an arrangement - rest on the session sources of the occasion it decides and give it a sign over a count; no name takes a body that drops them".into();
                c.untakeable.insert(id.clone());
            } else if !G::arrangement(&base.reader, old) && G::arrangement(&base.reader, new) {
                allowed = false;
                why="the base holds a judgment under this id and the hypothesis an arrangement - an arrangement is born when it is written in place, and no name takes one over a judgment: write it as its own decision".into();
                c.untakeable.insert(id.clone());
            } else {
                let decision = G::may_supersede(
                    &base.reader,
                    id,
                    old,
                    new,
                    Some(stamp),
                    page,
                    take.contains(id),
                )?;
                allowed = decision.allowed;
                why = decision.reason;
                let pred = R::predicate_of(&base.judgments[id], &base.reader.fields);
                if !allowed && truth(&pred) && world.reader.predicate(&pred)? == Some(true) {
                    why += &format!(
                        " - it would hold with {}'s readings ({})",
                        h.name,
                        short(&pred, 60)
                    );
                }
                if allowed && !G::arrangement(&base.reader, old) {
                    let dep = text(&base.reader.fields["deps"])?;
                    let now = strings(get(new, dep));
                    let gone = strings(get(old, dep))
                        .into_iter()
                        .filter(|d| !now.contains(d) && !drops.contains_key(d))
                        .collect::<Vec<_>>();
                    if !gone.is_empty() {
                        c.drops_needed.insert(id.clone(), gone);
                    }
                }
            }
            if !allowed {
                c.untaken.push(c.reversed.len());
            }
            c.reversed.push(Reversal {
                id: id.clone(),
                hyp: hi,
                old: old.clone(),
                new: new.clone(),
                allowed,
                why: why.clone(),
            });
        } else {
            let read = G::read_on(new, &world.reader);
            let old_day = G::read_on(old, &base.reader);
            if shaped(new, &base.reader.fields) {
                allowed = false;
                why="the base holds an entry under this id and the hypothesis a judgment - a subject does not change kind at the fold: set the entry, or give the judgment a new id".into();
            } else if read.as_ref().is_some_and(|d| d.as_str() > stamp) {
                allowed = false;
                why = format!(
                    "a reading dated {}, after today ({stamp}) - a day is the record's clock, and a day ahead is not read",
                    read.as_ref().unwrap()
                );
            } else if h.path.is_none()
                && matches!((old, new), (V::Map(_), V::Map(_)))
                && truth(get(new, "from"))
                && truth(get(old, "from"))
                && py(get(new, "from")) != py(get(old, "from"))
                && !same
            {
                if take.contains(id) {
                    allowed = true;
                    why = format!(
                        "a reading from {} where the base reads from {} - taken by name",
                        py(get(new, "from")),
                        py(get(old, "from"))
                    );
                } else {
                    allowed = false;
                    why = format!(
                        "a reading from {} where the base reads from {} - one id follows one source; take it by name: consolidate {} --take {id}",
                        py(get(new, "from")),
                        py(get(old, "from")),
                        h.name
                    );
                    c.sourced.insert(id.clone());
                }
            } else if read.is_none()
                && let Some(old_day) = old_day
            {
                allowed = false;
                why = format!("the reading is undated, and the base's is from {}", old_day);
            } else {
                let decision = G::may_supersede(
                    &base.reader,
                    id,
                    old,
                    &new_claim,
                    Some(read.as_deref().unwrap_or(stamp)),
                    None,
                    false,
                )?;
                allowed = decision.allowed;
                why = decision.reason;
            }
            if same && !allowed {
                continue;
            }
        }
        if !allowed && !base.judgments.contains_key(id) {
            c.refused.push(c.updates.len());
        }
        c.updates.push(Update {
            id: id.clone(),
            hyp: hi,
            old: old_claim,
            new: new_claim,
            allowed,
            why,
        });
    }
    let (before_fail, before_moved) = checks(&c.base, brief)?;
    let (after_fail, after_moved) = checks(c.view.as_ref().unwrap(), None)?;
    let missing_reference = regex::Regex::new(r"rests on (\S+), which is not an entry").unwrap();
    for mut line in after_fail {
        if before_fail.contains(&line) {
            continue;
        }
        let id = line.split(':').next().unwrap();
        let falsified = world.judgments.get(id).is_some_and(|b| {
            crate::ordinary_counts::flags(&world.reader, b).is_ok_and(|f| f.contains("falsified"))
        });
        if falsified {
            c.falsified.push(line);
        } else {
            if let Some(cap) = missing_reference.captures(&line) {
                let key = &cap[1];
                let others = all
                    .iter()
                    .filter(|(name, h)| {
                        !c.hyps.iter().any(|x| &x.name == *name)
                            && entries(get(h, "doc")).is_ok_and(|e| e.contains_key(key))
                    })
                    .map(|(n, _)| n.clone())
                    .collect::<Vec<_>>();
                if !others.is_empty() {
                    line += &format!(
                        " - held by hypothes{} {}: consolidate them together",
                        if others.len() == 1 { "is" } else { "es" },
                        others.join(", ")
                    );
                }
            }
            c.holes.push(line);
        }
    }
    c.moved = after_moved
        .into_iter()
        .filter(|l| !before_moved.contains(l))
        .collect();
    for (i, h) in c.hyps.iter().enumerate() {
        let v = get(&h.head, "wrong_if");
        let pred = if truth(v) { py(v) } else { String::new() };
        if pred.is_empty() {
            continue;
        }
        match world.reader.predicate(&s(&pred))?{Some(true)=>c.head_falsified.push((i,pred)),None=>c.holes.push(format!("{}: its wrong_if is not a comparison this reader decides ({}) - a falsifier nothing evaluates tests nothing",h.name,short(&s(&pred),60))),_=>{}}
    }
    let have = base
        .reader
        .ids
        .iter()
        .filter_map(|id| id.split_once('.').map(|(p, _)| p.to_owned()))
        .collect::<BTreeSet<_>>();
    c.new_subjects = c
        .arrived
        .iter()
        .filter_map(|(id, _)| id.split_once('.').map(|(p, _)| p.to_owned()))
        .filter(|p| !have.contains(p))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    c.candidates = report::candidates(&c, all)?;
    Ok(c)
}
fn text_values<'a>(v: &'a V, out: &mut Vec<&'a str>) {
    match v {
        V::Text(s) => out.push(s),
        V::List(a) => {
            for v in a {
                text_values(v, out)
            }
        }
        V::Map(m) => {
            for v in m.values() {
                text_values(v, out)
            }
        }
        _ => {}
    }
}
fn metadata(document: &V) -> V {
    map(document)
        .ok()
        .and_then(|m| m.get("meta"))
        .cloned()
        .unwrap_or_else(|| V::Map(Map::new()))
}
fn privacy(
    capture: &CapturedSource,
    route: &WriteRoute,
    hyps: &[Hypothesis],
    action: &V,
    extra: &[String],
) -> Result<()> {
    let base = capture.ordinary_document();
    let mut combined = base.clone();
    let templates =
        regex::Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*\}\}").unwrap();
    for h in hyps {
        combined = layer(&combined, &h.doc)?;
    }
    let original_entries = entries(base)?;
    for h in hyps {
        let context = layer(&combined, &h.doc)?;
        let heads = obj([
            ("hypothesis", h.head.clone()),
            ("record", metadata(&context)),
            ("original_record", metadata(base)),
            ("source_record", V::Map(Map::new())),
        ]);
        let present = entries(&context)?;
        let mut roots = h
            .ids
            .iter()
            .cloned()
            .chain(extra.iter().cloned())
            .collect::<BTreeSet<_>>();
        let mut texts = vec![];
        text_values(&heads, &mut texts);
        for value in texts {
            for capture in templates.captures_iter(value) {
                if !crate::reasoning_fields::BUILTINS.contains(&&capture[1]) {
                    roots.insert(capture[1].into());
                }
            }
            if present.contains_key(value) {
                roots.insert(value.into());
            }
            for id in R::ID.find_iter(value).map(|m| m.as_str()) {
                if present.contains_key(id) {
                    roots.insert(id.into());
                }
            }
        }
        let selected = if roots.is_empty() {
            V::Map(Map::new())
        } else {
            crate::pending_bundle::closure(&context,&roots.iter().cloned().collect::<Vec<_>>()).map_err(|_|error("refused - hypothesis source closure could not be checked; original retained"))?
        };
        let original_roots = roots
            .into_iter()
            .chain(entries(&selected)?.into_keys())
            .filter(|id| original_entries.contains_key(id))
            .collect::<BTreeSet<_>>();
        let original = if original_roots.is_empty() {
            V::Map(Map::new())
        } else {
            crate::pending_bundle::closure(base,&original_roots.into_iter().collect::<Vec<_>>()).map_err(|_|error("refused - hypothesis source closure could not be checked; original retained"))?
        };
        if [&heads, &h.doc, &selected, &original]
            .iter()
            .any(|v| crate::recording_privacy::private_marker(v))
        {
            let mut retained = map(&selected)?.clone();
            retained.extend(Map::from([
                ("hypothesis".into(), h.head.clone()),
                ("source_record_metadata".into(), V::Map(Map::new())),
                ("record_metadata".into(), metadata(&context)),
                ("original_record_metadata".into(), metadata(base)),
                ("original_record_closure".into(), original),
            ]));
            let mut intent = map(action)?.clone();
            intent.insert("hypothesis".into(), s(&h.name));
            let receipt = crate::recording_privacy::draft(
                route.project(),
                &V::Map(intent),
                &V::Map(retained),
                "private hypothesis or source permission; original retained without publication",
            )?;
            return Err(error(&format!(
                "private draft retained at {}; original hypothesis retained",
                text(get(&receipt, "path"))?
            )));
        }
    }
    Ok(())
}
fn one_hypothesis(capture: &CapturedSource, name: &str) -> Result<Hypothesis> {
    let h = map(capture.hypotheses())?.get(name).ok_or_else(|| {
        error(&format!(
            "refused - no hypothesis named {name} beside the record"
        ))
    })?;
    require(
        !truth(get(h, "error")),
        &format!(
            "refused - hypothesis {name} could not be read: {}",
            py(get(h, "error"))
        ),
    )?;
    let path = PathBuf::from(text(get(h, "path"))?);
    let raw = capture
        .files()
        .get(&path)
        .ok_or_else(|| error("hypothesis_source_unavailable"))?;
    let doc = get(h, "doc").clone();
    let body = entries(&doc)?;
    Ok(Hypothesis {
        name: name.into(),
        path: Some(path),
        doc,
        head: get(h, "head").clone(),
        ids: body.keys().cloned().collect(),
        raw: body,
        source: crate::history_yaml::decode_ordinary_source_value(raw)?,
        text: std::str::from_utf8(raw)
            .map_err(|_| error("invalid_utf8"))?
            .into(),
    })
}
pub(super) fn run(
    options: &Options,
    route: &WriteRoute,
    runtime_override: Option<&Runtime>,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<CommandOutput> {
    let entry = &route.paths()[0];
    let layout = crate::history_transaction::Layout::for_entry(
        entry
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| error("invalid_path"))?,
    )?;
    require(
        crate::history_transaction_fs::read(&entry.parent().unwrap().join(&layout.journal))?
            .is_none(),
        "recovery_required",
    )?;
    let loaded_runtime;
    let runtime = if runtime_override.is_some() {
        runtime_override
    } else {
        loaded_runtime = crate::public_workspace::runtime()?;
        loaded_runtime.as_ref()
    };
    let capture = crate::source_capture::capture_source_with_runtime(
        route.paths(),
        &route.project().root,
        ReadMode::Live,
        None,
        runtime,
    )?;
    require(
        get(
            &crate::reasoning_fields::capabilities(capture.ordinary_document(), None)?,
            "profile",
        ) != &s("core/v1"),
        "unsupported_capability: use core/v1 consumer",
    )?;
    if !options.dry_run {
        capture.snapshot().map_err(|e| {
            if e.0 == "invalid_yaml_key" {
                error("invalid_history_value: snapshot mappings require string keys")
            } else {
                e
            }
        })?;
    }
    let stamp = options
        .as_of
        .clone()
        .unwrap_or_else(|| chrono::Local::now().date_naive().to_string());
    let mut inventory = Inventory::default();
    let side = edit::ancillary(entry, &mut inventory)?;
    if let Some(name) = &options.refute {
        let write = edit::WriteContext {
            capture: &capture,
            route,
            side: &side,
            inventory: &inventory,
            stamp: &stamp,
            runtime,
            page_capture: None,
        };

        let h = one_hypothesis(&capture, name)?;
        let out = edit::refute(
            &write,
            &h,
            options.why.as_deref().unwrap_or(""),
            options.source.as_deref(),
            probe,
        )?;
        return Ok(CommandOutput {
            stdout: out,
            stderr: String::new(),
            code: 0,
        });
    }
    let hyps = read_hypotheses(&capture, &options.names, runtime)?;
    if !options.dry_run {
        privacy(
            &capture,
            route,
            &hyps,
            &obj([("kind", s("consolidate"))]),
            &[],
        )?;
    }
    if hyps.is_empty() {
        capture.verify()?;
        route.verify()?;
        return Ok(CommandOutput {
            stdout: "no hypotheses beside the record - nothing to consolidate\n".into(),
            stderr: String::new(),
            code: 0,
        });
    }
    let drops = map(&super::drops(&options.drops)?)?.clone();
    let page_capture = edit::page(&capture, &hyps, route, &inventory, runtime)?;
    let page = page_capture.as_ref().map(|capture| &capture.facts);
    let write = edit::WriteContext {
        capture: &capture,
        route,
        side: &side,
        inventory: &inventory,
        stamp: &stamp,
        runtime,
        page_capture: page_capture.as_ref(),
    };
    let c = union(UnionInput {
        doc: capture.ordinary_document(),
        all: map(capture.hypotheses())?,
        hyps,
        base: captured_projection(&capture, runtime)?,
        runtime,
        stamp: &stamp,
        take: &options.take,
        drops: &drops,
        brief: Some(&side.brief.projected()),
        page,
    })?;
    let mut output = report::lines(&c, chrono::Local::now().date_naive())?.join("\n") + "\n";
    capture.verify()?;
    inventory.verify()?;
    route.verify()?;
    if let Some(page) = &page_capture {
        page.verify()?;
    }
    if options.dry_run {
        let stray = report::stray(&c, &options.take);
        if let Some(why) = &stray {
            output.push('\n');
            output.push_str(why);
            output.push('\n');
        }
        return Ok(CommandOutput {
            stdout: output,
            stderr: String::new(),
            code: i32::from(stray.is_some() || c.red() || !c.drops_needed.is_empty()),
        });
    }
    output.push('\n');
    if let Some(why) = report::fold_refusal(&c, &options.take) {
        return Ok(CommandOutput {
            stdout: output,
            stderr: why + "\n",
            code: 1,
        });
    }
    match edit::fold(&c, &write, &drops, page, probe) {
        Ok(done) => {
            output.push_str(&done);
            Ok(CommandOutput {
                stdout: output,
                stderr: String::new(),
                code: 0,
            })
        }
        Err(e) => Ok(CommandOutput {
            stdout: output,
            stderr: e.0 + "\n",
            code: 1,
        }),
    }
}

pub(super) fn preview(request: &super::PreviewRequest<'_>) -> Result<super::Preview> {
    let empty = Map::new();
    let context = request.context.map(map).transpose()?;
    let conflicts = context
        .and_then(|m| m.get("conflicts"))
        .map(map)
        .transpose()?
        .unwrap_or(&empty);
    let base = Projection::new(
        request.document,
        request.hypotheses,
        conflicts,
        vec![],
        request.runtime,
    )
    .map_err(|e| ordinary_error(request.document, e))?;
    let mut pool = BTreeMap::new();
    for (name, h) in request.hypotheses {
        if get(h, "kind") == &s("contribution") {
            continue;
        }
        require(
            !truth(get(h, "error")),
            &format!(
                "refused - hypothesis {name} could not be read: {}",
                py(get(h, "error"))
            ),
        )?;
        let doc = if get(h, "doc") != &V::Null {
            get(h, "doc")
        } else {
            get(h, "document")
        };
        let merged = layer(request.document, doc)?;
        Reader::new(&merged, request.runtime).map_err(|e| {
            error(&format!(
                "refused - hypothesis {name} cannot be read over the base: {}",
                ordinary_error(&merged, e)
                    .0
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(160)
                    .collect::<String>()
            ))
        })?;
        let raw = entries(doc)?;
        pool.insert(
            name.clone(),
            Hypothesis {
                name: name.clone(),
                path: text(get(h, "path"))
                    .ok()
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from),
                doc: doc.clone(),
                head: get(h, "head").clone(),
                ids: raw.keys().cloned().collect(),
                raw,
                source: O::from_typed(doc),
                text: String::new(),
            },
        );
    }
    for proposal in request.proposals {
        require(
            !pool.contains_key(&proposal.name),
            &format!(
                "refused - {} names both an existing and a supplied hypothesis",
                proposal.name
            ),
        )?;
        let raw = entries(&proposal.document)?;
        pool.insert(
            proposal.name.clone(),
            Hypothesis {
                name: proposal.name.clone(),
                path: None,
                doc: proposal.document.clone(),
                head: proposal.head.clone(),
                ids: raw.keys().cloned().collect(),
                raw,
                source: O::from_typed(&proposal.document),
                text: String::new(),
            },
        );
    }
    let proposals = pool.into_values().collect();
    let stamp = request
        .as_of
        .map(str::to_owned)
        .unwrap_or_else(crate::source_clock::latest_day);
    let c = union(UnionInput {
        doc: request.document,
        all: request.hypotheses,
        hyps: proposals,
        base,
        runtime: request.runtime,
        stamp: &stamp,
        take: &[],
        drops: &empty,
        brief: None,
        page: None,
    })?;
    let report = report::lines(&c, chrono::Local::now().date_naive())?.join("\n") + "\n";
    Ok(super::Preview {
        report,
        exit_code: i32::from(c.red() || !c.drops_needed.is_empty()),
        blocked: c.blocked() || !c.drops_needed.is_empty(),
        candidate_document: c.view.as_ref().map(|_| c.doc.clone()),
    })
}

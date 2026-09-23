//! Ordinary authored admission: identity, references, citations and replacement.
use crate::source_text::ordinary_python_str as py;
use crate::{
    Error, Result,
    history_contract::*,
    history_view::truth,
    reasoning_authoring::{World, blocked_text, reopened_text},
    reasoning_fields as F, reasoning_language as L,
    reasoning_snapshot::entries,
    require,
    value::TypedValue as V,
};
use serde_json::Value as J;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};

pub(crate) trait Admission {
    fn fields(&self) -> &Map;
    fn raw(&self) -> &BTreeMap<String, V>;
    fn role(&self, name: &str) -> &str {
        self.fields()
            .get(name)
            .and_then(|v| text(v).ok())
            .unwrap_or("")
    }
    fn document(&self) -> &V;
    fn hypotheses(&self) -> Result<&Map>;
    fn as_of(&self) -> Option<&V>;
    fn core(&self) -> bool;
    fn same(&self, a: &V, b: &V) -> Result<bool> {
        crate::reasoning_authoring::same(a, b)
    }
    fn value(&mut self, id: &str) -> Result<V>;
    fn result(&mut self, id: &str) -> Result<J>;
    fn same_value(&mut self, id: &str, candidate: &V) -> Result<bool>;
    fn standing_predicate(&self, id: &str, expression: &V) -> Result<Option<bool>>;
}
impl Admission for World<'_> {
    fn fields(&self) -> &Map {
        self.fields()
    }
    fn raw(&self) -> &BTreeMap<String, V> {
        self.raw()
    }
    fn document(&self) -> &V {
        self.document()
    }
    fn hypotheses(&self) -> Result<&Map> {
        map(&map(self.snapshot.data())?["hypotheses"])
    }
    fn as_of(&self) -> Option<&V> {
        map(self.snapshot.data()).ok()?.get("as_of")
    }
    fn core(&self) -> bool {
        true
    }
    fn value(&mut self, id: &str) -> Result<V> {
        self.value(id)
    }
    fn result(&mut self, id: &str) -> Result<J> {
        self.result(id)
    }
    fn same_value(&mut self, id: &str, candidate: &V) -> Result<bool> {
        self.same_value(id, candidate)
    }
    fn standing_predicate(&self, id: &str, expression: &V) -> Result<Option<bool>> {
        self.standing_predicate(id, expression)
    }
}

static ID: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)*$").unwrap());
static REFS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*\}\}").unwrap()
});
static DOTTED: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+").unwrap());
static EXPR: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[<>=!+\-*/()]|\b(?:or|and|not)\b").unwrap());
static CMP: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*(<=|>=|==|!=|<|>)")
        .unwrap()
});
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn refs(v: &str) -> Vec<String> {
    REFS.captures_iter(v).map(|c| c[1].to_owned()).collect()
}
fn builtin(id: &str) -> bool {
    F::BUILTINS.contains(&id)
}
fn get<'a>(m: &'a Map, k: &str) -> &'a V {
    m.get(k).unwrap_or(&V::Null)
}
fn names(v: &V) -> Vec<String> {
    match v {
        V::List(a) => a
            .iter()
            .filter_map(|v| text(v).ok().map(str::to_owned))
            .collect(),
        _ => vec![],
    }
}
fn shaped(body: &V, deps: &str) -> bool {
    map(body).ok().and_then(|m| m.get(deps)).is_some_and(
        |v| matches!(v,V::List(a) if !a.is_empty()&&a.iter().all(|v|matches!(v,V::Text(_)))),
    )
}
fn judgment(world: &impl Admission, id: &str) -> bool {
    world
        .raw()
        .get(id)
        .and_then(|v| map(v).ok())
        .is_some_and(|m| m.contains_key(world.role("deps")))
}
fn source(body: &V) -> bool {
    map(body).is_ok_and(|m| {
        !["v", "rule", "quoted"].iter().any(|k| m.contains_key(*k))
            && ["asked", "url", "file", "read"]
                .iter()
                .any(|k| m.contains_key(*k))
    })
}
fn asked(world: &impl Admission, id: &str) -> bool {
    world
        .raw()
        .get(id)
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("asked"))
        .is_some_and(truth)
}
pub(crate) fn arrangement(world: &impl Admission, body: &V) -> bool {
    let dep = world.role("deps");
    if !shaped(body, dep) {
        return false;
    }
    let m = map(body).unwrap();
    let deps = names(&m[dep]);
    let pred = get(m, world.role("predicate"));
    let rs = if let V::Text(v) = pred {
        DOTTED.find_iter(v).map(|m| m.as_str().into()).collect()
    } else {
        L::legacy_references(pred)
    };
    deps.iter().any(|d| asked(world, d))
        && (deps.iter().any(|d| builtin(d)) || rs.iter().any(|r| builtin(r)))
}
fn layers(world: &impl Admission) -> Result<BTreeMap<String, BTreeMap<String, V>>> {
    let mut result = BTreeMap::new();
    for (name, h) in world.hypotheses()? {
        let h = map(h)?;
        if h.get("error").is_some_and(truth) {
            continue;
        }
        result.insert(
            name.clone(),
            entries(&h["document"])?
                .into_iter()
                .map(|(k, (_, v))| (k, v))
                .collect(),
        );
    }
    Ok(result)
}
fn held_by(layers: &BTreeMap<String, BTreeMap<String, V>>, id: &str) -> Vec<String> {
    layers
        .iter()
        .filter(|(_, m)| m.contains_key(id))
        .map(|(name, _)| name.clone())
        .collect()
}
pub(crate) fn collection_for(
    world: &impl Admission,
    id: &str,
    body: &V,
    explicit: Option<&str>,
) -> Result<String> {
    collection_for_document(world.document(), world.fields(), id, body, explicit)
}
pub(crate) fn collection_for_document(
    document: &V,
    fields: &Map,
    id: &str,
    body: &V,
    explicit: Option<&str>,
) -> Result<String> {
    let cols = F::collections(document)?;
    if let Some(e) = explicit.filter(|s| !s.is_empty()) {
        require(
            cols.contains_key(e) || map(document)?.contains_key(e),
            &format!("no collection {e} in this record"),
        )?;
        return Ok(e.into());
    }
    let deps = text(&fields["deps"])?;
    let jud = |b: &V| map(b).is_ok_and(|m| m.contains_key(deps));
    if cols.is_empty() {
        return Ok(if jud(body) {
            "judgments"
        } else if !matches!(body, V::Map(_)) {
            "open"
        } else if source(body) {
            "sources"
        } else {
            "known"
        }
        .into());
    }
    let head = id.split('.').next();
    let homes = cols
        .iter()
        .filter(|(_, m)| m.keys().any(|k| k.split('.').next() == head))
        .map(|(c, _)| c.clone())
        .collect::<Vec<_>>();
    if homes.len() == 1 {
        return Ok(homes[0].clone());
    }
    let most = |count: &dyn Fn(&str, &V) -> bool| {
        cols.iter()
            .map(|(c, m)| (c, m.iter().filter(|(k, b)| count(k, b)).count()))
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
    };
    if jud(body) {
        return Ok(most(&|_, b| jud(b))
            .filter(|(_, n)| *n > 0)
            .map(|(c, _)| c.clone())
            .unwrap_or_else(|| "judgments".into()));
    }
    if !matches!(body, V::Map(_)) {
        return Ok(["open", "questions"]
            .into_iter()
            .find(|c| cols.contains_key(*c))
            .unwrap_or("open")
            .into());
    }
    if source(body) {
        let mut cited = BTreeMap::<&str, usize>::new();
        for b in cols.values().flat_map(|m| m.values()) {
            if let Some(V::Text(id)) = map(b).ok().and_then(|m| m.get("from")) {
                for (c, m) in &cols {
                    if m.contains_key(id) {
                        *cited.entry(c).or_default() += 1;
                    }
                }
            }
        }
        if let Some((c, _)) = cited
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        {
            return Ok(c.into());
        }
        if cols.contains_key("sources") {
            return Ok("sources".into());
        }
        return Ok(cols
            .iter()
            .find(|(c, m)| {
                !["open", "questions", "judgments"].contains(&c.as_str())
                    && !m.is_empty()
                    && m.values().all(source)
            })
            .map(|(c, _)| c.clone())
            .unwrap_or_else(|| "sources".into()));
    }
    Ok(most(&|_, b| {
        !jud(b) && map(b).is_ok_and(|m| ["v", "rule", "quoted"].iter().any(|k| m.contains_key(*k)))
    })
    .filter(|(_, n)| *n > 0)
    .map(|(c, _)| c.clone())
    .unwrap_or_else(|| "known".into()))
}
fn scalar_type(
    world: &impl Admission,
    tree: &J,
    visiting: &mut BTreeSet<String>,
) -> Result<Option<&'static str>> {
    if let Some(id) = tree.get("ref").and_then(J::as_str) {
        if visiting.contains(id) || !world.raw().contains_key(id) {
            return Ok(None);
        }
        visiting.insert(id.into());
        let body = &world.raw()[id];
        let r = if let Ok(m) = map(body) {
            if let Some(rule @ V::Map(_)) = m.get("rule") {
                scalar_type(world, &L::lower(rule)?, visiting)?
            } else {
                raw_type(m.get("v").or_else(|| m.get("quoted")).unwrap_or(&V::Null))
            }
        } else {
            raw_type(body)
        };
        visiting.remove(id);
        return Ok(r);
    }
    for (key, t) in [("text", "text"), ("bool", "boolean"), ("num", "number")] {
        if tree.get(key).is_some() {
            return Ok(Some(t));
        }
    }
    if tree
        .get("op")
        .and_then(J::as_str)
        .is_some_and(|s| ["add", "sub", "mul", "div"].contains(&s))
    {
        for child in tree["args"].as_array().unwrap() {
            let t = scalar_type(world, child, visiting)?;
            require(
                !matches!(t, Some("text" | "boolean")),
                "text or boolean arithmetic needs an explicit choice of typed semantics",
            )?;
        }
        return Ok(Some("number"));
    }
    Ok(None)
}
fn raw_type(v: &V) -> Option<&'static str> {
    match v {
        V::Text(_) => Some("text"),
        V::Bool(_) => Some("boolean"),
        V::Integer(_) | V::Float(_) => Some("number"),
        _ => None,
    }
}
pub(crate) fn normalize(
    world: &impl Admission,
    action: &V,
    previous: Option<&BTreeMap<String, V>>,
) -> Result<(V, Vec<String>)> {
    let a = map(action)?;
    if !string_is(get(a, "kind"), "add") {
        return Ok((action.clone(), vec![]));
    }
    let Ok(body) = map(get(a, "body")) else {
        return Ok((action.clone(), vec![]));
    };
    let id = text(field(a, "id")?)?;
    let predicate = body.contains_key(world.role("deps"));
    let field = if predicate {
        world.role("predicate")
    } else {
        "rule"
    };
    let Some(V::Text(source)) = body.get(field) else {
        return Ok((action.clone(), vec![]));
    };
    if previous
        .unwrap_or(world.raw())
        .get(id)
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get(field))
        == Some(&s(source))
    {
        return Ok((action.clone(), vec![]));
    }
    let keep = |reason: String| {
        Ok((
            action.clone(),
            vec![format!("NOTE {id}.{field} kept as text: {reason}")],
        ))
    };
    if !predicate && ["v", "quoted"].iter().any(|k| body.contains_key(*k)) {
        return keep("a stored reading and a calculation need distinct fields/entries".into());
    }
    let expression = V::Map(Map::from([("expr".into(), s(source))]));
    let tree = match L::convert_authored(source, predicate) {
        Ok(t) => t,
        Err(_) => match L::lower(&expression) {
            Ok(t) => {
                if !L::required_modules(&t).contains(&"composition/v1".into()) {
                    return keep("implicit text is outside the characterized scalar subset".into());
                }
                t
            }
            Err(e) => return keep(e.0),
        },
    };
    let refs = L::references(&tree);
    World::check_builtin(&refs)?;
    if !predicate && refs.iter().any(|r| !world.raw().contains_key(r) && r != id) {
        return keep(
            "unknown or ambiguous reference; use rule={expr: \"...\"} for an intended calculation"
                .into(),
        );
    }
    let check = (|| -> Result<bool> {
        if predicate
            && tree
                .get("op")
                .and_then(J::as_str)
                .is_some_and(|s| ["eq", "ne", "lt", "le", "gt", "ge"].contains(&s))
        {
            let args = tree["args"].as_array().unwrap();
            let kinds = args
                .iter()
                .map(|c| scalar_type(world, c, &mut BTreeSet::new()))
                .collect::<Result<Vec<_>>>()?;
            return Ok(kinds.contains(&Some("text"))
                && (kinds.contains(&Some("number"))
                    || args.iter().all(|a| a.get("ref").is_some())));
        }
        scalar_type(world, &tree, &mut BTreeSet::new())?;
        Ok(false)
    })();
    match check {
        Ok(true) => {
            return keep(
                "comparisons between text readings need an explicit choice of typed semantics"
                    .into(),
            );
        }
        Err(e) => return keep(e.0),
        _ => {}
    }
    let mut a = a.clone();
    let mut body = body.clone();
    body.insert(field.into(), expression);
    a.insert("body".into(), V::Map(body));
    Ok((
        V::Map(a),
        vec![format!("{id}.{field}: stored as a readable expression")],
    ))
}
fn short(v: &V, n: usize) -> String {
    let s = py(v).split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > n {
        s.chars().take(n.saturating_sub(3)).collect::<String>() + "..."
    } else {
        s
    }
}
fn day(v: &V) -> Option<String> {
    let t = py(v);
    let d = t.trim_start().get(..10)?;
    crate::value::Date::new(d).ok().map(|_| d.into())
}
pub(crate) fn read_on(body: &V, world: &impl Admission) -> Option<String> {
    let m = map(body).ok()?;
    for k in ["of", "read"] {
        if let Some(d) = m.get(k).and_then(day) {
            return Some(d);
        }
    }
    let src = m
        .get("from")
        .and_then(|v| text(v).ok())
        .and_then(|s| world.raw().get(s))
        .and_then(|v| map(v).ok())?;
    for k in ["read", "of"] {
        if let Some(d) = src.get(k).and_then(day) {
            return Some(d);
        }
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SupersessionDecision {
    pub allowed: bool,
    pub reason: String,
}

fn page_facts<'a>(page: Option<&'a V>, id: &str) -> Result<Option<&'a Map>> {
    let Some(page) = page else { return Ok(None) };
    let page = map(page).map_err(|_| Error("invalid_page_facts".into()))?;
    let Some(facts) = page.get(id) else {
        return Ok(None);
    };
    Ok(Some(
        map(facts).map_err(|_| Error("invalid_page_facts".into()))?,
    ))
}

fn page_bool(facts: &Map, key: &str) -> Result<bool> {
    match facts.get(key) {
        Some(V::Bool(value)) => Ok(*value),
        Some(_) => Err(Error("invalid_page_facts".into())),
        None => Err(Error("invalid_page_facts".into())),
    }
}

fn page_text(facts: &Map, key: &str) -> Option<String> {
    facts
        .get(key)
        .and_then(|value| text(value).ok())
        .map(str::to_owned)
}

fn validate_page_facts(facts: &Map) -> Result<(bool, bool)> {
    let linked = page_bool(facts, "linked")?;
    let fired = page_bool(facts, "fired")?;
    for key in ["cut", "reading"] {
        if let Some(value) = facts.get(key) {
            text(value).map_err(|_| Error("invalid_page_facts".into()))?;
        }
    }
    if let Some(value) = facts.get("stood")
        && !matches!(value, V::Integer(_))
    {
        return Err(Error("invalid_page_facts".into()));
    }
    Ok((linked, fired))
}

/// Apply the single same-id replacement door used by ordinary authoring and
/// identity operations. `page` is already-captured arrangement evidence; this
/// function never rereads a brief or source to obtain it.
pub(crate) fn may_supersede(
    world: &impl Admission,
    id: &str,
    existing: &V,
    new: &V,
    captured_day: Option<&str>,
    page: Option<&V>,
    by_hand: bool,
) -> Result<SupersessionDecision> {
    use crate::public_consolidation::supersession as S;
    if judgment(world, id) || shaped(existing, world.role("deps")) {
        let decision = S::judgment(
            shaped(new, world.role("deps")),
            world.role("deps"),
            by_hand,
            || {
                let facts = page_facts(page, id)?;
                let stamp = captured_day
                    .map(str::to_owned)
                    .or_else(|| world.as_of().and_then(day))
                    .ok_or_else(|| Error("write requires an explicitly captured day".into()))?;
                let page = facts
                    .map(|facts| {
                        let (linked, fired) = validate_page_facts(facts)?;
                        Ok::<_, Error>(S::PageFacts {
                            linked,
                            fired,
                            cut: page_text(facts, "cut"),
                            reading: page_text(facts, "reading"),
                        })
                    })
                    .transpose()?;
                let pred = map(existing)?
                    .get(world.role("predicate"))
                    .unwrap_or(&V::Null);
                Ok(S::JudgmentEvidence {
                    stamp,
                    page,
                    born: map(existing)?.get("born").and_then(day),
                    predicate: short(&V::Text(crate::public_ordinary_readers::predicate_text(pred)), 60),
                })
            },
            || {
                let pred = map(existing)?
                    .get(world.role("predicate"))
                    .unwrap_or(&V::Null);
                world.standing_predicate(id, pred)
            },
        )?;
        return Ok(SupersessionDecision {
            allowed: decision.allowed,
            reason: decision.reason,
        });
    }
    let when = read_on(existing, world);
    let stamp = captured_day
        .map(str::to_owned)
        .or_else(|| world.as_of().and_then(day))
        .ok_or_else(|| Error("write requires an explicitly captured day".into()))?;
    let decision = S::reading(&stamp, when.as_deref());
    Ok(SupersessionDecision {
        allowed: decision.allowed,
        reason: decision.reason,
    })
}

struct Disagreement {
    kind: &'static str,
    old: V,
    new: V,
    may: bool,
    why: String,
    when: Option<String>,
}
fn disagreement(
    world: &mut impl Admission,
    a: &Map,
    page: Option<&V>,
) -> Result<Option<Disagreement>> {
    let id = text(field(a, "id")?)?;
    let Some(body) = world.raw().get(id).cloned() else {
        return Ok(None);
    };
    let kind = text(field(a, "kind")?)?;
    let new_body = get(a, "body");
    if judgment(world, id) {
        if kind != "add" || !shaped(new_body, world.role("deps")) {
            return Ok(None);
        }
        let old = map(&body)?
            .get("verdict")
            .or_else(|| map(&body).unwrap().get("title"));
        let new = map(new_body)?
            .get("verdict")
            .or_else(|| map(new_body).unwrap().get("title"));
        let (Some(old), Some(new)) = (old, new) else {
            return Ok(None);
        };
        let same = world.same(old, new)?;
        let kind = if same {
            let snapshot = world.role("snapshot");
            let clean = |v: &V| {
                map(v)
                    .unwrap()
                    .iter()
                    .filter(|(k, _)| {
                        !["born", "replaced", "reviewed", snapshot].contains(&k.as_str())
                    })
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Map>()
            };
            if clean(&body) == clean(new_body) {
                return Ok(None);
            }
            if arrangement(world, &body) {
                "arrangement"
            } else {
                "grounds"
            }
        } else {
            "verdict"
        };
        let decision = may_supersede(
            world,
            id,
            &body,
            new_body,
            a.get("as_of").and_then(day).as_deref(),
            page,
            false,
        )?;
        return Ok(Some(Disagreement {
            kind,
            old: old.clone(),
            new: new.clone(),
            may: decision.allowed,
            why: decision.reason,
            when: None,
        }));
    }
    if !matches!(body, V::Map(_)) {
        return Ok(None);
    }
    let old = world.value(id)?;
    if old == V::Null && world.result(id)?["status"] != "ok"
        || !world.core() && matches!(old, V::Map(_) | V::List(_))
    {
        return Ok(None);
    }
    let new = if kind == "set" {
        get(a, "value")
    } else {
        let Ok(m) = map(new_body) else {
            return Ok(None);
        };
        let Some(v) = m
            .get("v")
            .filter(|v| world.core() || **v != V::Null)
            .or_else(|| m.get("quoted"))
        else {
            return Ok(None);
        };
        if !world.core() && matches!(v, V::Null | V::List(_) | V::Map(_)) {
            return Ok(None);
        }
        v
    };
    if world.same_value(id, new)? {
        return Ok(None);
    }
    let decision = may_supersede(
        world,
        id,
        &body,
        if kind == "set" {
            get(a, "value")
        } else {
            new_body
        },
        a.get("as_of").and_then(day).as_deref(),
        None,
        false,
    )?;
    let when = read_on(&body, world);
    Ok(Some(Disagreement {
        kind: "value",
        old,
        new: new.clone(),
        may: decision.allowed,
        why: decision.reason,
        when,
    }))
}
fn shell(v: &str) -> String {
    if !v.is_empty()
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b))
    {
        v.into()
    } else {
        format!("'{}'", v.replace('\'', "'\"'\"'"))
    }
}
fn command(a: &Map, name: &str) -> String {
    let kind = text(get(a, "kind")).unwrap_or("");
    let mut p = vec![kind.into(), py(get(a, "id"))];
    if kind == "set" {
        let v = get(a, "value");
        p.push(match v {
            V::Bool(v) => v.to_string(),
            V::Text(v)
                if !regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_.\-]*$")
                    .unwrap()
                    .is_match(v)
                    || crate::history_yaml::decode_document(format!("value: {v}").as_bytes())
                        .ok()
                        .and_then(|v| map(&v).ok().map(|m| m["value"].clone()))
                        != Some(V::Text(v.clone())) =>
            {
                if v.contains('\n') {
                    serde_json::to_string(v).unwrap()
                } else {
                    format!("'{}'", v.replace('\'', "''"))
                }
            }
            _ => py(v),
        })
    } else if let Ok(body) = map(get(a, "body")) {
        for (k, v) in body {
            let rendered = match v {
                V::Bool(v) => v.to_string(),
                V::Map(_) | V::List(_) => {
                    v.to_json().map(|j| j.to_string()).unwrap_or_else(|_| py(v))
                }
                _ => py(v),
            };
            p.push(format!("{k}={rendered}"));
        }
    } else {
        p.push(py(get(a, "body")))
    }
    for (opt, key) in [
        ("--in", "into"),
        ("--as-of", "as_of"),
        ("--why", "why"),
        ("--source", "source"),
        ("--at", "at"),
    ] {
        if let Some(v) = a.get(key).filter(|v| truth(v)) {
            p.extend([opt.into(), py(v)]);
        }
    }
    if let Ok(d) = map(get(a, "drops")) {
        for (k, v) in d {
            p.extend(["--drop".into(), format!("{k}: {}", py(v))]);
        }
    }
    p.extend(["--hypothesis".into(), name.into()]);
    p.into_iter()
        .map(|p| if p.starts_with("--") { p } else { shell(&p) })
        .collect::<Vec<_>>()
        .join(" ")
}
fn hypothesis(world: &impl Admission, id: &str, claim: &V) -> Result<String> {
    let normalized = crate::reasoning_authoring::claim_key(claim)?;
    let base = format!(
        "{}_{}",
        id.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            })
            .collect::<String>(),
        &crate::identity::sha256(normalized.as_bytes())[..6]
    );
    let hypotheses = world.hypotheses()?;
    let mut name = base.clone();
    let mut suffix = 1;
    while let Some(h) = hypotheses.get(&name) {
        let old = entries(&map(h)?["document"])?;
        let same = old
            .get(id)
            .map(|(_, body)| -> Result<bool> {
                let existing_claim = map(body)
                    .ok()
                    .and_then(|m| {
                        ["verdict", "v", "quoted", "rule", "title"]
                            .iter()
                            .find_map(|k| m.get(*k).filter(|v| **v != V::Null))
                    })
                    .unwrap_or(body);
                if matches!(existing_claim, V::Map(_) | V::List(_))
                    || matches!(claim, V::Map(_) | V::List(_))
                {
                    Ok(crate::source_clock::python_equal(existing_claim, claim))
                } else {
                    Ok(crate::reasoning_authoring::claim_key(existing_claim)? == normalized)
                }
            })
            .transpose()?
            .unwrap_or(false);
        if same {
            break;
        }
        suffix += 1;
        name = format!("{base}_{suffix}");
    }
    Ok(name)
}
pub(crate) fn validate(world: &mut impl Admission, action: &V) -> Result<Vec<String>> {
    validate_with_page(world, action, None)
}

pub(crate) fn validate_with_page(
    world: &mut impl Admission,
    action: &V,
    page: Option<&V>,
) -> Result<Vec<String>> {
    let a = map(action)?;
    let id = text(field(a, "id")?)?;
    let kind = text(field(a, "kind")?)?;
    let empty = Map::new();
    let b = map(get(a, "body")).unwrap_or(&empty);
    let body = get(a, "body");
    let dep = world.role("deps").to_owned();
    let pred = world.role("predicate").to_owned();
    let seen = world.role("snapshot").to_owned();
    let layers = layers(world)?;
    let aimed = a.get("hypothesis").filter(|v| truth(v));
    let known = world.raw().contains_key(id);
    let is_jud = judgment(world, id);
    let mut out = vec![];
    let reframe = a.get("reframe") == Some(&V::Bool(true));
    if a.contains_key("reframe") && !matches!(a.get("reframe"), Some(V::Bool(_))) {
        out.push("reframe must be a boolean operation option".into());
    }
    if reframe {
        let old = world.raw().get(id).and_then(|v| map(v).ok());
        let stored = old.and_then(|m| m.get("v").or_else(|| m.get("quoted")));
        if kind != "add" || !world.core() || !known || is_jud || builtin(id)
            || aimed.is_some() || old.is_some_and(|m| m.contains_key("rule"))
            || !matches!(stored, Some(V::Bool(_) | V::Text(_) | V::Integer(_) | V::Float(_))) {
            out.push("reframe requires an existing stored scalar in an active core/v1 history; it cannot replace judgments, rules or hypotheses".into());
        }
        if !matches!(b.get("rule"), Some(V::Map(_)))
            || b.keys().any(|k| !["rule", "name"].contains(&k.as_str())) {
            out.push("reframe needs only a structured rule and optional name; historical citations and snapshots are preserved by the writer".into());
        }
        if !a.get("why").and_then(|v| text(v).ok()).is_some_and(|v| !v.trim().is_empty()) {
            out.push("reframe requires --why explaining why the rule represents the same subject".into());
        }
    }
    // `answer` and `correct` rewrite one existing entry in place; the command decides
    // whether that is allowed, and these checks keep what it writes in the record's shape.
    let amend = a.get("amend").filter(|v| **v != V::Null).map(py);
    if let Some(amend) = &amend {
        let place = entries(world.document())?;
        let question = |key: &str| {
            place
                .get(key)
                .is_some_and(|(collection, _)| ["open", "questions"].contains(&collection.as_str()))
        };
        let old = world.raw().get(id).and_then(|v| map(v).ok());
        let settled = |m: Option<&Map>| {
            m.is_some_and(|m| m.contains_key("answered") || m.contains_key("dropped"))
        };
        if kind != "add" || !known || builtin(id) || aimed.is_some() || reframe {
            out.push(format!("{id} is not an entry of the record - {amend} rewrites one that exists"));
        } else if amend == "answer" {
            let by = a.get("answer_by").filter(|v| **v != V::Null).map(py);
            if !question(id) {
                out.push(format!("{id} is not an open question - answer closes an entry of open:"));
            } else if settled(old) {
                out.push(format!("{id} is already answered - its answer is read with pull {id}"));
            } else if let Some(by) = &by {
                if by == id || question(by) {
                    out.push(format!("{by} is a question - an answer is an entry or a judgment of the record"));
                } else if !world.raw().contains_key(by) {
                    out.push(format!("{by} is not an entry - record the answer first, then answer {id} {by}"));
                } else if b
                    .get("answered")
                    .and_then(|v| map(v).ok())
                    .and_then(|m| m.get("by"))
                    .map(py)
                    .as_deref()
                    != Some(by.as_str())
                {
                    out.push("the answered question names what answered it under answered: by".into());
                }
            } else if !b.get("dropped").is_some_and(truth) {
                out.push(format!("answer {id} needs the entry that answered it, or --dropped with the reason it no longer matters"));
            }
        } else if amend == "correct" {
            if !is_jud && b.contains_key(&dep) {
                out.push(format!("{id} is not a judgment - a correction keeps what an entry is; resting it on something is a new decision"));
            }
            let changed = |key: &str| {
                old.and_then(|m| m.get(key)).map(V::digest).transpose().ok().flatten()
                    != b.get(key).map(V::digest).transpose().ok().flatten()
            };
            if ["answered", "dropped"].iter().any(|key| changed(key)) {
                out.push(format!("{id} records how a question was settled - that is written by answer, never corrected"));
            }
            for key in ["replaced", "reviewed", "born"] {
                if changed(key) {
                    out.push(format!("{key} is written by this tool - a correction leaves it as it is"));
                }
            }
            let resting = layers
                .iter()
                .filter(|(_, m)| {
                    m.contains_key(id)
                        || m.values().any(|body| {
                            map(body).is_ok_and(|m| names(get(m, &dep)).iter().any(|d| d == id))
                        })
                })
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>();
            if !resting.is_empty() {
                out.push(format!(
                    "hypothes{} {} hold{} {id} or rest{} on it - a correction there is a decision for the fold",
                    if resting.len() == 1 { "is" } else { "es" },
                    resting.join(", "),
                    if resting.len() == 1 { "s" } else { "" },
                    if resting.len() == 1 { "s" } else { "" },
                ));
            }
        } else {
            out.push(format!("{amend} is not a rewrite this reader knows"));
        }
    }
    let diff = if (kind == "add" || kind == "set") && known && amend.is_none() {
        disagreement(world, a, page)?
    } else {
        None
    };
    // Identity and tool-authored fields.
    if kind == "set" {
        if !known {
            if aimed.is_some() || held_by(&layers, id).is_empty() {
                out.push(format!("{id} is not an entry. A new entry is added, with its source: add {id} v=... from=..."))
            }
        } else if is_jud {
            out.push(format!("{id} is a judgment: it is reviewed, not set"))
        } else if builtin(id) {
            out.push(format!("{id} is counted by the reader, never set"))
        } else if let Some(V::Map(old)) = world.raw().get(id)
            && !old.contains_key("v")
            && !old.contains_key("quoted")
        {
            out.push(format!(
                "{id} holds no value of its own{}",
                if old.get("rule").is_some_and(truth) {
                    " - it is worked out from a rule; change the rule, not the result"
                } else {
                    ""
                }
            ))
        }
    }
    if kind == "add" {
        if !ID.is_match(id) {
            out.push(format!(
                "{id} is not an id: letters, digits, underscores, dots"
            ))
        } else if is_jud && !shaped(body, &dep) {
            out.push(format!("{id} is a judgment - what replaces it rests on something: give {dep}=[...] with the new verdict, or review it"))
        } else if aimed
            .and_then(|v| text(v).ok())
            .and_then(|h| layers.get(h))
            .map_or(known, |m| m.contains_key(id))
            || map(world.document())?
                .get("meta")
                .and_then(|v| map(v).ok())
                .is_some_and(|m| m.contains_key(id))
        {
            if let Some(h) = aimed {
                out.push(format!("{id} is already in hypothesis {} - set changes its value there, review its snapshot",py(h)))
            } else if diff.is_none() && !reframe && amend.is_none() {
                out.push(format!(
                    "{id} is already an entry - set changes its value, review its snapshot"
                ))
            }
        } else if builtin(id) {
            out.push(format!(
                "{id} is a name the reader computes; it cannot be written"
            ))
        }
        if b.contains_key(&seen) {
            out.push(format!(
                "{seen} is written by this tool, from what the dependencies hold - leave it out"
            ))
        }
    }
    if kind == "review" && !is_jud && !a.get("section").is_some_and(truth) {
        out.push(format!(
            "{id} is not a judgment{}",
            if known {
                " - an entry's value is set, not reviewed"
            } else {
                ""
            }
        ))
    }
    if kind == "add" {
        if let Some(ds) = b.get(&dep).filter(|v| **v != V::Null) {
            if !matches!(ds,V::List(a)if !a.is_empty()&&a.iter().all(|v|matches!(v,V::Text(_)))) {
                out.push(format!("{dep} must be a list of entry ids"));
            } else {
                let ds = names(ds);
                for d in &ds {
                    if !world.raw().contains_key(d)
                        && !builtin(d)
                        && blocked_text(body).is_empty()
                        && (aimed.is_some() || held_by(&layers, d).is_empty())
                    {
                        out.push(format!("rests on {d}, which is not an entry - add it first, or declare it missing with blocked_on"))
                    }
                }
                let p = get(b, &pred);
                let rs = if matches!(p, V::Map(_)) {
                    L::references(
                        &L::lower(p).map_err(|e| Error(format!("refused - {pred}: {e}")))?,
                    )
                } else {
                    DOTTED
                        .find_iter(&py(p))
                        .map(|m| m.as_str().into())
                        .collect()
                };
                for r in rs.into_iter().collect::<BTreeSet<_>>() {
                    if (matches!(p, V::Map(_)) || world.raw().contains_key(&r) || builtin(&r))
                        && !ds.contains(&r)
                    {
                        out.push(format!(
                            "wrong_if reads {r}, which the judgment does not rest on"
                        ))
                    }
                }
            }
        }
        for (f, v) in b {
            if let V::Text(v) = v {
                for r in refs(v) {
                    if !world.raw().contains_key(&r) && !builtin(&r) {
                        if aimed.is_none() && !held_by(&layers, &r).is_empty() {
                            continue;
                        }
                        out.push(format!("{f} references {r}, which is not an entry"))
                    } else if matches!(b.get(&dep), Some(V::List(_)))
                        && !names(&b[&dep]).contains(&r)
                        && r != id
                    {
                        out.push(format!("{f} references {r}, which the judgment does not rest on - a change to it would never reach this"))
                    }
                }
            }
        }
    }
    // Replacing a citation requires an actual recorded source and an exact location.
    if a.get("source").is_some_and(|v| *v != V::Null) || a.get("at").is_some_and(|v| *v != V::Null)
    {
        if kind != "set" {
            out.push(
                "--source and --at apply to set; add carries from= and at= in its fields".into(),
            )
        } else if ["source", "at"]
            .iter()
            .any(|k| !text(get(a, k)).is_ok_and(|s| !s.trim().is_empty()))
        {
            out.push("set requires --source and --at together, both nonempty".into())
        } else {
            let src = text(get(a, "source"))?;
            let sb = world.raw().get(src).and_then(|v| map(v).ok());
            if src == id
                || judgment(world, src)
                || builtin(src)
                || sb.is_none_or(|m| {
                    ["v", "quoted", "rule"].iter().any(|k| m.contains_key(*k))
                        || !["asked", "file", "url", "of", "read"]
                            .iter()
                            .any(|k| m.get(*k).is_some_and(truth))
                })
            {
                out.push(format!(
                    "{src} is not a recorded source; add the source before citing it"
                ))
            } else if let Some(V::Map(m)) = world.raw().get(id) {
                if ["src", "source"].iter().any(|k| m.contains_key(*k)) {
                    out.push("set --source writes from/at; reconcile the entry's src/source fields first".into())
                } else if ["from", "at"]
                    .iter()
                    .any(|k| matches!(m.get(*k), Some(V::Map(_) | V::List(_))))
                {
                    out.push("set --source needs scalar from/at fields".into())
                }
            }
        }
    }
    if kind == "add" {
        let reopened = reopened_text(body);
        if CMP
            .captures(&reopened)
            .is_some_and(|c| world.raw().contains_key(&c[1]))
        {
            out.push(format!("reopened_by reads as a comparison ({}) - a predicate belongs in wrong_if, where it is evaluated",short(&s(&reopened),60)))
        }
        if let Some(req) = b.get("request").filter(|v| **v != V::Null)
            && !text(req)
                .is_ok_and(|r| asked(world, r) && names(get(b, &dep)).contains(&r.to_owned()))
        {
            out.push(format!("request: {} is not a session source carrying what was asked, that the judgment also rests on",py(req)))
        }
        if let Some(name) = b.get("measure") {
            let pattern = regex::Regex::new(r"^[A-Za-z][A-Za-z0-9_-]*$").unwrap();
            let n = py(name);
            let v = b
                .get("v")
                .filter(|v| **v != V::Null)
                .or_else(|| b.get("quoted"))
                .unwrap_or(&V::Null);
            let bad = if !text(name).is_ok_and(|s| pattern.is_match(s)) {
                Some(format!("measure: {:?} is not a recipe name - letters, digits, underscores and dashes, opening with a letter; what runs lives in the allowlist beside the record, never here",short(name,90)).replace('"',"'"))
            } else if is_jud || b.contains_key(&dep) {
                Some(format!(
                    "measure: {n} on a judgment - a judgment is not measured; the entries it rests on are"
                ))
            } else if builtin(id) {
                Some(format!("measure: {n} on a name the reader computes"))
            } else if b.get("rule").is_some_and(|v| *v != V::Null)
                || text(v).is_ok_and(|s| {
                    EXPR.is_match(s)
                        && DOTTED
                            .find_iter(s)
                            .any(|m| world.raw().contains_key(m.as_str()))
                })
            {
                Some(format!(
                    "measure: {n} on an entry worked out from a rule - measure what it is worked out from"
                ))
            } else if *v == V::Null {
                Some(format!(
                    "measure: {n} on an entry with no value of its own - a recipe's line replaces a stored reading"
                ))
            } else if matches!(v, V::List(_) | V::Map(_)) {
                Some(format!(
                    "measure: {n} on a value that is a list or a mapping - a recipe prints one scalar"
                ))
            } else {
                None
            };
            if let Some(bad) = bad {
                out.push(bad)
            }
        }
    }
    // Retired aliases include every readable hypothesis, never an id still live.
    let live = world
        .raw()
        .keys()
        .chain(layers.values().flat_map(|m| m.keys()))
        .cloned()
        .collect::<BTreeSet<_>>();
    if !known && !live.contains(id) && dep != "also" {
        for m in std::iter::once(world.raw()).chain(layers.values()) {
            if let Some((into, _)) = m.iter().find(|(_, b)| {
                map(b)
                    .ok()
                    .and_then(|m| m.get("also"))
                    .is_some_and(|v| match v {
                        V::Text(v) => v == id,
                        V::List(a) => a.contains(&s(id)),
                        _ => false,
                    })
            }) {
                out.push(format!(
                    "{id} was retired into {into} - write {into} instead; it carries also: [{id}]"
                ));
                break;
            }
        }
    }
    if aimed.is_none() {
        if kind == "set" && !known {
            let held = held_by(&layers, id);
            if !held.is_empty() {
                out.push(format!(
                    "{id} is held only by hypothes{} {} - it is set there: {}",
                    if held.len() == 1 { "is" } else { "es" },
                    held.join(", "),
                    command(a, &held[0])
                ));
            }
        } else if kind == "set" {
            if let Some(d) = &diff
                && !d.may
            {
                out.push(format!("{id} holds {} as of {}, and {} says {} - the base keeps what it holds and a hypothesis holds the other: {}",short(&d.old,90),d.when.as_deref().unwrap_or("None"),d.why,short(&d.new,90),command(a,&hypothesis(world, id,&d.new)?)))
            }
        } else if kind == "add" {
            if let Some(d) = &diff
                && !(d.kind != "value" && d.may)
            {
                let claim = if d.kind == "grounds" {
                    s(&format!("{} (regrounded)", py(&d.new)))
                } else {
                    d.new.clone()
                };
                let cmd = command(a, &hypothesis(world, id, &claim)?);
                out.push(match d.kind{"verdict"=>format!("{id} is already a judgment, concluding {} - {}, so a different verdict under the same id contradicts it, and a hypothesis holds the other: {cmd}",crate::source_text::python_repr(&crate::history_yaml::SourceValue::from_typed(&s(&short(&d.old,60)))),d.why),"grounds"=>format!("{id} is already a judgment concluding the same, on other grounds - {}, so the same verdict on other grounds is a decision written again, and a hypothesis holds it until a person takes it: {cmd}",d.why),"arrangement"=>format!("{id} is already this arrangement - {}, so a hypothesis holds the re-decision: {cmd}",d.why),_=>if d.may{format!("{id} is already an entry, holding {}{} - a newer reading updates it: set {id} {}; one that disagrees opens a hypothesis: {cmd}",short(&d.old,90),d.when.as_ref().map(|w|format!(" as of {w}")).unwrap_or_default(),shell(&py(&d.new)))}else{format!("{id} is already an entry, holding {} as of {}, and {} says {} - the base keeps what it holds and a hypothesis holds the other: {cmd}",short(&d.old,90),d.when.as_deref().unwrap_or("None"),d.why,short(&d.new,90))}});
            }
            let ds = names(get(b, &dep));
            let mut cone = ds
                .iter()
                .map(|d| (d.clone(), "rests on"))
                .collect::<Vec<_>>();
            for v in b.values() {
                if let V::Text(v) = v {
                    for r in refs(v) {
                        if !cone.iter().any(|(d, _)| *d == r) {
                            cone.push((r, "references"))
                        }
                    }
                }
            }
            for (d, how) in cone {
                if world.raw().contains_key(&d) || builtin(&d) {
                    continue;
                }
                let held = held_by(&layers, &d);
                if !held.is_empty() {
                    out.push(format!("{how} {d}, which only hypothes{} {} hold{} - what rests on a hypothesis goes into it: {}",if held.len()==1{"is"}else{"es"},held.join(", "),if held.len()==1{"s"}else{""},command(a,&held[0])));
                }
            }
        }
    }
    if kind == "add" && !arrangement(world, body) && b.contains_key("replaced") && amend.is_none() {
        out.push(
            "replaced is written by this tool, when a decision replaces another - leave it out"
                .into(),
        )
    }
    if aimed.is_none()
        && kind == "add"
        && is_jud
        && shaped(body, &dep)
        && !arrangement(world, body)
        && !arrangement(world, &world.raw()[id])
        && diff.as_ref().is_some_and(|d| d.kind != "value" && d.may)
    {
        let gone = names(&map(&world.raw()[id])?[&dep])
            .into_iter()
            .filter(|d| !names(get(b, &dep)).contains(d))
            .collect::<Vec<_>>();
        let drop_value = get(a, "drops");
        let drops = map(drop_value).ok();
        let drop_keys = if !truth(drop_value) {
            vec![]
        } else {
            match drop_value {
                V::Map(m) => m.keys().cloned().collect::<Vec<_>>(),
                V::Text(s) => s.chars().map(|c| c.to_string()).collect(),
                V::List(a) => a
                    .iter()
                    .map(|v| text(v).map(str::to_owned))
                    .collect::<Result<Vec<_>>>()?,
                _ => return Err(Error("drops must be a mapping".into())),
            }
        };
        let missing = gone
            .iter()
            .filter(|d| match drop_value {
                V::Text(s) => !s.contains(d.as_str()),
                _ => !drop_keys.contains(d),
            })
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() && truth(drop_value) && drops.is_none() {
            return Err(Error("drops must be a mapping".into()));
        }
        if !missing.is_empty() {
            out.push(format!(
                "the new judgment no longer rests on {} - a dependency dropped is a decision with a reason: {}",
                missing.join(", "), { let prefix=command(a,"_"); let prefix=prefix.split(" --hypothesis ").next().unwrap(); format!("{} {}",prefix,missing.iter().map(|d|format!("--drop {}",shell(&format!("{d}: <why>")))).collect::<Vec<_>>().join(" ")) }
            ))
        }
        let extra = drop_keys
            .into_iter()
            .filter(|d| !gone.contains(d))
            .collect::<Vec<_>>();
        if !extra.is_empty() {
            out.push(format!(
                "--drop names {}, which the new judgment {}",
                extra.join(", "),
                if extra.iter().any(|d| names(get(b, &dep)).contains(d)) {
                    "still rests on"
                } else {
                    "never rested on here"
                }
            ))
        }
    }
    if world.core()
        && (a.get("section").is_some_and(truth)
            || arrangement(world, body)
            || world
                .raw()
                .get(id)
                .is_some_and(|b| judgment(world, id) && arrangement(world, b)))
    {
        out.push("unsupported_core_builtin: page arrangement/section review requires its own declared inputs".into())
    }
    Ok(out)
}

#[cfg(test)]
mod supersession_tests {
    use super::*;
    use serde_json::json;

    struct Fake {
        fields: Map,
        raw: BTreeMap<String, V>,
        fired: Option<bool>,
        as_of: Option<V>,
    }

    impl Admission for Fake {
        fn fields(&self) -> &Map {
            &self.fields
        }
        fn raw(&self) -> &BTreeMap<String, V> {
            &self.raw
        }
        fn document(&self) -> &V {
            &V::Null
        }
        fn hypotheses(&self) -> Result<&Map> {
            Err(Error("unused".into()))
        }
        fn as_of(&self) -> Option<&V> {
            self.as_of.as_ref()
        }
        fn core(&self) -> bool {
            false
        }
        fn value(&mut self, _id: &str) -> Result<V> {
            Ok(V::Null)
        }
        fn result(&mut self, _id: &str) -> Result<J> {
            Ok(json!({"status":"ok"}))
        }
        fn same_value(&mut self, _id: &str, _candidate: &V) -> Result<bool> {
            Ok(false)
        }
        fn standing_predicate(&self, _id: &str, _expression: &V) -> Result<Option<bool>> {
            Ok(self.fired)
        }
    }

    fn s(value: &str) -> V {
        V::Text(value.into())
    }
    fn fields() -> Map {
        Map::from([
            ("deps".into(), s("rests_on")),
            ("predicate".into(), s("wrong_if")),
            ("snapshot".into(), s("seen")),
        ])
    }
    fn list(values: &[&str]) -> V {
        V::List(values.iter().map(|value| s(value)).collect())
    }
    fn judgment(predicate: &str) -> V {
        V::Map(Map::from([
            ("verdict".into(), s("continue")),
            ("rests_on".into(), list(&["p.input"])),
            ("wrong_if".into(), s(predicate)),
        ]))
    }
    fn fake(existing: V, fired: Option<bool>) -> Fake {
        Fake {
            fields: fields(),
            raw: BTreeMap::from([(String::from("d.keep"), existing)]),
            fired,
            as_of: None,
        }
    }

    #[test]
    fn judgment_shape_is_enforced_by_the_shared_door() {
        let existing = judgment("p.input > 9");
        let world = fake(existing.clone(), Some(true));
        let decision = may_supersede(
            &world,
            "d.keep",
            &existing,
            &s("stop"),
            Some("2026-09-20"),
            None,
            false,
        )
        .unwrap();
        assert!(!decision.allowed);
        assert!(decision.reason.contains("must rest on something"));
    }

    #[test]
    fn fired_and_by_hand_judgment_replacements_are_admitted() {
        let existing = judgment("p.input > 9");
        let replacement = V::Map(Map::from([
            ("verdict".into(), s("stop")),
            ("rests_on".into(), list(&["p.input"])),
        ]));
        let fired = fake(existing.clone(), Some(true));
        assert!(
            may_supersede(
                &fired,
                "d.keep",
                &existing,
                &replacement,
                Some("2026-09-20"),
                None,
                false
            )
            .unwrap()
            .allowed
        );
        let holding = fake(existing.clone(), Some(false));
        assert!(
            !may_supersede(
                &holding,
                "d.keep",
                &existing,
                &replacement,
                Some("2026-09-20"),
                None,
                false
            )
            .unwrap()
            .allowed
        );
        assert!(
            may_supersede(
                &holding,
                "d.keep",
                &existing,
                &replacement,
                Some("2026-09-20"),
                None,
                true
            )
            .unwrap()
            .allowed
        );
    }

    #[test]
    fn page_facts_are_used_only_when_present_and_cut_tabs_refuse() {
        let existing = judgment("p.input > 9");
        let replacement = V::Map(Map::from([
            ("verdict".into(), s("stop")),
            ("rests_on".into(), list(&["p.input"])),
        ]));
        let world = fake(existing.clone(), Some(false));
        let cut = V::Map(Map::from([(
            "d.keep".into(),
            V::Map(Map::from([
                ("linked".into(), V::Bool(false)),
                ("fired".into(), V::Bool(false)),
                ("cut".into(), s("the tab was removed")),
            ])),
        )]));
        let decision = may_supersede(
            &world,
            "d.keep",
            &existing,
            &replacement,
            Some("2026-09-20"),
            Some(&cut),
            false,
        )
        .unwrap();
        assert!(!decision.allowed);
        assert!(decision.reason.contains("restore the tab"));
        let fired = V::Map(Map::from([(
            "d.keep".into(),
            V::Map(Map::from([
                ("linked".into(), V::Bool(true)),
                ("fired".into(), V::Bool(true)),
                ("reading".into(), s("count 4")),
            ])),
        )]));
        let decision = may_supersede(
            &world,
            "d.keep",
            &existing,
            &replacement,
            Some("2026-09-20"),
            Some(&fired),
            false,
        )
        .unwrap();
        assert!(decision.allowed);
        assert!(decision.reason.contains("count 4"));

        let malformed = V::Map(Map::from([(
            "d.keep".into(),
            V::Map(Map::from([("linked".into(), V::Bool(true))])),
        )]));
        assert!(
            may_supersede(
                &world,
                "d.keep",
                &existing,
                &replacement,
                Some("2026-09-20"),
                Some(&malformed),
                false
            )
            .is_err()
        );
        let malformed = V::Map(Map::from([(
            "d.keep".into(),
            V::Map(Map::from([
                ("linked".into(), V::Bool(true)),
                ("fired".into(), V::Bool(true)),
                (
                    "reading".into(),
                    V::Integer(crate::value::Integer::new("4").unwrap()),
                ),
            ])),
        )]));
        assert!(
            may_supersede(
                &world,
                "d.keep",
                &existing,
                &replacement,
                Some("2026-09-20"),
                Some(&malformed),
                false
            )
            .is_err()
        );

        let born = V::Map(Map::from([
            ("verdict".into(), s("continue")),
            ("rests_on".into(), list(&["p.input"])),
            ("wrong_if".into(), s("p.input > 9")),
            ("born".into(), s("2026-09-20")),
        ]));
        let world = fake(born.clone(), Some(true));
        let decision = may_supersede(
            &world,
            "d.keep",
            &born,
            &replacement,
            Some("2026-09-20"),
            Some(&fired),
            false,
        )
        .unwrap();
        assert!(!decision.allowed);
        assert!(decision.reason.contains("same day"));
    }

    #[test]
    fn dated_value_replacement_uses_captured_day() {
        let existing = V::Map(Map::from([
            (
                "v".into(),
                V::Integer(crate::value::Integer::new("1").unwrap()),
            ),
            ("of".into(), s("2026-09-19")),
        ]));
        let mut world = fake(existing.clone(), None);
        world.as_of = Some(s("2026-09-20"));
        let new = V::Map(Map::from([(
            "v".into(),
            V::Integer(crate::value::Integer::new("2").unwrap()),
        )]));
        assert!(
            may_supersede(
                &world,
                "p.input",
                &existing,
                &new,
                Some("2026-09-20"),
                None,
                false
            )
            .unwrap()
            .allowed
        );
        assert!(
            may_supersede(&world, "p.input", &existing, &new, None, None, false)
                .unwrap()
                .allowed
        );
        let decision = may_supersede(
            &world,
            "p.input",
            &existing,
            &new,
            Some("2026-09-19"),
            None,
            false,
        )
        .unwrap();
        assert!(!decision.allowed);
        assert!(decision.reason.contains("same day"));
    }
}

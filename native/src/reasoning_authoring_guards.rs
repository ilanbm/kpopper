//! Ordinary authored admission: identity, references, citations and replacement.
use crate::{
    Error, Result,
    history_contract::*,
    history_view::truth,
    reasoning_authoring::{World, blocked_text, py, reopened_text},
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
fn judgment(world: &World<'_>, id: &str) -> bool {
    world
        .raw
        .get(id)
        .and_then(|v| map(v).ok())
        .is_some_and(|m| m.contains_key(text(&world.fields["deps"]).unwrap()))
}
fn source(body: &V) -> bool {
    map(body).is_ok_and(|m| {
        !["v", "rule", "quoted"].iter().any(|k| m.contains_key(*k))
            && ["asked", "url", "file", "read"]
                .iter()
                .any(|k| m.contains_key(*k))
    })
}
fn asked(world: &World<'_>, id: &str) -> bool {
    world
        .raw
        .get(id)
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("asked"))
        .is_some_and(truth)
}
fn arrangement(world: &World<'_>, body: &V) -> bool {
    let dep = text(&world.fields["deps"]).unwrap();
    if !shaped(body, dep) {
        return false;
    }
    let m = map(body).unwrap();
    let deps = names(&m[dep]);
    let pred = get(m, text(&world.fields["predicate"]).unwrap());
    let rs = if let V::Text(v) = pred {
        DOTTED.find_iter(v).map(|m| m.as_str().into()).collect()
    } else {
        L::legacy_references(pred)
    };
    deps.iter().any(|d| asked(world, d))
        && (deps.iter().any(|d| builtin(d)) || rs.iter().any(|r| builtin(r)))
}
fn layers(world: &World<'_>) -> Result<BTreeMap<String, BTreeMap<String, V>>> {
    let mut result = BTreeMap::new();
    for (name, h) in map(&map(world.snapshot.data())?["hypotheses"])? {
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
    world: &World<'_>,
    id: &str,
    body: &V,
    explicit: Option<&str>,
) -> Result<String> {
    let cols = F::collections(&world.document)?;
    if let Some(e) = explicit.filter(|s| !s.is_empty()) {
        require(
            cols.contains_key(e) || map(&world.document)?.contains_key(e),
            &format!("no collection {e} in this record"),
        )?;
        return Ok(e.into());
    }
    let deps = text(&world.fields["deps"])?;
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
    Ok(most(&|id, b| {
        !judgment(world, id)
            && map(b).is_ok_and(|m| ["v", "rule", "quoted"].iter().any(|k| m.contains_key(*k)))
    })
    .filter(|(_, n)| *n > 0)
    .map(|(c, _)| c.clone())
    .unwrap_or_else(|| "known".into()))
}
fn scalar_type(
    world: &World<'_>,
    tree: &J,
    visiting: &mut BTreeSet<String>,
) -> Result<Option<&'static str>> {
    if let Some(id) = tree.get("ref").and_then(J::as_str) {
        if visiting.contains(id) || !world.raw.contains_key(id) {
            return Ok(None);
        }
        visiting.insert(id.into());
        let body = &world.raw[id];
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
    world: &World<'_>,
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
    let predicate = body.contains_key(text(&world.fields["deps"])?);
    let field = if predicate {
        text(&world.fields["predicate"])?
    } else {
        "rule"
    };
    let Some(V::Text(source)) = body.get(field) else {
        return Ok((action.clone(), vec![]));
    };
    if previous
        .unwrap_or(&world.raw)
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
    if !predicate && refs.iter().any(|r| !world.raw.contains_key(r) && r != id) {
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
fn read_on(body: &V, world: &World<'_>) -> Option<String> {
    let m = map(body).ok()?;
    for k in ["of", "read"] {
        if let Some(d) = m.get(k).and_then(day) {
            return Some(d);
        }
    }
    let src = m
        .get("from")
        .and_then(|v| text(v).ok())
        .and_then(|s| world.raw.get(s))
        .and_then(|v| map(v).ok())?;
    for k in ["read", "of"] {
        if let Some(d) = src.get(k).and_then(day) {
            return Some(d);
        }
    }
    None
}
fn clock_day(world: &World<'_>, a: &Map) -> Result<String> {
    a.get("as_of")
        .and_then(day)
        .or_else(|| {
            map(world.snapshot.data())
                .ok()
                .and_then(|m| m.get("as_of"))
                .and_then(day)
        })
        .ok_or_else(|| Error("write requires an explicitly captured day".into()))
}
struct Disagreement {
    kind: &'static str,
    old: V,
    new: V,
    may: bool,
    why: String,
    when: Option<String>,
}
fn disagreement(world: &mut World<'_>, a: &Map) -> Result<Option<Disagreement>> {
    let id = text(field(a, "id")?)?;
    let Some(body) = world.raw.get(id).cloned() else {
        return Ok(None);
    };
    let kind = text(field(a, "kind")?)?;
    let new_body = get(a, "body");
    if judgment(world, id) {
        if kind != "add" || !shaped(new_body, text(&world.fields["deps"])?) {
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
        let same = crate::reasoning_authoring::same(old, new)?;
        let kind = if same {
            let snapshot = text(&world.fields["snapshot"])?;
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
        let pred = map(&body)?
            .get(text(&world.fields["predicate"])?)
            .unwrap_or(&V::Null);
        let may = world.predicate(pred, None)? == Some(true);
        let why = if may {
            format!("its wrong_if holds ({})", short(pred, 60))
        } else {
            "the standing judgment holds, and its wrong_if has not fired".into()
        };
        return Ok(Some(Disagreement {
            kind,
            old: old.clone(),
            new: new.clone(),
            may,
            why,
            when: None,
        }));
    }
    if !matches!(body, V::Map(_)) {
        return Ok(None);
    }
    let old = world.value(id)?;
    if old == V::Null && world.result(id)?["status"] != "ok" {
        return Ok(None);
    }
    let new = if kind == "set" {
        get(a, "value")
    } else {
        let Ok(m) = map(new_body) else {
            return Ok(None);
        };
        let Some(v) = m.get("v").or_else(|| m.get("quoted")) else {
            return Ok(None);
        };
        v
    };
    if world.same_value(id, new)? {
        return Ok(None);
    }
    let when = read_on(&body, world);
    let stamp = clock_day(world, a)?;
    let may = when.as_ref().is_none_or(|w| stamp > *w);
    let why = match &when {
        None => "nothing dates the reading the base holds".into(),
        Some(w) if stamp > *w => format!("a reading from {stamp} that is newer than the base's"),
        Some(w) if stamp == *w => "a reading of the same day".into(),
        _ => format!("a reading from {stamp} that is older than the base's"),
    };
    Ok(Some(Disagreement {
        kind: "value",
        old,
        new: new.clone(),
        may,
        why,
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
fn hypothesis(world: &World<'_>, id: &str, claim: &V) -> Result<String> {
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
    let hypotheses = map(&map(world.snapshot.data())?["hypotheses"])?;
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
pub(crate) fn validate(world: &mut World<'_>, action: &V) -> Result<Vec<String>> {
    let a = map(action)?;
    let id = text(field(a, "id")?)?;
    let kind = text(field(a, "kind")?)?;
    let empty = Map::new();
    let b = map(get(a, "body")).unwrap_or(&empty);
    let body = get(a, "body");
    let dep = text(&world.fields["deps"])?.to_owned();
    let pred = text(&world.fields["predicate"])?.to_owned();
    let seen = text(&world.fields["snapshot"])?.to_owned();
    let layers = layers(world)?;
    let aimed = a.get("hypothesis").filter(|v| truth(v));
    let known = world.raw.contains_key(id);
    let is_jud = judgment(world, id);
    let mut out = vec![];
    let diff = if (kind == "add" || kind == "set") && known {
        disagreement(world, a)?
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
        } else if let Some(V::Map(old)) = world.raw.get(id)
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
            || map(&world.document)?
                .get("meta")
                .and_then(|v| map(v).ok())
                .is_some_and(|m| m.contains_key(id))
        {
            if let Some(h) = aimed {
                out.push(format!("{id} is already in hypothesis {} - set changes its value there, review its snapshot",py(h)))
            } else if diff.is_none() {
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
                    if !world.raw.contains_key(d)
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
                    if (matches!(p, V::Map(_)) || world.raw.contains_key(&r) || builtin(&r))
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
                    if !world.raw.contains_key(&r) && !builtin(&r) {
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
            let sb = world.raw.get(src).and_then(|v| map(v).ok());
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
            } else if let Some(V::Map(m)) = world.raw.get(id) {
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
            .is_some_and(|c| world.raw.contains_key(&c[1]))
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
                            .any(|m| world.raw.contains_key(m.as_str()))
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
        .raw
        .keys()
        .chain(layers.values().flat_map(|m| m.keys()))
        .cloned()
        .collect::<BTreeSet<_>>();
    if !known && !live.contains(id) && dep != "also" {
        for m in std::iter::once(&world.raw).chain(layers.values()) {
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
                if world.raw.contains_key(&d) || builtin(&d) {
                    continue;
                }
                let held = held_by(&layers, &d);
                if !held.is_empty() {
                    out.push(format!("{how} {d}, which only hypothes{} {} hold{} - what rests on a hypothesis goes into it: {}",if held.len()==1{"is"}else{"es"},held.join(", "),if held.len()==1{"s"}else{""},command(a,&held[0])));
                }
            }
        }
    }
    if kind == "add" && !arrangement(world, body) && b.contains_key("replaced") {
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
        && !arrangement(world, &world.raw[id])
        && diff.as_ref().is_some_and(|d| d.kind != "value" && d.may)
    {
        let gone = names(&map(&world.raw[id])?[&dep])
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
    if a.get("section").is_some_and(truth)
        || arrangement(world, body)
        || world
            .raw
            .get(id)
            .is_some_and(|b| judgment(world, id) && arrangement(world, b))
    {
        out.push("unsupported_core_builtin: page arrangement/section review requires its own declared inputs".into())
    }
    Ok(out)
}

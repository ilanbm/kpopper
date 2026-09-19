//! Merge current identity bodies without rewriting original reading evidence.
use crate::{
    Result,
    history_contract::*,
    history_view::truth,
    history_yaml::SourceValue as S,
    reasoning_authoring::{self as A, World},
    reasoning_authoring_guards::read_on,
    reasoning_language as L, require,
    source_text::python_str,
    value::TypedValue as V,
};
use std::sync::LazyLock;
const READING: &[&str] = &[
    "v", "quoted", "rule", "unit", "from", "at", "of", "read", "url", "file",
];
static EXPR: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[<>=!+\-*/()]|\b(?:or|and|not)\b").unwrap());
static ID: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+").unwrap());
fn get<'a>(body: &'a S, key: &str) -> &'a S {
    body.get(key).unwrap_or(&S::Scalar(V::Null))
}
fn val(body: &S, key: &str) -> V {
    get(body, key).typed()
}
fn norm(v: &S) -> String {
    python_str(v)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
pub(crate) fn legacy_same(a: &S, b: &S) -> Result<bool> {
    if norm(a) == norm(b) {
        return Ok(true);
    }
    if matches!(a, S::Map(_) | S::List(_)) || matches!(b, S::Map(_) | S::List(_)) {
        return Ok(false);
    }
    let (a, b) = (A::claim_key(&a.typed())?, A::claim_key(&b.typed())?);
    Ok(a == b && !a.to_lowercase().contains("nan"))
}
pub(crate) fn ids(value: &V) -> Vec<String> {
    if !truth(value) {
        return vec![];
    }
    A::py(value)
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
fn shaped(body: &S, fields: &Map) -> bool {
    body.get(text(&fields["deps"]).unwrap()).is_some_and(|v| matches!(v,S::List(a) if !a.is_empty() && a.iter().all(|v| matches!(v,S::Scalar(V::Text(_))))))
}
fn verdict(body: &S) -> &S {
    if val(body, "verdict") != V::Null {
        get(body, "verdict")
    } else {
        get(body, "title")
    }
}
fn claim(body: &S) -> &S {
    let v = if val(body, "v") == V::Null {
        get(body, "quoted")
    } else {
        get(body, "v")
    };
    let formula = |s: &S| matches!(s,S::Scalar(V::Text(t)) if EXPR.is_match(t)&&ID.is_match(t));
    if v.typed() != V::Null && !formula(v) {
        return v;
    }
    let rule = get(body, "rule");
    if matches!(rule, S::Map(_)) || matches!(rule,S::Scalar(V::Text(t)) if !t.trim().is_empty()) {
        return rule;
    }
    if formula(get(body, "v")) {
        get(body, "v")
    } else {
        &S::Scalar(V::Null)
    }
}
fn set(body: &mut Vec<(String, S)>, key: &str, value: S) {
    if let Some((_, v)) = body.iter_mut().find(|(k, _)| k == key) {
        *v = value;
    } else {
        body.push((key.into(), value));
    }
}
pub(crate) fn merge(
    survivor: &str,
    retired: &str,
    kept: &S,
    removed: &S,
    world: &mut World<'_>,
) -> Result<(S, bool)> {
    let (S::Map(k), S::Map(r)) = (kept, removed) else {
        return Err(error("identity_requires_mapping_body"));
    };
    require(
        k.is_empty() || shaped(kept, world.fields()) == shaped(removed, world.fields()),
        "identity_mixed_kind",
    )?;
    let (cs, cr) = (claim(kept), claim(removed));
    if !k.is_empty() && !matches!(cs, S::Map(_)) && !matches!(cr, S::Map(_)) {
        require(
            !legacy_same(cs, cr)? || A::same(&cs.typed(), &cr.typed())?,
            "identity_typed_value_conflict",
        )?;
    }
    let mut body;
    if shaped(kept, world.fields()) {
        let (vs, vr) = (verdict(kept), verdict(removed));
        if vs.typed() != V::Null && vr.typed() != V::Null && !legacy_same(vs, vr)? {
            let pred = val(kept, text(&world.fields()["predicate"])?);
            require(
                world.standing_predicate(survivor, &pred)? == Some(true),
                "identity_conflicting_verdicts",
            )?;
            body = r.clone();
        } else {
            body = k.clone();
        }
    } else {
        let rules = [kept, removed].iter().all(|b| {
            val(b, "rule") != V::Null && val(b, "v") == V::Null && val(b, "quoted") == V::Null
        });
        let equal = if rules && matches!(cs, S::Map(_)) && matches!(cr, S::Map(_)) {
            match (
                L::legacy_expression(&cs.typed(), false),
                L::legacy_expression(&cr.typed(), false),
            ) {
                (Ok(a), Ok(b)) => a == b,
                _ => cs.typed() == cr.typed(),
            }
        } else {
            legacy_same(cs, cr)?
        };
        let (ds, dr) = (
            read_on(&kept.typed(), world),
            read_on(&removed.typed(), world),
        );
        let mut take = k.is_empty();
        if !take && cr.typed() != V::Null && (cs.typed() == V::Null || !equal) {
            if dr
                .as_ref()
                .is_some_and(|r| ds.as_ref().is_none_or(|s| r >= s))
            {
                // Core inference treats any dependency field as a judgment,
                // including maps/empty lists that do not satisfy shaped(). Such
                // a survivor still goes through the judgment supersession door.
                let can_replace = if world.raw().get(survivor).is_some_and(|b| {
                    map(b).is_ok_and(|m| m.contains_key(text(&world.fields()["deps"]).unwrap()))
                }) {
                    shaped(cr, world.fields())
                        && world.standing_predicate(
                            survivor,
                            &val(kept, text(&world.fields()["predicate"])?),
                        )? == Some(true)
                } else {
                    ds.as_ref().is_none_or(|s| dr.as_ref().unwrap() > s)
                };
                require(can_replace, "identity_conflicting_readings")?;
                take = true;
            } else {
                require(
                    ds.is_some() || dr.is_some(),
                    "identity_conflicting_readings",
                )?;
            }
        } else if cs.typed() == V::Null
            && cr.typed() == V::Null
            && dr
                .as_ref()
                .is_some_and(|r| ds.as_ref().is_none_or(|s| r > s))
        {
            take = true;
        }
        body = if k.is_empty() {
            r.clone()
        } else if take {
            let mut body = vec![];
            for (key, value) in k {
                if READING.contains(&key.as_str()) && key != "unit" {
                    if let Some(v) = removed.get(key) {
                        body.push((key.clone(), v.clone()));
                    }
                } else {
                    body.push((key.clone(), value.clone()));
                }
            }
            for key in READING {
                if !body.iter().any(|(k, _)| k == key)
                    && let Some(v) = removed.get(key)
                {
                    body.push(((*key).into(), v.clone()));
                }
            }
            body
        } else {
            k.clone()
        };
        if !k.is_empty() {
            for (key, value) in r {
                if !body.iter().any(|(k, _)| k == key)
                    && !READING.contains(&key.as_str())
                    && !["also", "distinct_from"].contains(&key.as_str())
                {
                    body.push((key.clone(), value.clone()));
                }
            }
        }
    }
    if !truth(&val(&S::Map(body.clone()), "name")) {
        let name = A::named(&removed.typed());
        if !name.is_empty() {
            set(&mut body, "name", S::Scalar(V::Text(name)));
        }
    }
    body.retain(|(k, _)| !["also", "distinct_from"].contains(&k.as_str()));
    let mut aliases = vec![];
    let mut distinct = vec![];
    for source in [kept, removed] {
        let al = val(source, "also");
        let values = match al {
            V::Text(_) => vec![al],
            V::List(a) => a,
            _ => vec![],
        };
        for v in values {
            if v != V::Text(survivor.into())
                && !aliases
                    .iter()
                    .any(|old| crate::source_clock::python_equal(old, &v))
            {
                aliases.push(v);
            }
        }
        for v in ids(&val(source, "distinct_from")) {
            if v != survivor && v != retired && !distinct.contains(&v) {
                distinct.push(v);
            }
        }
    }
    let retired = V::Text(retired.into());
    if !aliases.contains(&retired) {
        aliases.push(retired);
    }
    set(&mut body, "also", S::from_typed(&V::List(aliases)));
    if !distinct.is_empty() {
        set(
            &mut body,
            "distinct_from",
            S::Scalar(V::Text(distinct.join(", "))),
        );
    }
    let body = S::Map(body);
    let taking = if shaped(kept, world.fields()) {
        !legacy_same(verdict(&body), verdict(kept))?
    } else {
        let readings = |s: &S| {
            V::Map(
                READING
                    .iter()
                    .filter(|k| **k != "unit")
                    .filter_map(|k| s.get(k).map(|v| ((*k).into(), v.typed())))
                    .collect(),
            )
        };
        k.is_empty() || readings(&body).digest()? != readings(kept).digest()?
    };
    Ok((body, taking))
}

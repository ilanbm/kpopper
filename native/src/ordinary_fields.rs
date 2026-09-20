//! Ordinary field roles over the source value domain. Canonical snapshot fields remain separate.
use crate::{
    Result, history_contract::error, ordinary_value::Value as V, ordinary_value::*, require,
    value::Integer,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};
static ID: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+").unwrap());
static EXPR: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"[<>=!+\-*/()]|(?:^|[^\p{L}\p{N}_])(?:or|and|not)(?:$|[^\p{L}\p{N}_])")
        .unwrap()
});
pub(crate) const BUILTINS: &[&str] = &[
    "graph.entries",
    "graph.judgments",
    "graph.open",
    "graph.flagged",
    "graph.blocked",
    "graph.broken",
    "graph.unchecked",
    "graph.moved",
    "graph.falsified",
    "graph.no_predicate",
    "graph.hypotheses",
    "graph.contested",
    "graph.prior_reversal_rate",
    "page.spill",
    "page.unserved",
    "page.drift",
    "page.recent_unserved",
    "page.covered",
];
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn one() -> V {
    V::Integer(Integer::new("1").unwrap())
}
pub(crate) fn collections(document: &V) -> Result<BTreeMap<String, Map>> {
    let doc = map(document)?;
    let core = doc
        .get("meta")
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("reasoning"))
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("profile"))
        .is_some_and(|v| string_is(v, "core/v1"));
    Ok(doc
        .iter()
        .filter_map(|(k, v)| {
            if ["meta", "schema", "record", "also"].contains(&k.as_str()) {
                return None;
            }
            if let V::Map(m) = v
                && !m.is_empty()
                && m.values().all(|v| core || !matches!(v, V::List(_)))
            {
                Some((k.clone(), m.clone()))
            } else {
                None
            }
        })
        .collect())
}
fn choose(
    schema: &Map,
    role: &str,
    candidates: &BTreeMap<String, usize>,
) -> Result<Option<String>> {
    if let Some(value) = schema.get(role).filter(|v| truth(v)) {
        return text(value)
            .map(|s| Some(s.into()))
            .map_err(|_| error("invalid_snapshot"));
    }
    let max = candidates.values().copied().max().unwrap_or(0);
    let found = candidates
        .iter()
        .filter(|(_, v)| **v == max)
        .map(|(k, _)| k.clone())
        .collect::<Vec<_>>();
    require(found.len() <= 1, "invalid_snapshot")?;
    Ok(found.into_iter().next())
}
pub fn snapshot_fields(document: &V) -> Result<Map> {
    inferred_fields(document, false)
}
/// Comparison roles follow the ordinary reader, including an explicit unreadable
/// result when no unique dependency role can be established.
pub(crate) fn semantic_roles(document: &V) -> Result<Option<(BTreeSet<String>, Map)>> {
    let fields = match inferred_fields(document, true) {
        Ok(fields) => fields,
        Err(e) if e.0 == "invalid_snapshot" => return Ok(None),
        Err(e) => return Err(e),
    };
    if fields.is_empty() {
        return Ok(Some((BTreeSet::new(), fields)));
    }
    let dependency = text(&fields["deps"])?;
    let judgments = collections(document)?
        .values()
        .flat_map(|m| m.iter())
        .filter_map(|(id, body)| {
            map(body)
                .ok()
                .filter(|m| m.contains_key(dependency))
                .map(|_| id.clone())
        })
        .collect();
    Ok(Some((judgments, fields)))
}

fn inferred_fields(document: &V, semantic: bool) -> Result<Map> {
    let doc = map(document)?;
    let empty = V::Map(Map::new());
    let schema = doc.get("schema").filter(|v| truth(v)).unwrap_or(&empty);
    let schema = map(schema).map_err(|_| {
        error(if semantic {
            "invalid_comparison_roles"
        } else {
            "invalid_snapshot"
        })
    })?;
    let mut roles = Map::from([
        ("deps".into(), s("rests_on")),
        ("snapshot".into(), s("seen")),
        ("predicate".into(), s("wrong_if")),
    ]);
    for role in ["deps", "snapshot", "predicate"] {
        if let Some(value) = schema.get(role) {
            if semantic {
                if truth(value) {
                    roles.insert(role.into(), value.clone());
                }
                continue;
            }
            require(text(value).is_ok_and(|s| !s.is_empty()), "invalid_snapshot")?;
            roles.insert(role.into(), value.clone());
        }
    }
    let collections = collections(document)?;
    let mut ids = collections
        .values()
        .flat_map(|m| m.keys().cloned())
        .collect::<BTreeSet<_>>();
    // Only candidates in direct strings/lists/mapping keys can vote for a field.
    // Their built-in references establish the same IDs needed by those votes.
    for body in collections.values().flat_map(|m| m.values()) {
        let V::Map(body) = body else {
            continue;
        };
        for value in body.values() {
            let mentions = match value {
                V::Text(v) => vec![v.as_str()],
                V::List(v) => v.iter().filter_map(|v| text(v).ok()).collect(),
                V::Map(v) => v.keys().map(String::as_str).collect(),
                _ => Vec::new(),
            };
            for value in mentions {
                for name in ID.find_iter(value) {
                    if BUILTINS.contains(&name.as_str()) {
                        ids.insert(name.as_str().into());
                    }
                }
            }
        }
    }
    let mut deps = BTreeMap::new();
    let mut unresolved = BTreeSet::new();
    let mut present = BTreeSet::new();
    for (name, body) in collections.values().flat_map(|m| m.iter()) {
        let V::Map(body) = body else {
            continue;
        };
        for (field, value) in body {
            present.insert(field.clone());
            if field == "also"
                || field == "refutes"
                    && name.starts_with("hyp.")
                    && body.get("v").is_some_and(|v| string_is(v, "refuted"))
            {
                continue;
            }
            if let V::List(a) = value
                && !a.is_empty()
                && a.iter().all(|v| text(v).is_ok())
            {
                if a.iter().all(|v| text(v).is_ok_and(|s| ids.contains(s))) {
                    *deps.entry(field.clone()).or_insert(0) += 1;
                } else {
                    unresolved.insert(field.clone());
                }
            }
        }
    }
    if schema
        .get("deps")
        .is_some_and(|v| truth(v) && text(v).is_ok_and(|v| !present.contains(v)))
    {
        if semantic {
            let mut judgment_fields = [
                "rests_on",
                "wrong_if",
                "seen",
                "verdict",
                "reopened_by",
                "blocked_on",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
            judgment_fields.extend(
                ["deps", "snapshot", "predicate"]
                    .iter()
                    .filter_map(|k| schema.get(*k))
                    .filter_map(|v| text(v).ok())
                    .map(str::to_owned),
            );
            let core = doc
                .get("meta")
                .and_then(|v| map(v).ok())
                .and_then(|m| m.get("reasoning"))
                .and_then(|v| map(v).ok())
                .is_some_and(|m| m.get("profile").is_some_and(|v| string_is(v, "core/v1")));
            let shaped = collections
                .values()
                .flat_map(|m| m.values())
                .filter_map(|v| map(v).ok())
                .any(|m| {
                    let matched = judgment_fields
                        .iter()
                        .filter(|k| m.contains_key(*k))
                        .map(String::as_str)
                        .collect::<Vec<_>>();
                    !matched.is_empty()
                        && !(matched == ["blocked_on"]
                            && core
                            && m.get("rule").is_some_and(|v| map(v).is_ok()))
                });
            let portable = !collections.is_empty()
                && collections
                    .keys()
                    .all(|k| ["known", "sources", "open", "questions"].contains(&k.as_str()))
                && ["deps", "snapshot", "predicate"]
                    .iter()
                    .all(|k| schema.get(*k).is_some_and(truth))
                && deps.is_empty()
                && unresolved
                    .iter()
                    .all(|k| ["labels", "tags", "v", "quoted"].contains(&k.as_str()))
                && !shaped;
            if !portable {
                return Ok(Map::new());
            }
        }
        return Ok(roles);
    }
    let Some(dep) = choose(schema, "deps", &deps)? else {
        if semantic {
            let newborn = !doc.is_empty() && doc.keys().all(|k| k == "meta");
            let core = doc
                .get("meta")
                .and_then(|v| map(v).ok())
                .and_then(|m| m.get("reasoning"))
                .and_then(|v| map(v).ok())
                .is_some_and(|m| m.get("profile").is_some_and(|v| string_is(v, "core/v1")));
            let shaped = collections
                .values()
                .flat_map(|m| m.values())
                .filter_map(|v| map(v).ok())
                .any(|m| {
                    let matched = [
                        "rests_on",
                        "wrong_if",
                        "seen",
                        "verdict",
                        "reopened_by",
                        "blocked_on",
                    ]
                    .into_iter()
                    .filter(|k| m.contains_key(*k))
                    .collect::<Vec<_>>();
                    !matched.is_empty()
                        && !(matched == ["blocked_on"]
                            && core
                            && m.get("rule").is_some_and(|v| map(v).is_ok()))
                });
            require(
                (newborn
                    || !collections.is_empty()
                        && collections.keys().all(|k| {
                            ["known", "sources", "open", "questions"].contains(&k.as_str())
                        }))
                    && unresolved.is_empty()
                    && !shaped,
                "invalid_snapshot",
            )?;
        }
        return Ok(roles);
    };
    roles.insert("deps".into(), s(&dep));
    let (mut snapshots, mut predicates) = (BTreeMap::new(), BTreeMap::new());
    for body in collections.values().flat_map(|m| m.values()) {
        let V::Map(body) = body else {
            continue;
        };
        if !body.contains_key(&dep) {
            continue;
        }
        let dep_value = &body[&dep];
        require(
            !truth(dep_value) || matches!(dep_value, V::Map(_) | V::List(_) | V::Text(_)),
            if semantic {
                "invalid_comparison_roles"
            } else {
                "invalid_snapshot"
            },
        )?;
        for (field, value) in body {
            if ["reopened_by", "request", "replaced", "also"].contains(&field.as_str()) {
                continue;
            }
            if let V::Map(m) = value {
                if !m.is_empty() && m.keys().all(|k| ids.contains(k)) {
                    *snapshots.entry(field.clone()).or_insert(0) += 1;
                } else if m.contains_key("op") || m.contains_key("expr") || field == "wrong_if" {
                    *predicates.entry(field.clone()).or_insert(0) += 1;
                }
            } else if let V::Text(value) = value
                && !value.is_empty()
                && !value.contains("{{")
            {
                let named = ID
                    .find_iter(value)
                    .map(|m| m.as_str())
                    .filter(|s| ids.contains(*s))
                    .collect::<Vec<_>>();
                let trimmed = value.trim_matches(|c: char| {
                    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
                });
                if !named.is_empty() && !named.contains(&trimmed) && EXPR.is_match(value) {
                    *predicates.entry(field.clone()).or_insert(0) += 1;
                }
            }
        }
    }
    for (role, candidates) in [("snapshot", snapshots), ("predicate", predicates)] {
        if semantic && let Some(value) = schema.get(role).filter(|v| truth(v)) {
            roles.insert(role.into(), value.clone());
        } else if let Some(name) = choose(schema, role, &candidates)? {
            roles.insert(role.into(), s(&name));
        } else if semantic {
            roles.insert(role.into(), V::Null);
        }
    }
    Ok(roles)
}
pub fn capabilities(document: &V, profile: Option<&str>) -> Result<V> {
    let doc = map(document)?;
    let empty = Map::new();
    let meta = doc.get("meta").and_then(|v| map(v).ok()).unwrap_or(&empty);
    let mut result = if let Some(value) = meta.get("reasoning") {
        let d = map(value).map_err(|_| error("invalid_capability"))?;
        require(
            d.len() == 3
                && ["version", "profile", "requires"]
                    .iter()
                    .all(|k| d.contains_key(*k)),
            "invalid_capability",
        )?;
        require(
            matches!(d["version"], V::Integer(_)) && matches!(d["profile"], V::Text(_)),
            "invalid_capability",
        )?;
        let V::List(req) = &d["requires"] else {
            return Err(error("invalid_capability"));
        };
        let names = req
            .iter()
            .map(|v| text(v).map(str::to_owned))
            .collect::<Result<Vec<_>>>()
            .map_err(|_| error("invalid_capability"))?;
        require(names.windows(2).all(|w| w[0] < w[1]), "invalid_capability")?;
        require(
            (is_int(&d["version"], "1") || is_int(&d["version"], "2"))
                && string_is(&d["profile"], "core/v1"),
            "unsupported_capability",
        )?;
        require(
            names
                .iter()
                .all(|s| ["arithmetic/v1", "composition/v1", "query/v1"].contains(&s.as_str())),
            "unsupported_capability",
        )?;
        require(
            names.iter().any(|s| s == "arithmetic/v1"),
            "invalid_capability",
        )?;
        d.clone()
    } else {
        Map::from([
            ("version".into(), one()),
            ("profile".into(), s("ordinary-reader/v1")),
            ("requires".into(), V::List(vec![])),
        ])
    };
    let numeric_two =
        |v: &V| crate::ordinary_value::python_equal(v, &V::Integer(Integer::new("2").unwrap()));
    let members = doc
        .iter()
        .filter(|(k, v)| {
            !["meta", "schema", "record", "also"].contains(&k.as_str()) && matches!(v, V::Map(_))
        })
        .flat_map(|(_, v)| map(v).unwrap().values())
        .collect::<Vec<_>>();
    let possible = members
        .iter()
        .filter_map(|v| map(v).ok())
        .flat_map(|m| m.values())
        .filter_map(|v| map(v).ok())
        .flat_map(|m| m.values())
        .filter_map(|v| map(v).ok())
        .filter_map(|m| m.get("computed"))
        .filter_map(|v| map(v).ok())
        .filter_map(|m| m.get("version"))
        .any(numeric_two);
    if !is_int(&result["version"], "2") && possible {
        let role = snapshot_fields(document)?;
        let field = text(&role["snapshot"])?;
        for body in members {
            if let Some(seen) = map(body)
                .ok()
                .and_then(|m| m.get(field))
                .and_then(|v| map(v).ok())
            {
                for old in seen.values() {
                    if map(old)
                        .ok()
                        .and_then(|m| m.get("computed"))
                        .and_then(|v| map(v).ok())
                        .and_then(|m| m.get("version"))
                        .is_some_and(numeric_two)
                    {
                        return Err(error("invalid_capability"));
                    }
                }
            }
        }
    }
    if let Some(profile) = profile {
        require(
            ["ordinary-reader/v1", "checked-reader/v1", "core/v1"].contains(&profile),
            "unsupported_capability",
        )?;
        require(
            !string_is(&result["profile"], "core/v1") || profile == "core/v1",
            "unsupported_capability",
        )?;
        if !string_is(&result["profile"], profile) {
            result.insert("declared_profile".into(), result["profile"].clone());
            result.insert("profile".into(), s(profile));
            result.insert(
                "requires".into(),
                V::List(if profile == "core/v1" {
                    vec![s("arithmetic/v1")]
                } else {
                    vec![]
                }),
            );
            result.insert(
                "experimental_override".into(),
                V::Bool(profile == "core/v1"),
            );
        }
    }
    Ok(V::Map(result))
}

//! Ordinary field roles over the source value domain. Canonical snapshot fields remain separate.
use crate::{
    Error, Result, history_contract::error, ordinary_value::Value as V, ordinary_value::*, require,
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
fn source_only_collections(collections: &BTreeMap<String, Map>) -> bool {
    collections.iter().all(|(name, members)| {
        ["known", "sources", "open", "questions"].contains(&name.as_str())
            || members.values().all(|body| {
                map(body).is_ok_and(|body| {
                    !["v", "quoted", "rule"]
                        .iter()
                        .any(|key| body.contains_key(key))
                        && ["asked", "file", "url", "read"]
                            .iter()
                            .any(|key| body.get(key).is_some_and(truth))
                })
            })
    })
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

const NO_DEPENDENCY_FIELD: &str = "no dependency field found: ";
const UNSEEN_DEPENDENCY_FIELD: &str = " for 'deps', and nothing this reader can see carries it: ";

/// Why a reader cannot take a record's field roles, in the Python reader's words: a
/// dependency field the schema names and no entry carries, or no field that reads as a
/// dependency list. Any other cause keeps its code.
pub(crate) fn unreadable(document: &V) -> Error {
    let why = || -> Result<Option<String>> {
        let doc = map(document)?;
        let lists = dependency_lists(doc, &collections(document)?);
        let declared = doc
            .get("schema")
            .and_then(|v| map(v).ok())
            .and_then(|schema| schema.get("deps"))
            .filter(|v| truth(v));
        Ok(match declared {
            Some(V::Text(dep)) if !lists.present.contains(dep) => {
                Some(unseen_dependency_field(dep, &lists.present))
            }
            None if lists.votes.is_empty() => Some(no_dependency_field(&lists.unresolved)),
            _ => None,
        })
    };
    match why() {
        Ok(Some(why)) => Error(why),
        _ => error("ordinary_fields_unreadable"),
    }
}

/// Whether a failure is `unreadable`'s account rather than a code.
pub(crate) fn explains_unreadable(failure: &Error) -> bool {
    failure.0.starts_with(NO_DEPENDENCY_FIELD)
        || failure.0.starts_with("schema names '") && failure.0.contains(UNSEEN_DEPENDENCY_FIELD)
}

fn unseen_dependency_field(dep: &str, present: &BTreeSet<String>) -> String {
    let seen = present
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let shown = if seen.chars().count() < 300 {
        seen.clone()
    } else {
        seen.chars().take(300).collect::<String>() + " ..."
    };
    format!(
        "schema names '{dep}'{UNSEEN_DEPENDENCY_FIELD}no judgment would be found, and the record would pass by having nothing left to check.{}",
        if seen.is_empty() {
            String::new()
        } else {
            format!("\nFields it can see: {shown}")
        }
    )
}

/// A record whose references are broken looks like one that declares nothing: no list
/// names only entries, so no field votes for the role. Name the fields that did list
/// names, and what they named that is not an entry, and leave the choice to a person.
fn no_dependency_field(unresolved: &BTreeMap<String, Vec<(String, Vec<String>)>>) -> String {
    if unresolved.is_empty() {
        return format!(
            "{NO_DEPENDENCY_FIELD}nothing declares what it rests on, so there is no graph to walk"
        );
    }
    let clip = |text: String| {
        if text.chars().count() < 90 {
            text
        } else {
            text.chars().take(90).collect::<String>() + " ..."
        }
    };
    let mut ranked = unresolved.iter().collect::<Vec<_>>();
    ranked.sort_by_key(|(_, holders)| std::cmp::Reverse(holders.len()));
    let mut lines = ranked
        .iter()
        .take(3)
        .map(|(field, holders)| {
            let (id, missing) = &holders[0];
            let names = missing[..missing.len().min(3)].join(", ")
                + if missing.len() > 3 { " ..." } else { "" };
            let more = if holders.len() > 1 {
                format!(", and {} more", holders.len() - 1)
            } else {
                String::new()
            };
            format!("  {field}: {} (in {id}{more})", clip(names))
        })
        .collect::<Vec<_>>();
    if ranked.len() > 3 {
        let rest = ranked[3..]
            .iter()
            .map(|(field, _)| field.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!(
            "  ... and {} more: {}",
            ranked.len() - 3,
            clip(rest)
        ));
    }
    format!(
        "{NO_DEPENDENCY_FIELD}no field lists names that are all entries in this record, so there is no graph to walk.\nThese list names that are not entries:\n{}\n\nEither those names are wrong, or one of these is a dependency field this reader cannot see by shape - and it does not guess between them. Fix the names, or say which:\n\nschema:\n  deps: <field name>",
        lines.join("\n")
    )
}

/// How the entries' lists of names read as a dependency field. A list whose names are
/// all entries votes for its field; one that names anything else votes for nothing and
/// is kept, holder by holder in the record's own order, with the names that were not
/// entries.
struct DependencyLists {
    ids: BTreeSet<String>,
    votes: BTreeMap<String, usize>,
    unresolved: BTreeMap<String, Vec<(String, Vec<String>)>>,
    present: BTreeSet<String>,
}
fn dependency_lists(doc: &Map, collections: &BTreeMap<String, Map>) -> DependencyLists {
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
    let mut votes = BTreeMap::new();
    let mut unresolved = BTreeMap::<String, Vec<_>>::new();
    let mut present = BTreeSet::new();
    let in_order = doc.keys().filter_map(|name| collections.get(name));
    for (name, body) in in_order.flat_map(|m| m.iter()) {
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
                let missing = a
                    .iter()
                    .filter_map(|v| text(v).ok())
                    .filter(|s| !ids.contains(*s))
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                if missing.is_empty() {
                    *votes.entry(field.clone()).or_insert(0) += 1;
                } else {
                    unresolved
                        .entry(field.clone())
                        .or_default()
                        .push((name.clone(), missing));
                }
            }
        }
    }
    DependencyLists {
        ids,
        votes,
        unresolved,
        present,
    }
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
    let DependencyLists {
        ids,
        votes: deps,
        unresolved,
        present,
    } = dependency_lists(doc, &collections);
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
                    .filter_map(|k| schema.get(k))
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
                        .filter(|k| m.contains_key(k))
                        .map(String::as_str)
                        .collect::<Vec<_>>();
                    !matched.is_empty()
                        && !(matched == ["blocked_on"]
                            && core
                            && m.get("rule").is_some_and(|v| map(v).is_ok()))
                });
            let portable = ["deps", "snapshot", "predicate"]
                    .iter()
                    .all(|k| schema.get(k).is_some_and(truth))
                && deps.is_empty()
                && unresolved
                    .keys()
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
            let header_only = !doc.is_empty() && doc.keys().all(|k| ["meta", "schema"].contains(&k.as_str()));
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
                    .filter(|k| m.contains_key(k))
                    .collect::<Vec<_>>();
                    !matched.is_empty()
                        && !(matched == ["blocked_on"]
                            && core
                            && m.get("rule").is_some_and(|v| map(v).is_ok()))
                });
            require(
                (header_only || !collections.is_empty() && source_only_collections(&collections))
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
                    .all(|k| d.contains_key(k)),
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

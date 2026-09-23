//! Ordinary graph counters are derived before predicates over counters are evaluated.
use crate::ordinary_value::py;
use crate::{
    Result, ordinary_fields as F, ordinary_language as L,
    ordinary_semantics::{Reader, predicate_of, predicate_refs},
    ordinary_value::Value as V,
    ordinary_value::{Map, map, text, truth},
    ordinary_value::{blocked_text, reopened_text},
    value::Integer,
};
use std::collections::BTreeSet;
fn get<'a>(m: &'a Map, k: &str) -> &'a V {
    m.get(k).unwrap_or(&V::Null)
}
fn strings(v: &V) -> Vec<String> {
    match v {
        V::List(a) => a
            .iter()
            .filter_map(|v| text(v).ok().map(str::to_owned))
            .collect(),
        _ => vec![],
    }
}
pub(crate) fn same_rule(a: &V, b: &V) -> bool {
    crate::ordinary_value::python_equal(a, b)
        || L::legacy_rule(a)
            .ok()
            .zip(L::legacy_rule(b).ok())
            .is_some_and(|(a, b)| a == b)
}
pub(crate) fn legacy_rule(old: &V, current: &V) -> Option<V> {
    if let (V::Text(t), V::Map(_)) = (old, current) {
        let t = V::Map(Map::from([("expr".into(), V::Text(t.clone()))]));
        L::legacy_rule(&t)
            .ok()
            .filter(|t| !L::legacy_references(t).is_empty())
    } else {
        None
    }
}
pub(crate) fn decimal(v: &V) -> Option<(bool, String, num_bigint::BigInt)> {
    let s = crate::history_yaml::numeric_text(&py(v)).replace([',', '_'], "");
    let s = s.trim();
    let (negative, s) = if let Some(s) = s.strip_prefix('-') {
        (true, s)
    } else {
        (false, s.strip_prefix('+').unwrap_or(s))
    };
    let (s, e) = s.split_once(['e', 'E']).unwrap_or((s, "0"));
    let e = e.parse::<num_bigint::BigInt>().ok()?;
    let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
    let digits = format!("{whole}{frac}");
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return Some((false, "0".into(), 0.into()));
    }
    let trim = digits.trim_end_matches('0');
    let exponent = e - num_bigint::BigInt::from(frac.len())
        + num_bigint::BigInt::from(digits.len() - trim.len());
    Some((negative, trim.into(), exponent))
}
fn pending(reader: &Reader<'_>, pred: &V) -> bool {
    let mut pending = predicate_refs(pred);
    let mut seen = BTreeSet::new();
    while let Some(k) = pending.pop() {
        if !seen.insert(k.clone()) {
            continue;
        }
        let b = reader.raw.get(&k).and_then(|b| map(b).ok());
        if F::BUILTINS.contains(&k.as_str()) && b.is_none_or(|b| get(b, "v") == &V::Null) {
            return true;
        }
        if let Some(b) = b {
            if matches!(b.get("rule"), Some(V::Map(_))) {
                pending.extend(L::legacy_references(&b["rule"]));
            } else {
                for key in ["rule", "v", "quoted"] {
                    if let Some(V::Text(t)) = b.get(key) {
                        pending.extend(
                            predicate_refs(&V::Text(t.clone()))
                                .into_iter()
                                .filter(|k| reader.ids.contains(k)),
                        );
                    }
                }
            }
        }
    }
    false
}
pub(crate) fn flags(reader: &Reader<'_>, body: &V) -> Result<BTreeSet<&'static str>> {
    let b = map(body)?;
    let fields = &reader.fields;
    let dep = text(&fields["deps"])?;
    let snapshot = text(&fields["snapshot"]).unwrap_or("");
    let deps = strings(get(b, dep));
    let empty = Map::new();
    let seen = map(get(b, snapshot)).unwrap_or(&empty);
    let pred = predicate_of(body, fields);
    let mut flags = BTreeSet::new();
    let blocked = !blocked_text(body).is_empty();
    for d in &deps {
        if !reader.ids.contains(d) {
            flags.insert(if blocked { "blocked" } else { "broken" });
        } else if !snapshot.is_empty() && !seen.contains_key(d) {
            flags.insert("unchecked");
        }
    }
    let rs = predicate_refs(&pred)
        .into_iter()
        .filter(|r| reader.ids.contains(r))
        .collect::<BTreeSet<_>>();
    let named = !rs.is_empty() && crate::ordinary_semantics::why_undecided(&pred).is_empty();
    let verdict = if named {
        reader.predicate(&pred)?
    } else {
        None
    };
    if !named && !blocked && (truth(&pred) || reopened_text(body).is_empty()) {
        flags.insert("no_predicate");
    } else if named && verdict == Some(true) {
        flags.insert("falsified");
    } else if named && verdict.is_none() && !pending(reader, &pred) {
        flags.insert("unknown");
    }
    for (d, old) in seen {
        let rule = reader
            .raw
            .get(d)
            .and_then(|b| map(b).ok())
            .map(|b| get(b, "rule"))
            .unwrap_or(&V::Null);
        if let Some(legacy) = legacy_rule(old, rule) {
            if same_rule(&legacy, rule) {
                flags.insert("unchecked");
            } else {
                flags.insert("moved");
            }
            continue;
        }
        let computed = map(old)
            .ok()
            .and_then(|m| m.get("computed"))
            .and_then(|v| map(v).ok());
        if reader.ids.contains(d) && (matches!(rule, V::Map(_)) || computed.is_some()) {
            let now = reader.value(d)?;
            let previous = computed.map(|m| get(m, "value")).unwrap_or(old);
            let changed_rule = computed.is_some_and(|m| !same_rule(get(m, "rule"), rule));
            let equal = crate::ordinary_value::same(previous, &now)?;
            if (changed_rule || (now != V::Null && *previous != V::Null && !equal))
                && !(rs.contains(d)
                    && (verdict == Some(true) || verdict == Some(false) && !changed_rule))
            {
                flags.insert("moved");
            }
            continue;
        }
        if !reader.ids.contains(d) || matches!(old, V::List(_) | V::Map(_)) {
            continue;
        }
        let now = reader.value(d)?;
        if now == V::Null || matches!(now, V::List(_) | V::Map(_)) {
            continue;
        }
        if py(old).split_whitespace().collect::<Vec<_>>()
            == py(&now).split_whitespace().collect::<Vec<_>>()
            || decimal(old).zip(decimal(&now)).is_some_and(|(a, b)| a == b)
        {
            continue;
        }
        if !(rs.contains(d) && verdict.is_some()) {
            flags.insert("moved");
        }
    }
    if crate::ordinary_semantics::reversal_pending(body).is_some() {
        let arrangement = deps.iter().any(|d| {
            reader
                .raw
                .get(d)
                .and_then(|b| map(b).ok())
                .is_some_and(|b| truth(get(b, "asked")))
        }) && (deps.iter().any(|d| F::BUILTINS.contains(&d.as_str()))
            || predicate_refs(&pred)
                .iter()
                .any(|d| F::BUILTINS.contains(&d.as_str())));
        if !arrangement {
            flags.insert("reversed");
        }
    }
    Ok(flags)
}
pub(crate) fn builtins(reader: &Reader<'_>) -> Result<Map> {
    let wanted = reader
        .ids
        .iter()
        .filter(|k| F::BUILTINS.contains(&k.as_str()))
        .collect::<Vec<_>>();
    if wanted.is_empty() {
        return Ok(Map::new());
    }
    let dep = text(&reader.fields["deps"])?;
    let judgments = reader
        .raw
        .iter()
        .filter(|(_, b)| map(b).is_ok_and(|b| b.contains_key(dep)))
        .collect::<Vec<_>>();
    let all = judgments
        .iter()
        .map(|(_, b)| flags(reader, b))
        .collect::<Result<Vec<_>>>()?;
    let mut counts = Map::new();
    let mut count = |key: &str, n: usize| {
        counts.insert(
            key.into(),
            V::Integer(Integer::new(&n.to_string()).unwrap()),
        );
    };
    count(
        "graph.entries",
        reader
            .ids
            .iter()
            .filter(|id| {
                !F::BUILTINS.contains(&id.as_str()) && !judgments.iter().any(|(j, _)| *j == *id)
            })
            .count(),
    );
    count("graph.judgments", judgments.len());
    let doc = map(&reader.document)?;
    let open = ["open", "questions"]
        .iter()
        .filter_map(|k| doc.get(k))
        .filter_map(|v| map(v).ok())
        .flat_map(|m| {
            m.iter()
                .filter(|(_, question)| !crate::public_amend::settled_value(question))
                .map(|(id, _)| id)
        })
        .collect::<BTreeSet<_>>();
    count("graph.open", open.len());
    count(
        "graph.flagged",
        all.iter().filter(|f| !f.is_empty()).count(),
    );
    for flag in [
        "blocked",
        "broken",
        "unchecked",
        "moved",
        "falsified",
        "no_predicate",
    ] {
        count(
            &format!("graph.{flag}"),
            all.iter().filter(|f| f.contains(flag)).count(),
        );
    }
    count(
        "graph.hypotheses",
        reader
            .hypotheses
            .values()
            .filter(|h| map(h).is_ok_and(|h| get(h, "kind") != &V::Text("contribution".into())))
            .count(),
    );
    let mut holders = std::collections::BTreeMap::<String, Vec<V>>::new();
    for h in reader.hypotheses.values() {
        let h = map(h)?;
        if h.get("error").is_some_and(truth) {
            continue;
        }
        let doc = &h["document"];
        for (id, body) in F::collections(doc)?.values().flat_map(|m| m.iter()) {
            let claim = if let Ok(body) = map(body) {
                ["verdict", "v", "quoted", "rule", "title"]
                    .into_iter()
                    .find_map(|f| {
                        body.get(f).filter(|v| **v != V::Null).map(|v| {
                            if f == "rule" && matches!(v, V::Map(_)) {
                                L::legacy_rule(v).unwrap_or_else(|_| v.clone())
                            } else {
                                v.clone()
                            }
                        })
                    })
                    .unwrap_or_else(|| V::Map(body.clone()))
            } else {
                body.clone()
            };
            holders.entry(id.clone()).or_default().push(claim);
        }
    }
    let mut contested = reader.knowledge_conflicts.clone();
    for (id, claims) in holders {
        if claims[1..].iter().any(|c| {
            if matches!(c, V::Map(_) | V::List(_)) || matches!(claims[0], V::Map(_) | V::List(_)) {
                !crate::ordinary_value::python_equal(c, &claims[0])
            } else {
                !crate::ordinary_semantics::same_legacy(c, &claims[0])
            }
        }) {
            contested.insert(id);
        }
    }
    count("graph.contested", contested.len());
    let mut high = BTreeSet::new();
    for (id, b) in &judgments {
        for d in strings(get(map(b)?, dep)) {
            if d.starts_with("prior.")
                && py(&reader.value(&d)?)
                    .parse::<f64>()
                    .is_ok_and(|v| v >= 0.8)
            {
                high.insert((*id).clone());
            }
        }
    }
    let mut refuted = BTreeSet::new();
    let re =
        regex::Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*\}\}").unwrap();

    for (id, b) in &reader.raw {
        let Ok(b) = map(b) else {
            continue;
        };
        if !id.starts_with("hyp.") || get(b, "v") != &V::Text("refuted".into()) {
            continue;
        }
        if b.contains_key("refutes") {
            refuted.extend(
                strings(&b["refutes"])
                    .into_iter()
                    .filter(|r| high.contains(r)),
            );
        } else if let Some(V::Text(name)) = b.get("name") {
            if high.contains(name.trim()) {
                refuted.insert(name.trim().into());
            }
            refuted.extend(
                re.captures_iter(name)
                    .map(|c| c[1].to_owned())
                    .filter(|r| high.contains(r)),
            );
        }
    }
    counts.insert(
        "graph.prior_reversal_rate".into(),
        V::from_json(&serde_json::json!(if high.is_empty() {
            0.0
        } else {
            refuted.len() as f64 / high.len() as f64
        }))?,
    );
    let names = [
        "entries the record holds",
        "judgments the record holds",
        "questions left open",
        "judgments that need a person",
        "judgments waiting on something declared missing",
        "judgments resting on something that is not an entry",
        "judgments never checked against one of their dependencies",
        "judgments a dependency moved under since they were reviewed",
        "judgments broken by their own condition",
        "judgments nothing evaluable would falsify",
        "hypotheses waiting beside the record",
        "ids two hypotheses hold with different claims",
        "share of high-confidence prior-resting judgments refuted",
        "flagged judgments no section of the page picked up",
        "intents no tab of the page serves",
        "share of what was added since the arrangement was born that nothing picks",
        "recent sessions in a row whose intent no tab serves",
        "entries and judgments some section of the page picks",
    ];
    Ok(wanted
        .into_iter()
        .map(|k| {
            let mut body = Map::from([(
                "name".into(),
                V::Text(names[F::BUILTINS.iter().position(|id| *id == k).unwrap()].into()),
            )]);
            let origin = if let Some(value) = counts.get(k) {
                body.insert("v".into(), value.clone());
                "counted from the record each time it is read"
            } else {
                "counted when the page is built"
            };
            body.insert("from".into(), V::Text(origin.into()));
            (k.clone(), V::Map(body))
        })
        .collect())
}

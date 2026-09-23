//! Ordinary-reader/v1 values and authored syntax. Legacy coercion is preserved;
//! explicit formulas use only the separately supplied, verified ordinary Lean program.
use crate::ordinary_value::py;
use crate::{
    Error, Result, ordinary_fields as F, ordinary_language as L,
    ordinary_runtime::Program,
    ordinary_value::Value as V,
    ordinary_value::{Map, field, map, text, truth},
    reasoning_runtime::{OperationalBounds, Runtime},
    require,
};
use serde_json::{Value as J, json};
use std::{collections::BTreeSet, sync::LazyLock};
pub(crate) static ID: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+").unwrap());
pub(crate) static CMP: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"^\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*(<=|>=|==|!=|<|>)\s*(.+?)\s*$",
    )
    .unwrap()
});
pub(crate) static EXPR: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[<>=!+\-*/()]|\b(?:or|and|not)\b").unwrap());
static SECOND: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[<>=!]=?|\b(?:or|and)\b").unwrap());
static REPLACED_DAY: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r" on ([0-9]{4}-[0-9]{2}-[0-9]{2})$").unwrap());
fn s(v: &str) -> V {
    V::Text(v.into())
}
pub(crate) fn predicate_refs(v: &V) -> Vec<String> {
    if matches!(v, V::Map(_)) {
        L::legacy_references(v)
    } else {
        ID.find_iter(&if truth(v) { py(v) } else { String::new() })
            .map(|m| m.as_str().into())
            .collect()
    }
}

/// Return the unresolved replacement day, matching Python's reversal_pending helper.
pub(crate) fn reversal_pending(body: &V) -> Option<String> {
    let fields = map(body).ok()?;
    let trail = fields.get("replaced")?;
    let last = match trail {
        V::List(values) => values.last().map(py)?,
        V::Text(value) => value.clone(),
        _ => return None,
    };
    let day = REPLACED_DAY.captures(&last)?.get(1)?.as_str().to_owned();
    let reviewed = fields.get("reviewed").map(py);
    let reviewed_day = reviewed
        .as_deref()
        .map(|value| {
            value.trim_start_matches(|c: char| {
                c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
            })
        })
        .and_then(|value| value.get(..10))
        .filter(|value| crate::value::Date::new(value).is_ok());
    if reviewed_day.is_some_and(|value| value >= day.as_str()) {
        None
    } else {
        Some(day)
    }
}

pub(crate) fn predicate_of(body: &V, fields: &Map) -> V {
    let v = map(body)
        .ok()
        .and_then(|b| text(&fields["predicate"]).ok().and_then(|f| b.get(f)))
        .unwrap_or(&V::Null);
    if matches!(v, V::Map(_)) {
        v.clone()
    } else {
        s(&if truth(v) { py(v) } else { String::new() })
    }
}
fn refs(v: &V) -> Vec<String> {
    L::legacy_references(v)
}
fn unquoted(source: &str) -> String {
    // The legacy regex removes paired quotes without interpreting escapes.
    let chars: Vec<_> = source.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if ['\'', '"'].contains(&chars[i])
            && let Some(end) = chars[i + 1..].iter().position(|c| *c == chars[i])
        {
            i += end + 2;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}
pub fn why_undecided(pred: &V) -> String {
    if matches!(pred, V::Map(_)) {
        let tree = match L::legacy_expression_detailed(pred, true) {
            Ok(t) => t,
            Err(e) => return e.0,
        };
        let m = map(&tree).unwrap();
        let V::List(args) = &m["args"] else {
            unreachable!()
        };
        if let (Some(left), Some(right)) = (map(&args[0]).ok(), map(&args[1]).ok())
            && let (Some(V::Text(name)), Some(V::Bool(v))) = (left.get("ref"), right.get("bool"))
        {
            let op = match text(&m["op"]).unwrap() {
                "eq" => "==",
                "ne" => "!=",
                "lt" => "<",
                "le" => "<=",
                "gt" => ">",
                _ => ">=",
            };
            if !["==", "!="].contains(&op) {
                return format!("orders a truth value ({op} {v}), which is matched, never ordered");
            }
            if F::BUILTINS.contains(&name.as_str()) {
                return format!(
                    "holds a count against a truth value ({name} {op} {v}), which never matches"
                );
            }
        }
        return String::new();
    }
    let source = if truth(pred) { py(pred) } else { String::new() };
    let Some(c) = CMP.captures(&source) else {
        return "is not one comparison this reader decides (a name, an operator, one value)".into();
    };
    if SECOND.is_match(&unquoted(c[3].trim())) {
        return "is not one comparison this reader decides (a name, an operator, one value)".into();
    }
    if ["true", "false"].contains(&c[3].trim().to_lowercase().as_str()) {
        if !["==", "!="].contains(&&c[2]) {
            return format!(
                "orders a truth value ({} {}), which is matched, never ordered",
                &c[2],
                c[3].trim()
            );
        }
        if F::BUILTINS.contains(&&c[1]) {
            return format!(
                "holds a count against a truth value ({} {} {}), which never matches",
                &c[1],
                &c[2],
                c[3].trim()
            );
        }
    }
    String::new()
}
/// Python float(str(value).replace(',', '')), used only by the legacy comparison.
pub(crate) fn same_legacy(a: &V, b: &V) -> bool {
    py(a).split_whitespace().collect::<Vec<_>>() == py(b).split_whitespace().collect::<Vec<_>>()
        || crate::ordinary_domain_counts::decimal(a)
            .zip(crate::ordinary_domain_counts::decimal(b))
            .is_some_and(|(a, b)| a == b)
}
pub(crate) fn number(v: &V) -> Option<f64> {
    let source = crate::history_yaml::numeric_text(&py(v)).replace(',', "");
    let source = source.trim();
    let valid=regex::Regex::new(r"(?i)^[+-]?(?:(?:[0-9](?:_?[0-9])*)?(?:\.(?:[0-9](?:_?[0-9])*)?)?(?:e[+-]?[0-9](?:_?[0-9])*)?|inf(?:inity)?|nan)$").unwrap();
    if source.is_empty() || !valid.is_match(source) {
        return None;
    }
    source.replace('_', "").parse().ok()
}
pub(crate) fn json_value(v: &V, _depth: usize) -> Result<J> {
    v.json_value()
}
/// Request legacy computations without promoting the document or creating an assessment.
/// Runtime failure is returned as an unavailable result, as in expressions.compute.
pub fn compute(
    raw: &Map,
    ids: &BTreeSet<String>,
    predicate: Option<&V>,
    program: Option<&Program>,
) -> J {
    let result = (|| -> Result<J> {
        let program =
            program.ok_or_else(|| Error("ordinary expression program is not configured".into()))?;
        let mut nodes = crate::ordinary_value::compute_record(raw, ids)?.value;
        for node in nodes["nodes"].as_object_mut().unwrap().values_mut() {
            if let Some(rule) = node["body"].get_mut("rule").filter(|r| r.is_object())
                && let Ok(tree) = L::legacy_expression(&V::from_json(rule)?, false)
            {
                *rule = tree.json_value()?;
            }
        }
        let pred = predicate.map(|p| L::legacy_expression(p, true).unwrap_or_else(|_| p.clone()));
        let request = json!({"operation":"compute","record":nodes,"predicate":pred.as_ref().map(|p|json_value(p,0)).transpose()?.unwrap_or(J::Null),"dependencies":predicate.map(refs).unwrap_or_default()});
        program.request(&request, &OperationalBounds::default())
    })();
    result.unwrap_or_else(|e|json!({"values":{},"predicate":{"holds_on_current_values":null,"reason":e.0},"error":e.0}))
}
pub fn value_of(
    raw: &Map,
    ids: &BTreeSet<String>,
    key: &str,
    program: Option<&Program>,
) -> Result<V> {
    let body = raw.get(key).unwrap_or(&V::Null);
    let Ok(m) = map(body) else {
        return Ok(if ids.contains(key) {
            body.clone()
        } else {
            V::Null
        });
    };
    if matches!(m.get("rule"), Some(V::Map(_))) {
        return V::from_json(&compute(raw, ids, None, program)["values"][key]["value"]);
    }
    let v = m
        .get("v")
        .filter(|v| **v != V::Null)
        .or_else(|| m.get("quoted"))
        .unwrap_or(&V::Null);
    if let V::Text(t) = v
        && EXPR.is_match(t)
        && ID.find_iter(t).any(|r| ids.contains(r.as_str()))
    {
        return Ok(V::Null);
    }
    Ok(v.clone())
}
pub fn evaluate(
    pred: &V,
    raw: &Map,
    ids: &BTreeSet<String>,
    program: Option<&Program>,
) -> Result<Option<bool>> {
    if matches!(pred, V::Map(_)) {
        if L::legacy_expression(pred, true).is_err() {
            return Ok(None);
        }
        return Ok(
            compute(raw, ids, Some(pred), program)["predicate"]["holds_on_current_values"]
                .as_bool(),
        );
    }
    let source = if truth(pred) { py(pred) } else { String::new() };
    let Some(c) = CMP.captures(&source) else {
        return Ok(None);
    };
    if !why_undecided(pred).is_empty() {
        return Ok(None);
    }
    let rhs = c[3].trim();
    let op = &c[2];
    if [&c[1], rhs].iter().any(|k| {
        raw.get(k)
            .and_then(|v| map(v).ok())
            .is_some_and(|m| matches!(m.get("rule"), Some(V::Map(_))))
    }) {
        let p = V::Map(Map::from([("expr".into(), s(&source))]));
        if L::legacy_expression(&p, true).is_err() {
            return Ok(None);
        }
        return Ok(
            compute(raw, ids, Some(&p), program)["predicate"]["holds_on_current_values"].as_bool(),
        );
    }
    let a = value_of(raw, ids, &c[1], program)?;
    if a == V::Null {
        return Ok(None);
    }
    if ["true", "false"].contains(&rhs.to_lowercase().as_str()) {
        let V::Bool(a) = a else {
            return Ok(None);
        };
        let same = a == (rhs.to_lowercase() == "true");
        return Ok(Some(if op == "==" { same } else { !same }));
    }
    let b = if ID.find(rhs).is_some_and(|m| m.as_str() == rhs) {
        value_of(raw, ids, rhs, program)?
    } else {
        s(rhs.trim_matches(['\'', '"']))
    };
    if b == V::Null || matches!(a, V::Bool(_)) != matches!(b, V::Bool(_)) {
        return Ok(None);
    }
    let answer = if let (Some(a), Some(b)) = (number(&a), number(&b)) {
        match op {
            "<" => a < b,
            ">" => a > b,
            "<=" => a <= b,
            ">=" => a >= b,
            "==" => a == b,
            _ => a != b,
        }
    } else {
        let (a, b) = (py(&a), py(&b));
        match op {
            "<" => a < b,
            ">" => a > b,
            "<=" => a <= b,
            ">=" => a >= b,
            "==" => a == b,
            _ => a != b,
        }
    };
    Ok(Some(answer))
}
pub struct Reader<'a> {
    pub(crate) document: V,
    pub(crate) fields: Map,
    pub(crate) raw: Map,
    pub(crate) ids: BTreeSet<String>,
    pub(crate) hypotheses: Map,
    pub(crate) knowledge_conflicts: BTreeSet<String>,
    pub(crate) runtime: Option<&'a Runtime>,
}
impl<'a> Reader<'a> {
    /// Followups retain source-only records even when no judgment field roles
    /// can be inferred, matching the legacy graph reader's explicit fallback.
    pub(crate) fn for_followups(document: &V, runtime: Option<&'a Runtime>) -> Result<Self> {
        if let Some(fields) = Self::roles(document)? {
            return Self::with_roles(document, fields, runtime);
        }
        let raw = F::collections(document)?
            .into_values()
            .flatten()
            .collect::<Map>();
        let keys = raw
            .values()
            .filter_map(|v| map(v).ok())
            .flat_map(|m| m.keys())
            .collect::<BTreeSet<_>>();
        let mut suffix = 0usize;
        let role = loop {
            let key = format!("__followup_unassigned_role_{suffix}");
            if !keys.contains(&key) {
                break key;
            }
            suffix += 1;
        };
        let fields = ["deps", "snapshot", "predicate"]
            .into_iter()
            .map(|name| (name.to_owned(), s(&role)))
            .collect();
        let mut reader = Self {
            document: document.clone(),
            fields,
            ids: raw.keys().cloned().collect(),
            raw,
            hypotheses: Map::new(),
            knowledge_conflicts: BTreeSet::new(),
            runtime,
        };
        reader
            .raw
            .extend(crate::ordinary_domain_counts::builtins(&reader)?);
        Ok(reader)
    }

    /// A record whose field roles cannot be read is refused with the reader's account of
    /// why, so the person holding it has something to act on.
    pub fn new(document: &V, runtime: Option<&'a Runtime>) -> Result<Self> {
        let fields = Self::roles(document)?.ok_or_else(|| F::unreadable(document))?;
        Self::with_roles(document, fields, runtime)
    }

    /// The field roles of an ordinary record, or none when they cannot be read.
    fn roles(document: &V) -> Result<Option<Map>> {
        require(
            map(&F::capabilities(document, None)?)?["profile"] == s("ordinary-reader/v1"),
            "ordinary_reader_requires_ordinary_profile",
        )?;
        Ok(F::semantic_roles(document)?
            .map(|(_, fields)| fields)
            .filter(|fields| !fields.is_empty()))
    }

    fn with_roles(document: &V, fields: Map, runtime: Option<&'a Runtime>) -> Result<Self> {
        let raw = map(document)?
            .iter()
            .filter(|(k, _)| !["meta", "schema", "record", "also"].contains(&k.as_str()))
            .filter_map(|(_, v)| map(v).ok())
            .flat_map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())))
            .collect::<Map>();
        let mut ids = F::collections(document)?
            .values()
            .flat_map(|m| m.keys().cloned())
            .collect::<BTreeSet<_>>();
        for body in raw.values() {
            if let Ok(body) = map(body) {
                for value in body.values() {
                    let mentions = match value {
                        V::Text(_) => predicate_refs(value),
                        V::List(a) => a
                            .iter()
                            .filter(|v| matches!(v, V::Text(_)))
                            .flat_map(predicate_refs)
                            .collect(),
                        V::Map(m) => {
                            let rs = refs(value);
                            if !rs.is_empty() {
                                rs
                            } else {
                                m.keys().flat_map(|k| predicate_refs(&s(k))).collect()
                            }
                        }
                        _ => vec![],
                    };
                    ids.extend(
                        mentions
                            .into_iter()
                            .filter(|k| F::BUILTINS.contains(&k.as_str())),
                    );
                }
            }
        }
        let mut reader = Self {
            document: document.clone(),
            fields,
            raw,
            ids,
            hypotheses: Map::new(),
            knowledge_conflicts: BTreeSet::new(),
            runtime,
        };
        reader
            .raw
            .extend(crate::ordinary_domain_counts::builtins(&reader)?);
        Ok(reader)
    }
    /// Supply already captured hypothesis documents and conflict IDs explicitly.
    /// These affect reader counters/guards; they never activate accepted history.
    pub fn with_layers(
        mut self,
        hypotheses: Map,
        knowledge_conflicts: BTreeSet<String>,
    ) -> Result<Self> {
        for h in hypotheses.values() {
            let h = map(h)?;
            if !h.get("error").is_some_and(truth) {
                let doc = field(h, "document")?;
                // A physical hypothesis may contain only one collection and
                // inherit the base record's field roles, as the legacy reader
                // does. Explicit roles are still validated when present.
                if F::semantic_roles(doc)?.is_none() {
                    F::collections(doc)?;
                }
            }
        }
        self.hypotheses = hypotheses;
        self.knowledge_conflicts = knowledge_conflicts;
        self.raw.retain(|k, _| !F::BUILTINS.contains(&k.as_str()));
        self.raw
            .extend(crate::ordinary_domain_counts::builtins(&self)?);
        Ok(self)
    }
    pub(crate) fn program(&self) -> Option<&Program> {
        self.runtime.and_then(Runtime::ordinary_program)
    }
    pub fn fields(&self) -> &Map {
        &self.fields
    }
    pub fn raw(&self) -> &Map {
        &self.raw
    }
    pub fn value(&self, key: &str) -> Result<V> {
        value_of(&self.raw, &self.ids, key, self.program())
    }
    pub fn predicate(&self, pred: &V) -> Result<Option<bool>> {
        evaluate(pred, &self.raw, &self.ids, self.program())
    }
}

pub(crate) fn layer(base: &V, groups: &V, names: &[String]) -> Result<V> {
    let mut doc = base.clone();
    for name in names {
        let group = map(field(map(groups)?, name)?)?;
        crate::require(
            !group.get("error").is_some_and(truth),
            "unresolved_history_hypothesis",
        )?;
        for (collection, members) in F::collections(field(group, "doc")?)? {
            for id in members.keys() {
                for (other, values) in crate::ordinary_value::map_mut(&mut doc)?.iter_mut() {
                    if other != &collection
                        && let V::Map(values) = values
                    {
                        values.remove(id);
                    }
                }
            }
            let target = crate::ordinary_value::map_mut(&mut doc)?
                .entry(collection)
                .or_insert_with(|| V::Map(Map::new()));
            if !matches!(target, V::Map(_)) {
                *target = V::Map(Map::new());
            }
            crate::ordinary_value::map_mut(target)?.extend(members);
        }
    }
    Ok(doc)
}
/// Why the base cannot be read under a hypothesis, on one line of at most 160
/// characters beside the other findings.
pub(crate) fn layer_failure(failure: &Error) -> String {
    failure
        .0
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect()
}
pub(crate) fn arrangement(reader: &Reader<'_>, body: &V) -> bool {
    let Ok(m) = map(body) else { return false };
    let dep = text(&reader.fields["deps"]).unwrap_or("");
    let Some(V::List(deps)) = m.get(dep) else {
        return false;
    };
    if deps.is_empty() || !deps.iter().all(|v| matches!(v, V::Text(_))) {
        return false;
    }
    let names = deps.iter().filter_map(|v| text(v).ok()).collect::<Vec<_>>();
    let pred = m
        .get(text(&reader.fields["predicate"]).unwrap_or(""))
        .unwrap_or(&V::Null);
    let refs = predicate_refs(pred);
    names.iter().any(|id| {
        reader
            .raw
            .get(id)
            .and_then(|v| map(v).ok())
            .and_then(|v| v.get("asked"))
            .is_some_and(truth)
    }) && (names.iter().any(|id| F::BUILTINS.contains(id))
        || refs.iter().any(|id| F::BUILTINS.contains(&id.as_str())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ordinary_value::{Map, NonFiniteFloat};
    #[test]
    fn source_nonfinite_plain_comparisons_and_derivative_calculations_match_python() {
        let program = crate::ordinary_reader::tests::program();
        for (kind, eq, greater) in [
            (NonFiniteFloat::NaN, false, false),
            (NonFiniteFloat::PositiveInfinity, true, true),
            (NonFiniteFloat::NegativeInfinity, true, false),
        ] {
            let source = V::Map(Map::from([
                ("v".into(), V::NonFinite(kind)),
                (
                    "quoted".into(),
                    V::from_json(&serde_json::json!(99)).unwrap(),
                ),
            ]));
            let raw = Map::from([("p.value".into(), source.clone())]);
            let ids = BTreeSet::from(["p.value".into()]);
            assert_eq!(
                value_of(&raw, &ids, "p.value", Some(&program)).unwrap(),
                V::NonFinite(kind)
            );
            for (expression, expected) in [
                ("p.value == p.value", eq),
                ("p.value != p.value", !eq),
                ("p.value > 0", greater),
            ] {
                assert_eq!(
                    evaluate(&s(expression), &raw, &ids, Some(&program)).unwrap(),
                    Some(expected)
                );
            }
            let result = compute(
                &raw,
                &ids,
                Some(&V::Map(Map::from([("expr".into(), s("p.value > 0"))]))),
                Some(&program),
            );
            assert_eq!(result["values"]["p.value"]["role"], "unavailable");
            assert!(result["values"]["p.value"]["value"].is_null());
            assert!(result["predicate"]["holds_on_current_values"].is_null());
            assert_eq!(raw["p.value"], source);
            let mut rule = map(&source).unwrap().clone();
            rule.insert("rule".into(), V::Map(Map::from([("num".into(), s("42"))])));
            assert_eq!(
                compute(
                    &Map::from([("p.value".into(), V::Map(rule))]),
                    &ids,
                    None,
                    Some(&program)
                )["values"]["p.value"]["value"],
                42
            );
        }
    }
}

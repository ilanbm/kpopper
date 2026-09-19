//! Ordinary-reader/v1 values and authored syntax. Legacy coercion is preserved;
//! explicit formulas use only the separately supplied, verified ordinary Lean program.
use crate::source_text::ordinary_python_str as py;
use crate::{
    Error, Result,
    history_contract::*,
    history_view::{map_mut, truth},
    ordinary_runtime::Program,
    reasoning_authoring::blocked_text,
    reasoning_authoring_guards::{self as G, Admission},
    reasoning_fields as F, reasoning_language as L,
    reasoning_runtime::{OperationalBounds, Runtime},
    require,
    value::TypedValue as V,
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

#[cfg(test)]
mod reversal_tests {
    use super::*;

    #[test]
    fn a_reversal_is_acknowledged_only_by_a_valid_review_day() {
        for (reviewed, pending) in [
            (V::Null, true),
            (s("unknown"), true),
            (s("2026-09-31"), true),
            (s("2026-09-15"), true),
            (s(" 2026-09-16 after reading"), false),
            (s("2026-09-17T12:30:00Z"), false),
        ] {
            let body = V::Map(Map::from([
                (
                    "replaced".into(),
                    V::List(vec![s("its condition fired on 2026-09-16")]),
                ),
                ("reviewed".into(), reviewed.clone()),
            ]));
            assert_eq!(reversal_pending(&body).is_some(), pending, "{reviewed:?}");
        }
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
fn get<'a>(m: &'a Map, key: &str) -> &'a V {
    m.get(key).unwrap_or(&V::Null)
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
        || crate::ordinary_counts::decimal(a)
            .zip(crate::ordinary_counts::decimal(b))
            .is_some_and(|(a, b)| a == b)
}
fn number(v: &V) -> Option<f64> {
    let source = crate::history_yaml::numeric_text(&py(v)).replace(',', "");
    let source = source.trim();
    let valid=regex::Regex::new(r"(?i)^[+-]?(?:(?:[0-9](?:_?[0-9])*)?(?:\.(?:[0-9](?:_?[0-9])*)?)?(?:e[+-]?[0-9](?:_?[0-9])*)?|inf(?:inity)?|nan)$").unwrap();
    if source.is_empty() || !valid.is_match(source) {
        return None;
    }
    source.replace('_', "").parse().ok()
}
pub(crate) fn json_value(v: &V, depth: usize) -> Result<J> {
    if depth > 128 {
        return Ok(J::Null);
    }
    Ok(match v {
        V::Map(m) => J::Object(
            m.iter()
                .map(|(k, v)| {
                    let key = match crate::history_yaml::projected_ordinary_key(k) {
                        Some(V::Text(value)) => value,
                        Some(V::Null) => "null".into(),
                        Some(V::Bool(value)) => value.to_string(),
                        Some(V::Integer(value)) => value.as_str().into(),
                        Some(V::Float(value)) => crate::identity::python_float(value.get()),
                        Some(V::Date(_) | V::DateTime(_) | V::List(_) | V::Map(_)) => {
                            return Err(Error(
                                "ordinary JSON requires scalar JSON-compatible keys".into(),
                            ));
                        }
                        None => k.clone(),
                    };
                    Ok((key, json_value(v, depth + 1)?))
                })
                .collect::<Result<_>>()?,
        ),
        V::List(a) => J::Array(
            a.iter()
                .map(|v| json_value(v, depth + 1))
                .collect::<Result<_>>()?,
        ),
        V::Date(_) | V::DateTime(_) => J::String(py(v)),
        _ => v.to_json().unwrap_or(J::Null),
    })
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
        let mut nodes = serde_json::Map::new();
        for (id, body) in raw.iter().filter(|(id, _)| ids.contains(*id)) {
            let mut body = match body {
                V::Map(m) => V::Map(m.clone()),
                v => V::Map(Map::from([("v".into(), v.clone())])),
            };
            let m = map_mut(&mut body)?;
            // A nonfinite primary value must not fall through to quoted/verdict/read.
            let reading = m
                .get("v")
                .filter(|v| **v != V::Null)
                .or_else(|| m.get("quoted"));
            if get(m, "rule") == &V::Null
                && reading.is_some_and(|v| matches!(v, V::Float(_)) && v.to_json().is_err())
            {
                *m = Map::from([("v".into(), V::Null)]);
            }
            if let Some(rule @ V::Map(_)) = m.get("rule")
                && let Ok(tree) = L::legacy_expression(rule, false)
            {
                m.insert("rule".into(), tree);
            }
            nodes.insert(id.clone(), json!({"body":json_value(&body,0)?}));
        }
        let pred = predicate.map(|p| L::legacy_expression(p, true).unwrap_or_else(|_| p.clone()));
        let request = json!({"operation":"compute","record":{"nodes":nodes},"predicate":pred.as_ref().map(|p|json_value(p,0)).transpose()?.unwrap_or(J::Null),"dependencies":predicate.map(refs).unwrap_or_default()});
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
        raw.get(*k)
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
    runtime: Option<&'a Runtime>,
}
impl<'a> Reader<'a> {
    pub fn new(document: &V, runtime: Option<&'a Runtime>) -> Result<Self> {
        require(
            map(&F::capabilities(document, None)?)?["profile"] == s("ordinary-reader/v1"),
            "ordinary_reader_requires_ordinary_profile",
        )?;
        let (_, fields) = F::semantic_roles(document)?
            .filter(|(_, f)| !f.is_empty())
            .ok_or_else(|| Error("ordinary_fields_unreadable".into()))?;
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
            .extend(crate::ordinary_counts::builtins(&reader)?);
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
        self.raw.extend(crate::ordinary_counts::builtins(&self)?);
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
    pub fn document(&self) -> &V {
        &self.document
    }
    pub fn value(&self, key: &str) -> Result<V> {
        value_of(&self.raw, &self.ids, key, self.program())
    }
    pub fn predicate(&self, pred: &V) -> Result<Option<bool>> {
        evaluate(pred, &self.raw, &self.ids, self.program())
    }
    pub(crate) fn for_action(&mut self, action: &V) -> Result<()> {
        let a = map(action)?;
        if string_is(get(a, "kind"), "add")
            && let Ok(b) = map(get(a, "body"))
            && b.contains_key(text(&self.fields["deps"])?)
            && self.fields["predicate"] == V::Null
            && b.contains_key("wrong_if")
        {
            self.fields.insert("predicate".into(), s("wrong_if"));
        }
        Ok(())
    }
    pub fn normalize(&self, action: &V) -> Result<(V, Vec<String>)> {
        let a = map(action)?;
        let Ok(b) = map(get(a, "body")) else {
            return Ok((action.clone(), vec![]));
        };
        if !string_is(get(a, "kind"), "add") {
            return Ok((action.clone(), vec![]));
        }
        let id = text(field(a, "id")?)?;
        let predicate = b.contains_key(text(&self.fields["deps"])?);
        let field = if predicate {
            text(&self.fields["predicate"]).unwrap_or("")
        } else {
            "rule"
        };
        let Some(V::Text(source)) = b.get(field) else {
            return Ok((action.clone(), vec![]));
        };
        if self
            .raw
            .get(id)
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get(field))
            == Some(&s(source))
        {
            return Ok((action.clone(), vec![]));
        }
        let keep = |why: String| {
            Ok((
                action.clone(),
                vec![format!("NOTE {id}.{field} kept as text: {why}")],
            ))
        };
        if !predicate && ["v", "quoted"].iter().any(|k| b.contains_key(*k)) {
            return keep("a stored reading and a calculation need distinct fields/entries".into());
        }
        let tree = match L::convert_authored(source, predicate) {
            Ok(t) => V::from_json(&t)?,
            Err(e) => return keep(e.0),
        };
        if !predicate
            && refs(&tree)
                .iter()
                .any(|r| !self.ids.contains(r) && r != id && !F::BUILTINS.contains(&r.as_str()))
        {
            return keep("unknown or ambiguous reference; use rule={expr: \"...\"} for an intended calculation".into());
        }
        let mut candidate = self.raw.clone();
        let mut body = b.clone();
        body.insert(field.into(), tree.clone());
        candidate.insert(id.into(), V::Map(body));
        let mut ids = self.ids.clone();
        ids.insert(id.into());
        ids.extend(
            refs(&tree)
                .into_iter()
                .filter(|r| F::BUILTINS.contains(&r.as_str())),
        );
        let result = compute(
            &candidate,
            &ids,
            if predicate { Some(&tree) } else { None },
            self.program(),
        );
        if let Some(e) = result.get("error").and_then(J::as_str) {
            return keep(e.into());
        }
        if predicate {
            let before = self.predicate(&s(source))?;
            let after = result["predicate"]["holds_on_current_values"].as_bool();
            let args = &tree.to_json()?["args"];
            let args = args
                .as_array()
                .ok_or_else(|| Error("invalid_ordinary_predicate".into()))?;
            let arithmetic = args.iter().any(|a| a.get("op").is_some());
            if after.is_none() || (before.is_some() && before != after && !arithmetic) {
                return keep(
                    "typed comparison is unavailable or changes the existing interpretation".into(),
                );
            }
            if args.iter().all(|a| a.get("ref").is_some())
                && args.iter().any(|a| {
                    self.value(a["ref"].as_str().unwrap())
                        .is_ok_and(|v| matches!(v, V::Text(_)))
                })
            {
                return keep(
                    "comparisons between text readings need an explicit choice of typed semantics"
                        .into(),
                );
            }
        } else if result["values"][id]["value"].is_null() {
            let why = result["values"][id]["reason"]
                .as_str()
                .unwrap_or("unavailable calculation");
            require(
                !["division_by_zero", "cyclic_reference", "missing_reference"].contains(&why)
                    || !blocked_text(get(a, "body")).is_empty(),
                &format!("refused - {id}: rule cannot be computed: {why}"),
            )?;
            return keep(why.into());
        }
        let mut a = a.clone();
        let mut b = b.clone();
        b.insert(
            field.into(),
            V::Map(Map::from([("expr".into(), s(source))])),
        );
        a.insert("body".into(), V::Map(b));
        Ok((
            V::Map(a),
            vec![format!("{id}.{field}: stored as a readable expression")],
        ))
    }
    pub fn validate(&mut self, action: &V) -> Result<Vec<String>> {
        let mut out = G::validate(self, action)?;
        let a = map(action)?;
        if !string_is(get(a, "kind"), "add") {
            return Ok(out);
        }
        let Ok(b) = map(get(a, "body")) else {
            return Ok(out);
        };
        let id = text(field(a, "id")?)?;
        let dep = text(&self.fields["deps"])?;
        let pred = text(&self.fields["predicate"]).unwrap_or("");
        let mut problems = vec![];
        for (key, predicate) in [("rule", false), (pred, true)] {
            let Some(v @ V::Map(_)) = b.get(key) else {
                continue;
            };
            if let Err(e) = L::legacy_expression(v, predicate) {
                problems.push(format!("{key}: {e}"));
                continue;
            }
            let rs = refs(v);
            let missing = rs
                .iter()
                .filter(|r| {
                    !self.ids.contains(*r) && *r != id && !F::BUILTINS.contains(&r.as_str())
                })
                .cloned()
                .collect::<Vec<_>>();
            if !missing.is_empty() && blocked_text(get(a, "body")).is_empty() {
                problems.push(format!("{key}: unknown references: {}", missing.join(", ")));
            }
            if predicate {
                let ds = match b.get(dep) {
                    Some(V::List(a)) => a.clone(),
                    _ => vec![],
                };
                let undeclared = rs
                    .iter()
                    .filter(|r| !ds.contains(&s(r)))
                    .cloned()
                    .collect::<Vec<_>>();
                if !undeclared.is_empty() {
                    problems.push(format!(
                        "predicate reads undeclared references: {}",
                        undeclared.join(", ")
                    ));
                }
            } else if b.contains_key("v") || b.contains_key("quoted") {
                problems.push("a structured rule cannot also store v or quoted".into());
            }
        }
        if problems.is_empty() && matches!(b.get("rule"), Some(V::Map(_))) {
            let mut raw = self.raw.clone();
            raw.insert(id.into(), V::Map(b.clone()));
            let mut ids = self.ids.clone();
            ids.insert(id.into());
            ids.extend(
                refs(&b["rule"])
                    .into_iter()
                    .filter(|r| F::BUILTINS.contains(&r.as_str())),
            );
            let result = compute(&raw, &ids, None, self.program());
            if result["values"][id]["value"].is_null() && blocked_text(get(a, "body")).is_empty() {
                problems.push(format!(
                    "rule cannot be computed: {}",
                    result["values"][id]["reason"]
                        .as_str()
                        .or_else(|| result["error"].as_str())
                        .unwrap_or("unavailable")
                ));
            }
        }
        out.extend(problems);
        let old_arrangement = self
            .raw
            .get(id)
            .is_some_and(|body| G::arrangement(self, body));
        if b.contains_key(dep) && !old_arrangement && self.predicate(get(b, pred))? == Some(true) {
            out.push(format!(
                "wrong_if already holds ({}) - the judgment would be born broken",
                py(get(b, pred))
            ));
        }
        if G::arrangement(self, get(a, "body")) {
            for key in ["born", "replaced"] {
                if b.contains_key(key) {
                    out.push(if key=="born"{"born is written by this tool - the day the arrangement is decided - leave it out".into()}else{"replaced is written by this tool, when a decision replaces another - leave it out".into()});
                }
            }
            let bad = self.one_comparison(&predicate_of(get(a, "body"), &self.fields))?;
            if !bad.is_empty() {
                out.push(format!("wrong_if {bad} - an arrangement's sign is decided by the build, or it is decoration"));
            }
        } else if old_arrangement
            && b.get(dep)
                .is_some_and(|v| matches!(v,V::List(a)if !a.is_empty()))
        {
            out.push("what replaces an arrangement is an arrangement - rest on the session sources of the occasion it decides and give it a sign over a count; an occasion read elsewhere is re-decided as that, never dropped".into());
        }
        let today = crate::source_clock::latest_day();
        for key in ["of", "read"] {
            if let Some(v) = b.get(key) {
                let stamp = py(v);
                if let Some(day) = stamp.trim_start().get(..10)
                    && crate::value::Date::new(day).is_ok()
                    && day > today.as_str()
                {
                    out.push(format!("{key}: {stamp} is after today ({today}) - a reading is dated the day it was read, never ahead"));
                }
            }
        }
        Ok(out)
    }
    fn one_comparison(&self, pred: &V) -> Result<String> {
        let bad = why_undecided(pred);
        if !bad.is_empty() {
            return Ok(bad);
        }
        let typed = matches!(pred, V::Map(_));
        let parts = if typed {
            let tree = L::legacy_expression(pred, true)?;
            let m = map(&tree)?;
            let V::List(args) = &m["args"] else {
                return Ok(String::new());
            };
            let (left, right) = (map(&args[0])?, map(&args[1])?);
            let Some(V::Text(name)) = left.get("ref") else {
                return Ok(String::new());
            };
            if right.contains_key("op") {
                return Ok(String::new());
            }
            let op = match text(&m["op"])? {
                "eq" => "==",
                "ne" => "!=",
                "lt" => "<",
                "le" => "<=",
                "gt" => ">",
                _ => ">=",
            };
            let rhs = if let Some(v) = right.get("ref").or_else(|| right.get("num")) {
                text(v)?.to_owned()
            } else {
                args[1].to_json()?.to_string()
            };
            (name.clone(), op.to_owned(), rhs, right.contains_key("ref"))
        } else {
            let source = py(pred);
            let Some(c) = CMP.captures(&source) else {
                return Ok(String::new());
            };
            (
                c[1].into(),
                c[2].into(),
                c[3].trim().into(),
                ID.find(c[3].trim())
                    .is_some_and(|m| m.as_str() == c[3].trim()),
            )
        };
        let (name, op, rhs, reference) = parts;
        let quoted = (rhs.starts_with('"') && rhs.ends_with('"'))
            || (rhs.starts_with('\'') && rhs.ends_with('\''));
        if !typed
            && !reference
            && !quoted
            && !["true", "false"].contains(&rhs.to_lowercase().as_str())
            && !regex::Regex::new(r"^-?\d[\d,]*(\.\d+)?$")
                .unwrap()
                .is_match(&rhs)
        {
            return Ok("does not name one value a sign carries (a number, a truth value, a text in quotes, or another entry)".into());
        }
        if reference
            && !F::BUILTINS.contains(&rhs.as_str())
            && (!self.ids.contains(&rhs) || self.value(&rhs)? == V::Null)
        {
            return Ok(format!(
                "compares against {rhs}, which holds no value the build can compare"
            ));
        }
        if F::BUILTINS.contains(&name.as_str())
            && reference
            && matches!(self.value(&rhs)?, V::Bool(_))
        {
            return Ok(format!(
                "holds a count against a truth value ({name} {op} {rhs}), which never matches"
            ));
        }
        if reference {
            return Ok(String::new());
        }
        if let Some(x) = number(&s(&rhs))
            && F::BUILTINS.contains(&name.as_str())
        {
            if op == "<" && x <= 0.0 || ["<=", "=="].contains(&op.as_str()) && x < 0.0 {
                return Ok(format!(
                    "can never hold - a count is never below zero ({name} {op} {rhs})"
                ));
            }
            if ["page.drift", "graph.prior_reversal_rate"].contains(&name.as_str())
                && (op == ">" && x >= 1.0 || [">=", "=="].contains(&op.as_str()) && x > 1.0)
            {
                return Ok(format!(
                    "can never hold - a share is never above one ({name} {op} {rhs})"
                ));
            }
        }
        Ok(String::new())
    }
    pub fn candidate(&self, action: &V) -> Result<V> {
        let a = map(action)?;
        let id = text(field(a, "id")?)?;
        let kind = text(field(a, "kind")?)?;
        let mut doc = self.document.clone();
        let existing = crate::reasoning_snapshot::entries(&doc)?;
        if kind == "add" {
            let body = field(a, "body")?;
            let collection = existing
                .get(id)
                .map(|(c, _)| Ok(c.clone()))
                .unwrap_or_else(|| {
                    G::collection_for_document(
                        &doc,
                        &self.fields,
                        id,
                        body,
                        a.get("into").and_then(|v| text(v).ok()),
                    )
                })?;
            map_mut(
                map_mut(&mut doc)?
                    .entry(collection)
                    .or_insert_with(|| V::Map(Map::new())),
            )?
            .insert(id.into(), body.clone());
        } else if kind == "set" {
            let (collection, body) = existing
                .get(id)
                .ok_or_else(|| Error("unresolved_history_subject".into()))?;
            let mut b = map(body)?.clone();
            let key = if b.contains_key("v") {
                "v"
            } else if b.contains_key("quoted") {
                "quoted"
            } else {
                return Err(Error("set needs an entry with v or quoted".into()));
            };
            b.insert(key.into(), field(a, "value")?.clone());
            b.insert("of".into(), field(a, "as_of")?.clone());
            if let Some(source) = a.get("source").filter(|v| **v != V::Null) {
                b.insert("from".into(), source.clone());
                b.insert("at".into(), field(a, "at")?.clone());
            }
            if let Some(scope) = a.get("_record_scope") {
                b.insert("scope".into(), scope.clone());
            }
            map_mut(map_mut(&mut doc)?.get_mut(collection).unwrap())?.insert(id.into(), V::Map(b));
        }
        Ok(doc)
    }
}
impl Admission for Reader<'_> {
    fn fields(&self) -> &Map {
        &self.fields
    }
    fn raw(&self) -> &Map {
        &self.raw
    }
    fn document(&self) -> &V {
        &self.document
    }
    fn hypotheses(&self) -> Result<&Map> {
        Ok(&self.hypotheses)
    }
    fn as_of(&self) -> Option<&V> {
        None
    }
    fn core(&self) -> bool {
        false
    }
    fn value(&mut self, id: &str) -> Result<V> {
        Reader::value(self, id)
    }
    fn result(&mut self, id: &str) -> Result<J> {
        Ok(json!({"status":if Reader::value(self,id)?==V::Null{"unknown"}else{"ok"}}))
    }
    fn same_value(&mut self, id: &str, candidate: &V) -> Result<bool> {
        Ok(same_legacy(&Reader::value(self, id)?, candidate))
    }
    fn same(&self, a: &V, b: &V) -> Result<bool> {
        Ok(same_legacy(a, b))
    }
    fn standing_predicate(&self, _id: &str, expression: &V) -> Result<Option<bool>> {
        self.predicate(expression)
    }
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn program() -> Program {
        let root = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                let os = if cfg!(target_os = "macos") {
                    "darwin"
                } else if cfg!(windows) {
                    "windows"
                } else {
                    std::env::consts::OS
                };
                let arch = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
                    "arm64"
                } else if cfg!(all(windows, target_arch = "x86_64")) {
                    "amd64"
                } else {
                    std::env::consts::ARCH
                };
                std::path::PathBuf::from(
                    std::env::var_os("HOME")
                        .or_else(|| std::env::var_os("USERPROFILE"))
                        .expect("set KPOP_TEST_ORDINARY_PROGRAM to a verified ordinary Lean build"),
                )
                .join(".cache/kpopper/lean")
                .join(format!("{os}-{arch}"))
                .join(env!("KPOP_ORDINARY_SOURCE_SHA256"))
            });
        Program::open(&root).unwrap()
    }
    #[test]
    fn legacy_values_predicates_and_normalization_match_final_python() {
        let c: J =
            serde_json::from_str(include_str!("../tests/fixtures/ordinary-reader.json")).unwrap();
        let raw = V::from_tagged(&c["raw"]).unwrap();
        let raw = map(&raw).unwrap();
        let ids = c["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        let p = program();
        for case in c["values"].as_array().unwrap() {
            let key = case["key"].as_str().unwrap();
            assert_eq!(
                value_of(raw, &ids, key, Some(&p)).unwrap(),
                V::from_tagged(&case["value"]).unwrap(),
                "{key}"
            );
        }
        for case in c["predicates"].as_array().unwrap() {
            let pred = V::from_tagged(&case["predicate"]).unwrap();
            assert_eq!(
                evaluate(&pred, raw, &ids, Some(&p)).unwrap(),
                case["value"].as_bool(),
                "{pred:?}"
            );
        }
        let cache = tempfile::tempdir().unwrap();
        let rt = crate::history_authoring::tests::runtime(cache.path()).with_ordinary_program(p);
        let doc = V::from_tagged(&c["document"]).unwrap();
        let mut reader = Reader::new(&doc, Some(&rt)).unwrap();
        for case in c["normalization"].as_array().unwrap() {
            let a = V::from_tagged(&case["action"]).unwrap();
            reader.for_action(&a).unwrap();
            let got = reader.normalize(&a);
            if let Some(refused) = case["refused"].as_str() {
                assert_eq!(got.unwrap_err().0, refused, "{a:?}");
            } else {
                let (action, notes) = got.unwrap();
                assert_eq!(action, V::from_tagged(&case["output"]).unwrap(), "{a:?}");
                assert_eq!(json!(notes), case["notes"], "{a:?}");
            }
        }
    }
    #[test]
    fn ordinary_counts_match_python_before_counter_predicates() {
        let c: J =
            serde_json::from_str(include_str!("../tests/fixtures/ordinary-reader.json")).unwrap();
        for case in c["counts"].as_array().unwrap() {
            let doc = V::from_tagged(&case["document"]).unwrap();
            let mut reader = Reader::new(&doc, None).unwrap();
            if let Some(layers) = case.get("layers") {
                let layers = V::from_tagged(layers).unwrap();
                let conflicts = case["conflicts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().into())
                    .collect();
                reader = reader
                    .with_layers(map(&layers).unwrap().clone(), conflicts)
                    .unwrap();
            }
            let builtins = reader
                .raw
                .iter()
                .filter(|(k, _)| F::BUILTINS.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            assert_eq!(
                V::Map(builtins),
                V::from_tagged(&case["builtins"]).unwrap(),
                "{}",
                case["name"]
            );
        }
    }
    #[test]
    fn ordinary_reads_without_formulas_need_no_program() {
        let raw = Map::from([("p.a".into(), V::Map(Map::from([("v".into(), s("1,234"))])))]);
        let ids = BTreeSet::from(["p.a".into()]);
        assert_eq!(
            evaluate(&s("p.a > 1000"), &raw, &ids, None).unwrap(),
            Some(true)
        );
        assert!(compute(&raw, &ids, None, None).get("error").is_some());
    }
}

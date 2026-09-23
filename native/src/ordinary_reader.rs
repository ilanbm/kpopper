//! Ordinary-reader/v1 values and authored syntax. Legacy coercion is preserved;
//! explicit formulas use only the separately supplied, verified ordinary Lean program.
use crate::ordinary_semantics as O;
pub(crate) use crate::ordinary_semantics::{CMP, EXPR, ID};
use crate::ordinary_value::Value as OV;
use crate::source_text::ordinary_python_str as py;
use crate::{
    Error, Result,
    history_contract::*,
    history_view::map_mut,
    ordinary_runtime::Program,
    reasoning_authoring::blocked_text,
    reasoning_authoring_guards::{self as G, Admission},
    reasoning_fields as F, reasoning_language as L,
    reasoning_runtime::Runtime,
    require,
    value::TypedValue as V,
};
use serde_json::{Value as J, json};
use std::collections::BTreeSet;
pub(crate) fn ordinary(value: &V) -> OV {
    OV::from_finite_projection(value)
}
pub(crate) fn ordinary_map(value: &Map) -> crate::ordinary_value::Map {
    match ordinary(&V::Map(value.clone())) {
        OV::Map(m) => m,
        _ => unreachable!(),
    }
}
fn s(v: &str) -> V {
    V::Text(v.into())
}
pub(crate) fn predicate_refs(v: &V) -> Vec<String> {
    O::predicate_refs(&ordinary(v))
}
pub(crate) fn reversal_pending(body: &V) -> Option<String> {
    O::reversal_pending(&ordinary(body))
}
pub(crate) fn predicate_of(body: &V, fields: &Map) -> V {
    O::predicate_of(&ordinary(body), &ordinary_map(fields))
        .finite_projection()
        .expect("finite predicate")
}
fn refs(v: &V) -> Vec<String> {
    L::legacy_references(v)
}
fn get<'a>(m: &'a Map, key: &str) -> &'a V {
    m.get(key).unwrap_or(&V::Null)
}
pub fn why_undecided(pred: &V) -> String {
    O::why_undecided(&ordinary(pred))
}
pub(crate) fn same_legacy(a: &V, b: &V) -> bool {
    O::same_legacy(&ordinary(a), &ordinary(b))
}
fn number(v: &V) -> Option<f64> {
    O::number(&ordinary(v))
}
pub(crate) fn json_value(v: &V, depth: usize) -> Result<J> {
    O::json_value(&ordinary(v), depth)
}
pub fn compute(
    raw: &Map,
    ids: &BTreeSet<String>,
    predicate: Option<&V>,
    program: Option<&Program>,
) -> J {
    O::compute(
        &ordinary_map(raw),
        ids,
        predicate.map(ordinary).as_ref(),
        program,
    )
}
pub fn value_of(
    raw: &Map,
    ids: &BTreeSet<String>,
    key: &str,
    program: Option<&Program>,
) -> Result<V> {
    O::value_of(&ordinary_map(raw), ids, key, program)?.finite_projection()
}
pub fn evaluate(
    pred: &V,
    raw: &Map,
    ids: &BTreeSet<String>,
    program: Option<&Program>,
) -> Result<Option<bool>> {
    O::evaluate(&ordinary(pred), &ordinary_map(raw), ids, program)
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
    // Rebuild from the current finite fields, rather than cache a second reader:
    // writer validation may change roles and report consumers may narrow IDs.
    pub(crate) fn ordinary(&self) -> O::Reader<'a> {
        O::Reader {
            document: ordinary(&self.document),
            fields: ordinary_map(&self.fields),
            raw: ordinary_map(&self.raw),
            ids: self.ids.clone(),
            hypotheses: ordinary_map(&self.hypotheses),
            knowledge_conflicts: self.knowledge_conflicts.clone(),
            runtime: self.runtime,
        }
    }
    fn from_ordinary(reader: O::Reader<'a>) -> Result<Self> {
        Ok(Self {
            document: reader.document.finite_projection()?,
            fields: map(&OV::Map(reader.fields).finite_projection()?)?.clone(),
            raw: map(&OV::Map(reader.raw).finite_projection()?)?.clone(),
            ids: reader.ids,
            hypotheses: map(&OV::Map(reader.hypotheses).finite_projection()?)?.clone(),
            knowledge_conflicts: reader.knowledge_conflicts,
            runtime: reader.runtime,
        })
    }
    pub(crate) fn for_followups(document: &V, runtime: Option<&'a Runtime>) -> Result<Self> {
        Self::from_ordinary(O::Reader::for_followups(&ordinary(document), runtime)?)
    }
    pub fn new(document: &V, runtime: Option<&'a Runtime>) -> Result<Self> {
        Self::from_ordinary(O::Reader::new(&ordinary(document), runtime)?)
    }
    pub fn with_layers(
        self,
        hypotheses: Map,
        knowledge_conflicts: BTreeSet<String>,
    ) -> Result<Self> {
        Self::from_ordinary(
            self.ordinary()
                .with_layers(ordinary_map(&hypotheses), knowledge_conflicts)?,
        )
    }
    pub(crate) fn program(&self) -> Option<&Program> {
        self.runtime.and_then(Runtime::ordinary_program)
    }
    pub fn fields(&self) -> &Map {
        &self.fields
    }
    /// The field a written judgment keeps what it saw in: the record's own, or
    /// `seen` while no judgment in the record carries one to infer it from.
    pub(crate) fn snapshot_field(&self) -> Result<&str> {
        match self.fields.get("snapshot") {
            Some(value) if crate::history_view::truth(value) => text(value),
            _ => Ok("seen"),
        }
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
            let members = map_mut(&mut doc)?
                .entry(collection)
                .or_insert_with(|| V::Map(Map::new()));
            // A bare collection header is YAML null until its first entry.
            if *members == V::Null {
                *members = V::Map(Map::new());
            }
            map_mut(members)?.insert(id.into(), body.clone());
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

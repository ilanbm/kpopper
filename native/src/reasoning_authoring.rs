//! Captured writer worlds. Admission preserves authored guards and delegates all
//! computation to Lean; candidate observations never grant publication authority.
use crate::{
    Error, Result,
    history_contract::*,
    history_view::{list, map_mut, truth},
    reasoning_assessment as A,
    reasoning_evaluate::Evaluator,
    reasoning_fields as F, reasoning_language as L, reasoning_query as Q,
    reasoning_runtime::{OperationalBounds, Runtime},
    reasoning_snapshot::{CaptureOptions, Snapshot, digest, entries},
    require,
    value::{Integer, TypedValue as V},
};
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, BTreeSet};
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn val(v: &J) -> Result<V> {
    V::from_json_bounded(v, 16 * 1024 * 1024 / 8)
}
fn strings(v: &V) -> Result<Vec<String>> {
    list(v)?
        .iter()
        .map(|v| text(v).map(str::to_owned))
        .collect()
}
pub(crate) fn py(v: &V) -> String {
    crate::source_text::python_str(&crate::history_yaml::SourceValue::from_typed(v))
}
pub(crate) fn named(body: &V) -> String {
    map(body)
        .ok()
        .and_then(|m| {
            ["name", "title", "label", "what", "desc"]
                .iter()
                .find_map(|k| m.get(*k).filter(|v| truth(v)))
        })
        .map(|v| py(v).trim().to_owned())
        .unwrap_or_default()
}
pub fn blocked_text(body: &V) -> String {
    let Ok(m) = map(body) else {
        return String::new();
    };
    for k in ["blocked_on", "unverified", "status"] {
        if let Some(v) = m.get(k).filter(|v| truth(v)) {
            if let V::Map(m) = v {
                let why = m
                    .iter()
                    .filter(|(k, _)| *k != "missing")
                    .filter_map(|(_, v)| text(v).ok())
                    .collect::<Vec<_>>()
                    .join(" ");
                if !why.is_empty() {
                    return why.trim().into();
                }
                let missing = match m.get("missing") {
                    Some(V::Text(v)) => vec![v.clone()],
                    Some(V::List(a)) => a.iter().map(py).collect(),
                    _ => vec![],
                };
                return format!("waiting on {}", missing.join(", ")).trim().into();
            }
            return py(v);
        }
    }
    String::new()
}
pub(crate) fn reopened_text(body: &V) -> String {
    map(body)
        .ok()
        .and_then(|m| m.get("reopened_by"))
        .filter(|v| truth(v))
        .map(py)
        .unwrap_or_default()
}
pub fn admitted(result: &J, blocked: bool) -> bool {
    let ds = result["diagnostics"].as_array();
    (result["status"] == "ok" && ds.is_none_or(|d| d.is_empty()))
        || blocked
            && ["ok", "unknown"].iter().any(|s| result["status"] == *s)
            && ds.is_some_and(|ds| {
                !ds.is_empty()
                    && ds.iter().all(|d| {
                        ["missing_reference", "missing_field"]
                            .iter()
                            .any(|s| d["code"] == *s)
                    })
            })
}
pub fn require_result(result: J, blocked: bool) -> Result<J> {
    if admitted(&result, blocked) {
        return Ok(result);
    }
    let reasons = result["diagnostics"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| d["code"].as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Err(Error(format!(
        "cannot compute core/v1 evidence: {}",
        if reasons.is_empty() {
            result["status"].as_str().unwrap_or("unknown")
        } else {
            &reasons
        }
    )))
}
pub(crate) fn query_expression(document: &V, rule: &V) -> Result<Option<J>> {
    let Ok(m) = map(rule) else { return Ok(None) };
    if m.len() != 1 || !m.contains_key("query") {
        return Ok(None);
    }
    let scope = map(&m["query"])?
        .get("scope")
        .and_then(|v| text(v).ok())
        .ok_or_else(|| Error("invalid query scope".into()))?;
    let known = entries(document)?;
    let fields = known
        .get(scope)
        .and_then(|(_, v)| map(v).ok())
        .and_then(|m| m.get("collection_scope"))
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("fields"))
        .map(strings)
        .transpose()?
        .ok_or_else(|| Error("invalid query scope".into()))?;
    Ok(Some(Q::lower(&rule.to_json()?, &fields)?))
}
pub fn declaration(document: &V) -> Result<V> {
    let declared = map(document)?
        .get("meta")
        .and_then(|v| map(v).ok())
        .is_some_and(|m| m.contains_key("reasoning"));
    let mut modules = BTreeSet::from(["arithmetic/v1".into()]);
    if declared {
        modules.extend(strings(
            &map(&F::capabilities(document, None)?)?["requires"],
        )?)
    }
    let fields = F::snapshot_fields(document)?;
    let predicate = text(&fields["predicate"])?;
    for (_, body) in entries(document)?.values() {
        let m = map(body).ok();
        let tree = if m
            .and_then(|m| m.get("rule"))
            .is_some_and(|v| !matches!(v, V::Map(_)))
        {
            None
        } else {
            query_expression(document, m.and_then(|m| m.get("rule")).unwrap_or(&V::Null))
                .ok()
                .flatten()
                .or_else(|| {
                    L::node_expression(&V::Map(Map::from([("body".into(), body.clone())]))).ok()
                })
        };
        if let Some(t) = tree {
            modules.extend(if t.get("query").is_some() {
                Q::required_modules(&t)
            } else {
                L::required_modules(&t)
            })
        }
        if let Some(v @ V::Map(_)) = m.and_then(|m| m.get(predicate))
            && let Ok(t) = L::lower(v)
        {
            modules.extend(L::required_modules(&t))
        }
    }
    val(&json!({"version":2,"profile":"core/v1","requires":modules}))
}
pub fn declare_document(document: &V) -> Result<V> {
    let d = declaration(document)?;
    let mut result = document.clone();
    let meta = map_mut(&mut result)?
        .entry("meta".into())
        .or_insert_with(|| V::Map(Map::new()));
    map_mut(meta)?.insert("reasoning".into(), d);
    Ok(result)
}
pub fn validate_declared(document: &V) -> Result<()> {
    let cap = F::capabilities(document, None)?;
    let cap = map(&cap)?;
    if string_is(&cap["profile"], "core/v1") {
        let declared = strings(&cap["requires"])?;
        let missing = strings(&map(&declaration(document)?)?["requires"])?
            .into_iter()
            .filter(|m| !declared.contains(m))
            .collect::<Vec<_>>();
        require(
            missing.is_empty(),
            &format!(
                "unsupported_capability: modules must be declared: {}",
                missing.join(", ")
            ),
        )?
    }
    Ok(())
}
pub fn selected(document: &V, requested: Option<&str>) -> Result<bool> {
    let cap = F::capabilities(document, None)?;
    require(
        requested.is_none_or(|s| s == "core/v1"),
        "unsupported writer profile",
    )?;
    Ok(requested == Some("core/v1") || string_is(&map(&cap)?["profile"], "core/v1"))
}
pub fn authored_value(value: &V) -> Result<V> {
    crate::reasoning_values::validate(value)?;
    fn decode(value: &V) -> Result<V> {
        let m = map(value)?;
        Ok(match text(&m["type"])? {
            "number" => {
                if string_is(&m["denominator"], "1") {
                    V::Integer(Integer::new(text(&m["numerator"])?)?)
                } else {
                    V::Map(Map::from([(
                        "rational".into(),
                        V::List(vec![m["numerator"].clone(), m["denominator"].clone()]),
                    )]))
                }
            }
            "list" => V::List(
                list(&m["items"])?
                    .iter()
                    .map(decode)
                    .collect::<Result<_>>()?,
            ),
            "record" => V::Map(
                map(&m["fields"])?
                    .iter()
                    .map(|(k, v)| Ok((k.clone(), decode(v)?)))
                    .collect::<Result<_>>()?,
            ),
            _ => m.get("value").cloned().unwrap_or(V::Null),
        })
    }
    decode(value)
}
pub struct World<'a> {
    pub(crate) snapshot: Snapshot,
    pub(crate) document: V,
    pub(crate) fields: Map,
    pub(crate) raw: BTreeMap<String, V>,
    pub(crate) runtime: Option<&'a Runtime>,
    pub(crate) bounds: OperationalBounds,
    readings: BTreeMap<String, J>,
    assessment: Option<V>,
    operation_assessment: bool,
}
impl<'a> World<'a> {
    pub(crate) fn snapshot_id(&self) -> &str {
        self.snapshot.snapshot_id()
    }
    pub(crate) fn seed_assessment(&mut self, report: &V) -> Result<()> {
        let report = crate::reasoning_history_assessment::validate_v2(&self.snapshot, report)?;
        for (id, node) in map(&map(&report)?["nodes"])? {
            let c = &map(node)?["computation"];
            if *c != V::Null {
                self.readings.insert(id.clone(), c.to_json()?);
            }
        }
        self.assessment = Some(report);
        self.operation_assessment = true;
        Ok(())
    }

    pub(crate) fn standing_predicate(&self, id: &str, expression: &V) -> Result<Option<bool>> {
        if self.operation_assessment {
            let report = self
                .assessment
                .as_ref()
                .ok_or_else(|| Error("operation assessment missing".into()))?;
            let node = map(&map(report)?["nodes"])?.get(id);
            return Ok(node
                .and_then(|v| map(v).ok())
                .and_then(|m| map(&m["state"]).ok())
                .and_then(|m| map(&m["falsifier"]).ok())
                .and_then(|m| text(&m["status"]).ok())
                .and_then(|s| match s {
                    "holds" => Some(true),
                    "does_not_hold" => Some(false),
                    _ => None,
                }));
        }
        self.predicate(expression, None)
    }
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    pub fn document(&self) -> &V {
        &self.document
    }
    pub fn fields(&self) -> &Map {
        &self.fields
    }
    pub fn raw(&self) -> &BTreeMap<String, V> {
        &self.raw
    }

    pub fn new(
        document: &V,
        original: Option<&Snapshot>,
        runtime: Option<&'a Runtime>,
        bounds: OperationalBounds,
    ) -> Result<Self> {
        let options = if let Some(s) = original {
            let data = map(s.data())?;
            CaptureOptions {
                context: Some(data["context"].clone()),
                hypotheses: Some(data["hypotheses"].clone()),
                as_of: Some(data["as_of"].clone()),
                ..Default::default()
            }
        } else {
            CaptureOptions::default()
        };
        let snapshot = Snapshot::from_data(document, options)?;
        let fields = F::snapshot_fields(document)?;
        F::capabilities(document, None)?;
        bounds.validate()?;
        let raw = entries(document)?
            .into_iter()
            .map(|(id, (_, body))| (id, body))
            .collect::<BTreeMap<_, _>>();
        for (id, body) in &raw {
            if let V::Map(m) = body {
                if let Some(V::Map(seen)) = m.get(text(&fields["snapshot"])?) {
                    for old in seen.values() {
                        if map(old)
                            .ok()
                            .and_then(|m| m.get("computed"))
                            .and_then(|v| map(v).ok())
                            .is_some_and(|m| m.contains_key("version"))
                            && let Some(e) = A::history(old, document)?.2
                        {
                            return Err(Error(format!(
                                "{e}: cannot write against unsupported core history"
                            )));
                        }
                    }
                }
                for key in ["rule", text(&fields["predicate"])?] {
                    if let Some(v @ V::Map(_)) = m.get(key) {
                        let checked = (|| {
                            if query_expression(document, v)?.is_none() {
                                Self::check_builtin(&L::references(&L::lower(v)?))?
                            }
                            Ok(())
                        })();
                        checked.map_err(|e: Error| Error(format!("{id}.{key}: {e}")))?
                    }
                }
            }
        }
        Ok(Self {
            snapshot,
            document: document.clone(),
            fields,
            raw,
            runtime,
            bounds,
            readings: BTreeMap::new(),
            assessment: None,
            operation_assessment: false,
        })
    }
    pub(crate) fn check_builtin(names: &[String]) -> Result<()> {
        let bad = names
            .iter()
            .filter(|s| F::BUILTINS.contains(&s.as_str()))
            .cloned()
            .collect::<BTreeSet<_>>();
        require(
            bad.is_empty(),
            &format!(
                "unsupported_core_builtin: {}",
                bad.into_iter().collect::<Vec<_>>().join(", ")
            ),
        )
    }
    pub fn result(&mut self, id: &str) -> Result<J> {
        Self::check_builtin(&[id.into()])?;
        if !self.readings.contains_key(id) {
            let mut e = Evaluator::new(&self.snapshot, self.runtime, None, self.bounds.clone())?;
            self.readings
                .insert(id.into(), A::dependency_result(&self.snapshot, id, &mut e)?);
        }
        Ok(self.readings[id].clone())
    }
    pub fn value(&mut self, id: &str) -> Result<V> {
        let r = self.result(id)?;
        if r["status"] == "operational_error" {
            require_result(r.clone(), false)?;
        }
        if r["status"] != "ok" {
            return Ok(V::Null);
        }
        authored_value(&val(&r["value"])?)
    }
    pub fn predicate(&self, expression: &V, declared: Option<&[String]>) -> Result<Option<bool>> {
        if !matches!(expression, V::Map(_)) {
            return Ok(None);
        }
        let tree = L::lower(expression)?;
        let refs = L::references(&tree);
        Self::check_builtin(&refs)?;
        let mut e = Evaluator::new(&self.snapshot, self.runtime, None, self.bounds.clone())?;
        let r = e.evaluate(val(&tree)?, declared.unwrap_or(&refs).to_vec())?;
        if r["status"] == "operational_error" {
            require_result(r.clone(), false)?;
        }
        Ok(if r["status"] == "ok" && r["value"]["type"] == "boolean" {
            r["value"]["value"].as_bool()
        } else {
            None
        })
    }
    pub fn same_value(&mut self, id: &str, candidate: &V) -> Result<bool> {
        let tree = L::literal(candidate)?;
        let mut doc = declare_document(&self.document)?;
        let meta = map_mut(map_mut(&mut doc)?.get_mut("meta").unwrap())?;
        let d = map_mut(meta.get_mut("reasoning").unwrap())?;
        let mut modules = strings(&d["requires"])?
            .into_iter()
            .collect::<BTreeSet<_>>();
        modules.extend(L::required_modules(&tree));
        d.insert(
            "requires".into(),
            V::List(modules.into_iter().map(V::Text).collect()),
        );
        let world = Self::new(
            &doc,
            Some(&self.snapshot),
            self.runtime,
            self.bounds.clone(),
        )?;
        let mut engine = Evaluator::new(&world.snapshot, self.runtime, None, self.bounds.clone())?;
        let proposed = require_result(engine.evaluate(val(&tree)?, vec![])?, false)?;
        let current = require_result(self.result(id)?, false)?;
        Ok(current["value"] == proposed["value"])
    }
    pub fn candidate(&self, action: &V) -> Result<Self> {
        let a = map(action)?;
        let kind = text(field(a, "kind")?)?;
        let id = text(field(a, "id")?)?;
        let mut doc = self.document.clone();
        let existing = entries(&doc)?;
        if kind == "add" {
            let body = field(a, "body")?;
            let collection = if let Some((c, _)) = existing.get(id) {
                c.clone()
            } else {
                crate::reasoning_authoring_guards::collection_for(
                    self,
                    id,
                    body,
                    a.get("into").and_then(|v| text(v).ok()),
                )?
            };
            let members = map_mut(&mut doc)?
                .entry(collection)
                .or_insert_with(|| V::Map(Map::new()));
            map_mut(members)?.insert(id.into(), body.clone());
        } else if kind == "set" && existing.contains_key(id) {
            let (collection, body) = &existing[id];
            let mut body = body.clone();
            let m = map_mut(&mut body)?;
            let key = if m.contains_key("v") {
                "v"
            } else if m.contains_key("quoted") {
                "quoted"
            } else {
                return Err(Error("set needs an entry with v or quoted".into()));
            };
            m.insert(key.into(), field(a, "value")?.clone());
            let day = a
                .get("as_of")
                .filter(|v| truth(v))
                .cloned()
                .ok_or_else(|| Error("set requires an explicitly captured day".into()))?;
            m.insert("of".into(), day);
            if let Some(source) = a.get("source").filter(|v| **v != V::Null) {
                let at = a
                    .get("at")
                    .filter(|v| **v != V::Null)
                    .ok_or_else(|| Error("a changed source needs its exact at location".into()))?;
                m.insert("from".into(), source.clone());
                m.insert("at".into(), at.clone());
            }
            if let Some(scope) = a.get("_record_scope") {
                m.insert("scope".into(), scope.clone());
            }
            map_mut(map_mut(&mut doc)?.get_mut(collection).unwrap())?.insert(id.into(), body);
        }
        Self::new(
            &declare_document(&doc)?,
            Some(&self.snapshot),
            self.runtime,
            self.bounds.clone(),
        )
    }
    pub fn history(&mut self, id: &str) -> Result<V> {
        Self::check_builtin(&[id.into()])?;
        let body = self.raw.get(id).cloned().unwrap_or(V::Null);
        if let Ok(m) = map(&body) {
            if m.contains_key(text(&self.fields["deps"])?) {
                return Ok(s(&m
                    .get("verdict")
                    .filter(|v| truth(v))
                    .or_else(|| m.get("title").filter(|v| truth(v)))
                    .map(py)
                    .unwrap_or_else(|| id.into())));
            }
            if !["v", "quoted", "rule", "collection_scope"]
                .iter()
                .any(|k| m.contains_key(*k))
            {
                let when = m
                    .get("read")
                    .filter(|v| truth(v))
                    .or_else(|| m.get("of").filter(|v| truth(v)));
                return Ok(s(&when.map(|v| format!("read {}", py(v))).unwrap_or_else(
                    || {
                        let n = named(&body);
                        if n.is_empty() { "present".into() } else { n }
                    },
                )));
            }
            if let Some(V::Text(v)) = m.get("rule") {
                return Ok(s(v));
            }
        }
        let result = require_result(self.result(id)?, false)?;
        let mut h = Map::from([
            ("version".into(), V::Integer(Integer::new("2")?)),
            ("value".into(), val(&result["value"])?),
            ("basis".into(), val(&result["basis"])?),
        ]);
        if let Ok(m) = map(&body)
            && let Some(v) = m.get(if m.contains_key("collection_scope") {
                "collection_scope"
            } else {
                "rule"
            })
        {
            h.insert("rule".into(), v.clone());
        }
        Ok(V::Map(Map::from([("computed".into(), V::Map(h))])))
    }
    pub fn assessment(&mut self) -> Result<V> {
        if self.assessment.is_none() {
            let report = A::assess(
                &self.snapshot,
                None,
                "focused-review/v1",
                self.runtime,
                self.bounds.clone(),
            )?;
            for node in map(&map(&report)?["nodes"])?.values() {
                let node = map(node)?;
                let state = map(&node["state"])?;
                let mut results = vec![&node["computation"]];
                if let Some(c) = map(&state["falsifier"])?.get("computation") {
                    results.push(c)
                }
                for dep in map(&map(&state["basis"])?["dependencies"])?.values() {
                    results.push(&map(dep)?["computation"])
                }
                for r in results {
                    if map(r)
                        .ok()
                        .and_then(|m| m.get("status"))
                        .is_some_and(|v| string_is(v, "operational_error"))
                    {
                        require_result(r.to_json()?, false)?;
                    }
                }
            }
            self.assessment = Some(report)
        }
        Ok(self.assessment.clone().unwrap())
    }
    pub fn state(&mut self, id: &str) -> Result<V> {
        let report = self.assessment()?;
        let node = map(field(map(&map(&report)?["nodes"])?, id)?)?;
        let state = map(&node["state"])?;
        let pair = if string_is(&map(&state["falsifier"])?["status"], "holds") {
            ("FIRED", "wrong_if holds under core/v1".into())
        } else {
            let issues = list(&map(&state["integrity"])?["issues"])?;
            if !issues.is_empty() {
                (
                    "UNKNOWN",
                    issues
                        .iter()
                        .map(|v| text(&map(v).unwrap()["code"]).unwrap().to_owned())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect::<Vec<_>>()
                        .join(", "),
                )
            } else if map(&map(&state["basis"])?["dependencies"])?
                .values()
                .any(|d| {
                    let d = map(d).unwrap();
                    string_is(&d["comparison"], "changed")
                        || truth(&d["rule_changed"])
                        || string_is(&d["basis_comparison"], "changed")
                })
            {
                (
                    "MOVED",
                    "dependency value, rule or computational basis changed since review".into(),
                )
            } else if truth(&node["attention"]) {
                ("REVIEW", "core/v1 assessment requests review".into())
            } else {
                ("HOLDS", "core/v1 assessment has no review finding".into())
            }
        };
        Ok(V::List(vec![s(pair.0), s(&pair.1)]))
    }
    pub fn normalize(
        &self,
        action: &V,
        previous: Option<&BTreeMap<String, V>>,
    ) -> Result<(V, Vec<String>)> {
        crate::reasoning_authoring_guards::normalize(self, action, previous)
    }
    pub fn validate(&mut self, action: &V) -> Result<Vec<String>> {
        let mut out = crate::reasoning_authoring_guards::validate(self, action)?;
        let a = map(action)?;
        let kind = text(field(a, "kind")?)?;
        let id = text(field(a, "id")?)?;
        let empty = V::Map(Map::new());
        let body = a.get("body").unwrap_or(&empty);
        let b = map(body).ok();
        let deps = text(&self.fields["deps"])?.to_owned();
        let predicate = text(&self.fields["predicate"])?.to_owned();
        if let Some(V::List(d)) = b.and_then(|m| m.get(&deps)) {
            Self::check_builtin(
                &d.iter()
                    .filter_map(|v| text(v).ok().map(str::to_owned))
                    .collect::<Vec<_>>(),
            )?
        }
        let stored = kind == "set"
            || kind == "add" && b.is_none_or(|m| m.contains_key("v") || m.contains_key("quoted"));
        if stored {
            let r = self
                .candidate(action)
                .and_then(|mut c| require_result(c.result(id)?, !blocked_text(body).is_empty()));
            if let Err(e) = r {
                if e.0.starts_with("cannot compute core/v1 evidence:")
                    || e.0.starts_with("unsupported_core_builtin:")
                {
                    return Err(e);
                }
                out.push(format!("value: {e}"))
            }
        }
        if kind == "add"
            && let Some(b) = b
        {
            if b.contains_key(&deps) && blocked_text(body).is_empty() {
                let p = b.get(&predicate).unwrap_or(&V::Null);
                if !matches!(p, V::Map(_))
                    && (*p != V::Null && *p != s("") || reopened_text(body).is_empty())
                {
                    out.push("condition cannot be computed under core/v1; choose explicit typed operands or declare the unavailable condition with blocked_on".into());
                }
            }
            let candidate = self.candidate(action)?;
            for (field, predicate) in [("rule", false), (predicate.as_str(), true)] {
                let Some(expression @ V::Map(_)) = b.get(field) else {
                    continue;
                };
                let r = (|| -> Result<()> {
                    let query = query_expression(&candidate.document, expression)?;
                    let tree = if let Some(t) = &query {
                        t.clone()
                    } else {
                        L::lower(expression)?
                    };
                    Self::check_builtin(&if query.is_some() {
                        vec![]
                    } else {
                        L::references(&tree)
                    })?;
                    if !predicate && (b.contains_key("v") || b.contains_key("quoted")) {
                        out.push("a structured rule cannot also store v or quoted".into());
                    }
                    let mut engine = Evaluator::new(
                        &candidate.snapshot,
                        self.runtime,
                        None,
                        self.bounds.clone(),
                    )?;
                    let declared = if predicate {
                        b.get(&deps)
                            .map(strings)
                            .transpose()
                            .map_err(|_| {
                                Error("cannot compute core/v1 evidence: invalid_expression".into())
                            })?
                            .unwrap_or_default()
                    } else {
                        vec![id.into()]
                    };
                    let result = require_result(
                        engine.evaluate(
                            val(&if predicate { tree } else { json!({"ref":id}) })?,
                            declared,
                        )?,
                        !blocked_text(body).is_empty(),
                    )?;
                    if predicate && result["status"] == "ok" {
                        if result["value"]["type"] != "boolean" {
                            out.push("predicate must compute a boolean".into())
                        } else if result["value"]["value"] == true {
                            out.push(
                                "wrong_if already holds - the judgment would be born broken".into(),
                            )
                        }
                    }
                    Ok(())
                })();
                if let Err(e) = r {
                    if e.0.starts_with("cannot compute core/v1 evidence:")
                        || e.0.starts_with("unsupported_core_builtin:")
                    {
                        return Err(e);
                    }
                    out.push(format!("{field}: {e}"))
                }
            }
        }
        Ok(out)
    }
}

pub fn same(left: &V, right: &V) -> Result<bool> {
    fn number(v: &V) -> Option<(num_bigint::BigInt, num_bigint::BigInt)> {
        let parsed = match v {
            V::Integer(_) | V::Float(_) => crate::reasoning_scope::query_value(v, 0).ok(),
            V::Map(m) if m.len() == 1 && m.contains_key("rational") => {
                let parts = list(&m["rational"]).ok()?;
                if parts.len() != 2
                    || !parts
                        .iter()
                        .all(|v| matches!(v, V::Text(_) | V::Integer(_)))
                {
                    return None;
                }
                let n = python_integer(&py(&parts[0]))?;
                let d = python_integer(&py(&parts[1]))?;
                return (d != num_bigint::BigInt::from(0)).then_some((n, d));
            }
            _ => None,
        }?;
        let m = map(&parsed).ok()?;
        Some((
            text(m.get("numerator")?).ok()?.parse().ok()?,
            text(m.get("denominator")?).ok()?.parse().ok()?,
        ))
    }
    match (number(left), number(right)) {
        (Some((a, b)), Some((c, d))) => Ok(a * d == c * b),
        _ => Ok(digest(left)? == digest(right)?),
    }
}
pub(crate) fn claim_key(v: &V) -> Result<String> {
    fn part(v: String) -> String {
        format!("{}:{v}", v.chars().count())
    }
    Ok(match v {
        V::Map(m) => {
            let mut parts = m
                .iter()
                .map(|(k, v)| Ok((claim_key(&s(k))?, claim_key(v)?)))
                .collect::<Result<Vec<_>>>()?;
            parts.sort();
            format!(
                "{{{}}}",
                parts
                    .into_iter()
                    .map(|(a, b)| part(a) + &part(b))
                    .collect::<String>()
            )
        }
        V::List(a) => format!(
            "[{}]",
            a.iter()
                .map(|v| claim_key(v).map(part))
                .collect::<Result<Vec<_>>>()?
                .join("")
        ),
        _ => {
            let source = py(v)
                .split(python_space)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let cleaned = decimal_text(&source.replace(',', ""));
            let special = regex::Regex::new(r"(?i)^([+-]?)(inf(?:inity)?|s?nan([0-9]*))$").unwrap();
            if let Some(c) = special.captures(&cleaned) {
                let word = c[2].to_ascii_lowercase();
                let value = if word.starts_with("inf") {
                    "Infinity".into()
                } else {
                    format!(
                        "{}{}",
                        if word.starts_with('s') { "sNaN" } else { "NaN" },
                        c.get(3)
                            .map(|m| m.as_str())
                            .unwrap_or("")
                            .trim_start_matches('0')
                    )
                };
                return Ok(format!("n{}{value}", if &c[1] == "-" { "-" } else { "" }));
            }
            let numeric = regex::Regex::new(
                r"^([+-]?)(?:([0-9]+)(?:\.([0-9]*))?|\.([0-9]+))(?:[eE]([+-]?[0-9]+))?$",
            )
            .unwrap();
            if let Some(c) = numeric.captures(&cleaned) {
                let fraction = c
                    .get(3)
                    .or_else(|| c.get(4))
                    .map(|m| m.as_str())
                    .unwrap_or("");
                let whole = c.get(2).map(|m| m.as_str()).unwrap_or("");
                let mut digits = format!("{whole}{fraction}")
                    .trim_start_matches('0')
                    .to_owned();
                if digits.is_empty() {
                    "n0".into()
                } else {
                    let mut exp = c
                        .get(5)
                        .map(|m| m.as_str())
                        .unwrap_or("0")
                        .parse::<num_bigint::BigInt>()
                        .map_err(|_| Error("invalid claim exponent".into()))?
                        - num_bigint::BigInt::from(fraction.len());
                    while digits.ends_with('0') {
                        digits.pop();
                        exp += 1;
                    }
                    format!("n{}{digits}e{exp}", if &c[1] == "-" { "-" } else { "" })
                }
            } else {
                format!("t{source}")
            }
        }
    })
}

// Unicode decimal digits used by Python 3.14.5.
const DECIMAL_ZEROES: &[u32] = &[
    0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66,
    0xde6, 0xe50, 0xed0, 0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90,
    0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0, 0xff10,
    0x104a0, 0x10d30, 0x10d40, 0x11066, 0x110f0, 0x11136, 0x111d0, 0x112f0, 0x11450, 0x114d0,
    0x11650, 0x116c0, 0x116d0, 0x116da, 0x11730, 0x118e0, 0x11950, 0x11bf0, 0x11c50, 0x11d50,
    0x11da0, 0x11f50, 0x16130, 0x16a60, 0x16ac0, 0x16b50, 0x16d70, 0x1ccf0, 0x1d7ce, 0x1d7d8,
    0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0, 0x1e5f1, 0x1e950, 0x1fbf0,
];
fn decimal_digits(s: &str) -> String {
    s.chars()
        .map(|c| {
            DECIMAL_ZEROES
                .iter()
                .find(|&&z| (z..z + 10).contains(&(c as u32)))
                .map(|z| char::from(b'0' + (c as u32 - z) as u8))
                .unwrap_or(c)
        })
        .collect()
}
fn decimal_text(s: &str) -> String {
    decimal_digits(s).replace('_', "")
}
fn python_integer(s: &str) -> Option<num_bigint::BigInt> {
    let s = decimal_digits(s.trim_matches(python_space));
    let valid = regex::Regex::new(r"^[+-]?[0-9](?:_?[0-9])*$").unwrap();
    if !valid.is_match(&s) {
        return None;
    }
    s.replace('_', "").parse().ok()
}
fn python_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    #[test]
    fn writer_identity_and_numeric_equality_match_python() {
        let data: J = serde_json::from_str(include_str!(
            "../tests/fixtures/reasoning-authoring-identities.json"
        ))
        .unwrap();
        for c in data["claim_keys"].as_array().unwrap() {
            let value = V::from_tagged(&c["value"]).unwrap();
            assert_eq!(
                claim_key(&value).unwrap(),
                c["key"].as_str().unwrap(),
                "{value:?}"
            );
        }
        for c in data["same"].as_array().unwrap() {
            let a = V::from_tagged(&c["left"]).unwrap();
            let b = V::from_tagged(&c["right"]).unwrap();
            assert_eq!(
                same(&a, &b).unwrap(),
                c["same"].as_bool().unwrap(),
                "{a:?} {b:?}"
            );
        }
    }
}

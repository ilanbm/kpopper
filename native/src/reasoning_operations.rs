//! Prospective operation observations. Derivation retains the captured source
//! context; live publication still requires the prepared-operation authority gate.
use crate::{
    Error, Result,
    history_contract::*,
    history_view::{list, map_mut, truth},
    reasoning_authoring::{self as A, World},
    reasoning_context::CapturedAssessment,
    reasoning_evaluate::Evaluator,
    reasoning_fields, reasoning_language as L,
    reasoning_runtime::{OperationalBounds, Runtime},
    reasoning_snapshot::{CaptureOptions, Snapshot, digest},
    require,
    value::TypedValue as V,
};
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, BTreeSet};
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn val(j: J) -> Result<V> {
    V::from_json_bounded(&j, 64 * 1024 * 1024 / 8)
}
fn names(v: &V) -> Result<Vec<String>> {
    list(v)?
        .iter()
        .map(|v| text(v).map(str::to_owned))
        .collect()
}
pub fn selected(document: &V) -> Result<bool> {
    Ok(map(&reasoning_fields::capabilities(document, None)?)?["profile"] == s("core/v1"))
}
pub fn require_ordinary(snapshot: &Snapshot) -> Result<()> {
    let d = map(snapshot.data())?;
    let context = map(&d["context"])?;
    let meta = map(&d["document"])?.get("meta").and_then(|v| map(v).ok());
    let named = map(&d["hypotheses"])?.values().any(|h| {
        map(h)
            .ok()
            .and_then(|m| m.get("kind"))
            .is_some_and(|v| string_is(v, "named-history-hypothesis/v1"))
    });
    require(
        !context.contains_key("history")
            && !meta.is_some_and(|m| m.contains_key("history"))
            && !named,
        "prospective_history_required: use the prepared history operation",
    )
}
#[derive(Clone, Debug)]
pub struct OperationDocument {
    snapshot: Snapshot,
}
impl OperationDocument {
    pub fn bind(document: &V, snapshot: Snapshot) -> Result<Self> {
        require(
            digest(document)? == digest(&map(snapshot.data())?["document"])?,
            "operation_document_changed: derive a new prospective snapshot",
        )?;
        Ok(Self { snapshot })
    }
    pub fn from_snapshot(snapshot: Snapshot, allow_history: bool) -> Result<Self> {
        require(
            selected(&map(snapshot.data())?["document"])?,
            "record_profile_changed: retry the operational read",
        )?;
        if !allow_history {
            require_ordinary(&snapshot)?;
        }
        Ok(Self { snapshot })
    }
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    pub fn document(&self) -> &V {
        &map(self.snapshot.data()).unwrap()["document"]
    }
    pub fn derive(&self, document: &V, selection: &[String], proposals: &[V]) -> Result<Self> {
        require_ordinary(&self.snapshot)?;
        let prior = map(self.snapshot.data())?;
        let hypotheses = map(&prior["hypotheses"])?;
        for name in selection {
            if hypotheses
                .get(name)
                .and_then(|v| map(v).ok())
                .and_then(|m| m.get("kind"))
                .is_some_and(|v| string_is(v, "contribution"))
            {
                return Err(Error(
                    "pending_publication_required: a contribution is not an ordinary fold".into(),
                ));
            }
        }
        let mut context = prior["context"].clone();
        let c = map_mut(&mut context)?;
        c.insert("read_mode".into(), s("supplied"));
        let mut inputs = vec![];
        for p in proposals {
            let p = map(p)?;
            inputs.push(V::Map(
                [
                    ("name", field(p, "name")?),
                    ("document", field(p, "doc")?),
                    ("head", field(p, "head")?),
                ]
                .into_iter()
                .map(|(k, v)| (k.into(), v.clone()))
                .collect(),
            ));
        }
        let mut op = val(
            json!({"version":1,"phase":"prospective","kind":"hypothesis_union","base_snapshot":self.snapshot.snapshot_id(),"selection":selection.iter().collect::<BTreeSet<_>>(),"inputs":[]}),
        )?;
        map_mut(&mut op)?.insert("inputs".into(), V::List(inputs));
        c.insert("operation".into(), op);
        let remaining = V::Map(
            hypotheses
                .iter()
                .filter(|(k, _)| !selection.contains(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        );
        let mut document = A::declare_document(document)?;
        let mut required = names(&map(&map(&map(&document)?["meta"])?["reasoning"])?["requires"])?
            .into_iter()
            .collect::<BTreeSet<_>>();
        for p in proposals {
            let doc = field(map(p)?, "doc")?;
            if selected(doc)? {
                required.extend(names(
                    &map(&reasoning_fields::capabilities(doc, None)?)?["requires"],
                )?);
            }
        }
        let r = map_mut(
            map_mut(map_mut(&mut document)?.get_mut("meta").unwrap())?
                .get_mut("reasoning")
                .unwrap(),
        )?;
        r.insert(
            "requires".into(),
            V::List(required.into_iter().map(V::Text).collect()),
        );
        let snapshot = Snapshot::from_data(
            &document,
            CaptureOptions {
                context: Some(context),
                hypotheses: Some(remaining),
                as_of: Some(prior["as_of"].clone()),
                ..Default::default()
            },
        )?;
        Self::bind(&document, snapshot)
    }
}
pub struct OperationWorld<'a> {
    world: World<'a>,
    context: CapturedAssessment,
}
impl<'a> OperationWorld<'a> {
    pub fn new(
        document: &OperationDocument,
        runtime: Option<&'a Runtime>,
        bounds: OperationalBounds,
    ) -> Result<Self> {
        let snapshot = document.snapshot();
        let mut world = World::new(document.document(), Some(snapshot), runtime, bounds.clone())?;
        require(
            world.snapshot_id() == snapshot.snapshot_id(),
            "operation_snapshot_mismatch",
        )?;
        let context = CapturedAssessment::from_snapshot(
            snapshot.clone(),
            None,
            "focused-review/v1",
            runtime,
            bounds,
            None,
        )?;
        world.seed_assessment(context.base_assessment())?;
        Ok(Self { world, context })
    }
    pub fn world(&self) -> &World<'a> {
        &self.world
    }
    pub fn result(&mut self, id: &str) -> Result<J> {
        self.world.result(id)
    }
    pub fn validate(&mut self, action: &V) -> Result<Vec<String>> {
        self.world.validate(action)
    }
    pub fn context(&self) -> &CapturedAssessment {
        &self.context
    }
    pub fn predicate_for(&self, id: &str) -> Option<bool> {
        map(self.context.base_assessment())
            .ok()
            .and_then(|r| map(&r["nodes"]).ok())
            .and_then(|m| m.get(id))
            .and_then(|n| map(n).ok())
            .and_then(|m| map(&m["state"]).ok())
            .and_then(|m| map(&m["falsifier"]).ok())
            .and_then(|m| text(&m["status"]).ok())
            .and_then(|s| match s {
                "holds" => Some(true),
                "does_not_hold" => Some(false),
                _ => None,
            })
    }
    pub fn condition(&self, expression: &V) -> Result<(Option<bool>, Option<J>)> {
        if !matches!(expression, V::Map(_)) {
            return Ok((None, None));
        }
        let tree = L::lower(expression)?;
        let refs = L::references(&tree);
        let mut engine = Evaluator::new(
            self.context.snapshot(),
            self.world.runtime,
            None,
            self.world.bounds.clone(),
        )?;
        let result = engine.evaluate(val(tree)?, refs)?;
        let truth = if result["status"] == "ok" && result["value"]["type"] == "boolean" {
            result["value"]["value"].as_bool()
        } else {
            None
        };
        Ok((truth, Some(result)))
    }
    pub fn check(&self) -> Result<(Vec<String>, Vec<String>)> {
        let f = findings(&self.context)?;
        let m = map(&f)?;
        let mut problems = names(&m["falsified"])?;
        problems.extend(names(&m["holes"])?);
        Ok((problems, names(&m["moved"])?))
    }
}
pub fn findings(context: &CapturedAssessment) -> Result<V> {
    let report = map(context.assessment())?;
    let mut falsified = vec![];
    let mut holes = vec![];
    let mut moved = vec![];
    let mut notes = vec![];
    let mut uncertain = BTreeMap::<String, BTreeSet<String>>::new();
    let history = map(&report["history"])?;
    if history["authority_status"] == s("active")
        && (!truth(&map(&history["coverage"])?["complete"])
            || !truth(&map(&history["integrity"])?["complete"]))
    {
        holes.push("history: incomplete captured coverage or integrity".into());
    }
    for (id, node) in map(&report["nodes"])? {
        let n = map(node)?;
        let state = map(&n["state"])?;
        let blocked = !A::blocked_text(&n["body"]).is_empty();
        let mut u = BTreeSet::new();
        let dependencies = map(&map(&state["basis"])?["dependencies"])?;
        let missing = dependencies
            .iter()
            .filter_map(|(d, v)| {
                let m = map(v).ok()?;
                let c = map(&m["current"]).ok()?;
                (c["status"] != s("ok")).then_some(d.clone())
            })
            .collect::<BTreeSet<_>>();
        for issue in list(&map(&state["integrity"])?["issues"])? {
            let issue = map(issue)?;
            let code = text(&issue["code"])?;
            let allowed = blocked
                && (["missing_dependency", "missing_reference", "missing_field"].contains(&code)
                    || code == "missing_snapshot"
                        && issue
                            .get("related_ids")
                            .map(names)
                            .transpose()?
                            .unwrap_or_default()
                            .iter()
                            .all(|d| missing.contains(d)));
            (if allowed { &mut notes } else { &mut holes }).push(format!("{id}: {code}"));
            u.insert(code.into());
        }
        if !truth(&map(&n["coverage"])?["complete"]) {
            holes.push(format!("{id}: incomplete history coverage"));
        }
        if map(&state["contention"])?["status"] == s("detected") {
            holes.push(format!("{id}: contested captured alternatives"));
        }
        let condition = map(&state["falsifier"])?;
        let status = text(&condition["status"])?;
        if status == "holds" {
            falsified.push(format!("{id}: wrong_if holds under core/v1"));
        } else if ["unknown", "error"].contains(&status) {
            let c = condition.get("computation").unwrap_or(&V::Null);
            let allowed = (*c == V::Null
                && !matches!(condition.get("expression"), Some(V::Map(_))))
                || *c != V::Null && blocked && A::admitted(&c.to_json()?, true);
            (if allowed { &mut notes } else { &mut holes })
                .push(format!("{id}: falsifier {status}"));
            u.insert(format!("falsifier {status}"));
        }
        if let Ok(c) = map(&n["computation"]) {
            let status = text(&c["status"])?;
            if !["ok", "unknown"].contains(&status) {
                holes.push(format!("{id}: computation {status}"))
            } else if status == "unknown"
                && map(&n["body"])
                    .ok()
                    .and_then(|m| m.get("rule"))
                    .is_some_and(|v| matches!(v, V::Map(_)))
            {
                (if blocked && A::admitted(&n["computation"].to_json()?, true) {
                    &mut notes
                } else {
                    &mut holes
                })
                .push(format!("{id}: computation unknown"));
                u.insert("computation unknown".into());
            }
        }
        for (d, reading) in dependencies {
            let r = map(reading)?;
            if r["comparison"] == s("changed")
                || truth(&r["rule_changed"])
                || r["basis_comparison"] == s("changed")
            {
                let mut evidence = Map::new();
                for k in [
                    "current",
                    "at_review",
                    "comparison",
                    "rule_changed",
                    "basis_comparison",
                ] {
                    evidence.insert(k.into(), r[k].clone());
                }
                evidence.insert("basis".into(), map(&r["computation"])?["basis"].clone());
                moved.push(format!(
                    "{id}: {d} value/rule/basis changed [{}]",
                    &digest(&V::Map(evidence))?[..16]
                ));
            }
        }
        if map(&n["support"])?["status"] == s("reserved") {
            notes.push(format!("{id}: declared support is reserved"));
            u.insert("support reserved".into());
        }
        if !u.is_empty() {
            uncertain.insert(id.clone(), u);
        }
    }
    let canonical = |v: Vec<String>| v.into_iter().collect::<BTreeSet<_>>();
    val(
        json!({"falsified":canonical(falsified),"holes":canonical(holes),"moved":canonical(moved),"notes":canonical(notes),"uncertain":uncertain}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standing_predicate_uses_the_captured_assessment_without_a_second_runtime_call() {
        let data: J =
            serde_json::from_str(include_str!("../tests/fixtures/reasoning-context.json")).unwrap();
        let context = CapturedAssessment::from_data(&data["contexts"][0]["serialized"]).unwrap();
        let doc = &map(context.snapshot().data()).unwrap()["document"];
        let mut world = World::new(
            doc,
            Some(context.snapshot()),
            None,
            OperationalBounds::default(),
        )
        .unwrap();
        let expression = V::from_json(&json!({"expr":"p.b > 2"})).unwrap();
        assert!(world.standing_predicate("d.a", &expression).is_err());
        world.seed_assessment(context.base_assessment()).unwrap();
        assert_eq!(
            world.standing_predicate("d.a", &expression).unwrap(),
            Some(true)
        );
    }
}

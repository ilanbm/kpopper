//! Computational projection from a fully verified node history. Only witnesses expand `saw`.
use crate::{
    Result, history_contract::*, history_node_capture::Capture,
    history_projection::CapturedHistory, history_view::list, require, value::TypedValue as V,
};
use std::collections::BTreeSet;
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn object(v: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(v.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn strings(v: impl IntoIterator<Item = String>) -> V {
    V::List(v.into_iter().map(V::Text).collect())
}

pub fn capture(c: &Capture) -> Result<CapturedHistory> {
    let states = map(&map(c.state())?["subjects"])?;
    let mut subjects = Map::new();
    let mut heads = Map::new();
    let mut open = Map::new();
    let mut dispositions = Map::new();
    let mut wanted = BTreeSet::new();
    let mut reviews = states
        .keys()
        .map(|k| (k.clone(), Vec::new()))
        .collect::<std::collections::BTreeMap<_, _>>();
    for (id, value) in c.history.objects() {
        require(
            !crate::history_authority::has_temporal_metadata(value)?,
            "node_temporal_projection_unsupported",
        )?;
        let o = map(value)?;
        if !string_is(&o["kind"], "act") {
            continue;
        }
        let body = map(&o["body"])?;
        let subject = text(&o["subject"])?;
        let state = map(states
            .get(subject)
            .ok_or_else(|| error("invalid_review_scope"))?)?;
        if string_is(&body["act"], "review")
            && list(&state["heads"])?
                .iter()
                .chain(list(&state["proposals"])?)
                .any(|v| v == &body["of"])
        {
            wanted.insert(id.clone());
            reviews
                .get_mut(subject)
                .unwrap()
                .push(c.object(subject, id)?);
        }
    }
    for (subject, value) in states {
        let state = map(value)?;
        heads.insert(subject.clone(), state["heads"].clone());
        let acts = strings(
            map(&state["open_acts"])?
                .values()
                .map(list)
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .map(|v| text(v).map(str::to_owned))
                .collect::<Result<BTreeSet<_>>>()?,
        );
        open.insert(subject.clone(), acts.clone());
        subjects.insert(
            subject.clone(),
            object([
                ("acceptance", state["acceptance"].clone()),
                ("heads", state["heads"].clone()),
                ("open_acts", acts.clone()),
            ]),
        );
        dispositions.insert(
            subject.clone(),
            object([
                ("marks", state["marks"].clone()),
                ("proposals", state["proposals"].clone()),
                ("contested_claims", state["disputed_acts"].clone()),
                ("reviews", V::List(reviews.remove(subject).unwrap())),
                ("implied", state["implied"].clone()),
            ]),
        );
        for id in list(&state["heads"])?
            .iter()
            .chain(list(&state["proposals"])?)
            .chain(list(&acts)?)
        {
            wanted.insert(text(id)?.into());
        }
    }
    let mut objects = Map::new();
    let mut pins = Map::new();
    let mut findings = Vec::new();
    while let Some(id) = wanted.pop_first() {
        if objects.contains_key(&id) {
            continue;
        }
        let compact = c
            .history
            .objects()
            .get(&id)
            .ok_or_else(|| error("incomplete_closure"))?;
        let subject = text(&map(compact)?["subject"])?;
        let value = c.object(subject, &id)?;
        let o = map(&value)?;
        if !string_is(&o["kind"], "act") {
            for reference in map(&o["pins"])?.values() {
                wanted.insert(text(reference)?.into());
            }
            for finding in crate::history_projection::pin_gap_findings(&value)? {
                if !findings.contains(&finding) {
                    findings.push(finding);
                }
            }
            pins.insert(
                id.clone(),
                object([
                    ("subject", s(subject)),
                    ("version", s(&id)),
                    ("status", s("recorded")),
                    ("object", value.clone()),
                ]),
            );
        } else if let Some(read) = map(&o["body"])?.get("read") {
            for reference in map(read)?.values() {
                wanted.insert(text(reference)?.into());
            }
        }
        objects.insert(id, value);
    }
    let authority = &c.snapshot.authority;
    crate::history_node_publication::validate_authority(authority)?;
    let baseline = object([
        ("version", V::from_json(&serde_json::json!(1))?),
        ("record_id", map(authority)?["record_id"].clone()),
        (
            "authority_generation",
            map(authority)?["generation"].clone(),
        ),
        (
            "committed_set_digest",
            s(&V::Map(
                c.snapshot
                    .transactions
                    .iter()
                    .map(|(op, tx)| (op.clone(), s(&tx.digest)))
                    .collect(),
            )
            .digest()?),
        ),
        ("heads", V::Map(heads)),
        ("open_acts", V::Map(open)),
    ]);
    let complete = findings.is_empty();
    let mut projection = object([
        ("projection_version", V::from_json(&serde_json::json!(2))?),
        ("authority", authority.clone()),
        ("baseline", baseline),
        ("identity_schemes", strings(["typed-history/v2".into()])),
        ("rules", map(c.state())?["rules"].clone()),
        ("rules_digest", s(&map(c.state())?["rules"].digest()?)),
        ("closure_digest", s(&c.snapshot.revision)),
        (
            "coverage",
            object([
                ("scope", s("all")),
                ("subjects", strings(subjects.keys().cloned())),
                ("complete", V::Bool(complete)),
            ]),
        ),
        ("subjects", V::Map(subjects)),
        ("pins", V::Map(pins)),
        ("dispositions", V::Map(dispositions)),
        (
            "integrity",
            object([
                ("complete", V::Bool(complete)),
                ("findings", V::List(findings)),
            ]),
        ),
    ]);
    let strict = c.snapshot.transactions.values().any(|tx| {
        tx.context
            .as_ref()
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get("options"))
            .and_then(|v| map(v).ok())
            .is_some_and(|m| m.get("strict") == Some(&V::Bool(true)))
    });
    if strict {
        crate::history_view::map_mut(&mut projection)?.insert(
            "requires".into(),
            strings([crate::history_authority::ROOT_DISPOSITION.into()]),
        );
    }
    let template = crate::history_authority::document_template(c.document())?;
    crate::history_adapter::capture_history(&objects, &projection, Some(&template))
}

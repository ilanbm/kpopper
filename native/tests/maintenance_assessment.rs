// Pure draft boundary; integration supplies captured declared dependencies and native claim context.
pub use kpop_native::{Error, Result};
pub mod followup_store {
    pub use kpop_native::followup_store::digest;
}
#[path = "../src/maintenance_assessment.rs"]
mod maintenance_assessment;
use chrono::{Duration, TimeZone, Utc};
use maintenance_assessment::{ActiveUse, Node, assess};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
fn inputs() -> (
    BTreeMap<String, Node>,
    Value,
    Value,
    BTreeSet<String>,
    chrono::DateTime<Utc>,
) {
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let nodes = BTreeMap::from([
        (
            "d.use".into(),
            Node {
                dependencies: vec!["s.source".into()],
                dependency_error: None,
                identity: "consumer1".into(),
            },
        ),
        (
            "s.source".into(),
            Node {
                identity: "source1".into(),
                ..Default::default()
            },
        ),
    ]);
    let observation = json!({"value":"selected report", "observed_at":now.to_rfc3339(), "evidence":"fixture://report"});
    let hash = kpop_native::followup_store::digest(&observation).unwrap();
    let mut items = json!({"policy":{"id":"policy","state":"waiting","spec":{"related":["s.source"],"maintenance":{"kind":"source","source_ref":"source@identity","policy_digest":"policy-digest", "evidence_requirement":"host_attested","use_policy":"allow_cached_until_expiry","max_age_hours":24}},"attempts":[{"type":"maintenance_inspection","source_ref":"source@identity","policy_digest":"policy-digest","outcome":"observed","observation_digest":hash,"receipt_at":now.to_rfc3339(),"inspected_at":now.to_rfc3339()}]}});
    items["policy"]["model_alignment"] = json!({"source_ref":"source@identity","observation_digest":hash,"scope_identity":maintenance_assessment::scope_identity(&items["policy"]["spec"],&nodes).unwrap(),"outcome":"no_model_change_needed"});
    (
        nodes,
        items,
        json!({"source@identity":observation}),
        BTreeSet::from(["policy".into()]),
        now,
    )
}
#[test]
fn exact_expiry_recomputes_over_unchanged_inputs_and_keeps_failure_warning() {
    let (nodes, mut items, observations, current, now) = inputs();
    let frozen = observations.clone();
    items["policy"]["attempts"].as_array_mut().unwrap().push(json!({"type":"maintenance_inspection","source_ref":"source@identity","outcome":"unavailable","reason":"provider refused"}));
    let subjects = vec!["d.use".into()];
    let before = assess(
        &subjects,
        &nodes,
        &items,
        &observations,
        &current,
        &BTreeMap::new(),
        now + Duration::hours(24) - Duration::nanoseconds(1),
        "record-digest",
    )
    .unwrap();
    assert_eq!(before["status"], "adequate");
    assert_eq!(
        before["policies"][0]["warnings"][0]["reason"],
        "latest_attempt_unavailable"
    );
    let expired = assess(
        &subjects,
        &nodes,
        &items,
        &observations,
        &current,
        &BTreeMap::new(),
        now + Duration::hours(24),
        "record-digest",
    )
    .unwrap();
    assert_eq!(expired["status"], "stale");
    assert_eq!(observations, frozen);
    assert_eq!(expired["applicability"], "unassessed");
    assert_eq!(expired["authority"], "not_established");
}
#[test]
fn live_requires_current_admitted_context_and_latest_success_then_failure_is_unknown() {
    let (nodes, mut items, observations, current, now) = inputs();
    items["policy"]["spec"]["maintenance"]["use_policy"] = json!("require_live");
    let subjects = vec!["d.use".into()];
    assert_eq!(
        assess(
            &subjects,
            &nodes,
            &items,
            &observations,
            &current,
            &BTreeMap::new(),
            now,
            "record"
        )
        .unwrap()["status"],
        "unknown"
    );
    let contexts = BTreeMap::from([(
        "policy".into(),
        ActiveUse {
            started_at: now - Duration::seconds(1),
            expires_at: now + Duration::minutes(30),
        },
    )]);
    assert_eq!(
        assess(
            &subjects,
            &nodes,
            &items,
            &observations,
            &current,
            &contexts,
            now,
            "record"
        )
        .unwrap()["status"],
        "adequate"
    );
    items["policy"]["attempts"].as_array_mut().unwrap().push(json!({"type":"maintenance_inspection","source_ref":"source@identity","outcome":"unavailable","reason":"provider refused"}));
    let failed = assess(
        &subjects,
        &nodes,
        &items,
        &observations,
        &current,
        &contexts,
        now + Duration::hours(24),
        "record",
    )
    .unwrap();
    assert_eq!(failed["status"], "unknown");
    assert!(
        failed["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["kind"] == "stale")
    );
}
#[test]
fn closure_cycles_missing_policy_and_origin_adequacy_are_independent_unknowns() {
    let (mut nodes, mut items, observations, current, now) = inputs();
    nodes
        .get_mut("s.source")
        .unwrap()
        .dependencies
        .push("d.use".into());
    items["policy"]["spec"]["maintenance"]["evidence_requirement"] = json!("trusted_origin");
    let result = assess(
        &["d.use".into(), "missing".into()],
        &nodes,
        &items,
        &observations,
        &current,
        &BTreeMap::new(),
        now,
        "record",
    )
    .unwrap();
    assert_eq!(result["status"], "unknown");
    for expected in [
        "dependency_cycle",
        "dependency_missing",
        "trusted_origin_unestablished",
    ] {
        assert!(
            result["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["reason"] == expected)
        );
    }
    let missing = assess(
        &["s.source".into()],
        &BTreeMap::from([("s.source".into(), Node::default())]),
        &json!({}),
        &observations,
        &current,
        &BTreeMap::new(),
        now,
        "record",
    )
    .unwrap();
    assert_eq!(missing["status"], "unknown");
}

#[test]
fn requested_consumer_coverage_uses_forward_declared_closure() {
    let (mut nodes, items, observations, current, now) = inputs();
    nodes.insert("fact.other".into(), Node::default());
    let r = assess(&["d.use".into(),"fact.other".into(),"typo.missing".into()],&nodes,&items,&observations,&current,&BTreeMap::new(),now,"record").unwrap();
    assert_eq!(r["covered_subjects"], json!(["d.use"]));
    assert_eq!(r["uncovered_subjects"], json!(["fact.other"]));
    assert_eq!(r["unresolved_subjects"], json!(["typo.missing"]));
    assert_eq!(r["authority"], "not_established");
}

#[test]
fn dependent_check_is_information_not_input_coverage_or_authority() {
    let (nodes, mut items, observations, current, now) = inputs();
    items["policy"]["spec"]["related"] = json!(["d.use"]);
    let r=assess(&["s.source".into()],&nodes,&items,&observations,&current,&BTreeMap::new(),now,"record").unwrap();
    assert_eq!(r["covered_subjects"], json!([]));
    assert_eq!(r["uncovered_subjects"], json!(["s.source"]));
    assert_eq!(r["related_check_context"][0]["id"], "policy");
    assert_eq!(r["related_check_context"][0]["coverage"], "informational_only");
    assert_eq!(r["related_check_context"][0]["adequacy"], "unassessed");
    assert_eq!(r["authority"], "not_established");
}

#[test]
fn retired_scope_stays_historical_and_not_a_new_offer() {
    let (nodes, mut items, observations, current, now) = inputs();
    items["policy"]["state"] = json!("cancelled");
    let r=assess(&["d.use".into()],&nodes,&items,&observations,&current,&BTreeMap::new(),now,"record").unwrap();
    assert_eq!(r["covered_subjects"], json!([]));
    assert_eq!(r["uncovered_subjects"], json!([]));
    assert_eq!(r["retired_subjects"], json!(["d.use"]));
    assert_eq!(r["retired_policies"][0]["id"], "policy");
}

#[test]
fn declaration_dates_source_and_effective_next_are_preserved() {
    let (nodes, mut items, observations, current, now) = inputs();
    items["policy"]["spec"]["maintenance"]["due_at"] = json!("2026-12-24T09:00:00Z");
    items["policy"]["spec"]["maintenance"]["source_id"] = json!("provider-input");
    items["policy"]["next_at"] = json!("2026-12-31T09:00:00Z");
    let r=assess(&["d.use".into()],&nodes,&items,&observations,&current,&BTreeMap::new(),now,"record").unwrap();
    let p=&r["policies"][0];
    assert_eq!(p["declared_choices"]["due_at"], "2026-12-24T09:00:00Z");
    assert_eq!(p["declared_choices"]["source_id"], "provider-input");
    assert_eq!(p["effective_next_at"], "2026-12-31T09:00:00Z");
}

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

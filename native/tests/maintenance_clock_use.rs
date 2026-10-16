pub use kpop_native::{Error, Result};
pub mod followup_store {
    pub use kpop_native::followup_store::digest;
}
#[path = "../src/maintenance_assessment.rs"]
mod maintenance_assessment;

use chrono::{Duration, TimeZone, Utc};
use maintenance_assessment::{Node, assess};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn clock_assessment(
    now: chrono::DateTime<Utc>,
    due_at: &str,
    next_at: Option<&str>,
    state: &str,
    claim: Value,
    attempts: Value,
) -> Value {
    let nodes = BTreeMap::from([("facts.count".into(), Node::default())]);
    let mut item = json!({
        "state":state,
        "claim":claim,
        "spec":{
            "related":["facts.count"],
            "maintenance":{
                "kind":"clock",
                "due_at":due_at,
                "policy_digest":"clock-policy"
            }
        },
        "attempts":attempts
    });
    if let Some(next_at) = next_at {
        item["next_at"] = json!(next_at);
    }
    let items = json!({"clock-policy":item});
    assess(
        &["facts.count".into()],
        &nodes,
        &items,
        &json!({}),
        &BTreeSet::from(["clock-policy".into()]),
        &BTreeMap::new(),
        now,
        "fixture-record",
    )
    .unwrap()
}

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap()
}

#[test]
fn overdue_never_checked_clock_is_unknown() {
    let now = now();
    let result = clock_assessment(
        now,
        &(now - Duration::minutes(1)).to_rfc3339(),
        None,
        "waiting",
        Value::Null,
        json!([]),
    );

    assert_eq!(result["status"], "unknown");
    assert_eq!(result["policies"][0]["status"], "unknown");
    assert!(
        result["policies"][0]["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("clock_check_due_or_unperformed"))
    );
}

#[test]
fn not_yet_due_clock_is_adequate_without_conflating_other_assessments() {
    let now = now();
    let result = clock_assessment(
        now,
        &(now + Duration::minutes(1)).to_rfc3339(),
        None,
        "waiting",
        Value::Null,
        json!([]),
    );

    assert_eq!(result["status"], "adequate");
    assert_eq!(result["policies"][0]["status"], "adequate");
    assert_eq!(result["reasons"][0]["kind"], "evidence");
    assert_eq!(result["reasons"][0]["reason"], "current_native_clock");
    assert_eq!(result["applicability"], "unassessed");
    assert_eq!(result["authority"], "not_established");
    assert_eq!(result["consumer_artifact"], "not_established");
}

#[test]
fn checked_clock_rearms_a_future_deadline() {
    let now = now();
    let next_at = now + Duration::days(7);
    let result = clock_assessment(
        now,
        &(now - Duration::days(1)).to_rfc3339(),
        Some(&next_at.to_rfc3339()),
        "waiting",
        Value::Null,
        json!([{
            "outcome":"checked",
            "finished_at":now.to_rfc3339(),
            "request":{"next_at":next_at.to_rfc3339()}
        }]),
    );

    assert_eq!(result["status"], "adequate");
    assert_eq!(result["policies"][0]["status"], "adequate");
    assert_eq!(result["reasons"][0]["kind"], "evidence");
}

#[test]
fn future_retry_deadline_without_checked_attempt_is_unknown() {
    let now = now();
    let result = clock_assessment(
        now,
        &(now - Duration::days(1)).to_rfc3339(),
        Some(&(now + Duration::hours(1)).to_rfc3339()),
        "waiting",
        Value::Null,
        json!([]),
    );

    assert_eq!(result["status"], "unknown");
    assert!(
        result["policies"][0]["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("clock_rearm_not_checked"))
    );
}

#[test]
fn parked_or_interrupted_clock_check_is_unknown() {
    let now = now();
    let due_at = (now + Duration::days(1)).to_rfc3339();
    let parked = clock_assessment(now, &due_at, None, "needs_user", Value::Null, json!([]));
    assert_eq!(parked["status"], "unknown");
    assert!(
        parked["policies"][0]["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("clock_check_needs_user"))
    );

    let interrupted = clock_assessment(
        now,
        &due_at,
        None,
        "waiting",
        json!({"expires_at":(now - Duration::minutes(1)).to_rfc3339()}),
        json!([]),
    );
    assert_eq!(interrupted["status"], "unknown");
    assert!(
        interrupted["policies"][0]["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("clock_check_running_or_interrupted"))
    );
}

#[test]
fn active_clock_check_is_not_adequate_during_intervention() {
    let now = now();
    let result = clock_assessment(
        now,
        &(now + Duration::days(1)).to_rfc3339(),
        None,
        "waiting",
        json!({"expires_at":(now + Duration::minutes(10)).to_rfc3339()}),
        json!([]),
    );

    assert_eq!(result["status"], "unknown");
    assert!(
        result["policies"][0]["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("clock_check_running_or_interrupted"))
    );
}

//! Scoped use assessment. Recording, adequacy, applicability and authority are separate.
//! This module reads finite inputs and the caller's current trusted clock; it caches nothing.
use crate::{Error, Result, followup_store::digest};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default)]
pub(crate) struct Node {
    pub dependencies: Vec<String>,
    pub dependency_error: Option<String>,
    pub identity: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ActiveUse {
    pub started_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

fn instant(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|v| v.with_timezone(&Utc))
}

fn visit(
    id: &str,
    nodes: &BTreeMap<String, Node>,
    active: &mut BTreeSet<String>,
    closure: &mut BTreeSet<String>,
    reasons: &mut Vec<Value>,
) {
    if active.len() >= 64 || closure.len() >= 1000 {
        if !reasons
            .iter()
            .any(|r| r["reason"] == "declared_closure_limit")
        {
            reasons.push(json!({"subject":id,"kind":"unknown","reason":"declared_closure_limit","detail":"Assessment stops at depth64 or1000 declared nodes"}));
        }
        return;
    }
    if active.contains(id) {
        reasons.push(json!({"subject":id,"kind":"unknown","reason":"dependency_cycle"}));
        return;
    }
    if !closure.insert(id.to_owned()) {
        return;
    }
    let Some(node) = nodes.get(id) else {
        reasons.push(json!({"subject":id,"kind":"unknown","reason":"dependency_missing"}));
        return;
    };
    if let Some(error) = &node.dependency_error {
        reasons.push(
            json!({"subject":id,"kind":"unknown","reason":"dependency_unresolved","detail":error}),
        );
    }
    active.insert(id.to_owned());
    for dependency in &node.dependencies {
        visit(dependency, nodes, active, closure, reasons);
    }
    active.remove(id);
}

pub(crate) fn assess(
    subjects: &[String],
    nodes: &BTreeMap<String, Node>,
    items: &Value,
    observations: &Value,
    task_current: &BTreeSet<String>,
    active_use: &BTreeMap<String, ActiveUse>,
    now: DateTime<Utc>,
    record_identity: &str,
) -> Result<Value> {
    if subjects.is_empty() || subjects.len() > 100 {
        return Err(Error("Use assessment needs 1–100 explicit subjects".into()));
    }
    let mut closure = BTreeSet::new();
    let mut reasons = Vec::new();
    for subject in subjects {
        visit(
            subject,
            nodes,
            &mut BTreeSet::new(),
            &mut closure,
            &mut reasons,
        );
    }
    let mut policies = Vec::new();
    let mut covered = BTreeSet::new();
    for (id, item) in items
        .as_object()
        .ok_or_else(|| Error("Invalid assessment items".into()))?
    {
        let Some(m) = item["spec"].get("maintenance") else {
            continue;
        };
        let related = item["spec"]["related"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|id| closure.contains(*id))
            .collect::<Vec<_>>();
        if related.is_empty() {
            continue;
        }
        let mut policy_closure = BTreeSet::new();
        for id in &related {
            visit(
                id,
                nodes,
                &mut BTreeSet::new(),
                &mut policy_closure,
                &mut Vec::new(),
            );
        }
        covered.extend(policy_closure);
        let mut policy_reasons = Vec::new();
        let mut unknown = false;
        let mut stale = false;
        let mut warn = Vec::new();
        if matches!(item["state"].as_str(), Some("done" | "cancelled")) {
            unknown = true;
            policy_reasons.push("maintenance_closed");
        }
        if !task_current.contains(id) {
            unknown = true;
            policy_reasons.push("task_changed_or_unavailable");
        }
        if m["kind"] == "clock" {
            policy_reasons.push("current_native_clock");
            let effective_due = item["next_at"]
                .as_str()
                .or_else(|| m["due_at"].as_str())
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc));
            if item["state"] == "needs_user" {
                unknown = true;
                policy_reasons.push("clock_check_needs_user");
            } else if !item["claim"].is_null() {
                unknown = true;
                policy_reasons.push("clock_check_running_or_interrupted");
            } else if effective_due.is_none() {
                unknown = true;
                policy_reasons.push("clock_deadline_unestablished");
            } else if effective_due.is_some_and(|due| due <= now) {
                unknown = true;
                policy_reasons.push("clock_check_due_or_unperformed");
            } else if item["next_at"].is_string() {
                let rearmed_by_checked_attempt = item["attempts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|attempt| {
                        attempt["outcome"] == "checked"
                            && instant(&attempt["request"]["next_at"]) == instant(&item["next_at"])
                            && instant(&attempt["finished_at"])
                                .is_some_and(|finished| finished <= now)
                    });
                if !rearmed_by_checked_attempt {
                    unknown = true;
                    policy_reasons.push("clock_rearm_not_checked");
                }
            }
        } else {
            let reference = m["source_ref"].as_str().unwrap_or("");
            let observation = &observations[reference];
            let attempts = item["attempts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|a| a["type"] == "maintenance_inspection" && a["source_ref"] == reference)
                .collect::<Vec<_>>();
            let latest = attempts.last().copied();
            let success = attempts.iter().rev().copied().find(|a| {
                a["outcome"] == "observed"
                    && digest(observation).is_ok_and(|hash| a["observation_digest"] == hash)
            });
            if latest.is_some_and(|a| a["outcome"] == "unavailable") {
                warn.push(json!({"reason":"latest_attempt_unavailable", "detail":latest.unwrap()["reason"]}));
            }
            if m["evidence_requirement"] != "host_attested" {
                unknown = true;
                policy_reasons.push("trusted_origin_unestablished");
            }
            let observed_at = instant(&observation["observed_at"]);
            match (observed_at, m["max_age_hours"].as_f64()) {
                (Some(observed), Some(age)) if age.is_finite() && age > 0.0 => {
                    let delta = now.signed_duration_since(observed);
                    let seconds =
                        delta.num_seconds() as f64 + f64::from(delta.subsec_nanos()) / 1e9;
                    if observed > now {
                        unknown = true;
                        policy_reasons.push("observation_from_future");
                    } else if seconds >= age * 3600.0 {
                        stale = true;
                        policy_reasons.push("evidence_expired");
                    }
                }
                _ => {
                    unknown = true;
                    policy_reasons.push("observation_or_age_missing");
                }
            }
            let alignment = &item["model_alignment"];
            let scope_identity = scope_identity(&item["spec"], nodes).ok();
            if alignment["source_ref"] != m["source_ref"]
                || alignment["observation_digest"] != digest(observation)?
                || scope_identity
                    .as_ref()
                    .is_none_or(|scope| alignment["scope_identity"] != *scope)
                || !matches!(
                    alignment["outcome"].as_str(),
                    Some("no_model_change_needed" | "reviewed_model_update")
                )
            {
                unknown = true;
                policy_reasons.push("source_model_alignment_unassessed_or_pending");
            }
            if success.is_none() {
                unknown = true;
                policy_reasons.push("guarded_observation_unestablished");
            }
            if m["use_policy"] == "require_live" {
                let live = active_use.get(id).is_some_and(|context| {
                    now < context.expires_at
                        && success.is_some_and(|a| {
                            latest == Some(a)
                                && a["policy_digest"] == m["policy_digest"]
                                && instant(&a["receipt_at"])
                                    .is_some_and(|time| time >= context.started_at && time <= now)
                                && instant(&a["inspected_at"])
                                    .is_some_and(|time| time >= context.started_at && time <= now)
                        })
                });
                if !live {
                    unknown = true;
                    policy_reasons.push("current_live_inspection_unestablished");
                }
            }
        }
        let status = if unknown {
            "unknown"
        } else if stale {
            "stale"
        } else {
            "adequate"
        };
        for reason in &policy_reasons {
            reasons.push(json!({"policy":id,"subjects":related,"kind":if *reason == "evidence_expired" { "stale" } else if *reason == "current_native_clock" { "evidence" } else { "unknown" },"reason":reason}));
        }
        policies.push(json!({"id":id,"subjects":related,"status":status,"reasons":policy_reasons,"warnings":warn,
            "source_ref":m["source_ref"],"policy_digest":m["policy_digest"],"recording_assurance":"host_attested_or_current_native_clock"}));
    }
    // A declared leaf with no applicable check remains unknown; stable knowledge
    // is not automatically enrolled into maintenance by this explicit-use query.
    for id in &closure {
        if nodes.get(id).is_some_and(|n| n.dependencies.is_empty()) && !covered.contains(id) {
            reasons.push(json!({"subject":id,"kind":"unknown","reason":"required_leaf_has_no_maintenance_policy"}));
        }
    }
    let status = if reasons.iter().any(|r| r["kind"] == "unknown") {
        "unknown"
    } else if reasons.iter().any(|r| r["kind"] == "stale") {
        "stale"
    } else {
        "adequate"
    };
    Ok(
        json!({"status":status,"subjects":subjects,"declared_closure":closure,"policies":policies,"reasons":reasons,
        "assessed_at":now.to_rfc3339(),"record_identity":record_identity,
        "applicability":"unassessed","authority":"not_established","consumer_artifact":"not_established",
        "limitation":"Declared dependencies only; no truth, hidden-dependency coverage or legal applicability claim"}),
    )
}

pub(crate) fn scope_identity(spec: &Value, nodes: &BTreeMap<String, Node>) -> Result<String> {
    let mut closure = BTreeSet::new();
    let mut reasons = Vec::new();
    for id in spec["related"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        visit(id, nodes, &mut BTreeSet::new(), &mut closure, &mut reasons);
    }
    if reasons
        .iter()
        .any(|r| r["reason"] != "dependency_unresolved")
        || closure
            .iter()
            .any(|id| nodes.get(id).is_none_or(|n| n.identity.is_empty()))
    {
        return Err(Error(
            "Declared model scope is unresolved; retain alignment as pending".into(),
        ));
    }
    let identities = closure
        .into_iter()
        .map(|id| {
            let identity = nodes
                .get(&id)
                .map(|n| n.identity.clone())
                .unwrap_or_default();
            (id, identity)
        })
        .collect::<BTreeMap<_, _>>();
    digest(&json!({"schema":"kpopper.maintenance-model-scope/v1","nodes":identities}))
}

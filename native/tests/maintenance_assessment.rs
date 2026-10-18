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

#[test]
fn known_direct_declaration_survives_incomplete_and_cyclic_scope() {
    for cyclic in [false, true] {
        let (mut nodes, mut items, observations, current, now) = inputs();
        items["policy"]["spec"]["related"] = json!(["d.use"]);
        if cyclic {
            nodes.get_mut("s.source").unwrap().dependencies.push("d.use".into());
        } else {
            nodes.get_mut("d.use").unwrap().dependencies.push("missing.input".into());
        }
        let r = assess(&["d.use".into()], &nodes, &items, &observations, &current, &BTreeMap::new(), now, "record").unwrap();
        assert_eq!(r["covered_subjects"], json!(["d.use"]));
        assert_eq!(r["unresolved_subjects"], json!(["d.use"]));
        assert_eq!(r["uncovered_subjects"], json!([]));
        assert_eq!(r["status"], "unknown");
        assert_eq!(r["authority"], "not_established");
        assert_eq!(r["policies"][0]["id"], "policy");
    }
}

#[test]
fn input_context_and_leaf_reason_are_invariant_under_mixed_request() {
    let (nodes, mut items, observations, current, now) = inputs();
    items["policy"]["spec"]["related"] = json!(["d.use"]);
    items["policy"]["model_alignment"]["scope_identity"] = json!(maintenance_assessment::scope_identity(&items["policy"]["spec"], &nodes).unwrap());
    let parent = assess(&["d.use".into()], &nodes, &items, &observations, &current, &BTreeMap::new(), now, "record").unwrap();
    assert_eq!(parent["status"], "adequate");
    assert!(!parent["reasons"].as_array().unwrap().iter().any(|reason| reason["reason"] == "required_leaf_has_no_maintenance_policy"));
    let alone = assess(&["s.source".into()], &nodes, &items, &observations, &current, &BTreeMap::new(), now, "record").unwrap();
    let mixed = assess(&["s.source".into(), "d.use".into()], &nodes, &items, &observations, &current, &BTreeMap::new(), now, "record").unwrap();
    assert_eq!(mixed["related_check_context"], alone["related_check_context"]);
    assert_eq!(mixed["covered_subjects"], json!(["d.use"]));
    assert_eq!(mixed["uncovered_subjects"], alone["uncovered_subjects"]);
    for r in [&alone, &mixed] {
        assert!(r["reasons"].as_array().unwrap().iter().any(|reason| reason["subject"] == "s.source" && reason["reason"] == "required_leaf_has_no_maintenance_policy"));
        assert_eq!(r["authority"], "not_established");
    }
}

#[test]
fn requested_scope_adequacy_is_monotone_across_shared_input_matrix() {
    let (mut nodes, mut items, observations, current, now) = inputs();
    nodes.insert("source.mid".into(), Node { dependencies: vec!["s.source".into()], identity: "shared-consumer".into(), ..Default::default() });
    nodes.insert("fact.sibling".into(), Node { dependencies: vec!["s.source".into()], identity: "shared-consumer".into(), ..Default::default() });
    nodes.get_mut("d.use").unwrap().dependencies = vec!["source.mid".into()];
    items["policy"]["spec"]["related"] = json!(["d.use"]);
    items["policy"]["model_alignment"]["scope_identity"] = json!(maintenance_assessment::scope_identity(&items["policy"]["spec"], &nodes).unwrap());
    let assess_ids = |ids: Vec<String>| assess(&ids, &nodes, &items, &observations, &current, &BTreeMap::new(), now, "record").unwrap();
    assert_eq!(assess_ids(vec!["d.use".into()])["status"], "adequate");
    let ids = ["s.source", "source.mid", "fact.sibling", "d.use"];
    // Every nonempty subset and both input orders must retain the original
    // requested use's unknown leaf verdict; a sibling check cannot bless it.
    for mask in 1..16 {
        let request = ids.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, id)| (*id).to_owned()).collect::<Vec<_>>();
        for reverse in [false, true] {
            let mut request = request.clone();
            if reverse { request.reverse(); }
            let r = assess_ids(request.clone());
            if mask & 7 == 0 { assert_eq!(r["status"], "adequate"); continue; }
            assert_eq!(r["status"], "unknown", "request {request:?}");
            assert!(r["reasons"].as_array().unwrap().iter().any(|reason| reason["subject"] == "s.source" && reason["reason"] == "required_leaf_has_no_maintenance_policy"), "request {request:?}");
            assert_eq!(r["authority"], "not_established");
            for id in request.iter().filter(|id| id.as_str() != "d.use") {
                assert!(r["uncovered_subjects"].as_array().unwrap().contains(&json!(id)));
            }
        }
    }
}

#[test]
fn retired_direct_subject_is_history_not_its_own_input_context() {
    let (nodes, mut items, observations, current, now) = inputs();
    items["policy"]["spec"]["related"] = json!(["d.use"]);
    items["policy"]["state"] = json!("cancelled");
    for ids in [vec!["d.use".into()], vec!["d.use".into(), "s.source".into()]] {
        let r = assess(&ids, &nodes, &items, &observations, &current, &BTreeMap::new(), now, "record").unwrap();
        assert_eq!(r["retired_subjects"], json!(["d.use"]));
        for context in r["related_check_context"].as_array().unwrap() {
            assert!(!context["requested_inputs"].as_array().unwrap().contains(&json!("d.use")));
        }
    }
}

#[test]
fn deleted_policy_subject_cannot_establish_consumer_coverage() {
    let (mut nodes, mut items, observations, current, now) = inputs();
    nodes.get_mut("d.use").unwrap().dependencies.push("fact.gone".into());
    items["policy"]["spec"]["related"] = json!(["fact.gone"]);
    for ids in [vec!["d.use".into()], vec!["fact.gone".into()], vec!["d.use".into(), "fact.gone".into()]] {
        let r = assess(&ids, &nodes, &items, &observations, &current, &BTreeMap::new(), now, "record").unwrap();
        assert_eq!(r["covered_subjects"], json!([]));
        assert_eq!(r["status"], "unknown");
        assert_eq!(r["unresolved_subjects"].as_array().unwrap().len(), ids.len());
        assert_eq!(r["authority"], "not_established");
    }
}

#[test]
fn combined_depth_and_width_limits_return_unknown_in_either_order() {
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    for wide in [false, true] {
        let nodes = limit_graph(wide);
        for ids in [vec!["d.atop".into(), "d.btop".into()], vec!["d.btop".into(), "d.atop".into()]] {
            let r = assess(&ids, &nodes, &json!({}), &json!({}), &BTreeSet::new(), &BTreeMap::new(), now, "record").unwrap();
            assert_eq!(r["status"], "unknown", "wide={wide} ids={ids:?}");
            assert!(r["reasons"].as_array().unwrap().iter().any(|reason| reason["reason"] == "declared_closure_limit"), "wide={wide} ids={ids:?}");
            assert_eq!(r["covered_subjects"], json!([]));
            assert_eq!(r["authority"], "not_established");
        }
    }
}

fn limit_graph(wide: bool) -> BTreeMap<String, Node> {
    let mut nodes = BTreeMap::new();
    if wide {
        for (root, prefix) in [("d.atop", "a"), ("d.btop", "b")] {
            let leaves = (0..600).map(|i| format!("{prefix}.{i}")).collect::<Vec<_>>();
            for leaf in &leaves { nodes.insert(leaf.clone(), Node::default()); }
            nodes.insert(root.into(), Node { dependencies: leaves, ..Default::default() });
        }
    } else {
        nodes.insert("fact.deep".into(), Node::default());
        let mut next = "fact.deep".to_owned();
        for i in (1..=9).rev() {
            let id = format!("x{i:02}");
            nodes.insert(id.clone(), Node { dependencies: vec![next], ..Default::default() });
            next = id;
        }
        nodes.insert("d.hub".into(), Node { dependencies: vec![next], ..Default::default() });
        let mut next = "d.hub".to_owned();
        for i in (1..=61).rev() {
            let id = format!("c{i:02}");
            nodes.insert(id.clone(), Node { dependencies: vec![next], ..Default::default() });
            next = id;
        }
        nodes.insert("d.atop".into(), Node { dependencies: vec![next], ..Default::default() });
        nodes.insert("d.btop".into(), Node { dependencies: vec!["d.hub".into()], ..Default::default() });
    }
    nodes
}

#[test]
fn limit_hidden_active_policy_is_present_without_false_leaf_absence() {
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let mut nodes = limit_graph(false);
    nodes.insert("fact.base".into(), Node::default());
    nodes.get_mut("d.atop").unwrap().dependencies.push("fact.base".into());
    for checked_subject in ["fact.deep", "x09"] {
    let items = json!({"leaf-clock":{"state":"waiting","spec":{"related":[checked_subject,"fact.base"],"maintenance":{"kind":"clock","due_at":"2026-12-24T09:00:00Z","cadence_days":1,"timezone":"UTC","check_time":"09:00","policy_digest":"clock-digest"}}}});
    for ids in [vec!["d.atop".into(), "d.btop".into()], vec!["d.btop".into(), "d.atop".into()]] {
        let r = assess(&ids, &nodes, &items, &json!({}), &BTreeSet::from(["leaf-clock".into()]), &BTreeMap::new(), now, "record").unwrap();
        assert_eq!(r["status"], "unknown");
        assert!(r["reasons"].as_array().unwrap().iter().any(|reason| reason["reason"] == "declared_closure_limit"));
        assert!(!r["reasons"].as_array().unwrap().iter().any(|reason| reason["reason"] == "required_leaf_has_no_maintenance_policy"));
        assert_eq!(r["policies"][0]["id"], "leaf-clock");
        assert_eq!(r["policies"][0]["subjects"], json!([checked_subject, "fact.base"]));
        assert_eq!(r["policies"][0]["declared_choices"]["cadence_days"], 1);
        assert_eq!(r["policies"][0]["declared_choices"]["due_at"], "2026-12-24T09:00:00Z");
        assert_eq!(r["authority"], "not_established");
    }
    }
}

#[test]
fn shared_walk_policy_dropped() {
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let mut nodes = BTreeMap::new();
    let n = |deps: &[&str]| Node { dependencies: deps.iter().map(|s| s.to_string()).collect(), ..Default::default() };
    // s2 -> [p, h]; p -> c01 -> ... -> c61 -> h; h -> d (leaf, depth 2 from s2).
    // A fresh walk of s2 reaches h first with 63 ancestors, so d (64) is cut, and s2 -> h is then skipped.
    nodes.insert("d".to_string(), n(&[]));
    nodes.insert("h".to_string(), n(&["d"]));
    let mut next = "h".to_string();
    for i in (1..=61).rev() { let id = format!("c{i:02}"); nodes.insert(id.clone(), n(&[&next])); next = id; }
    nodes.insert("p".to_string(), n(&[&next]));
    nodes.insert("s2".to_string(), n(&["p", "h"]));
    // s1 -> e01 -> ... -> e62 -> p : p is reached with 63 ancestors, so the shared walk caches p without h.
    let mut next = "p".to_string();
    for i in (1..=62).rev() { let id = format!("e{i:02}"); nodes.insert(id.clone(), n(&[&next])); next = id; }
    nodes.insert("s1".to_string(), n(&[&next]));
    let items = json!({"d-clock":{"state":"waiting","spec":{"related":["d"],"maintenance":{"kind":"clock","due_at":"2026-12-24T09:00:00Z","cadence_days":1,"timezone":"UTC","check_time":"09:00","policy_digest":"x"}}}});
    for ids in [vec!["s1".to_string(), "s2".to_string()], vec!["s2".to_string(), "s1".to_string()]] {
        let r = assess(&ids, &nodes, &items, &json!({}), &BTreeSet::from(["d-clock".to_string()]), &BTreeMap::new(), now, "record").unwrap();
        let in_shared = r["declared_closure"].as_array().unwrap().iter().any(|v| v == "d");
        let listed = r["policies"].as_array().unwrap().iter().any(|p| p["id"] == "d-clock");
        println!("ids={ids:?} status={} d_in_shared_closure={in_shared} d-clock_listed={listed}", r["status"]);
        assert_eq!(r["status"], "unknown");
        assert!(listed, "known check on depth-2 dependency d omitted (ids={ids:?})");
    }
}

#[test]
fn scope_identity_preserves_complete_dag_and_resolution_boundaries() {
    let (mut nodes, _, _, _, _) = inputs();
    nodes.insert("d.other".into(), Node { dependencies: vec!["s.source".into()], identity: "other1".into(), dependency_error: None });
    let ordered = json!({"related":["d.use","d.other"]});
    let reversed = json!({"related":["d.other","d.use"]});
    let original = maintenance_assessment::scope_identity(&ordered, &nodes).unwrap();
    assert_eq!(maintenance_assessment::scope_identity(&reversed, &nodes).unwrap(), original);
    nodes.get_mut("d.use").unwrap().dependency_error = Some("readback unresolved".into());
    assert_eq!(maintenance_assessment::scope_identity(&ordered, &nodes).unwrap(), original);
    nodes.get_mut("d.use").unwrap().dependencies.push("missing".into());
    assert!(maintenance_assessment::scope_identity(&ordered, &nodes).is_err());
    nodes.get_mut("d.use").unwrap().dependencies.pop();
    nodes.get_mut("s.source").unwrap().dependencies.push("d.use".into());
    assert!(maintenance_assessment::scope_identity(&ordered, &nodes).is_err());
    nodes.get_mut("s.source").unwrap().dependencies.clear();
    nodes.get_mut("s.source").unwrap().identity.clear();
    assert!(maintenance_assessment::scope_identity(&ordered, &nodes).is_err());
    for wide in [false, true] {
        let mut bounded = limit_graph(wide);
        for (id, node) in &mut bounded { node.identity = id.clone(); }
        assert!(maintenance_assessment::scope_identity(&json!({"related":["d.atop","d.btop"]}), &bounded).is_err());
        assert!(maintenance_assessment::scope_identity(&json!({"related":["d.btop","d.atop"]}), &bounded).is_err());
    }
}

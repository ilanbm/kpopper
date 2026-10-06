use chrono::{Duration, TimeZone, Utc};
use kpop_native::{followup_store::Store, maintenance_contract};
use serde_json::{Value, json};
use std::fs;
fn fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    Store,
    Value,
    chrono::DateTime<Utc>,
) {
    let t = tempfile::tempdir().unwrap();
    let w = t.path().join("w");
    fs::create_dir(&w).unwrap();
    fs::write(w.join("PROVENANCE.yaml"),"meta:\n  name: Use fixture\nschema:\n  deps: rests_on\n  snapshot: seen\n  predicate: wrong_if\nknown:\n  fact.input:\n    name: Input\n    v: 1\n  fact.untouched:\n    name: Other input\n    v: 1\njudgments:\n  decision.use:\n    name: Selected decision\n    verdict: Input is positive\n    rests_on: [fact.input]\n    wrong_if: fact.input < 0\n    seen: {fact.input: 1}\n  decision.untouched:\n    name: Other decision\n    verdict: Other input is positive\n    rests_on: [fact.untouched]\n    wrong_if: fact.untouched < 0\n    seen: {fact.untouched: 1}\n").unwrap();
    let state = t.path().join("state");
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let store = Store::at_in_state(&w, &state, now).unwrap();
    store.setup(None, "UTC", None, true).unwrap();
    let d = json!({"schema":"kpopper.maintenance-declaration/v1","kind":"source","id":"source-check","title":"Selected source check","why":"Keep selected evidence current","how":"Inspect selected source","scope":"Read and propose; review separately","related":["decision.use"],"cadence_days":7,"timezone":"UTC","check_time":"09:00","use_policy":"allow_cached_until_expiry","evidence_requirement":"host_attested","first_due_at":"2026-10-12T09:00:00Z","source":{"source_id":"external-provider-id","locator":"https://example.test/status","publisher":"Example","selection":"Selected input","adapter":"host/v1","tool_policy":"existing authorized read tool","allowed_roots":["https://example.test/"],"evidence_format":"selected state","max_age_hours":24}});
    let item = store
        .add(maintenance_contract::compile(&d).unwrap()["spec"].clone())
        .unwrap();
    (t, w, state, store, item, now)
}
fn inspect(store: &Store, item: &Value, time: chrono::DateTime<Utc>, v: i32) -> Value {
    store.inspect_maintenance(json!({"id":"source-check","policy_digest":item["spec"]["maintenance"]["policy_digest"],"source_ref":item["spec"]["maintenance"]["source_ref"],"inspection":item["spec"]["maintenance"]["inspection"],"inspected_at":time.to_rfc3339(),"evidence":"fixture://actual-selected-input","value":v})).unwrap()
}
#[test]
fn fresh_changed_observation_cannot_upgrade_unchanged_canonical_consumer() {
    let (_t, w, state, store, item, now) = fixture();
    let before = fs::read(w.join("PROVENANCE.yaml")).unwrap();
    inspect(&store, &item, now, 1);
    assert_eq!(
        store.assess_use(&["decision.use".into()], &[]).unwrap()["status"],
        "unknown"
    );
    store
        .record_model_alignment(
            "source-check",
            "no_model_change_needed",
            "fixture://reviewed-declared-scope",
            "fixture://user-review-grant",
        )
        .unwrap();
    assert_eq!(
        store.assess_use(&["decision.use".into()], &[]).unwrap()["status"],
        "adequate"
    );
    let same = Store::at_in_state(&w, &state, now + Duration::seconds(30)).unwrap();
    inspect(&same, &item, now + Duration::seconds(30), 1);
    assert_eq!(
        same.assess_use(&["decision.use".into()], &[]).unwrap()["status"],
        "adequate"
    );
    assert!(
        same.show("source-check").unwrap()["model_alignment"]["carried_from_observation"]
            .is_string()
    );
    let later = Store::at_in_state(&w, &state, now + Duration::minutes(1)).unwrap();
    inspect(&later, &item, now + Duration::minutes(1), 2);
    let assessment = later.assess_use(&["decision.use".into()], &[]).unwrap();
    assert_eq!(assessment["status"], "unknown");
    assert!(
        assessment["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["reason"] == "source_model_alignment_unassessed_or_pending")
    );
    assert_eq!(fs::read(w.join("PROVENANCE.yaml")).unwrap(), before);
    later
        .record_model_alignment(
            "source-check",
            "candidate_pending",
            "fixture://hypothesis-pending",
            "fixture://user-review-grant",
        )
        .unwrap();
    assert_eq!(
        later.assess_use(&["decision.use".into()], &[]).unwrap()["status"],
        "unknown"
    );
    // Native affects is read-only and keeps unrelated consumers out of the declared impact set.
    let affected = std::process::Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&w)
        .args(["affects", "fact.input"])
        .output()
        .unwrap();
    assert!(
        affected.status.success(),
        "{}",
        String::from_utf8_lossy(&affected.stderr)
    );
    let text = String::from_utf8_lossy(&affected.stdout);
    assert!(text.contains("decision.use"));
    assert!(!text.contains("decision.untouched"));
    assert_eq!(fs::read(w.join("PROVENANCE.yaml")).unwrap(), before);
}
#[test]
fn manual_live_use_needs_current_claim_inspection_and_reassesses_after_expiry() {
    let (_t, w, state, store, item, now) = fixture();
    // A stricter same-source use policy is explicit in the compiled replacement; it needs no renewed permission.
    // Recompile from the original task declaration is required for native digest authority; use the stored local task text's spec through a new complete declaration below.
    let m = &item["spec"]["maintenance"];
    let declaration = json!({"schema":"kpopper.maintenance-declaration/v1","kind":"source","id":"source-check","title":"Selected source check","why":"Keep selected evidence current","how":"Inspect selected source","scope":item["spec"]["scope"],"related":["decision.use"],"cadence_days":7,"timezone":"UTC","check_time":"09:00","use_policy":"require_live","evidence_requirement":"host_attested","first_due_at":m["due_at"],"source":{"source_id":m["source_id"],"locator":m["inspection"]["locator"],"publisher":m["inspection"]["publisher"],"selection":m["inspection"]["selection"],"adapter":m["inspection"]["adapter"],"tool_policy":m["inspection"]["tool_policy"],"allowed_roots":m["inspection"]["allowed_roots"],"evidence_format":m["inspection"]["evidence_format"],"max_age_hours":24}});
    let live = store
        .refresh(
            "source-check",
            maintenance_contract::compile(&declaration).unwrap()["spec"].clone(),
            "fixture://explicit-stricter-use",
        )
        .unwrap();
    let row = store.scan(20).unwrap()["items"][0].clone();
    assert_eq!(row["state"], "waiting");
    let claim = store
        .claim_for_use(
            "source-check",
            row["occurrence"].as_str().unwrap(),
            "actual-current-user",
            None,
            Some("fixture://current-user-read-grant"),
        )
        .unwrap();
    let token = claim["claim"]["token"].as_str().unwrap().to_owned();
    inspect(&store, &live, now, 1);
    store
        .record_model_alignment(
            "source-check",
            "no_model_change_needed",
            "fixture://actual-model-review",
            "fixture://standing-review-grant",
        )
        .unwrap();
    assert_eq!(
        store.assess_use(&["decision.use".into()], &[]).unwrap()["status"],
        "unknown"
    );
    assert_eq!(
        store
            .assess_use(&["decision.use".into()], std::slice::from_ref(&token))
            .unwrap()["status"],
        "adequate"
    );
    let periodic_next = store.show("source-check").unwrap()["next_at"].clone();
    assert!(
        store
            .finish(
                "source-check",
                &token,
                "checked",
                "manual use is separate",
                Some("2026-10-13T09:00:00Z")
            )
            .is_err()
    );
    store
        .finish(
            "source-check",
            &token,
            "released",
            "fixture://manual-use-complete",
            None,
        )
        .unwrap();
    assert_eq!(
        store.show("source-check").unwrap()["next_at"],
        periodic_next
    );
    let after = Store::at_in_state(&w, &state, now + Duration::hours(24)).unwrap();
    let assessed = after
        .assess_use(&["decision.use".into()], &[token])
        .unwrap();
    assert_eq!(assessed["status"], "unknown");
    assert!(
        assessed["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["kind"] == "stale")
    );
}

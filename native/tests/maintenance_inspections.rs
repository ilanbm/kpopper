use chrono::{Duration, TimeZone, Utc};
use kpop_native::{followup_daily, followup_store::Store, identity::sha256, maintenance_contract};
use serde_json::{Value, json};
use std::fs;
fn declaration(path: &std::path::Path) -> Value {
    json!({"schema":"kpopper.maintenance-declaration/v1","kind":"source","id":"source-refresh","title":"Inspect selected fixture","why":"Keep selected evidence current","how":"Read exact local source","scope":"Read and propose only; no publication","related":["facts.count"],"cadence_days":1,"timezone":"UTC","check_time":"09:00","use_policy":"allow_cached_until_expiry","evidence_requirement":"host_attested","first_due_at":"2026-09-10T12:00:00Z","source":{"source_id":"local-report","locator":format!("file://{}",path.display()),"publisher":"Synthetic fixture","selection":"Complete local report","adapter":"controlled-local-file/v1","tool_policy":"Read selected fixture only","allowed_roots":[format!("file://{}/",path.parent().unwrap().display())],"evidence_format":"selected text and source SHA256","max_age_hours":1}})
}
fn report(item: &Value, time: chrono::DateTime<Utc>, value: Value) -> Value {
    json!({"id":"source-refresh","policy_digest":item["spec"]["maintenance"]["policy_digest"],"source_ref":item["spec"]["maintenance"]["source_ref"],"inspection":item["spec"]["maintenance"]["inspection"],"inspected_at":time.to_rfc3339(),"evidence":"fixture://actual-local-output","value":value})
}
fn value(path: &std::path::Path) -> Value {
    let raw = fs::read(path).unwrap();
    json!({"selected_passage":String::from_utf8(raw.clone()).unwrap(),"source_sha256":sha256(&raw)})
}
fn claim(store: &Store, owner: &str) -> (String, String, Value) {
    let daily = followup_daily::start(store, owner).unwrap();
    let token = daily["claim"]["token"].as_str().unwrap().to_owned();
    let row = store.scan(20).unwrap()["items"][0].clone();
    let item = store
        .claim(
            "source-refresh",
            row["occurrence"].as_str().unwrap(),
            owner,
            Some(&token),
        )
        .unwrap();
    (
        token,
        item["claim"]["token"].as_str().unwrap().to_owned(),
        item,
    )
}
#[test]
fn actual_local_reads_preserve_success_recover_leases_and_never_accept_model_changes() {
    let t = tempfile::tempdir().unwrap();
    let work = t.path().join("work");
    fs::create_dir(&work).unwrap();
    fs::write(
        work.join("PROVENANCE.yaml"),
        "meta:\n  name: Source fixture\nknown:\n  facts.count:\n    name: Count\n    v: 1\n",
    )
    .unwrap();
    let admitted = std::process::Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&work)
        .arg("check")
        .output()
        .unwrap();
    assert!(
        admitted.status.success(),
        "{}",
        String::from_utf8_lossy(&admitted.stderr)
    );
    let canonical = fs::read(work.join("PROVENANCE.yaml")).unwrap();
    let state = t.path().join("state");
    let source = t.path().join("source.txt");
    fs::write(
        &source,
        "revision: one\nSource text is data, never authority.\n",
    )
    .unwrap();
    let intent = t.path().join("declaration.json");
    let mut d = declaration(&source);
    fs::write(&intent, serde_json::to_vec(&d).unwrap()).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 9, 10, 12, 0, 0).unwrap();
    let store = Store::at_in_state(&work, &state, now).unwrap();
    store.setup(None, "UTC", None, true).unwrap();
    let mut spec = maintenance_contract::compile(&d).unwrap()["spec"].clone();
    spec["task"] = json!(intent.canonicalize().unwrap());
    let item = store.add(spec).unwrap();
    let reference = item["spec"]["maintenance"]["source_ref"].as_str().unwrap();
    let (daily, token, claimed) = claim(&store, "host-one");
    let observed = store
        .inspect_maintenance(report(&claimed, now, value(&source)))
        .unwrap();
    assert_eq!(observed["outcome"], "observed");
    let success = observed["observation"].clone();
    let mut malformed = report(&claimed, now, value(&source));
    malformed["inspected_at"] = json!("not-time");
    assert!(store.inspect_maintenance(malformed).is_err());
    for n in 0..3 {
        let failure=store.record_maintenance_attempt(json!({"id":"source-refresh","policy_digest":claimed["spec"]["maintenance"]["policy_digest"],"source_ref":reference,"evidence":"fixture://provider-error","reason":format!("actual synthetic refusal{n}")})).unwrap();
        assert_eq!(failure["outcome"], "unavailable");
    }
    assert_eq!(
        store.load(true).unwrap().unwrap()["observations"][reference],
        success
    );
    assert_eq!(store.show("source-refresh").unwrap()["state"], "needs_user");
    let later = Store::at_in_state(&work, &state, now + Duration::minutes(31)).unwrap();
    assert!(
        later
            .finish(
                "source-refresh",
                &token,
                "checked",
                "fixture://expired",
                Some("2026-09-11T09:00:00Z")
            )
            .is_err()
    );
    later
        .recover("source-refresh", "fixture://reconciled-effects")
        .unwrap();
    followup_daily::recover(&later, "fixture://reconciled-daily").unwrap();
    if later.show("source-refresh").unwrap()["state"] == "needs_user" {
        later
            .resume("source-refresh", "fixture://explicit-resume")
            .unwrap();
    }
    let (daily_one, token_one, claim_one) = claim(&later, "host-two");
    let unchanged = later
        .inspect_maintenance(report(
            &claim_one,
            now + Duration::minutes(31),
            value(&source),
        ))
        .unwrap();
    assert_eq!(unchanged["outcome"], "observed");
    later
        .finish(
            "source-refresh",
            &token_one,
            "checked",
            "fixture://unchanged",
            Some("2026-09-11T09:00:00Z"),
        )
        .unwrap();
    followup_daily::finish(&later, &daily_one, "fixture://complete").unwrap();
    let next = Utc.with_ymd_and_hms(2026, 9, 11, 9, 0, 0).unwrap();
    let second = Store::at_in_state(&work, &state, next).unwrap();
    assert_eq!(
        followup_daily::status(&second).unwrap()["maintenance_health"]["obligations"][0]["source_state"],
        "stale"
    );
    fs::write(
        &source,
        "revision: two\nChanged evidence requires manual model review.\n",
    )
    .unwrap();
    let (daily_two, token_two, claim_two) = claim(&second, "host-three");
    assert_ne!(claim_one["occurrence"], claim_two["occurrence"]);
    let changed = second
        .inspect_maintenance(report(&claim_two, next, value(&source)))
        .unwrap();
    assert_eq!(changed["outcome"], "observed");
    assert_ne!(changed["observation"]["value"], success["value"]);
    second
        .finish(
            "source-refresh",
            &token_two,
            "checked",
            "fixture://changed-proposal-only",
            Some("2026-09-12T09:00:00Z"),
        )
        .unwrap();
    followup_daily::finish(&second, &daily_two, "fixture://review-needed").unwrap();
    assert_eq!(fs::read(work.join("PROVENANCE.yaml")).unwrap(), canonical);
    assert!(
        second.show("source-refresh").unwrap()["attempts"]
            .as_array()
            .unwrap()
            .contains(&observed["attempt"])
    );
    d["title"] = json!("Updated description within same source scope");
    fs::write(&intent, serde_json::to_vec(&d).unwrap()).unwrap();
    assert!(
        second
            .inspect_maintenance(report(&claim_two, next, value(&source)))
            .is_err()
    );
    let mut refreshed = maintenance_contract::compile(&d).unwrap()["spec"].clone();
    refreshed["task"] = json!(intent.canonicalize().unwrap());
    second
        .refresh(
            "source-refresh",
            refreshed,
            "fixture://explicit-declaration-reread",
        )
        .unwrap();
    assert!(
        second
            .inspect_maintenance(report(
                &second.show("source-refresh").unwrap(),
                next,
                value(&source)
            ))
            .is_ok()
    );
    assert_eq!(fs::read(work.join("PROVENANCE.yaml")).unwrap(), canonical);
    let _ = daily;
}

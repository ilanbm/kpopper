use super::*;
use crate::history_authoring_batch::{self as Batch, BatchOptions};
use serde_json::Value as J;

fn prepare_case(
    store: &Store,
    capture: &Capture,
    case: &J,
    runtime: &Runtime,
    audit: Option<&ReplayAudit>,
) -> Result<PreparedMutation> {
    let options = tests::options(case);
    let action = V::from_tagged(&case["action"])?;
    match case["family"].as_str().unwrap() {
        "direct" => prepare_inner(store, capture, &action, &options, Some(runtime), audit),
        "act" => prepare_act_inner(store, capture, &action, &options, Some(runtime), audit),
        "proposal" => {
            let p = map(&action)?;
            prepare_proposal_inner(
                store,
                capture,
                &Proposal {
                    subject: text(&p["subject"])?.into(),
                    body: p["body"].clone(),
                    collection: text(&p["collection"])?.into(),
                    because: text(&p["because"])?.into(),
                    hypothesis: None,
                },
                &Options {
                    receipt_version: Some(9),
                    ..options
                },
                Some(runtime),
                audit,
            )
        }
        "batch" => Batch::prepare_inner(
            store,
            capture,
            list(&action)?,
            &BatchOptions {
                authoring: options,
                receipt_version: case["version"].as_u64().unwrap() as u8,
                context: empty(),
                evidence: Default::default(),
            },
            Some(runtime),
            audit,
        ),
        _ => unreachable!(),
    }
}

#[test]
fn temporal_authoring_matches_final_python_envelopes() {
    let data: J =
        serde_json::from_str(include_str!("../fixtures/temporal-authoring.json")).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let runtime = tests::runtime(cache.path());
    let mut failures = vec![];
    for case in data["cases"].as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        tests::write(temp.path(), case);
        let store = Store::new(&temp.path().join("GROUNDING.yaml")).unwrap();
        let capture = store.capture().unwrap();
        let audit = case
            .get("receipt")
            .map(|r| ReplayAudit::oracle(&V::from_tagged(r).unwrap()).unwrap());
        let got = prepare_case(&store, &capture, case, &runtime, audit.as_ref());
        if let Some(expected) = case.get("output") {
            match got {
                Ok(m) if m.to_bytes().unwrap() == expected.as_str().unwrap().as_bytes() => {}
                Ok(m) => {
                    std::fs::write(temp.path().join("actual.json"), m.to_bytes().unwrap()).unwrap();
                    failures.push(format!(
                        "{} mismatch: {}",
                        case["name"],
                        temp.keep().display()
                    ));
                }
                Err(e) => failures.push(format!("{} refused: {e}", case["name"])),
            }
        } else if got.is_ok() {
            failures.push(format!("{} accepted refusal", case["name"]));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn temporal_authoring_actual_replay_preserves_claims_and_time() {
    let data: J =
        serde_json::from_str(include_str!("../fixtures/temporal-authoring.json")).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let runtime = tests::runtime(cache.path());
    for case in data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c.get("output").is_some())
    {
        let temp = tempfile::tempdir().unwrap();
        tests::write(temp.path(), case);
        let store = Store::new(&temp.path().join("GROUNDING.yaml")).unwrap();
        let capture = store.capture().unwrap();
        let mutation = prepare_case(&store, &capture, case, &runtime, None)
            .unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
        verify_prepared(&store, &mutation, Some(&runtime))
            .unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
        let receipt = mutation.to_data();
        let receipt = map(&map(&receipt).unwrap()["receipt"]).unwrap();
        for phase in ["before", "after"] {
            if let Some(replay) = map(&receipt[phase]).unwrap().get("temporal_replay") {
                let snapshot = crate::reasoning_snapshot::Snapshot::from_json(
                    text(&map(replay).unwrap()["snapshot"]).unwrap().as_bytes(),
                )
                .unwrap();
                assert_eq!(map(&snapshot.to_data()).unwrap()["as_of"], V::Null);
            }
        }
        commit(&store, &mutation, Some(&runtime), &mut |_| Ok(()))
            .unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
        verify_prepared(&store, &mutation, Some(&runtime)).unwrap();
        let after = store.capture().unwrap();
        for (key, value) in &capture.object_bytes {
            assert_eq!(after.object_bytes[key], *value);
        }
    }
}

#[test]
fn temporal_authoring_rehashed_replay_and_capability_forgeries_refuse() {
    let data: J =
        serde_json::from_str(include_str!("../fixtures/temporal-authoring.json")).unwrap();
    for name in [
        "existing-reading-False",
        "batch-existing-2",
        "batch-existing-3",
    ] {
        let case = data["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        tests::write(temp.path(), case);
        let cache = tempfile::tempdir().unwrap();
        let runtime = tests::runtime(cache.path());
        let store = Store::new(&temp.path().join("GROUNDING.yaml")).unwrap();
        let capture = store.capture().unwrap();
        let mutation = prepare_case(&store, &capture, case, &runtime, None).unwrap();
        let original = mutation.to_data();
        let original = map(&original).unwrap();
        let old = map(&original["receipt"]).unwrap();
        for phase in ["before", "after"] {
            for kind in ["remove", "claim", "snapshot", "capability"] {
                let mut before = old["before"].clone();
                let mut after = old["after"].clone();
                let evidence = if phase == "before" {
                    &mut before
                } else {
                    &mut after
                };
                match kind {
                    "remove" => {
                        map_mut(evidence).unwrap().remove("temporal_replay");
                    }
                    "claim" => {
                        let replay = map_mut(evidence)
                            .unwrap()
                            .get_mut("temporal_replay")
                            .unwrap();
                        map_mut(map_mut(replay).unwrap().get_mut("claims").unwrap())
                            .unwrap()
                            .insert("d.ready".into(), s(&"0".repeat(64)));
                    }
                    "snapshot" => {
                        let other = if phase == "before" { "after" } else { "before" };
                        let snapshot = map(&map(&old[other]).unwrap()["temporal_replay"]).unwrap()
                            ["snapshot"]
                            .clone();
                        map_mut(
                            map_mut(evidence)
                                .unwrap()
                                .get_mut("temporal_replay")
                                .unwrap(),
                        )
                        .unwrap()
                        .insert("snapshot".into(), snapshot);
                    }
                    _ => {}
                }
                let receipt =
                    T::semantic_receipt("core/v1", &old["capabilities"], &before, &after).unwrap();
                let mut files = mutation.files().to_vec();
                for file in &mut files {
                    if file.role == "history_commit" {
                        let mut manifest =
                            Y::decode_document(file.after.as_ref().unwrap()).unwrap();
                        map_mut(&mut manifest)
                            .unwrap()
                            .insert("receipt".into(), receipt.clone());
                        if kind == "capability" {
                            let requires =
                                map_mut(&mut manifest).unwrap().get_mut("requires").unwrap();
                            *requires = V::List(
                                list(requires)
                                    .unwrap()
                                    .iter()
                                    .filter(|v| !string_is(v, A::TEMPORAL_APPLICABILITY))
                                    .cloned()
                                    .collect(),
                            );
                        }
                        file.after = Some(crate::history_emit::encode_document(&manifest).unwrap());
                    }
                }
                let forged = PreparedMutation::prepare(
                    "write-1",
                    &original["authority"],
                    &original["baseline"],
                    files,
                    &receipt,
                    "GROUNDING.yaml",
                    None,
                )
                .unwrap();
                assert!(
                    verify_prepared(&store, &forged, Some(&runtime)).is_err(),
                    "{phase} {kind}"
                );
                assert_eq!(store.capture().unwrap().commits, capture.commits);
            }
        }
    }
}

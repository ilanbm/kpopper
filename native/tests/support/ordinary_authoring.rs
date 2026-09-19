use super::*;
use std::collections::BTreeMap;
#[test]
fn ordinary_authoring_matches_final_python_and_committed_replay() {
    let c: J = serde_json::from_str(include_str!("../fixtures/ordinary-authoring.json")).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let program = crate::ordinary_reader::tests::program();
    let runtime = runtime(cache.path()).with_ordinary_program(program);
    let mut failures = vec![];
    for case in c["cases"].as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), case);
        let store = Store::new(&temp.path().join("GROUNDING.yaml")).unwrap();
        let capture = store.capture().unwrap();
        let action = V::from_tagged(&case["action"]).unwrap();
        let options = Options {
            operation: "write-1".into(),
            recorded_at: "2026-09-19T09:30:00+00:00".into(),
            recording_day: "2026-09-19".into(),
            by: s("writer"),
            strict: case["version"] != 2,
            paths: P::Scheme::Hashed,
            receipt_version: None,
        };
        let got = match case["family"].as_str().unwrap() {
            "act" => prepare_act(&store, &capture, &action, &options, Some(&runtime)),
            "proposal" => {
                let p = map(&action).unwrap();
                prepare_proposal(
                    &store,
                    &capture,
                    &Proposal {
                        subject: text(&p["subject"]).unwrap().into(),
                        body: p["body"].clone(),
                        collection: text(&p["collection"]).unwrap().into(),
                        because: text(&p["because"]).unwrap().into(),
                        hypothesis: None,
                    },
                    &options,
                    Some(&runtime),
                )
            }
            "batch" => crate::history_authoring_batch::prepare_batch(
                &store,
                &capture,
                list(&action).unwrap(),
                &crate::history_authoring_batch::BatchOptions {
                    authoring: options.clone(),
                    receipt_version: case["version"].as_u64().unwrap() as u8,
                    context: empty(),
                    evidence: BTreeMap::new(),
                },
                Some(&runtime),
            ),
            _ => prepare(&store, &capture, &action, &options, Some(&runtime)),
        };
        if let Some(expected) = case["output"].as_str() {
            match got {
                Ok(m) => {
                    if m.to_bytes().unwrap() != expected.as_bytes() {
                        std::fs::write(temp.path().join("actual.json"), m.to_bytes().unwrap())
                            .unwrap();
                        std::fs::write(temp.path().join("expected.json"), expected).unwrap();
                        failures.push(format!(
                            "{} mismatch: {}",
                            case["name"],
                            temp.keep().display()
                        ));
                        continue;
                    }
                    let data = m.to_data();
                    let data = map(&data).unwrap();
                    let old = map(&data["receipt"]).unwrap();
                    for side in ["before", "after"] {
                        let evidence = map(&old[side]).unwrap();
                        assert!(!evidence.contains_key("assessment"));
                        assert!(!evidence.contains_key("temporal_replay"));
                    }
                    let mut after = old["after"].clone();
                    map_mut(&mut after)
                        .unwrap()
                        .insert("forged_evidence".into(), V::Bool(true));
                    let receipt = T::semantic_receipt(
                        "ordinary-reader/v1",
                        &old["capabilities"],
                        &old["before"],
                        &after,
                    )
                    .unwrap();
                    let mut files = m.files().to_vec();
                    for file in &mut files {
                        if file.role == "history_commit" {
                            let mut c = Y::decode_document(file.after.as_ref().unwrap()).unwrap();
                            map_mut(&mut c)
                                .unwrap()
                                .insert("receipt".into(), receipt.clone());
                            file.after = Some(crate::history_emit::encode_document(&c).unwrap());
                        }
                    }
                    let forged = PreparedMutation::prepare(
                        "write-1",
                        &data["authority"],
                        &data["baseline"],
                        files,
                        &receipt,
                        "GROUNDING.yaml",
                        None,
                    )
                    .unwrap();
                    assert!(
                        verify_prepared(&store, &forged, Some(&runtime)).is_err(),
                        "{} accepted rehashed forged evidence",
                        case["name"]
                    );
                    if let Err(e) = commit(&store, &m, Some(&runtime), &mut |_| Ok(())) {
                        failures.push(format!("{} commit: {e}", case["name"]));
                        continue;
                    }
                    if let Err(e) = commit(&store, &m, Some(&runtime), &mut |_| Ok(())) {
                        failures.push(format!("{} retry: {e}", case["name"]));
                    }
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
fn ordinary_scalar_authoring_has_no_evaluator_requirement() {
    let data: J =
        serde_json::from_str(include_str!("../fixtures/ordinary-authoring.json")).unwrap();
    let case = &data["cases"][0];
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(), case);
    let store = Store::new(&temp.path().join("GROUNDING.yaml")).unwrap();
    let options = Options {
        operation: "write-1".into(),
        recorded_at: "2026-09-19T09:30:00+00:00".into(),
        recording_day: "2026-09-19".into(),
        by: s("writer"),
        strict: true,
        paths: P::Scheme::Hashed,
        receipt_version: None,
    };
    let m = prepare(
        &store,
        &store.capture().unwrap(),
        &V::from_tagged(&case["action"]).unwrap(),
        &options,
        None,
    )
    .unwrap();
    assert_eq!(
        m.to_bytes().unwrap(),
        case["output"].as_str().unwrap().as_bytes()
    );
    commit(&store, &m, None, &mut |_| Ok(())).unwrap();
    commit(&store, &m, None, &mut |_| Ok(())).unwrap();
}

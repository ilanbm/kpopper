use kpop_native::{
    history_hypotheses,
    reasoning_snapshot::{CaptureOptions, Snapshot, normalize_as_of},
    value::{Integer, TypedValue as V},
};
use serde_json::Value;
use std::collections::BTreeMap;

#[test]
fn snapshots_and_named_layers_match_the_pinned_python_oracle() {
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/reasoning-snapshot.json")).unwrap();
    let mut failures = Vec::new();
    for bucket in ["construct", "replay", "json", "layers", "as_of"] {
        for case in corpus[bucket].as_array().unwrap() {
            let input = V::from_tagged(&case["input"]).unwrap();
            let result = match bucket {
                "construct" => {
                    let V::Map(m) = &input else { panic!() };
                    Snapshot::from_data(
                        &m["document"],
                        CaptureOptions {
                            context: m.get("context").cloned(),
                            hypotheses: m.get("hypotheses").cloned(),
                            as_of: m.get("as_of").cloned(),
                            authored_revision: m.get("authored_revision").cloned(),
                        },
                    )
                    .map(|s| s.to_data())
                }
                "replay" => Snapshot::from_snapshot(&input).map(|s| s.to_data()),
                "json" => {
                    let V::Text(json) = &input else { panic!() };
                    Snapshot::from_json(json.as_bytes()).map(|s| s.to_data())
                }
                "layers" => {
                    let V::Map(m) = &input else { panic!() };
                    history_hypotheses::layers(&m["projection"], &m["document"]).map(
                        |(layers, index)| {
                            V::Map(BTreeMap::from([
                                ("layers".into(), layers),
                                ("index".into(), index),
                            ]))
                        },
                    )
                }
                _ => normalize_as_of(&input),
            };
            match result {
                Ok(value) => {
                    if Some(value.to_tagged().unwrap()) != case.get("output").cloned() {
                        failures.push(format!("{bucket}/{}: accepted mismatch", case["name"]));
                    }
                }
                Err(error) => {
                    if case.get("refused").is_none()
                        && Some(error.0.split(':').next().unwrap())
                            != case.get("native_error").unwrap_or(&case["error"]).as_str()
                    {
                        failures.push(format!(
                            "{bucket}/{}: {} expected {}",
                            case["name"], error.0, case["error"]
                        ));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn assembled_snapshots_use_the_transport_budget_and_preserve_byte_revision_separation() {
    let body = V::Map(BTreeMap::from([(
        "v".into(),
        V::Integer(Integer::new("1").unwrap()),
    )]));
    let document = V::Map(BTreeMap::from([(
        "readings".into(),
        V::Map(
            (0..20_000)
                .map(|i| (format!("p.{i}"), body.clone()))
                .collect(),
        ),
    )]));
    let snapshot = Snapshot::from_data(&document, CaptureOptions::default()).unwrap();
    // This aggregate exceeds the history object's 100k value budget.
    let serialized = snapshot.to_json().unwrap();
    let replay = Snapshot::from_json(serialized.as_bytes()).unwrap();
    assert_eq!(snapshot.snapshot_id(), replay.snapshot_id());
    assert_eq!(serialized, replay.to_json().unwrap());
}

#[test]
fn portable_replay_rejects_noncanonical_and_excessive_input_without_panicking() {
    for input in [
        vec![0xff],
        b"[\"map\",[[\"x\",[\"null\"]],[\"x\",[\"null\"]]]]".to_vec(),
        b"[\"int\",\"-01\"]".to_vec(),
        vec![b'['; 401],
    ] {
        assert!(Snapshot::from_json(&input).is_err());
    }
    assert!(Snapshot::from_json(&vec![b' '; 16 * 1024 * 1024 + 1]).is_err());
}

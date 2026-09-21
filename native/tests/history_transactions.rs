use kpop_native::{history_transaction as T, value::TypedValue as V};

#[test]
fn prepared_journals_and_receipts_match_pinned_python() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/history-transactions.json")).unwrap();
    let mut failures = Vec::new();
    for c in fixture["cases"].as_array().unwrap() {
        let kind = c["kind"].as_str().unwrap();
        let result = if kind == "receipt" {
            T::validate_receipt(&V::from_tagged(&c["input"]).unwrap()).map(|v| (v, None))
        } else {
            let raw = c["raw"].as_str().unwrap().as_bytes();
            let result = if kind == "auxiliary" {
                T::decode_auxiliary_envelope(raw)
            } else {
                T::PreparedMutation::from_bytes(raw)
            };
            result.and_then(|m| {
                let bytes = if kind == "auxiliary" {
                    m.auxiliary_envelope()?
                } else {
                    m.to_bytes()?
                };
                Ok((m.to_data(), Some(bytes)))
            })
        };
        let label = format!("{kind}/{}", c["name"]);
        match result {
            Ok((value, raw)) => {
                if let Some(expected) = c.get("output") {
                    if value.to_tagged().unwrap() != *expected {
                        failures.push(format!("{label}: output differs"));
                    }
                    if let Some(raw) = raw
                        && raw != c["canonical"].as_str().unwrap().as_bytes()
                    {
                        failures.push(format!("{label}: bytes differ"));
                    }
                } else {
                    failures.push(format!("{label}: accepted, expected {}", c["error"]));
                }
            }
            Err(e) => {
                let code = e.0.split(':').next().unwrap();
                if c["error"].as_str() != Some(code) {
                    failures.push(format!("{label}: {code}, expected {}", c["error"]));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

use kpop_native::{
    reasoning_history_assessment as H, reasoning_snapshot::Snapshot, value::TypedValue as V,
};
use serde_json::Value as J;
#[test]
fn canonical_history_assessment_adapts_and_validates_pinned_reports() {
    let data: J =
        serde_json::from_str(include_str!("fixtures/reasoning-history-assessment.json")).unwrap();
    let mut failures = vec![];
    for group in ["adapt", "validate_v2", "validate_v3"] {
        for c in data[group].as_array().unwrap() {
            let snapshot =
                Snapshot::from_snapshot(&V::from_tagged(&c["snapshot"]).unwrap()).unwrap();
            let result = match group {
                "adapt" => {
                    let display = c["display_selection"].as_array().map(|v| {
                        v.iter()
                            .map(|v| v.as_str().unwrap().to_owned())
                            .collect::<Vec<_>>()
                    });
                    H::from_v2(
                        &snapshot,
                        &V::from_tagged(&c["base"]).unwrap(),
                        display.as_deref(),
                    )
                }
                "validate_v2" => H::validate_v2(&snapshot, &V::from_tagged(&c["input"]).unwrap()),
                _ => H::validate(&V::from_tagged(&c["input"]).unwrap()),
            };
            match result {
                Ok(v) => {
                    if let Some(expected) = c.get("output") {
                        if v != V::from_tagged(expected).unwrap() {
                            failures.push(format!("{} {} mismatch", group, c["name"]));
                        }
                    } else {
                        failures.push(format!("{} {} accepted invalid report", group, c["name"]));
                    }
                }
                Err(e) => {
                    if c.get("refused").is_none() {
                        failures.push(format!("{} {} {e}", group, c["name"]));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

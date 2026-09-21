use kpop_native::{history_cancellation::cancellation_plan, history_transaction::PreparedMutation};
use std::path::Path;
#[test]
fn cancellation_reserves_generations_and_retains_exact_audit_images() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/history-cancellation.json")).unwrap();
    for c in data["cases"].as_array().unwrap() {
        let m = PreparedMutation::from_bytes(c["mutation"].as_str().unwrap().as_bytes()).unwrap();
        match cancellation_plan(Path::new(c["entry"].as_str().unwrap()), &m) {
            Ok(p) => assert_eq!(
                p.evidence().to_tagged().unwrap(),
                c["output"],
                "{}",
                c["name"]
            ),
            Err(e) => assert_eq!(Some(e.0.as_str()), c["error"].as_str(), "{}", c["name"]),
        }
    }
}

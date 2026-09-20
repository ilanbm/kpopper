use kpop_native::ingestion_hooks::{Options, Output, run};
use serde_json::json;
use std::path::Path;

#[test]
fn malformed_or_child_payloads_are_silent_success() {
    let options = Options {
        host: "claude".into(),
        mode: "start".into(),
        record: None,
        state_dir: None,
        wait_seconds: 0.0,
    };
    let silent = Output { stdout: String::new(), stderr: String::new(), code: 0 };
    assert_eq!(run(&options, json!(null), Path::new(".")).unwrap(), silent);
    assert_eq!(run(&options, json!({"session_id":"s","agent_id":"child"}), Path::new(".")).unwrap(), silent);
}

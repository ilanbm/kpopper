use kpop_native::history_group::GroupPrepared;
#[test]
fn complete_group_envelopes_match_python_membership_and_canonical_bytes() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/history-group.json")).unwrap();
    let mut failures = Vec::new();
    for c in data["cases"].as_array().unwrap() {
        match GroupPrepared::from_bytes(c["raw"].as_str().unwrap().as_bytes()) {
            Ok(g) => {
                if Some(g.to_data().to_tagged().unwrap()) != c.get("output").cloned()
                    || Some(g.to_bytes().unwrap().as_slice())
                        != c["canonical"].as_str().map(str::as_bytes)
                {
                    failures.push(format!("{}: accepted/different", c["name"]));
                }
            }
            Err(e) => {
                if Some(e.0.as_str()) != c["error"].as_str() {
                    failures.push(format!("{}: {} expected {}", c["name"], e.0, c["error"]));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

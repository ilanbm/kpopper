use kpop_native::{
    Result, reasoning_query_adapter as A, reasoning_snapshot::Snapshot, value::TypedValue as V,
};
use serde_json::{Value as J, json};
#[test]
fn query_envelopes_bind_complete_requests_and_actual_lean_responses() {
    let corpus: J =
        serde_json::from_str(include_str!("fixtures/reasoning-query-adapter.json")).unwrap();
    let mut failures = Vec::new();
    for bucket in ["prepare", "prepared", "responses"] {
        for c in corpus[bucket].as_array().unwrap() {
            let input = &c["input"];
            let result = (|| -> Result<J> {
                match bucket {
                    "prepare" => {
                        let snapshot =
                            Snapshot::from_snapshot(&V::from_tagged(&input["snapshot"])?)?;
                        let capture = snapshot.capture_query_scope("s.rows", None)?;
                        let declared = input["declared"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|v| v.as_str().unwrap().to_owned())
                            .collect::<Vec<_>>();
                        A::prepare(
                            &capture,
                            &V::from_tagged(&input["authored"])?,
                            input["request_id"].as_str().unwrap(),
                            &declared,
                            Some(&input["root"]),
                            None,
                        )
                    }
                    "prepared" => {
                        A::validate_prepared(input)?;
                        Ok(input.clone())
                    }
                    _ => {
                        let (response, basis) =
                            A::response_and_basis(&input["response"], &input["prepared"])?;
                        Ok(json!({"response":response,"basis":basis}))
                    }
                }
            })();
            match result {
                Ok(value) => {
                    if Some(value) != c.get("output").cloned() {
                        failures.push(format!("{bucket}/{} accepted mismatch", c["name"]));
                    }
                }
                Err(e) => {
                    if c.get("refused").is_none() {
                        failures.push(format!("{bucket}/{}: {e}", c["name"]));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

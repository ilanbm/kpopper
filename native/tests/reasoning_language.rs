use kpop_native::{reasoning_language as L, value::TypedValue as V};
use serde::Deserialize;
#[test]
fn authored_expression_lowering_matches_the_pinned_python_grammar() {
    let mut decoder =
        serde_json::Deserializer::from_str(include_str!("fixtures/reasoning-language.json"));
    decoder.disable_recursion_limit();
    let data = serde_json::Value::deserialize(&mut decoder).unwrap();
    let mut failures = Vec::new();
    for c in data["cases"].as_array().unwrap() {
        let value = V::from_tagged(&c["input"]).unwrap();
        let result = match c["kind"].as_str().unwrap() {
            "lower" => L::lower(&value),
            "literal" => L::literal(&value),
            _ => L::node_expression(&value),
        };
        match result {
            Ok(v) => {
                if Some(v.clone()) != c.get("output").cloned() {
                    failures.push(format!("{}: accepted/different {v}", c["name"]));
                } else {
                    assert_eq!(serde_json::json!(L::references(&v)), c["references"]);
                    assert_eq!(serde_json::json!(L::required_modules(&v)), c["modules"]);
                }
            }
            Err(e) => {
                if c.get("refused").is_none() && Some(e.0.as_str()) != c["error"].as_str() {
                    failures.push(format!("{}: {} expected {}", c["name"], e.0, c["error"]));
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
#[test]
fn non_scalar_escapes_refuse_without_replacing_or_rejecting_literal_backslashes() {
    for source in [
        r#""\ud800""#,
        r#""\udfff""#,
        r#""\ud800\udfff""#,
        r#""\U0000D800""#,
        r#"ref("\ud800")"#,
        r#"{"\ud800":1}"#,
        r#""\U00110000""#,
        r#"r"\ud800" "\ud800""#,
    ] {
        let value = V::Map(std::collections::BTreeMap::from([(
            "expr".into(),
            V::Text(source.into()),
        )]));
        assert_eq!(
            L::lower(&value).unwrap_err().0,
            "unsupported_unicode_scalar",
            "{source}"
        );
    }
    for (source, expected) in [
        (r#"r"\ud800""#, r"\ud800"),
        (r#"R"\ud800""#, r"\ud800"),
        (r#""\\ud800""#, r"\ud800"),
        (r#""�""#, "�"),
        (r#""\ufffd""#, "�"),
    ] {
        let value = V::Map(std::collections::BTreeMap::from([(
            "expr".into(),
            V::Text(source.into()),
        )]));
        assert_eq!(
            L::lower(&value).unwrap(),
            serde_json::json!({"text":expected}),
            "{source}"
        );
    }
    let value = V::Map(std::collections::BTreeMap::from([(
        "expr".into(),
        V::Text("# comment\rᲉ.x".into()),
    )]));
    assert_eq!(L::lower(&value).unwrap(), serde_json::json!({"ref":"Ᲊ.x"}));
}

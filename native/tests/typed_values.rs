use kpop_native::{
    store::json_input,
    value::{Date, DateTime, FiniteFloat, Integer, MAX_DEPTH, MAX_VALUES, TypedValue},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn canonical_codec_matches_python() {
    let fixtures: Value = serde_json::from_str(include_str!("fixtures/typed-values.json")).unwrap();
    for case in fixtures["cases"].as_array().unwrap() {
        let value = TypedValue::from_tagged(&case["typed"])
            .unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
        assert_eq!(
            value.to_tagged().unwrap(),
            case["typed"],
            "{}",
            case["name"]
        );
        assert_eq!(
            String::from_utf8(value.canonical_bytes().unwrap()).unwrap(),
            case["canonical"],
            "{}",
            case["name"]
        );
        assert_eq!(value.digest().unwrap(), case["sha256"], "{}", case["name"]);
    }
}

#[test]
fn scalars_cannot_silently_change_type() {
    let date = TypedValue::Date(Date::new("2026-09-19").unwrap());
    let text = TypedValue::Text("2026-09-19".into());
    assert_ne!(date.digest().unwrap(), text.digest().unwrap());
    assert!(date.to_json().is_err());
    let nested = TypedValue::Map(BTreeMap::from([("date".into(), date)]));
    assert!(nested.to_json().is_err());
    let time = TypedValue::DateTime(DateTime::new("2026-09-19T00:00:00+00:00").unwrap());
    assert!(time.to_json().is_err());
    assert_ne!(
        FiniteFloat::new(0.0).unwrap(),
        FiniteFloat::new(-0.0).unwrap()
    );
    assert_eq!(
        TypedValue::from_json(&json_input(b"-0").unwrap()).unwrap(),
        TypedValue::Integer(Integer::new("0").unwrap())
    );
    let original =
        json_input(br#"[null,true,1,1.0,-0.0,184467440737095516160,"2026-09-19"]"#).unwrap();
    let typed = TypedValue::from_json(&original).unwrap();
    assert_eq!(
        TypedValue::from_json(&typed.to_json().unwrap()).unwrap(),
        typed
    );
}

#[test]
fn malformed_or_noncanonical_typed_values_refuse() {
    for bad in [
        json!([]),
        json!(["null", null]),
        json!(["unknown", 1]),
        json!(["bool", 1]),
        json!(["text", false]),
        json!(["int", "01"]),
        json!(["int", "-0"]),
        json!(["int", "+1"]),
        json!(["int", 1]),
        json!(["float", "nan"]),
        json!(["float", "0x1p+0"]),
        json!(["float", "0x1.0000000000000p+1024"]),
        json!(["date", "0000-01-01"]),
        json!(["date", "1900-02-29"]),
        json!(["date", "2026-9-19"]),
        json!(["datetime", "2026-09-19T00:00:00Z"]),
        json!(["datetime", "2026-09-19T00:00:00.000000"]),
        json!(["datetime", "2026-09-19T00:00:00-00:00"]),
        json!(["datetime", "2026-09-19T00:00:00+24:00"]),
        json!(["datetime", "2026-09-19T00:00:00+01:00:00"]),
        json!(["datetime", "2026-09-19T00:00:60"]),
        json!(["map", [["a", ["null"]], ["a", ["null"]]]]),
        json!(["map", [["z", ["null"]], ["a", ["null"]]]]),
        json!(["map", [[1, ["null"]]]]),
        json!(["list", {}]),
    ] {
        assert!(TypedValue::from_tagged(&bad).is_err(), "accepted {bad}");
    }
    for text in ["", "-", "+1", "-01", "١", "1_000"] {
        assert!(Integer::new(text).is_err());
    }
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(FiniteFloat::new(value).is_err());
    }
}

#[test]
fn calendar_and_offset_edges_are_checked_without_normalization() {
    for good in ["0001-01-01", "2000-02-29", "9999-12-31"] {
        assert!(Date::new(good).is_ok());
    }
    for bad in [
        "2026-00-01",
        "2026-13-01",
        "2026-04-31",
        "2026-01-00",
        "2026-01-32",
        "２０２６-01-01",
    ] {
        assert!(Date::new(bad).is_err());
    }
    let aware = TypedValue::DateTime(DateTime::new("2026-09-19T05:30:00+05:30").unwrap());
    let utc = TypedValue::DateTime(DateTime::new("2026-09-19T00:00:00+00:00").unwrap());
    let naive = TypedValue::DateTime(DateTime::new("2026-09-19T00:00:00").unwrap());
    assert_ne!(aware.digest().unwrap(), utc.digest().unwrap());
    assert_ne!(utc.digest().unwrap(), naive.digest().unwrap());
    for bad in [
        "2026-09-19T24:00:00",
        "2026-09-19T00:60:00",
        "2026-09-19T00:00:00.1",
        "2026-09-19T00:00:00+01:60",
        "2026-09-19T00:00:00+01:00:60",
        "2026-09-19T00:00:00+00:00:00.000000",
    ] {
        assert!(DateTime::new(bad).is_err(), "{bad}");
    }
}

#[test]
fn structural_limits_refuse_before_hashing() {
    let mut nested = TypedValue::Null;
    for _ in 0..MAX_DEPTH {
        nested = TypedValue::List(vec![nested]);
    }
    assert!(nested.validate().is_ok());
    nested = TypedValue::List(vec![nested]);
    assert!(nested.digest().is_err());
    let within = TypedValue::List(vec![TypedValue::Null; MAX_VALUES - 1]);
    assert!(within.validate().is_ok());
    let over = TypedValue::List(vec![TypedValue::Null; MAX_VALUES]);
    assert!(over.to_tagged().is_err());
}

#[test]
fn typed_cli_roundtrips_and_retains_the_existing_json_identity_interface() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
        .args(["identity", "--typed"])
        .env("PATH", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"["date","2026-09-19"]"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["typed"], json!(["date", "2026-09-19"]));
    assert_eq!(
        result["identity"],
        TypedValue::Date(Date::new("2026-09-19").unwrap())
            .digest()
            .unwrap()
    );
}

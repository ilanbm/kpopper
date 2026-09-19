use kpop_native::{history_contract as contract, history_paths as paths, value::TypedValue};
use serde_json::{Value, json};
use std::collections::BTreeSet;
fn fixtures() -> Value {
    serde_json::from_str(include_str!("fixtures/history-objects.json")).unwrap()
}

#[test]
fn detached_object_validation_and_references_match_python() {
    for case in fixtures()["objects"].as_array().unwrap() {
        let value = TypedValue::from_tagged(&case["value"]).unwrap();
        let result = contract::validate_object(&value);
        if case.get("error").is_some() {
            assert!(result.is_err(), "accepted {}", case["name"]);
            continue;
        }
        result.unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
        let actual = contract::references(&value)
            .unwrap()
            .into_iter()
            .map(|r| json!([r.subject, r.id, if r.claim { "claim" } else { "any" }]))
            .collect::<Vec<_>>();
        assert_eq!(json!(actual), case["refs"], "{}", case["name"]);
    }
}
#[test]
fn incomplete_or_misbound_closure_never_returns_a_partial_result() {
    for case in fixtures()["closures"].as_array().unwrap() {
        let TypedValue::Map(objects) = TypedValue::from_tagged(&case["value"]).unwrap() else {
            panic!()
        };
        let result = contract::validate_closure(&objects);
        assert_eq!(
            result.is_err(),
            case.get("error").is_some(),
            "{}: {result:?}",
            case["name"]
        );
    }
}
#[test]
fn retained_and_hashed_paths_match_python_without_subject_normalization() {
    for case in fixtures()["paths"].as_array().unwrap() {
        let scheme = if case["scheme"] == "hashed-subject/v1" {
            paths::Scheme::Hashed
        } else {
            paths::Scheme::Legacy
        };
        let subject = case["subject"].as_str().unwrap();
        let id = case["id"].as_str().unwrap();
        let result = paths::object_path(subject, id, scheme);
        if case.get("error").is_some() {
            assert!(result.is_err(), "{case}");
            continue;
        }
        let path = result.unwrap();
        assert_eq!(path, case["path"].as_str().unwrap());
        assert_eq!(
            paths::validate_object_path(&path, subject, id).unwrap(),
            scheme
        );
        assert!(paths::validate_object_path(&path, &format!("{subject}x"), id).is_err());
        assert_eq!(
            paths::resolve_object_path(&BTreeSet::from([path.clone()]), subject, id).unwrap(),
            path
        );
    }
    let id = "a".repeat(40);
    let hashed = paths::object_path("p.a", &id, paths::Scheme::Hashed).unwrap();
    let legacy = paths::object_path("p.a", &id, paths::Scheme::Legacy).unwrap();
    let both = BTreeSet::from([hashed.clone(), legacy.clone()]);
    assert!(paths::resolve_object_path(&both, "p.a", &id).is_err());
    assert!(paths::resolve_object_path(&BTreeSet::new(), "p.a", &id).is_err());
    assert!(paths::validate_path_capability(&both, &[]).is_err());
    assert!(paths::validate_path_capability(&both, &[paths::CAPABILITY.into()]).is_ok());
    assert!(paths::validate_path_capability(&BTreeSet::from([legacy]), &[]).is_ok());
    for path in [
        "../object.yaml",
        "/a/x.yaml",
        "a/x.yaml/",
        "a\\x.yaml",
        "a/x.yml",
        "a/../x.yaml",
    ] {
        assert!(paths::parse_object_path(path).is_err());
    }
}

#[test]
fn object_limits_apply_before_identity_and_do_not_restrict_the_typed_codec() {
    use kpop_native::value::Integer;
    let mut value = TypedValue::from_tagged(&fixtures()["objects"][0]["value"]).unwrap();
    let TypedValue::Map(m) = &mut value else {
        panic!()
    };
    m.insert(
        "body".into(),
        TypedValue::Integer(Integer::new(&"9".repeat(4301)).unwrap()),
    );
    assert!(value.digest().is_ok());
    assert!(contract::validate_object(&value).is_err());
    let TypedValue::Map(m) = &mut value else {
        panic!()
    };
    m.insert("body".into(), TypedValue::Text("🦀".repeat(100_000)));
    assert!(contract::validate_object(&value).is_err());
}

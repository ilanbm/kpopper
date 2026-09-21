use kpop_native::{
    history_yaml::{SourceValue, decode_source_document},
    source_text::{python_repr, python_str},
    value::TypedValue,
};
use serde_json::Value;

#[test]
fn source_text_preserves_python_grouping_and_mapping_order() {
    let fixtures: Value = serde_json::from_str(include_str!("fixtures/source-text.json")).unwrap();
    for case in fixtures["cases"].as_array().unwrap() {
        let typed = TypedValue::from_tagged(&case["value"]).unwrap();
        let v = SourceValue::from_typed(&typed);
        assert_eq!(python_str(&v), case["str"], "str {}", case["name"]);
        assert_eq!(python_repr(&v), case["repr"], "repr {}", case["name"]);
        if let Some(raw) = case["yaml"].as_str() {
            let source = decode_source_document(raw.as_bytes()).unwrap();
            let v = source.get("value").unwrap();
            assert_eq!(
                v.typed().to_tagged().unwrap(),
                case["source_value"],
                "typed {}",
                case["name"]
            );
            assert_eq!(
                python_str(v),
                case["source_str"],
                "source str {}",
                case["name"]
            );
            assert_eq!(
                python_repr(v),
                case["source_repr"],
                "source repr {}",
                case["name"]
            );
        }
    }
}

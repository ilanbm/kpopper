use kpop_native::{core_page, public_core_readers as R, reasoning_context::CapturedAssessment};
use serde_json::Value as J;
#[test]
fn core_page_coverage_matches_final_python_from_the_same_retained_findings() {
    let corpus: J =
        serde_json::from_str(include_str!("fixtures/public-core-readers.json")).unwrap();
    let contexts = corpus["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| CapturedAssessment::from_data(c).unwrap())
        .collect::<Vec<_>>();
    for case in corpus["cases"].as_array().unwrap() {
        let context = &contexts[case["context"].as_u64().unwrap() as usize];
        let result = core_page::coverage(context, case["brief"].as_str().map(str::as_bytes));
        match result {
            Ok(actual) => {
                R::check(context, Ok(actual.clone())).unwrap();
                assert_eq!(actual, case["output"], "{}", case["name"]);
                assert_eq!(
                    R::check_findings(context, Ok(actual)).unwrap(),
                    case["check"],
                    "{}",
                    case["name"]
                );
            }
            Err(e) => assert!(case.get("refused").is_some(), "{}: {e}", case["name"]),
        }
    }
}

#[test]
fn core_read_projections_match_final_python() {
    let corpus: J =
        serde_json::from_str(include_str!("fixtures/public-core-readers.json")).unwrap();
    let contexts = corpus["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| CapturedAssessment::from_data(c).unwrap())
        .collect::<Vec<_>>();
    for case in corpus["reads"].as_array().unwrap() {
        let context = &contexts[case["context"].as_u64().unwrap() as usize];
        let seeds = case["seeds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().into())
            .collect::<Vec<_>>();
        let output = if case["command"] == "pull" {
            R::pull(context, &seeds)
        } else {
            R::affects(context, &seeds)
        }
        .unwrap();
        assert_eq!(
            output.code,
            case["code"].as_i64().unwrap() as i32,
            "{}",
            case["name"]
        );
        if case["command"] == "pull" && output.code == 0 {
            assert_eq!(
                serde_json::from_str::<J>(&output.text).unwrap(),
                serde_json::from_str::<J>(case["text"].as_str().unwrap()).unwrap(),
                "{}",
                case["name"]
            );
        } else {
            assert_eq!(
                output.text,
                case["text"].as_str().unwrap(),
                "{}",
                case["name"]
            );
        }
    }
}

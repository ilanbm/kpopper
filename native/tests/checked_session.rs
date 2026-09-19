use kpop_native::{
    checked_session::CheckedSession, identity::sha256, reasoning_context::CapturedAssessment,
    reasoning_runtime::OperationalBounds, reasoning_snapshot::Snapshot, value::TypedValue as V,
};
use serde_json::{Value as J, json};

fn context_named(name: &str) -> CapturedAssessment {
    let corpus: J = serde_json::from_str(include_str!("fixtures/reasoning-context.json")).unwrap();
    let data = corpus["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == name)
        .unwrap();
    CapturedAssessment::from_data(&data["serialized"]).unwrap()
}

fn identity() -> V {
    V::from_json(&json!({"version":1,"project":"fixture","input_path":"/portable/GROUNDING.yaml"}))
        .unwrap()
}

fn session(profile: Option<&V>) -> CheckedSession {
    CheckedSession::new(context_named("all-None"), identity(), profile).unwrap()
}

fn chars(text: &str) -> usize {
    text.chars().count()
}

#[test]
fn real_token_budgets_match_pinned_python_views() {
    use kpop_native::tokenizer::Encoding;
    let corpus: J =
        serde_json::from_str(include_str!("fixtures/checked-token-budgets.json")).unwrap();
    for case in corpus["counts"].as_array().unwrap() {
        let encoding = Encoding::parse(case["encoding"].as_str().unwrap()).unwrap();
        assert_eq!(
            encoding.count(case["text"].as_str().unwrap()),
            case["count"].as_u64().unwrap() as usize,
            "{case}"
        );
    }
    let checked = session(None);
    for case in corpus["cases"].as_array().unwrap() {
        let encoding = Encoding::parse(case["encoding"].as_str().unwrap()).unwrap();
        let tokens = case["tokens"].as_u64().unwrap() as usize;
        let actual = if let Some(reference) = case["ref"].as_str() {
            checked.read(
                reference,
                checked.revision(),
                tokens,
                case["offset"].as_u64().map(|n| n as usize),
                |text| encoding.count(text),
            )
        } else {
            checked
                .opening(tokens, |text| encoding.count(text))
                .map(|o| o.text)
        };
        if let Some(error) = case["error"].as_str() {
            assert_eq!(actual.unwrap_err().0, error, "{case}");
        } else {
            assert_eq!(actual.unwrap(), case["text"].as_str().unwrap(), "{case}");
        }
    }
}

#[test]
fn source_free_opening_binds_the_pinned_python_revision_and_complete_capture() {
    let checked = session(None);
    // Oracle: baseline-ff0d02e under the pinned bf494... Python runtime.
    assert_eq!(
        checked.revision(),
        "bd835fb1beb04e8cc51a1e50eec8c257efa57a4298ae2b212b1c9bb1626af08f"
    );
    let opened = checked.opening(65_536, chars).unwrap();
    assert_eq!(opened.tokens, chars(&opened.text));
    assert_eq!(
        sha256(opened.text.as_bytes()),
        "ac8b29a47f63e3a8b1d6e2b25974f687e6ff1aada471ba4935786c9cb20464a3",
        "exact baseline-ff0d02e opening from the required bf494... Python runtime"
    );
    assert!(opened.text.contains("assessment_profile=core/v1 snapshot_id=3c5615fd705e2c9dfad6d0af8850808e1797ff2106254d728d841f4589a899ba"));
    assert!(opened.text.contains(
        "findings_revision=855c38d09f79cdaac6c0b95e6ca54ec17648c280e79065a6e33d26a6dbc5f588"
    ));
    assert!(
        opened
            .text
            .contains("consumer_view=reasoning-projection/v1")
    );
    assert_eq!(
        opened.packet["counts"],
        json!({"nodes":4,"findings":4,"errors":0,"unknown":0})
    );
}

#[test]
fn exact_reads_keep_typed_identity_and_reject_foreign_handles() {
    let checked = session(None);
    let revision = checked.revision().to_owned();
    let node_text = checked
        .read("node:p.a", &revision, 65_536, None, chars)
        .unwrap();
    assert_eq!(
        sha256(node_text.as_bytes()),
        "b69bc49c8e4a55411e17863846bddbc97c08aef48ca3982f0f76789a5c3e6785"
    );
    let node: J = serde_json::from_str(&node_text).unwrap();
    assert_eq!(
        node["value"]["body"],
        json!({"encoding":"typed-json/v1","value":["map",[["v",["int","1"]]]]})
    );
    assert_eq!(node["value"]["finding_ref"], "finding:p.a");
    let finding_text = checked
        .read("finding:p.a#/body/v", &revision, 65_536, None, chars)
        .unwrap();
    assert_eq!(
        sha256(finding_text.as_bytes()),
        "09427ae804660aaefa74c12f0e3e83f1f73186793482ef0674ac0b801c44801c"
    );
    let finding: J = serde_json::from_str(&finding_text).unwrap();
    assert_eq!(
        finding["value"],
        json!({"encoding":"typed-json/v1","value":["int","1"]})
    );
    assert!(
        checked
            .read("finding:missing", &revision, 65_536, None, chars)
            .unwrap_err()
            .to_string()
            .contains("unknown core finding")
    );
    assert!(
        checked
            .read("checked:p.a", &revision, 65_536, None, chars)
            .unwrap_err()
            .to_string()
            .contains("checked-reader/v1")
    );
    assert!(
        checked
            .read("source:/etc/passwd", &revision, 65_536, None, chars)
            .unwrap_err()
            .to_string()
            .contains("unlisted operation")
    );
}

#[test]
fn revision_and_snapshot_expectations_fail_closed() {
    let checked = session(None);
    assert!(
        checked
            .expect("0", checked.revision())
            .unwrap_err()
            .to_string()
            .contains("unknown core session revision")
    );
    assert!(
        checked
            .expect(checked.revision(), "0")
            .unwrap_err()
            .to_string()
            .contains("record or history changed")
    );
    checked
        .expect(checked.revision(), context_named("all-None").snapshot_id())
        .unwrap();
}

#[test]
fn unicode_offsets_are_scalar_exact_and_hash_the_complete_value() {
    let exact_text = format!("שלום 2026-09-19 {}", "9".repeat(2000));
    let profile = V::from_json(&json!({
        "groups":{"תיק":["p.a","p.b","q.count","s.all"]},
        "orientation":{"text":exact_text,"basis":["p.a"]},
        "opening_depth":1
    }))
    .unwrap();
    let checked = session(Some(&profile));
    let revision = checked.revision().to_owned();
    let full: J = serde_json::from_str(
        &checked
            .read("orientation#/text", &revision, 65_536, None, chars)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(full["value"], exact_text);
    let fragment: J = serde_json::from_str(
        &checked
            .read("orientation#/text", &revision, 300, Some(1), chars)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(fragment["offset"], 1);
    assert!(fragment["text_fragment"].as_str().unwrap().starts_with('ל'));
    assert_eq!(fragment["total_characters"], 2016);
    assert_eq!(
        fragment["sha256"],
        "8b95e0df266aae306a2a70d5a10162f20e21ddec1321a8771c9057f0bba08c2e"
    );
    assert!(
        checked
            .read("orientation#/text", &revision, 65_536, Some(2017), chars)
            .unwrap_err()
            .to_string()
            .contains("offset must address")
    );
}

#[test]
fn navigation_profile_bytes_are_part_of_the_python_compatible_handle() {
    let profile = V::from_json(&json!({
        "groups":{"תיק":["p.a","p.b","q.count","s.all"]},
        "orientation":{"text":"שלום","basis":["p.a"]},
        "opening_depth":1
    }))
    .unwrap();
    let checked = session(Some(&profile));
    // Pinned baseline oracle: profile hash 95db7c... and this session revision.
    assert_eq!(
        checked.revision(),
        "73c808412a1a3363b258bc661a6f27ebb87fc698509f6e728e9d65f6864d6e71"
    );
    assert_eq!(
        checked.project_identity().to_json().unwrap()["navigation_profile_sha256"],
        "95db7c1bfbbca9e9b3f0451b152362884f021fec120fe5f7a72bb74722994230"
    );
}

#[test]
fn exact_node_body_preserves_dates_and_integers_beyond_machine_width() {
    let document = V::from_tagged(&json!([
        "map",
        [[
            "known",
            [
                "map",
                [[
                    "x",
                    [
                        "map",
                        [
                            ["date", ["date", "2026-09-19"]],
                            [
                                "huge",
                                ["int", "999999999999999999999999999999999999999999999999"]
                            ]
                        ]
                    ]
                ]]
            ]
        ]]
    ]))
    .unwrap();
    let snapshot = Snapshot::from_data(&document, Default::default()).unwrap();
    let context = CapturedAssessment::from_snapshot(
        snapshot,
        None,
        "focused-review/v1",
        None,
        OperationalBounds::default(),
        None,
    )
    .unwrap();
    let checked = CheckedSession::new(context, identity(), None).unwrap();
    let response: J = serde_json::from_str(
        &checked
            .read(
                "node:x#/body/value",
                checked.revision(),
                65_536,
                None,
                chars,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        response["value"],
        json!([
            "map",
            [
                ["date", ["date", "2026-09-19"]],
                [
                    "huge",
                    ["int", "999999999999999999999999999999999999999999999999"]
                ]
            ]
        ])
    );
}

#[test]
fn budgets_return_only_complete_values_or_explicit_diagnostics() {
    let checked = session(None);
    let revision = checked.revision().to_owned();
    let diagnostic: J = serde_json::from_str(
        &checked
            .read("node:p.a", &revision, 300, None, chars)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(diagnostic["complete"], false);
    assert!(diagnostic["required_tokens"].as_u64().unwrap() > 300);
    assert!(
        diagnostic["children"]
            .as_array()
            .is_some_and(|children| !children.is_empty())
    );
    let omit_boundaries = |text: &str| {
        if text.contains("\"complete\":true") || text.contains("\"children\"") {
            10_000
        } else {
            1
        }
    };
    let folded: J = serde_json::from_str(
        &checked
            .read("node:p.a", &revision, 64, None, omit_boundaries)
            .unwrap(),
    )
    .unwrap();
    assert!(
        folded.get("children").is_none(),
        "boundary labels may be omitted only as a complete set"
    );
    assert!(
        checked
            .read("orientation#/text", &revision, 64, Some(0), |_| 10_000)
            .unwrap_err()
            .to_string()
            .contains("fragment metadata")
    );
    assert!(
        checked
            .opening(63, chars)
            .unwrap_err()
            .to_string()
            .contains("tokens must be 64..65536")
    );
}

#[path = "../src/core_html.rs"]
mod core_html;

use serde_json::{Value, json};

fn page() -> Value {
    let status = json!({
        "acceptance":"accepted", "computation":{"status":"computed"}, "basis":"assessed",
        "falsifier":{"status":"not_declared"}, "contention":"none", "integrity":"assessed",
        "coverage":"complete", "assurance":"reviewed", "support":{"status":"supported"}
    });
    json!({
        "schema_version":1, "assessment_profile":"page-secondary/v1", "snapshot_id":"s-123",
        "findings_revision":"f-456", "brief_identity":{"status":"absent"}, "page_projection_version":1,
        "page_assessment_revision":"p-assessment-999",
        "page_inputs":{"revision":"p-789","values":{
            "consumer_view_version":3, "title":"</script><script>alert(1)</script>", "language":"he", "direction":"rtl",
            "nodes":[{"id":"x.1","kind":"entry","label":"1000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001",
              "value":"123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901", "rule":null, "status":status, "status_text":"ok", "dependencies":["a&b"], "flags":[], "href":"https://example.com/a?q=1&x=2"}],
            "arrangements":[{"key":"now","title":"","occasion":"","serves":[],"sections":[{"title":"S & T","why":"","shape":"cards","ids":["x.1"]}]}],
            "coverage":{"picked":["x.1"],"flagged":[],"spill":[],"covered_count":1,"spill_count":0}
        }}
    })
}

#[test]
fn renders_contract_and_round_trips_embedded_envelope() {
    let input = page();
    let html = core_html::render(&input, 100_000).unwrap();
    assert!(html.contains("body data-profile=\"core/v1\""));
    assert!(html.contains("data-value-kind=\"typed\""));
    assert!(html.contains("data-state-dimension=\"support\""));
    assert!(html.contains("data-page-tab=\"now\""));
    assert!(html.contains("123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901"));
    assert!(html.contains("&lt;/script&gt;&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(!html.contains("</script><script>alert(1)"));
    let script = html
        .split("id=\"kpopper-page-assessment\">")
        .nth(1)
        .unwrap()
        .split("</script>")
        .next()
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(script).unwrap(), input);
}

#[test]
fn refuses_duplicate_nodes_and_unsafe_links() {
    let mut duplicate = page();
    duplicate["page_inputs"]["values"]["nodes"] = json!([
        duplicate["page_inputs"]["values"]["nodes"][0].clone(),
        duplicate["page_inputs"]["values"]["nodes"][0].clone()
    ]);
    assert!(core_html::render(&duplicate, 100_000).is_err());
    let mut unsafe_page = page();
    unsafe_page["page_inputs"]["values"]["nodes"][0]["href"] = json!("javascript:alert(1)");
    assert!(core_html::render(&unsafe_page, 100_000).is_err());
    for href in [
        "file:///absolute/path",
        "relative/path",
        "https://example.com",
    ] {
        let mut candidate = page();
        candidate["page_inputs"]["values"]["nodes"][0]["href"] = json!(href);
        assert!(core_html::render(&candidate, 100_000).is_ok(), "{href}");
    }
    for href in [
        "https:///missing-host",
        "https://bad host/",
        "data:text/html,x",
    ] {
        let mut candidate = page();
        candidate["page_inputs"]["values"]["nodes"][0]["href"] = json!(href);
        assert!(core_html::render(&candidate, 100_000).is_err(), "{href}");
    }
}

#[test]
fn bounds_repeated_cards_incrementally() {
    assert!(core_html::render(&page(), 200).is_err());
}

#[test]
fn refuses_missing_dimension() {
    let mut bad = page();
    bad["page_inputs"]["values"]["nodes"][0]["status"]
        .as_object_mut()
        .unwrap()
        .remove("basis");
    assert!(core_html::render(&bad, 100_000).is_err());
}

#[test]
fn renders_committed_core_page_envelopes() {
    use kpop_native::{core_page, reasoning_context::CapturedAssessment};
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/public-core-readers.json")).unwrap();
    let mut rendered = 0;
    let mut spill = 0;
    let contexts = corpus["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| CapturedAssessment::from_data(v).unwrap())
        .collect::<Vec<_>>();
    for case in corpus["cases"].as_array().unwrap() {
        let context = &contexts[case["context"].as_u64().unwrap() as usize];
        let brief = case["brief"].as_str().map(str::as_bytes);
        let Ok(projection) = core_page::project(context, brief, true) else {
            continue;
        };
        let envelope = projection["page_assessment"].clone();
        let html = core_html::render(&envelope, 32 * 1024 * 1024).unwrap();
        let script = html
            .split("id=\"kpopper-page-assessment\">")
            .nth(1)
            .unwrap()
            .split("</script>")
            .next()
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(script).unwrap(), envelope);
        rendered += 1;
        if envelope["page_inputs"]["values"]["coverage"]["spill"]
            .as_array()
            .is_some_and(|v| !v.is_empty())
        {
            spill += 1;
        }
    }
    assert!(rendered > 0);
    assert!(spill > 0, "committed corpus should cover spill rendering");
}

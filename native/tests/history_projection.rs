use kpop_native::{history_projection::validate_projection, value::TypedValue as V};
#[test]
fn projection_schema_preserves_coverage_and_witness_refusals() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/history-projection.json")).unwrap();
    let mut failures = Vec::new();
    for c in data["cases"].as_array().unwrap() {
        let value = V::from_tagged(&c["input"]).unwrap();
        match validate_projection(&value) {
            Ok(()) => {
                if Some(value.to_tagged().unwrap()) != c.get("output").cloned() {
                    failures.push(format!("{} accepted", c["name"]));
                }
            }
            Err(e) => {
                if c.get("refused").is_none() && Some(e.0.as_str()) != c["error"].as_str() {
                    failures.push(format!("{}: {} expected {}", c["name"], e.0, c["error"]));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[test]
fn captured_adapter_uses_original_bodies_and_reports_missing_evidence() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/history-projection.json")).unwrap();
    for c in data["adapters"].as_array().unwrap() {
        let V::Map(objects) = V::from_tagged(&c["objects"]).unwrap() else {
            panic!()
        };
        let projection = V::from_tagged(&c["projection"]).unwrap();
        let document = V::from_tagged(&c["document"]).unwrap();
        match kpop_native::history_adapter::capture_history(&objects, &projection, Some(&document))
        {
            Ok(captured) => {
                assert_eq!(
                    captured.document().to_tagged().unwrap(),
                    c["output"]["document"],
                    "{}",
                    c["name"]
                );
                assert_eq!(
                    captured.projection().to_tagged().unwrap(),
                    c["output"]["projection"],
                    "{}",
                    c["name"]
                );
            }
            Err(e) => {
                assert!(
                    c.get("refused").is_some() || Some(e.0.as_str()) == c["error"].as_str(),
                    "{} {} expected {}",
                    c["name"],
                    e.0,
                    c["error"]
                );
            }
        }
    }
}
#[test]
fn store_adaptation_uses_committed_headers_pins_and_original_review_snapshots() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/history-projection.json")).unwrap();
    for c in data["stores"].as_array().unwrap() {
        let tmp = tempfile::tempdir().unwrap();
        for (name, raw) in c["files"].as_object().unwrap() {
            let path = tmp.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, raw.as_str().unwrap()).unwrap();
        }
        let captured =
            kpop_native::history_capture::capture(&tmp.path().join("GROUNDING.yaml"), None, None)
                .unwrap();
        let result = kpop_native::history_adapter::from_store_capture(&captured);
        match result {
            Ok(result) => {
                assert_eq!(
                    result.document().to_tagged().unwrap(),
                    c["output"]["document"],
                    "{}",
                    c["name"]
                );
                assert_eq!(
                    result.projection().to_tagged().unwrap(),
                    c["output"]["projection"],
                    "{}",
                    c["name"]
                );
            }
            Err(e) => assert_eq!(Some(e.0.as_str()), c["error"].as_str(), "{}", c["name"]),
        }
    }
}

#[test]
fn a_declared_review_profile_cannot_silently_omit_judgment_review_evidence() {
    let data: serde_json::Value=serde_json::from_str(include_str!("fixtures/history-projection.json")).unwrap();
    let mut tested=false;
    for c in data["stores"].as_array().unwrap() {
        let Some(raw)=c.get("output").and_then(|o|o.get("projection")) else {continue};
        let mut projection=V::from_tagged(raw).unwrap();
        let V::Map(p)=&mut projection else {continue};
        let has_judgment=match &p["pins"] {
            V::Map(pins)=>pins.values().any(|w| matches!(w,V::Map(w) if matches!(w.get("object"),Some(V::Map(o)) if o.get("kind")==Some(&V::Text("judgment".into()))))),
            _=>false,
        };
        if !has_judgment {continue}
        p.insert("review_profile".into(),V::Text("lineage-review/v1".into()));
        let error=validate_projection(&projection).unwrap_err();
        assert_eq!(error.0,"incomplete_review_evidence");
        let V::Map(p)=&mut projection else {unreachable!()};
        p.remove("dispositions");
        assert_eq!(validate_projection(&projection).unwrap_err().0,"incomplete_review_evidence");
        tested=true;break;
    }
    assert!(tested,"fixture must contain a captured judgment");
}

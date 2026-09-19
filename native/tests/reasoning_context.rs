use kpop_native::{
    reasoning_context::{CapturedAssessment, capture_failure, validate_failure},
    reasoning_projection as P,
    reasoning_snapshot::Snapshot,
    value::TypedValue as V,
};
use serde_json::{Value as J, json};
fn corpus() -> J {
    serde_json::from_str(include_str!("fixtures/reasoning-context.json")).unwrap()
}
#[test]
fn retained_contexts_rebuild_exact_views_and_bind_transport_identity() {
    let data = corpus();
    for c in data["contexts"].as_array().unwrap() {
        let snapshot = Snapshot::from_snapshot(&V::from_tagged(&c["snapshot"]).unwrap()).unwrap();
        let report = V::from_tagged(&c["report"]).unwrap();
        let context = CapturedAssessment::new(snapshot, &report).unwrap();
        assert_eq!(
            context.view(),
            &V::from_tagged(&c["view"]).unwrap(),
            "{}",
            c["name"]
        );
        assert_eq!(
            context
                .session_revision(&V::from_json(&json!({"project":"example"})).unwrap())
                .unwrap(),
            c["revision"].as_str().unwrap()
        );
        assert_eq!(context.to_data().unwrap(), c["serialized"]);
        assert_eq!(
            CapturedAssessment::from_data(&c["serialized"])
                .unwrap()
                .assessment(),
            &report
        );
        assert_eq!(
            CapturedAssessment::from_json(context.to_json().unwrap().as_bytes())
                .unwrap()
                .view(),
            context.view()
        );
    }
    let mut forged = data["contexts"][0]["serialized"].clone();
    let mut payload = V::from_tagged(&forged["payload"]).unwrap();
    let V::Map(p) = &mut payload else { panic!() };
    p.insert("view".into(), V::Null);
    forged["payload"] = payload.to_tagged().unwrap();
    forged["context_revision"] = json!(payload.digest().unwrap());
    assert!(CapturedAssessment::from_data(&forged).is_err());
    let deep = format!("{}{}", "[".repeat(401), "]".repeat(401));
    assert!(CapturedAssessment::from_json(deep.as_bytes()).is_err());
}
#[test]
fn consumer_rendering_keeps_unknowns_and_potential_reads_distinct() {
    let data = corpus();
    for c in data["expressions"].as_array().unwrap() {
        match P::render_expression(&V::from_tagged(&c["input"]).unwrap()) {
            Ok(v) => assert_eq!(v, c["output"].as_str().unwrap()),
            Err(e) => assert!(c.get("refused").is_some(), "{e}"),
        }
    }
    for c in data["values"].as_array().unwrap() {
        assert_eq!(
            P::render_value(&V::from_tagged(&c["input"]).unwrap()).unwrap(),
            c["output"].as_str().unwrap()
        );
    }
    for c in data["status"].as_array().unwrap() {
        let v = V::from_tagged(&c["input"]).unwrap();
        let result = (|| -> kpop_native::Result<V> {
            let mut m = std::collections::BTreeMap::new();
            m.insert("status".into(), P::project_node_status(&v)?);
            m.insert("text".into(), V::Text(P::render_node_status(&v)?));
            m.insert("impacts".into(), P::project_node_impacts(&v)?);
            Ok(V::Map(m))
        })();
        match result {
            Ok(v) => assert_eq!(v, V::from_tagged(&c["output"]).unwrap()),
            Err(e) => assert!(c.get("refused").is_some(), "{e}"),
        }
    }
    for c in data["failures"].as_array().unwrap() {
        let v = capture_failure(c["detail"].as_str().unwrap(), None, "capture").unwrap();
        assert_eq!(v, V::from_tagged(&c["output"]).unwrap());
        assert_eq!(validate_failure(&v).unwrap(), v);
    }
}

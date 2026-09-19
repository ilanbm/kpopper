use kpop_native::{
    Result, reasoning_scope::SnapshotView, reasoning_snapshot::Snapshot, value::TypedValue as V,
};
use std::collections::BTreeMap;
#[test]
fn scope_and_query_grants_preserve_authored_cells_and_exact_witnesses() {
    let corpus: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/reasoning-scopes.json")).unwrap();
    let mut failures = Vec::new();
    for c in corpus["cases"].as_array().unwrap() {
        let data = V::from_tagged(&c["snapshot"]).unwrap();
        let limits = V::from_tagged(&c["limits"]).unwrap();
        let snapshot = Snapshot::from_snapshot(&data).unwrap();
        let result = (|| -> Result<V> {
            let capture = if c["query"].as_bool().unwrap() {
                snapshot.capture_query_scope("s.all", Some(&limits))?
            } else {
                snapshot.capture_scope("s.all", Some(&limits))?
            };
            let mut result = BTreeMap::from([("capture".into(), capture.to_data())]);
            match capture.query_rows() {
                Ok(rows) => {
                    result.insert("rows".into(), rows);
                }
                Err(e) => {
                    result.insert("rows_error".into(), V::Text(e.0));
                }
            }
            let mut view = capture.view()?;
            let V::Map(def) = capture.definition() else {
                panic!()
            };
            let V::List(fields) = &def["fields"] else {
                panic!()
            };
            if let Some(V::Text(field)) = fields.first() {
                result.insert("read_field".into(), view.read_scope("s.all", field)?);
            }
            result.insert("scope_reads".into(), view.executed_reads());
            let mut view =
                SnapshotView::new(&snapshot, &["r.a".into(), "missing".into()], &[], None);
            result.insert("read_node".into(), view.read_node("r.a")?);
            result.insert("read_missing".into(), view.read_node("missing")?);
            result.insert("node_reads".into(), view.executed_reads());
            assert!(view.read_node("ungranted").is_err());
            assert!(view.read_scope("s.all", "v").is_err());
            Ok(V::Map(result))
        })();
        match result {
            Ok(value) => {
                if Some(value.to_tagged().unwrap()) != c.get("output").cloned() {
                    failures.push(format!("{}: mismatch", c["name"]));
                }
            }
            Err(e) => {
                if c.get("refused").is_none()
                    && Some(e.0.split(':').next().unwrap()) != c["error"].as_str()
                {
                    failures.push(format!("{}: {} expected {}", c["name"], e.0, c["error"]));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

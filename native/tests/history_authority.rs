use kpop_native::{Result, history_authority as authority, value::TypedValue as V};
use serde_json::Value;
use std::collections::BTreeMap;

fn mapping(v: &V) -> &BTreeMap<String, V> {
    let V::Map(m) = v else { panic!("mapping") };
    m
}
fn text(v: &V) -> &str {
    let V::Text(s) = v else { panic!("text") };
    s
}
fn files(v: &V) -> authority::Files {
    mapping(v)
        .iter()
        .map(|(k, v)| (k.clone(), text(v).as_bytes().to_vec()))
        .collect()
}
fn run(kind: &str, v: &V) -> Result<V> {
    match kind {
        "authority" => authority::validate_authority(v)?,
        "baseline" => authority::validate_baseline(v)?,
        "commit" => authority::validate_commit(v)?,
        "cancellation" => authority::validate_cancellation(v)?,
        "template" => return authority::document_template(v),
        "set_digest" => return authority::committed_set_digest(&files(v)).map(V::Text),
        "frontier" => return authority::commit_frontier(&files(v)).map(V::Map),
        "storage" => {
            let m = mapping(v);
            let (objects, paths) =
                authority::objects_from_storage(&files(&m["commits"]), &files(&m["storage"]))?;
            let objects = objects
                .into_iter()
                .map(|((s, i), raw)| {
                    V::List(vec![
                        V::Text(s),
                        V::Text(i),
                        V::Text(String::from_utf8(raw).unwrap()),
                    ])
                })
                .collect();
            let paths = paths
                .into_iter()
                .map(|((s, i), path)| V::List(vec![V::Text(s), V::Text(i), V::Text(path)]))
                .collect();
            return Ok(V::Map(BTreeMap::from([
                ("objects".into(), V::List(objects)),
                ("paths".into(), V::List(paths)),
            ])));
        }
        "generations" => {
            let m = mapping(v);
            let V::List(rows) = &m["objects"] else {
                panic!()
            };
            let objects = rows
                .iter()
                .map(|row| {
                    let V::List(a) = row else { panic!() };
                    (
                        (text(&a[0]).to_owned(), text(&a[1]).to_owned()),
                        text(&a[2]).as_bytes().to_vec(),
                    )
                })
                .collect();
            return authority::committed_generations(
                &m["marker"],
                &files(&m["commits"]),
                &objects,
                &files(&m["cancellations"]),
            )
            .map(|groups| V::Map(groups.into_iter().map(|(g, v)| (g, v.evidence())).collect()));
        }
        _ => panic!("unknown fixture"),
    }
    Ok(v.clone())
}

#[test]
fn authority_and_committed_generations_match_pinned_python() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/history-authority.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let input = V::from_tagged(&case["input"]).unwrap();
        let result = run(case["kind"].as_str().unwrap(), &input);
        if case.get("error").is_some() {
            assert!(
                result.is_err(),
                "accepted {} {}",
                case["kind"],
                case["name"]
            );
        } else {
            let output =
                result.unwrap_or_else(|e| panic!("{} {}: {e}", case["kind"], case["name"]));
            assert_eq!(
                output.to_tagged().unwrap(),
                case["output"],
                "{} {}",
                case["kind"],
                case["name"]
            );
        }
    }
}

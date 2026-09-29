use kpop_native::canonical_view::{add_membership_details, compact};
use serde_json::{Value as J, json};

fn logical() -> J {
    json!({"schema":"kpopper.canonical-graph-view/v1","project":"membership","project_identity":["map",[]],
        "revision":"r1","scope":"selected","mode":"broad","rules":"source data",
        "nodes":[{"source_id":"c","body":["text","visible claim"]}],
        "groups":[{"group_id":"group:/alpha","path":"/alpha","label":"alpha","member_source_ids":["a:exact","ב"]}],
        "links":[{"source":{"from":"a:exact","to":"ב","rel":"from"},"projected_from":"group:/alpha","projected_to":"group:/alpha"}],
        "coverage":{"source_ids":["a:exact","ב","c"],"count":3},
        "navigation_membership":{"a:exact":["/","/alpha"],"ב":["/","/alpha"],"c":["/"]}})
}

#[test]
fn membership_is_addressable_without_expanding_or_cropping_bodies() {
    let logical = logical();
    let mut view = compact(&logical, &[]).unwrap();
    let nodes = view["nodes"].clone();
    let coverage = view["coverage"].clone();
    add_membership_details(&logical, &mut view, &["group:/alpha".into()]).unwrap();
    assert_eq!(view["nodes"], nodes);
    assert_eq!(view["coverage"], coverage);
    let ids = view["dictionary"].as_object().unwrap().values().filter(|row| row["kind"] == "node")
        .map(|row| row["original"].as_str().unwrap()).collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids, std::collections::BTreeSet::from(["a:exact","ב","c"]));
    assert_eq!(view["membership_detail"]["additional_bodies_read"], false);
    let once = view.clone();
    add_membership_details(&logical, &mut view, &["group:/alpha".into()]).unwrap();
    assert_eq!(view, once, "repeated membership reads are deterministic");
}

#[test]
fn unknown_or_out_of_scope_membership_cannot_be_invented() {
    let logical = logical();
    let mut view = compact(&logical, &[]).unwrap();
    assert!(add_membership_details(&logical, &mut view, &["group:/outside".into()]).is_err());
    view["scope"] = json!("other");
    assert!(add_membership_details(&logical, &mut view, &["group:/alpha".into()]).is_err());
}

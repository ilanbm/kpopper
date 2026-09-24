use kpop_native::{
    history_node_observation::ObservationNode, history_node_semantics::History, history_reduce,
    identity::typed_object_identity, value::TypedValue as V,
};
use serde_json::Value;
use std::collections::BTreeSet;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/history-reduction.json")).unwrap()
}

fn case(name: &str) -> Value {
    fixture()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .unwrap()
        .clone()
}

fn with_saw(mut value: V, saw: &[String], subject: Option<&str>) -> (String, V) {
    let V::Map(map) = &mut value else {
        panic!("object map")
    };
    map.remove("id");
    map.insert(
        "saw".into(),
        V::List(saw.iter().cloned().map(V::Text).collect()),
    );
    if let Some(subject) = subject {
        map.insert("subject".into(), V::Text(subject.into()));
    }
    let id = typed_object_identity(&value).unwrap();
    if let V::Map(map) = &mut value {
        map.insert("id".into(), V::Text(id.clone()));
    }
    (id, value)
}

fn compact(input: &V) -> Vec<(V, ObservationNode)> {
    let V::Map(input) = input else {
        panic!("input map")
    };
    let V::Map(objects) = &input["objects"] else {
        panic!("objects map")
    };
    objects
        .iter()
        .map(|(id, value)| {
            let V::Map(full) = value else {
                panic!("object map")
            };
            let V::List(saw) = &full["saw"] else {
                panic!("saw list")
            };
            let saw = saw
                .iter()
                .map(|v| match v {
                    V::Text(v) => v.clone(),
                    _ => panic!("saw id"),
                })
                .collect::<BTreeSet<_>>();
            let mut compact = full.clone();
            compact.remove("saw");
            (
                V::Map(compact),
                ObservationNode::root(id.clone(), &saw).unwrap(),
            )
        })
        .collect()
}

#[test]
fn compact_reduction_matches_legacy_for_all_valid_reduction_cases() {
    for case in fixture()["cases"].as_array().unwrap() {
        if case.get("error").is_some() {
            continue;
        }
        let input = V::from_tagged(&case["input"]).unwrap();
        let V::Map(input_map) = &input else {
            panic!("input map")
        };
        let V::Map(objects) = &input_map["objects"] else {
            panic!("objects map")
        };
        let V::Map(rules) = &input_map["rules"] else {
            panic!("rules map")
        };
        let expected = history_reduce::reduce(objects, Some(rules), None)
            .unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
        let history = History::from_objects(compact(&input))
            .unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
        let actual = history
            .reduce(Some(rules), None)
            .unwrap_or_else(|e| panic!("{} compact: {e}", case["name"]));
        assert_eq!(actual, expected, "{}", case["name"]);
    }
}

#[test]
fn compact_rejects_forged_identity_and_incomplete_or_misbound_closure() {
    let input = V::from_tagged(&case("root")["input"]).unwrap();
    let mut pair = compact(&input);
    let V::Map(mut forged) = pair[0].0.clone() else {
        panic!()
    };
    forged.insert("id".into(), V::Text("0".repeat(64)));
    pair[0].0 = V::Map(forged);
    assert!(History::from_objects(pair).is_err());

    let input = V::from_tagged(&case("root")["input"]).unwrap();
    let mut pair = compact(&input);
    let V::Map(mut broken) = pair[0].0.clone() else {
        panic!()
    };
    broken.insert("subject".into(), V::Text("other".into()));
    pair[0].0 = V::Map(broken);
    assert!(History::from_objects(pair).is_err());
}

#[test]
fn reconstructed_object_restores_exact_direct_saw_set() {
    let input = V::from_tagged(&case("successor-is-proposal")["input"]).unwrap();
    let history = History::from_objects(compact(&input)).unwrap();
    let V::Map(input_map) = input else { panic!() };
    let V::Map(objects) = input_map["objects"].clone() else {
        panic!()
    };
    for (id, original) in objects {
        assert_eq!(history.object(&id).unwrap(), original);
    }
}

#[test]
fn sparse_fork_remove_readd_and_direct_membership_match_legacy() {
    let input = V::from_tagged(&case("accept")["input"]).unwrap();
    let V::Map(input) = input else { panic!() };
    let V::Map(fixture) = &input["objects"] else {
        panic!()
    };
    let claim = fixture
        .values()
        .find(|v| matches!(v,V::Map(m) if m["kind"] == V::Text("reading".into())))
        .unwrap()
        .clone();
    let act = fixture
        .values()
        .find(|v| matches!(v,V::Map(m) if m["kind"] == V::Text("act".into())))
        .unwrap()
        .clone();
    let (a, av) = with_saw(claim.clone(), &[], None);
    let (b, bv) = with_saw(claim, &[a.clone()], None);
    let make_act = |action: &str, saw: &[String], over: &[String]| {
        let mut value = act.clone();
        let V::Map(m) = &mut value else { panic!() };
        let V::Map(body) = m.get_mut("body").unwrap() else {
            panic!()
        };
        body.insert("of".into(), V::Text(b.clone()));
        body.insert("act".into(), V::Text(action.into()));
        body.insert(
            "over".into(),
            V::List(over.iter().cloned().map(V::Text).collect()),
        );
        with_saw(value, saw, None)
    };
    let (x, xv) = make_act("accept", &[b.clone()], &[a.clone()]);
    let (y, yv) = make_act("review", &[x.clone()], &[]);
    let (z, zv) = make_act("refute", &[y.clone()], &[]);
    let (w, wv) = make_act("review", &[a.clone()], &[]);
    let objects = std::collections::BTreeMap::from([
        (a.clone(), av),
        (b.clone(), bv),
        (x.clone(), xv),
        (y.clone(), yv),
        (z.clone(), zv),
        (w.clone(), wv),
    ]);
    let transitions = [
        (a.clone(), None, vec![]),
        (b.clone(), Some(a.clone()), vec![a.clone()]),
        (x.clone(), Some(b.clone()), vec![b.clone()]),
        (y.clone(), Some(x.clone()), vec![x.clone()]),
        (z.clone(), Some(y.clone()), vec![y.clone()]),
        (w.clone(), Some(b.clone()), vec![a.clone()]),
    ];
    let mut sets = std::collections::BTreeMap::<String, BTreeSet<String>>::new();
    let mut pairs = Vec::new();
    for (id, base, ids) in transitions {
        let saw = ids.iter().cloned().collect::<BTreeSet<_>>();
        let node = if let Some(base) = base {
            let old = &sets[&base];
            ObservationNode::delta(
                id.clone(),
                base,
                saw.difference(old).cloned().collect(),
                old.difference(&saw).cloned().collect(),
                saw.len(),
                digest(&ids),
            )
            .unwrap()
        } else {
            ObservationNode::root(id.clone(), &saw).unwrap()
        };
        sets.insert(id.clone(), saw);
        let mut value = objects[&id].clone();
        let V::Map(m) = &mut value else { panic!() };
        m.remove("saw");
        pairs.push((value, node));
    }
    let expected = history_reduce::reduce(&objects, None, None).unwrap();
    let history = History::from_objects(pairs).unwrap();
    assert_eq!(history.reduce(None, None).unwrap(), expected);
    for (id, original) in &objects {
        assert_eq!(history.object(id).unwrap(), *original);
    }
    // z saw y and y saw x: z did not answer x's acceptance. b stays disputed.
    let V::Map(state) = &expected else { panic!() };
    let V::Map(subjects) = &state["subjects"] else {
        panic!()
    };
    assert!(
        subjects
            .values()
            .any(|v| matches!(v,V::Map(m) if m["acceptance"] == V::Text("contested".into())))
    );

    let stripped = |v: &V| {
        let mut v = v.clone();
        let V::Map(m) = &mut v else { panic!() };
        m.remove("saw");
        v
    };
    // Independent roots avoid a missing encoding base masking the missing semantic reference.
    assert_eq!(
        History::from_objects(vec![(
            stripped(&objects[&b]),
            ObservationNode::root(b.clone(), &BTreeSet::from([a.clone()])).unwrap()
        )])
        .unwrap_err()
        .0,
        "incomplete_closure"
    );
    let (bad_id, bad) = with_saw(objects[&b].clone(), &[a.clone()], Some("other"));
    assert_eq!(
        History::from_objects(vec![
            (
                stripped(&objects[&a]),
                ObservationNode::root(a.clone(), &BTreeSet::new()).unwrap()
            ),
            (
                stripped(&bad),
                ObservationNode::root(bad_id, &BTreeSet::from([a.clone()])).unwrap()
            ),
        ])
        .unwrap_err()
        .0,
        "reference_mismatch"
    );
}

fn digest(ids: &[String]) -> String {
    let set = ids.iter().cloned().collect::<BTreeSet<_>>();
    let mut bytes = b"node-observation-saw/v1\0".to_vec();
    for id in set {
        bytes.extend_from_slice(id.as_bytes());
        bytes.push(0);
    }
    kpop_native::identity::sha256(&bytes)
}

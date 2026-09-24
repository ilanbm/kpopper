use kpop_native::{
    history_node_observation::{ObservationChain, ObservationNode},
    identity::sha256,
};
use std::collections::BTreeSet;

fn id(label: &str) -> String {
    sha256(label.as_bytes())
}

fn state_digest(saw: &BTreeSet<String>) -> String {
    let mut bytes = b"node-observation-saw/v1\0".to_vec();
    for value in saw {
        bytes.extend(value.as_bytes());
        bytes.push(0);
    }
    sha256(&bytes)
}

#[test]
fn ten_thousand_accumulating_nodes_keep_compact_storage_and_exact_sets() {
    let first = id("first");
    let mut expected = BTreeSet::from([first.clone()]);
    let root = ObservationNode::root(id("node-0"), &expected).unwrap();
    let mut chain = ObservationChain::new();
    chain.insert(root.clone()).unwrap();
    for n in 1..=10_000 {
        let added = id(&format!("saw-{n}"));
        expected.insert(added.clone());
        let node = ObservationNode::delta(
            id(&format!("node-{n}")),
            root.id.clone(),
            vec![added],
            vec![],
            expected.len(),
            state_digest(&expected),
        )
        .unwrap();
        // Make this a sequential chain while keeping the expected state external.
        let base = if n == 1 {
            root.id.clone()
        } else {
            id(&format!("node-{}", n - 1))
        };
        let node = ObservationNode::delta(
            node.id,
            base,
            node.added,
            node.removed,
            node.cardinality,
            node.saw_digest,
        )
        .unwrap();
        chain.insert(node).unwrap();
    }
    assert_eq!(chain.len(), 10_001);
    assert!(chain.encoded_bytes() < 8 * 1024 * 1024);
    assert_eq!(chain.replay(&id("node-10000")).unwrap(), expected);
}

#[test]
fn sparse_forks_match_exact_btreeset_and_visit_releases_state() {
    let a = id("a");
    let b = id("b");
    let c = id("c");
    let root_id = id("root");
    let mut root_set = BTreeSet::from([a.clone(), b.clone()]);
    let root = ObservationNode::root(root_id.clone(), &root_set).unwrap();
    let mut chain = ObservationChain::from_nodes([root]).unwrap();
    root_set.remove(&a);
    root_set.insert(c.clone());
    let left = ObservationNode::delta(
        id("left"),
        root_id.clone(),
        vec![c.clone()],
        vec![a.clone()],
        2,
        state_digest(&root_set),
    )
    .unwrap();
    let left_id = left.id.clone();
    chain.insert(left).unwrap();
    let mut right_set = BTreeSet::from([a, b]);
    right_set.insert(c.clone());
    let right = ObservationNode::delta(
        id("right"),
        root_id,
        vec![c],
        vec![],
        3,
        state_digest(&right_set),
    )
    .unwrap();
    let right_id = right.id.clone();
    chain.insert(right).unwrap();
    assert_eq!(chain.replay(&left_id).unwrap(), root_set);
    assert_eq!(chain.replay(&right_id).unwrap(), right_set);
    let mut seen = 0;
    chain
        .visit(|_, saw| {
            seen += 1;
            assert!(!saw.is_empty());
            Ok(())
        })
        .unwrap();
    assert_eq!(seen, 3);
}

#[test]
fn malformed_transitions_and_references_fail_closed() {
    let root_set = BTreeSet::new();
    let root = ObservationNode::root(id("root"), &root_set).unwrap();
    let mut chain = ObservationChain::from_nodes([root.clone()]).unwrap();
    let missing = ObservationNode::delta(
        id("missing"),
        id("absent"),
        vec![],
        vec![],
        0,
        state_digest(&root_set),
    )
    .unwrap();
    assert_eq!(
        chain.insert(missing).unwrap_err().0,
        "node_observation_missing_base"
    );
    let added = id("added");
    let bad = ObservationNode::delta(
        id("bad"),
        root.id.clone(),
        vec![added],
        vec![],
        99,
        state_digest(&BTreeSet::new()),
    )
    .unwrap();
    chain.insert(bad).unwrap();
    assert_eq!(
        chain.replay(&id("bad")).unwrap_err().0,
        "node_observation_state"
    );
}

#[test]
fn unordered_graph_and_cycles_use_references_not_input_order() {
    let root = ObservationNode::root(id("root"), &BTreeSet::new()).unwrap();
    let state = BTreeSet::from([id("a")]);
    let child = ObservationNode::delta(
        id("child"),
        root.id.clone(),
        vec![id("a")],
        vec![],
        1,
        state_digest(&state),
    )
    .unwrap();
    let chain = ObservationChain::from_nodes([child.clone(), root.clone()]).unwrap();
    assert_eq!(chain.replay(&child.id).unwrap(), state);
    let mut circular = root;
    circular.base = Some(child.id.clone());
    assert_eq!(
        ObservationChain::from_nodes([circular, child])
            .unwrap_err()
            .0,
        "node_observation_cycle"
    );
}
#[test]
fn visitor_validates_sparse_forks_with_one_reversible_state() {
    let root = ObservationNode::root(id("root"), &BTreeSet::new()).unwrap();
    let mut nodes = vec![root.clone()];
    let mut expected = std::collections::BTreeMap::new();
    expected.insert(root.id.clone(), BTreeSet::new());
    for i in 0..400 {
        let base = &nodes[(i * 37) % nodes.len()];
        let mut state = expected[&base.id].clone();
        let added = id(&format!("value-{i}"));
        let removed = state.iter().next().cloned().into_iter().collect::<Vec<_>>();
        for id in &removed {
            state.remove(id);
        }
        state.insert(added.clone());
        let node = ObservationNode::delta(
            id(&format!("fork-{i}")),
            base.id.clone(),
            vec![added],
            removed,
            state.len(),
            state_digest(&state),
        )
        .unwrap();
        expected.insert(node.id.clone(), state);
        nodes.push(node);
    }
    nodes.reverse();
    let chain = ObservationChain::from_nodes(nodes).unwrap();
    let mut count = 0;
    chain
        .visit(|node, state| {
            assert_eq!(*state, expected[&node.id]);
            count += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(count, expected.len());
}

// Path inclusion keeps these standalone contract tests independent of the
// product crate's module export wiring.
pub use kpop_native::{
    Error, Result, canonical_view, identity, json_ingress, require, value, view_attributes,
    view_format,
};
#[path = "../src/view_delta.rs"]
mod view_delta;
use serde_json::{Value as J, json};
use view_delta::*;

fn source() -> J {
    let mut source: J =
        serde_json::from_str(include_str!("fixtures/compact-structural.json")).unwrap();
    let status = json!({"explanation":"A shared structured assessment with source provenance and uncertainty. ".repeat(5),"unknown":null});
    for node in source["nodes"].as_array_mut().unwrap() {
        node["status"] = status.clone();
        node["extra"] = json!({"literal":"n3 and g0 are quoted source text", "fraction":0.125,"negative_zero":-0.0});
        if node["source_id"] == "chain.04" {
            node["body"] = json!([
                "map",
                [
                    ["big", ["int", "900719925474099312345"]],
                    ["bool", ["bool", false]],
                    ["date", ["date", "2026-09-27"]],
                    ["float", ["float", "-0x0.0p+0"]],
                    ["name", ["text", "שלום 🌍\n\"quotes\" n3"]],
                    ["nested", ["list", [["null"], ["map", []]]]]
                ]
            ]);
        }
    }
    for edge in [
        json!({"from":"chain.10","to":"chain.00","rel":"cycle"}),
        json!({"from":"chain.00","to":"outside","rel":null}),
        json!({"from":"chain.00","to":null}),
        json!({"from":"chain.00","rel":{"typed":"relation"}}),
        json!({"from":false,"to":4,"rel":false}),
    ] {
        source["links"]
            .as_array_mut()
            .unwrap()
            .push(json!({"source":edge}));
    }
    source
}

fn selected(source: &J, ids: &[&str], edges: bool, membership: bool) -> J {
    let focus = ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();
    let logical = canonical_view::fold_unrequested(source, &focus, &[]).unwrap();
    let packet = canonical_view::compact(&logical, &[]).unwrap();
    let handles = if edges {
        packet["links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                format!(
                    "edgeset:{}:{}",
                    packet["view_id"].as_str().unwrap(),
                    r[4].as_str().unwrap()
                )
            })
            .collect()
    } else {
        vec![]
    };
    let mut packet = canonical_view::compact(&logical, &handles).unwrap();
    if membership {
        let groups = packet["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| {
                packet["dictionary"][g[0].as_str().unwrap()]["original"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        canonical_view::add_membership_details(&logical, &mut packet, &groups).unwrap();
    }
    packet
}

fn pair() -> (J, J) {
    let source = source();
    (
        selected(
            &source,
            &["chain.00", "chain.01", "chain.02", "chain.03"],
            true,
            false,
        ),
        selected(
            &source,
            &[
                "chain.01",
                "chain.02",
                "chain.03",
                "chain.04",
                "isolate.null",
            ],
            true,
            true,
        ),
    )
}
fn wrapped(kind: &str, payload: &str) -> Result<String> {
    Ok(serde_json::to_string(
        &json!({"kind":kind,"content":payload}),
    )?)
}

#[test]
fn exact_roundtrip_alias_rebinding_and_additive_original_id_resolution() {
    let (base, target) = pair();
    assert_eq!(base["dictionary"]["n3"]["original"], "chain.03");
    assert_eq!(target["dictionary"]["n3"]["original"], "chain.04");
    let base = MaterializedView::from_full(&base).unwrap();
    let delta = between(&base, &target).unwrap();
    let applied = apply(&base, &decode(&render(&delta).unwrap()).unwrap()).unwrap();
    assert_eq!(applied.packet(), &target);
    assert_eq!(applied.sha256().unwrap(), digest(&target).unwrap());
    assert_eq!(delta.focus.fold, vec!["chain.00"]);
    assert!(
        applied.resolve("chain.00").is_some(),
        "folded bodies remain received"
    );
    assert!(
        applied.resolve("n3").is_none(),
        "aliases never resolve across reads"
    );
    assert!(
        applied.resolve("chain.10").is_none(),
        "membership disclosure is not receipt of a body"
    );
    assert_eq!(
        applied.resolve("chain.04").unwrap().body[1][4][1][1],
        "שלום 🌍\n\"quotes\" n3"
    );
    assert_eq!(
        applied.resolve("isolate.null").unwrap().body,
        json!(["null"])
    );
    assert_eq!(applied.received().len(), 6);
    assert_eq!(applied.packet()["expanded_edges"], target["expanded_edges"]);
    // A later refocus reuses the retained body instead of resending it.
    let refocus = between(&applied, base.packet()).unwrap();
    assert!(refocus.evidence_additions.is_empty());
    assert_eq!(apply(&applied, &refocus).unwrap().packet(), base.packet());
}

#[test]
fn corpus_roundtrips_cover_row_order_empty_focus_and_literal_metadata() {
    let source = source();
    let cases = [
        vec![],
        vec!["chain.00"],
        vec!["chain.04", "exception.right"],
        vec!["isolate.alpha", "isolate.null"],
        vec!["chain.00", "chain.01", "chain.03", "chain.04"],
    ];
    for ids in &cases {
        for target_ids in &cases {
            for expanded in [false, true] {
                let base =
                    MaterializedView::from_full(&selected(&source, ids, expanded, false)).unwrap();
                let mut target = selected(&source, target_ids, !expanded, true);
                target["nodes"].as_array_mut().unwrap().reverse();
                target["custom_null"] = J::Null;
                target["custom_string"] = json!("n0\n@delta \"anything\"");
                let delta = between(&base, &target).unwrap();
                assert_eq!(apply(&base, &delta).unwrap().packet(), &target);
            }
        }
    }
}

#[test]
fn metadata_missing_null_and_individual_attributes_remain_exact() {
    let (full, mut target) = pair();
    let mut full = full;
    full["remove_me"] = J::Null;
    target["null_is_present"] = J::Null;
    target["nodes"][0][2]["new_source_field"] = json!({"unexpected":[1,"1",null,false]});
    target["nodes"][0][2]
        .as_object_mut()
        .unwrap()
        .remove("dependencies");
    let base = MaterializedView::from_full(&full).unwrap();
    let delta = between(&base, &target).unwrap();
    assert!(delta.metadata.unset.contains(&"remove_me".into()));
    assert_eq!(apply(&base, &delta).unwrap().packet(), &target);
}

#[test]
fn wrong_base_revision_scope_project_digest_and_replay_are_rejected() {
    let (full, target) = pair();
    let base = MaterializedView::from_full(&full).unwrap();
    let delta = between(&base, &target).unwrap();
    let applied = apply(&base, &delta).unwrap();
    assert!(apply(&applied, &delta).is_err());
    let sibling = selected(&source(), &["exception.right"], false, false);
    assert!(apply(&MaterializedView::from_full(&sibling).unwrap(), &delta).is_err());
    for field in ["revision", "scope", "project", "project_identity"] {
        let mut changed = target.clone();
        changed[field] = if field == "project_identity" {
            json!(["text", "other"])
        } else {
            json!("other")
        };
        assert!(between(&base, &changed).is_err(), "{field}");
    }
    for field in [
        "base_sha256",
        "target_sha256",
        "base_evidence_sha256",
        "target_evidence_sha256",
        "project_sha256",
    ] {
        let mut serialized = serde_json::to_value(&delta).unwrap();
        serialized[field] = json!("0".repeat(64));
        let changed: Delta = serde_json::from_value(serialized).unwrap();
        assert!(apply(&base, &changed).is_err(), "{field}");
    }
    let mut changed = full.clone();
    changed["nodes"][0][1] = json!(["text", "changed under identical revision"]);
    assert!(between(&base, &changed).is_err());
    // Pure no-op content has no receipt identity; V2 must enforce replay/heads.
    let noop = between(&base, &full).unwrap();
    assert_eq!(apply(&base, &noop).unwrap().packet(), &full);
}

#[test]
fn mutation_truncation_reordering_duplicate_and_unresolved_reference_fail_closed() {
    let (full, target) = pair();
    let base = MaterializedView::from_full(&full).unwrap();
    let delta = between(&base, &target).unwrap();
    let wire = render(&delta).unwrap();
    for end in [0, wire.len() / 2, wire.len() - 1, wire.len() - 2] {
        let end = (0..=end).rev().find(|n| wire.is_char_boundary(*n)).unwrap();
        assert!(decode(&wire[..end]).is_err());
    }
    assert!(decode(&(wire.clone() + " ")).is_err());
    assert!(
        decode(&wire.replacen("\"schema\":", "\"schema\":\"duplicate\",\"schema\":", 1)).is_err()
    );
    let mut mutations = Vec::new();
    let mut d = delta.clone();
    d.evidence_additions[0].body = json!(["text", "bad"]);
    mutations.push(d);
    let mut d = delta.clone();
    d.evidence_additions.clear();
    mutations.push(d);
    let mut d = delta.clone();
    d.evidence_additions.reverse();
    mutations.push(d);
    let mut d = delta.clone();
    d.focus.upsert.reverse();
    mutations.push(d);
    let mut d = delta.clone();
    d.focus.fold.push("never.present".into());
    mutations.push(d);
    let mut d = delta.clone();
    d.focus.order.push(d.focus.order[0].clone());
    mutations.push(d);
    let mut d = delta.clone();
    d.focus.order.reverse();
    mutations.push(d);
    let mut d = delta.clone();
    d.metadata.set.insert("navigation_facets".into(), json!(7));
    mutations.push(d);
    let mut d = delta.clone();
    d.metadata.set.insert("dictionary".into(), json!([]));
    mutations.push(d);
    for mutation in mutations {
        assert!(apply(&base, &mutation).is_err());
    }
    assert_eq!(base.packet(), &full, "failed apply never mutates base");
}

#[test]
fn complete_byte_comparison_includes_wrapper_and_fallback_is_explicit() {
    let (full, target) = pair();
    let base = MaterializedView::from_full(&full).unwrap();
    let missing = choose(
        None,
        &target,
        view_format::ViewFormat::CheckedTextTagged,
        wrapped,
    )
    .unwrap();
    assert_eq!(missing.kind, DeliveryKind::Full(FullReason::MissingBase));
    assert_eq!(missing.full_bytes, missing.wire.len());
    assert!(missing.wire.contains("full_checkpoint"));
    assert_eq!(
        missing.wire_sha256,
        identity::sha256(missing.wire.as_bytes())
    );
    assert_eq!(missing.target_sha256, digest(&target).unwrap());
    let mut wrong = target.clone();
    wrong["revision"] = json!("different");
    assert_eq!(
        choose(Some(&base), &wrong, view_format::ViewFormat::Json, wrapped)
            .unwrap()
            .kind,
        DeliveryKind::Full(FullReason::IncompatibleBase)
    );
    let inflated = choose(
        Some(&base),
        &target,
        view_format::ViewFormat::Json,
        |kind, payload| {
            Ok(wrapped(kind, payload)?
                + &if kind == "delta" {
                    "wrapper metadata".repeat(10000)
                } else {
                    String::new()
                })
        },
    )
    .unwrap();
    assert_eq!(
        inflated.kind,
        DeliveryKind::Full(FullReason::DeltaNotSmaller)
    );
    let mut large = full.clone();
    large["unchanged_large_metadata"] = json!("large unchanged record evidence ".repeat(400));
    let large = MaterializedView::from_full(&large).unwrap();
    let small = choose(
        Some(&large),
        large.packet(),
        view_format::ViewFormat::CheckedTextTagged,
        wrapped,
    )
    .unwrap();
    assert_eq!(small.kind, DeliveryKind::Delta);
    assert_eq!(small.delta_bytes, Some(small.wire.len()));
    assert!(
        !small.wire.contains("large unchanged record evidence"),
        "no hidden full target"
    );
}

#[test]
fn rust_python_golden_contract() {
    let (full, target) = pair();
    let base = MaterializedView::from_full(&full).unwrap();
    let delta = between(&base, &target).unwrap();
    let applied = apply(&base, &delta).unwrap();
    let delivery = choose(
        Some(&base),
        &target,
        view_format::ViewFormat::CheckedTextTagged,
        wrapped,
    )
    .unwrap();
    let golden = json!({"base":full,"target":target,"delta":delta,"base_text":view_format::render(base.packet(),view_format::ViewFormat::CheckedTextTagged).unwrap(),
        "delta_text":render(&delta).unwrap(),"received":applied.received(),
        "sizes":{"full_wrapped_bytes":delivery.full_bytes,"delta_wrapped_bytes":delivery.delta_bytes},
        "resolution":{"chain.00":base.resolve("chain.00").unwrap().body,"chain.04":applied.resolve("chain.04").unwrap().body,"n3":null,"chain.10":null}});
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/view-delta-v1.json");
    if std::env::var_os("KPOPPER_UPDATE_DELTA_GOLDEN").is_some() {
        std::fs::write(&path, serde_json::to_string_pretty(&golden).unwrap() + "\n").unwrap();
    }
    let expected: J = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(golden, expected);
}

#[test]
fn reused_body_golden_is_smaller_without_changing_target_selection() {
    let mut source = source();
    for node in source["nodes"].as_array_mut().unwrap() {
        node["body"] = value::TypedValue::from_json(&json!({"id":node["source_id"],"source_text":"A retained source statement with its original typed value and provenance. ".repeat(24)})).unwrap().to_tagged().unwrap();
    }
    let base_packet = selected(
        &source,
        &[
            "chain.00", "chain.01", "chain.02", "chain.03", "chain.04", "chain.05", "chain.06",
            "chain.07",
        ],
        false,
        false,
    );
    let target = selected(
        &source,
        &[
            "chain.01", "chain.02", "chain.03", "chain.04", "chain.05", "chain.06", "chain.07",
            "chain.08",
        ],
        false,
        false,
    );
    let base = MaterializedView::from_full(&base_packet).unwrap();
    let delta = between(&base, &target).unwrap();
    let applied = apply(&base, &delta).unwrap();
    let delivery = choose(
        Some(&base),
        &target,
        view_format::ViewFormat::CheckedTextTagged,
        wrapped,
    )
    .unwrap();
    assert_eq!(delivery.kind, DeliveryKind::Delta);
    assert_eq!(delta.evidence_additions.len(), 1);
    let golden = json!({"base":base_packet,"target":target,"delta":delta,
        "base_text":view_format::render(base.packet(),view_format::ViewFormat::CheckedTextTagged).unwrap(),"delta_text":render(&delta).unwrap(),
        "received":applied.received(),"sizes":{"full_wrapped_bytes":delivery.full_bytes,"delta_wrapped_bytes":delivery.delta_bytes},
        "resolution":{"chain.00":base.resolve("chain.00").unwrap().body,"chain.08":applied.resolve("chain.08").unwrap().body,"n3":null,"chain.10":null}});
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/view-delta-v1-reuse.json");
    if std::env::var_os("KPOPPER_UPDATE_DELTA_GOLDEN").is_some() {
        std::fs::write(&path, serde_json::to_string_pretty(&golden).unwrap() + "\n").unwrap();
    }
    let expected: J = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(golden, expected);
}

#[test]
#[ignore = "optional private packet corpus, enabled explicitly with KPOPPER_DELTA_CORPUS"]
fn captured_normal_packets_reconstruct_exactly_in_both_directions() {
    let root = std::path::PathBuf::from(
        std::env::var_os("KPOPPER_DELTA_CORPUS").expect("corpus directory required"),
    );
    let broad: J =
        serde_json::from_str(&std::fs::read_to_string(root.join("broad.json")).unwrap()).unwrap();
    let focus: J =
        serde_json::from_str(&std::fs::read_to_string(root.join("focus.json")).unwrap()).unwrap();
    for (name, base, target) in [
        ("broad-to-focus", &broad, &focus),
        ("focus-to-broad", &focus, &broad),
    ] {
        let base = MaterializedView::from_full(base).unwrap();
        assert_eq!(
            apply(&base, &between(&base, target).unwrap())
                .unwrap()
                .packet(),
            target
        );
        let delivery = choose(
            Some(&base),
            target,
            view_format::ViewFormat::CheckedTextTagged,
            wrapped,
        )
        .unwrap();
        println!(
            "{name}: {:?}; full={} delta={:?}",
            delivery.kind, delivery.full_bytes, delivery.delta_bytes
        );
    }
}

#[test]
fn source_metadata_named_shared_assessments_is_literal_without_pool_rules() {
    for literal in [
        json!({"source_literal":"n3 is text"}),
        J::Null,
        json!(["a0", false]),
    ] {
        let mut input = source();
        input["shared_assessments"] = literal.clone();
        let base_packet = selected(&input, &["chain.00", "chain.01"], false, false);
        let target = selected(&input, &["chain.01", "chain.02"], false, false);
        let base = MaterializedView::from_full(&base_packet).unwrap();
        let restored = apply(&base, &between(&base, &target).unwrap()).unwrap();
        assert_eq!(restored.packet(), &target);
        assert_eq!(restored.packet()["shared_assessments"], literal);
    }
}

#[test]
fn noncanonical_projection_reordering_is_rejected_even_when_target_is_identical() {
    let (full, target) = pair();
    let base = MaterializedView::from_full(&full).unwrap();
    let mut delta = between(&base, &target).unwrap();
    delta
        .metadata
        .set
        .get_mut("dictionary")
        .unwrap()
        .as_array_mut()
        .unwrap()
        .reverse();
    assert!(apply(&base, &delta).is_err());
}

#[test]
fn invalid_row_shapes_fail_before_materialization() {
    let (_, target) = pair();
    for (section, column, bad) in [
        ("groups", 1, json!("not a count")),
        ("links", 3, json!(-1)),
        ("expanded_edges", 1, json!(false)),
    ] {
        let mut packet = target.clone();
        packet[section][0][column] = bad;
        assert!(MaterializedView::from_full(&packet).is_err(), "{section}");
    }
    let mut packet = target;
    packet["navigation_facets"]["groups"][0][1] = json!(null);
    assert!(MaterializedView::from_full(&packet).is_err());
}

#[test]
fn unknown_manifest_reference_returns_error_without_panicking() {
    let (full, mut target) = pair();
    let base = MaterializedView::from_full(&full).unwrap();
    target["attribute_compaction"]["nodes"]["n99"] = json!({"status":"a0"});
    assert!(
        std::panic::catch_unwind(|| MaterializedView::from_full(&target))
            .unwrap()
            .is_err()
    );
    assert!(
        std::panic::catch_unwind(|| between(&base, &target))
            .unwrap()
            .is_err()
    );
    assert!(
        std::panic::catch_unwind(|| choose(None, &target, view_format::ViewFormat::Json, wrapped))
            .unwrap()
            .is_err()
    );
}

#[test]
fn full_supported_typed_depth_fits_the_delta_envelope_and_unbounded_json_is_refused() {
    for depth in [62, value::MAX_DEPTH] {
        let mut source = source();
        let mut body = json!(["null"]);
        for _ in 0..depth {
            body = json!(["list", [body]]);
        }
        source["nodes"][2]["body"] = body;
        let base =
            MaterializedView::from_full(&selected(&source, &["chain.00"], false, false)).unwrap();
        let target = selected(&source, &["chain.02"], false, false);
        let delta = between(&base, &target).unwrap();
        assert_eq!(
            apply(&base, &decode(&render(&delta).unwrap()).unwrap())
                .unwrap()
                .packet(),
            &target
        );
    }
    let too_deep = format!(
        "{HEADER}\n{RULE}\n{}null{}\n",
        "[".repeat(MAX_JSON_DEPTH + 1),
        "]".repeat(MAX_JSON_DEPTH + 1)
    );
    assert!(decode(&too_deep).unwrap_err().0.contains("nesting"));
}

#[test]
fn self_consistent_forgeries_fail_semantic_guards_before_target_digest_checks() {
    let (full, target) = pair();
    let base = MaterializedView::from_full(&full).unwrap();
    let delta = between(&base, &target).unwrap();
    let received = apply(&base, &delta).unwrap().received().clone();
    // Recompute both complete hashes: invalid typed evidence must still fail.
    let mut forged = delta.clone();
    let bad = json!(["text", 5]);
    let body_hash = digest(&bad).unwrap();
    let evidence = forged
        .evidence_additions
        .iter_mut()
        .find(|e| e.source_id == "isolate.null")
        .unwrap();
    evidence.body = bad.clone();
    evidence.body_sha256 = body_hash.clone();
    let mut forged_received = received.clone();
    forged_received.insert(evidence.source_id.clone(), evidence.clone());
    forged
        .focus
        .upsert
        .iter_mut()
        .find(|n| n.source_id == "isolate.null")
        .unwrap()
        .body_sha256 = body_hash;
    let mut forged_target = target.clone();
    let alias = target["dictionary"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, v)| v["kind"] == "node" && v["original"] == "isolate.null")
        .unwrap()
        .0;
    forged_target["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|n| n[0] == *alias)
        .unwrap()[1] = bad;
    forged.target_sha256 = digest(&forged_target).unwrap();
    forged.target_evidence_sha256 = digest(&json!(forged_received)).unwrap();
    assert!(
        apply(&base, &forged)
            .unwrap_err()
            .0
            .contains("invalid_typed_scalar")
    );
    // A valid body for an unselected ID is still forbidden despite a matching
    // forged ledger hash and an unchanged (correct) selected-target digest.
    let mut forged = delta.clone();
    let extra = Evidence {
        source_id: "zzz.unselected".into(),
        body: json!(["null"]),
        body_sha256: digest(&json!(["null"])).unwrap(),
    };
    let mut forged_received = received.clone();
    forged_received.insert(extra.source_id.clone(), extra.clone());
    forged.evidence_additions.push(extra);
    forged.target_evidence_sha256 = digest(&json!(forged_received)).unwrap();
    assert!(
        apply(&base, &forged)
            .unwrap_err()
            .0
            .contains("unselected evidence")
    );
    // Reordered operations reconstruct the same target/ledger if treated as
    // sets, so these expected errors cannot be attributed to the final hashes.
    let mut forged = delta.clone();
    forged.evidence_additions.reverse();
    assert!(
        apply(&base, &forged)
            .unwrap_err()
            .0
            .contains("noncanonical order")
    );
    // Readding the identical old body would also leave the correct ledger hash.
    let mut forged = delta;
    forged
        .evidence_additions
        .push(base.resolve("chain.00").unwrap().clone());
    forged
        .evidence_additions
        .sort_by(|a, b| a.source_id.cmp(&b.source_id));
    assert!(
        apply(&base, &forged)
            .unwrap_err()
            .0
            .contains("evidence is additive")
    );
}

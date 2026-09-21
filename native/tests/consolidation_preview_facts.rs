use kpop_native::{
    history_yaml::decode_full_ordinary_source_value,
    ordinary_value::{Map, Value as O, map},
    public_consolidation::{self as C, PreviewDecision, PreviewFacts},
    reasoning_runtime::{OperationalBounds, Runtime, target_name},
};
use serde_json::{Value as J, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

fn decode(value: &J) -> O {
    decode_full_ordinary_source_value(value.as_str().unwrap().as_bytes())
        .unwrap()
        .projected()
}
fn decisions(values: &BTreeMap<String, PreviewDecision>) -> J {
    json!(
        values
            .iter()
            .map(|(id, d)| (
                id.clone(),
                json!({"hypothesis":d.hypothesis,"allowed":d.allowed,"reason":d.reason})
            ))
            .collect::<BTreeMap<_, _>>()
    )
}
fn facts(f: &PreviewFacts, changed: &BTreeSet<String>) -> J {
    json!({
        "base_ids": f.base_ids, "contested": f.contested, "refused": decisions(&f.refused),
        "reversals": decisions(&f.reversals), "untaken": decisions(&f.untaken),
        "untakeable": f.untakeable, "drops_needed":f.drops_needed,
        "moved":f.moved,"falsified":f.falsified,"holes":f.holes,"head_falsified":f.head_falsified,"red":f.red,
        "judgments":f.judgments.iter().map(|(id,j)|(id.clone(),json!({"predicate_references":j.predicate_references,"page_references":j.page_references}))).collect::<BTreeMap<_,_>>(),
        "page_bound":f.page_bound(changed).iter().map(|j|json!({"id":j.id,"pages":j.pages,"readings":j.readings})).collect::<Vec<_>>()
    })
}
fn runtime(cache: &Path) -> Runtime {
    let target = target_name().unwrap();
    let archive = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/reasoning/native")
        .join(format!("{target}.kpopper-runtime"));
    let program = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap())
                .join(".cache/kpopper/lean")
                .join(target)
                .join(env!("KPOP_ORDINARY_SOURCE_SHA256"))
        });
    Runtime::open(&archive, cache, OperationalBounds::default())
        .unwrap()
        .with_ordinary_program(
            kpop_native::ordinary_runtime::Program::open(&program)
                .expect("provide the verified ordinary program through KPOP_TEST_ORDINARY_PROGRAM"),
        )
}

#[test]
fn candidate_facts_match_python_remeasure_and_captured_page_regressions() {
    let fixture: J =
        serde_json::from_str(include_str!("fixtures/consolidation-preview-facts.json")).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let runtime = runtime(cache.path());
    for row in fixture["cases"].as_array().unwrap() {
        let document = decode(&row["record"]);
        let hypotheses = row["hypotheses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| {
                (
                    h["name"].as_str().unwrap().into(),
                    O::Map(Map::from([
                        ("doc".into(), decode(&h["document"])),
                        ("head".into(), decode(&h["head"])),
                        ("path".into(), O::Text(h["path"].as_str().unwrap().into())),
                    ])),
                )
            })
            .collect::<Map>();
        let proposals = row["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| C::OrdinaryPreviewHypothesis {
                name: h["name"].as_str().unwrap().into(),
                document: decode(&h["document"]),
                head: decode(&h["head"]),
            })
            .collect::<Vec<_>>();
        let evidence = C::PreviewEvidence {
            brief: row["brief"].as_str().map(str::as_bytes),
        };
        let changed = row["changed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().into())
            .collect::<BTreeSet<_>>();
        let preview = C::preview_ordinary_with_evidence(
            &C::OrdinaryPreviewRequest {
                document: &document,
                hypotheses: &hypotheses,
                proposals: &proposals,
                context: None,
                as_of: row["as_of"].as_str(),
                runtime: Some(&runtime),
            },
            &evidence,
        )
        .unwrap();
        assert_eq!(
            facts(&preview.facts, &changed),
            row["expected"],
            "full {}",
            row["name"]
        );
        if row["nonfinite"] != true {
            let document = document.try_typed().unwrap();
            let kpop_native::value::TypedValue::Map(hypotheses) =
                O::Map(hypotheses).try_typed().unwrap()
            else {
                panic!("expected hypotheses map")
            };
            let proposals = proposals
                .into_iter()
                .map(|h| C::PreviewHypothesis {
                    name: h.name,
                    document: h.document.try_typed().unwrap(),
                    head: h.head.try_typed().unwrap(),
                })
                .collect::<Vec<_>>();
            let finite = C::preview_with_evidence(
                &C::PreviewRequest {
                    document: &document,
                    hypotheses: &hypotheses,
                    proposals: &proposals,
                    context: None,
                    as_of: row["as_of"].as_str(),
                    runtime: Some(&runtime),
                },
                &evidence,
            )
            .unwrap();
            assert_eq!(
                facts(&finite.facts, &changed),
                row["expected"],
                "finite {}",
                row["name"]
            );
            assert_eq!(finite.report, preview.report, "report {}", row["name"]);
        }
    }
}

#[test]
fn irrelevant_malformed_brief_is_not_interpreted_by_supplied_preview() {
    let document = decode(&json!(
        "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.a: {v: 1}\n"
    ));
    let proposals = [C::OrdinaryPreviewHypothesis {
        name: "tree/here".into(),
        document: decode(&json!("known:\n  p.a: {v: 2}\n")),
        head: O::Map(Map::new()),
    }];
    let preview = C::preview_ordinary_with_evidence(
        &C::OrdinaryPreviewRequest {
            document: &document,
            hypotheses: &Map::new(),
            proposals: &proposals,
            context: None,
            as_of: Some("2026-09-19"),
            runtime: None,
        },
        &C::PreviewEvidence {
            brief: Some(b"tabs: ["),
        },
    )
    .unwrap();
    assert!(!preview.facts.red);
    assert_eq!(
        map(preview.candidate_document.as_ref().unwrap())
            .unwrap()
            .len(),
        2
    );
}

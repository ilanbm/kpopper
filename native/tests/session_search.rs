use kpop_native::session_search;

use serde_json::{Map, Value};
use session_search::{
    SearchMode, SearchRequest, SemanticProvider, SemanticRanking, search_checked_session,
};
use std::collections::{BTreeMap, BTreeSet};

const ORACLE: &str = include_str!("fixtures/session_search_oracle.json");

fn object(value: &Value) -> &Map<String, Value> {
    value.as_object().unwrap()
}

fn nodes(value: &Value) -> BTreeMap<String, Value> {
    object(value)
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn groups(root: &Value, pages: bool) -> BTreeMap<String, BTreeSet<String>> {
    if pages {
        return BTreeMap::from([
            ("/".into(), nodes(&root["page_nodes"]).into_keys().collect()),
            ("/hint".into(), BTreeSet::from(["branch.z".into()])),
        ]);
    }
    object(&root["groups"])
        .iter()
        .map(|(key, values)| {
            (
                key.clone(),
                values
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|value| value.as_str().unwrap().to_owned())
                    .collect(),
            )
        })
        .collect()
}

fn request(
    query: &str,
    ids: Option<Vec<&str>>,
    tokens: usize,
    limit: usize,
    branch: Option<&str>,
    mode: SearchMode,
    cursor: Option<String>,
) -> SearchRequest {
    SearchRequest {
        query: query.into(),
        ids: ids.map(|ids| ids.into_iter().map(str::to_owned).collect()),
        tokens,
        limit,
        branch: branch.map(str::to_owned),
        mode,
        cursor,
    }
}

fn assert_oracle(
    root: &Value,
    name: &str,
    graph_nodes: &BTreeMap<String, Value>,
    graph_groups: &BTreeMap<String, BTreeSet<String>>,
    request: &SearchRequest,
    semantic: Option<&dyn SemanticProvider>,
) {
    let expected = &root["cases"][name];
    let actual = search_checked_session(
        root["project"].as_str().unwrap(),
        root["revision"].as_str().unwrap(),
        graph_nodes,
        graph_groups,
        request,
        |text| kpop_native::tokenizer::Encoding::O200kBase.count(text),
        semantic,
    );
    if expected["ok"] == true {
        let actual = actual.unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(actual.text, expected["text"], "{name}");
        assert_eq!(actual.tokens, expected["tokens"], "{name}");
        assert_eq!(actual.packet, expected["response"], "{name}");
    } else {
        assert_eq!(actual.unwrap_err().to_string(), expected["error"], "{name}");
    }
}

#[test]
fn python_full_response_oracles_cover_unicode_ids_fallback_and_branch_ties() {
    let root: Value = serde_json::from_str(ORACLE).unwrap();
    let graph_nodes = nodes(&root["nodes"]);
    let graph_groups = groups(&root, false);
    let cases = [
        (
            "hebrew_lexical",
            request("פרטיות", None, 1000, 8, None, SearchMode::Lexical, None),
        ),
        (
            "german_casefold",
            request("STRASSE", None, 1000, 8, None, SearchMode::Lexical, None),
        ),
        (
            "opaque_overlap",
            request(
                "\"p.script copies\" then p.script",
                None,
                1400,
                8,
                None,
                SearchMode::Lexical,
                None,
            ),
        ),
        (
            "hyphen_longest",
            request(
                "p.script-copies",
                None,
                1000,
                8,
                None,
                SearchMode::Lexical,
                None,
            ),
        ),
        (
            "aliases_unknown",
            request(
                "",
                Some(vec![
                    "node:he.policy",
                    "he.policy",
                    "node:missing",
                    "source:he.policy",
                    "node:he.policy#/body",
                ]),
                1200,
                8,
                None,
                SearchMode::Hybrid,
                None,
            ),
        ),
        (
            "branch_tie",
            request(
                "shared tie",
                None,
                1200,
                8,
                Some("/hint"),
                SearchMode::Lexical,
                None,
            ),
        ),
        (
            "hybrid_fallback",
            request("emoji café", None, 1200, 8, None, SearchMode::Hybrid, None),
        ),
        (
            "unicode_body_payload",
            request("東京 КЛЮЧ", None, 1200, 8, None, SearchMode::Lexical, None),
        ),
    ];
    for (name, request) in cases {
        assert_oracle(&root, name, &graph_nodes, &graph_groups, &request, None);
    }
}

struct SuppliedSemantic {
    global_better: bool,
}

impl SemanticProvider for SuppliedSemantic {
    fn rank(
        &self,
        documents: &BTreeMap<String, String>,
        _query: &str,
    ) -> std::result::Result<SemanticRanking, String> {
        let scores = documents
            .keys()
            .map(|key| {
                (
                    key.clone(),
                    if self.global_better && key == "global.a" {
                        0.9
                    } else if ["branch.z", "global.a"].contains(&key.as_str()) {
                        0.8
                    } else {
                        0.1
                    },
                )
            })
            .collect();
        Ok(SemanticRanking {
            scores,
            metadata: object(&serde_json::json!({
                "document_count": documents.len(),
                "chunk_count": documents.len() + 2,
                "index_reused": true,
                "model": {
                    "name": "supplied-e5",
                    "revision": "r1",
                    "weights_sha256": "a".repeat(64),
                    "tokenizer_sha256": "b".repeat(64),
                    "ignored": "not exposed",
                }
            }))
            .clone(),
        })
    }
}

#[test]
fn supplied_semantic_scores_match_python_without_running_a_model() {
    let root: Value = serde_json::from_str(ORACLE).unwrap();
    let graph_nodes = nodes(&root["nodes"]);
    let graph_groups = groups(&root, false);
    let semantic_request = request(
        "query",
        None,
        1500,
        4,
        Some("/hint"),
        SearchMode::Semantic,
        None,
    );
    assert_oracle(
        &root,
        "semantic_supplied_tie",
        &graph_nodes,
        &graph_groups,
        &semantic_request,
        Some(&SuppliedSemantic {
            global_better: false,
        }),
    );
    let request = request(
        "query",
        None,
        1500,
        2,
        Some("/hint"),
        SearchMode::Semantic,
        None,
    );
    assert_oracle(
        &root,
        "semantic_global_beats_branch",
        &graph_nodes,
        &graph_groups,
        &request,
        Some(&SuppliedSemantic {
            global_better: true,
        }),
    );
}

#[test]
fn python_pages_preserve_every_budget_omitted_hit_and_refuse_sixty_four_tokens() {
    let root: Value = serde_json::from_str(ORACLE).unwrap();
    let graph_nodes = nodes(&root["page_nodes"]);
    let graph_groups = groups(&root, true);
    let first = request(
        "shared observation",
        None,
        500,
        24,
        None,
        SearchMode::Lexical,
        None,
    );
    assert_oracle(
        &root,
        "budget_page_1",
        &graph_nodes,
        &graph_groups,
        &first,
        None,
    );
    let cursor = root["cases"]["budget_page_1"]["response"]["next_cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = request(
        "shared observation",
        None,
        500,
        24,
        None,
        SearchMode::Lexical,
        Some(cursor),
    );
    assert_oracle(
        &root,
        "budget_page_2",
        &graph_nodes,
        &graph_groups,
        &second,
        None,
    );
    let refusal = request(
        "shared observation",
        None,
        64,
        24,
        None,
        SearchMode::Lexical,
        None,
    );
    assert_oracle(
        &root,
        "budget_64_refusal",
        &graph_nodes,
        &graph_groups,
        &refusal,
        None,
    );
}

#[test]
fn python_cursor_oracles_reject_duplicate_fields_and_every_bound_change() {
    let root: Value = serde_json::from_str(ORACLE).unwrap();
    let graph_nodes = nodes(&root["page_nodes"]);
    let graph_groups = groups(&root, true);
    let cursor = root["cases"]["budget_page_1"]["response"]["next_cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    let cases = [
        (
            "binding_query_tamper",
            request(
                "observation",
                None,
                1000,
                24,
                None,
                SearchMode::Lexical,
                Some(cursor.clone()),
            ),
        ),
        (
            "binding_ids_tamper",
            request(
                "shared observation",
                Some(vec![graph_nodes.keys().next().unwrap()]),
                1000,
                24,
                None,
                SearchMode::Lexical,
                Some(cursor.clone()),
            ),
        ),
        (
            "binding_branch_tamper",
            request(
                "shared observation",
                None,
                1000,
                24,
                Some("/hint"),
                SearchMode::Lexical,
                Some(cursor.clone()),
            ),
        ),
        (
            "binding_mode_tamper",
            request(
                "shared observation",
                None,
                1000,
                24,
                None,
                SearchMode::Hybrid,
                Some(cursor.clone()),
            ),
        ),
    ];
    for (name, request) in cases {
        assert_oracle(&root, name, &graph_nodes, &graph_groups, &request, None);
    }
    let duplicate_cursor = root["cases"]["budget_page_1"]["response"]["next_cursor"]
        .as_str()
        .unwrap();
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(duplicate_cursor)
        .unwrap();
    let held: Value = serde_json::from_slice(&raw).unwrap();
    let duplicate = format!(
        "{{\"version\":1,\"offset\":{},\"binding\":\"{}\",\"offset\":{}}}",
        held["offset"],
        held["binding"].as_str().unwrap(),
        held["offset"]
    );
    use base64::Engine as _;
    let duplicate = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(duplicate);
    let duplicate_request = request(
        "shared observation",
        None,
        1000,
        24,
        None,
        SearchMode::Lexical,
        Some(duplicate),
    );
    assert_oracle(
        &root,
        "duplicate_cursor_key",
        &graph_nodes,
        &graph_groups,
        &duplicate_request,
        None,
    );

    for (name, invalid) in [
        ("invalid_cursor_base64", "!not-base64".to_owned()),
        ("invalid_cursor_too_long", "x".repeat(513)),
    ] {
        let request = request(
            "shared observation",
            None,
            1000,
            24,
            None,
            SearchMode::Lexical,
            Some(invalid),
        );
        assert_oracle(&root, name, &graph_nodes, &graph_groups, &request, None);
    }
    let outside = format!(
        "{{\"binding\":\"{}\",\"offset\":25,\"version\":1}}",
        held["binding"].as_str().unwrap()
    );
    let outside = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(outside);
    let offset_request = request(
        "shared observation",
        None,
        1000,
        24,
        None,
        SearchMode::Lexical,
        Some(outside),
    );
    assert_oracle(
        &root,
        "invalid_cursor_offset",
        &graph_nodes,
        &graph_groups,
        &offset_request,
        None,
    );

    let bound_request = request(
        "shared observation",
        None,
        1000,
        24,
        None,
        SearchMode::Lexical,
        Some(cursor),
    );
    for (project, revision) in [
        ("different-project", root["revision"].as_str().unwrap()),
        (root["project"].as_str().unwrap(), &"2".repeat(64)),
    ] {
        let error = search_checked_session(
            project,
            revision,
            &graph_nodes,
            &graph_groups,
            &bound_request,
            |text| kpop_native::tokenizer::Encoding::O200kBase.count(text),
            None,
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "search cursor belongs to a different search or ranking; start a fresh search"
        );
    }
}

#[test]
fn ranking_fingerprint_rejects_changed_validated_input_at_same_advertised_revision() {
    let root: Value = serde_json::from_str(ORACLE).unwrap();
    let mut graph_nodes = nodes(&root["page_nodes"]);
    let graph_groups = groups(&root, true);
    let first = graph_nodes.keys().next().unwrap().clone();
    graph_nodes.get_mut(&first).unwrap()["body"]["v"] = "no longer a match".into();
    let cursor = root["cases"]["budget_page_1"]["response"]["next_cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    let request = request(
        "shared observation",
        None,
        1000,
        24,
        None,
        SearchMode::Lexical,
        Some(cursor),
    );
    assert_oracle(
        &root,
        "changed_ranking",
        &graph_nodes,
        &graph_groups,
        &request,
        None,
    );
}

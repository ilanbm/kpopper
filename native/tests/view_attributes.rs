use kpop_native::{
    reasoning_projection::{self, render_projected_status, render_value},
    value::TypedValue as V,
    view_attributes::{self, ENCODING_SCHEMA},
};
use serde_json::{Value as J, json};
use std::collections::BTreeMap;

fn text_value(text: &str) -> J {
    json!(["text", text])
}

fn body(fields: &[(&str, J)]) -> J {
    let mut fields = fields.to_vec();
    fields.sort_by_key(|(key, _)| *key);
    json!([
        "map",
        fields
            .into_iter()
            .map(|(key, value)| json!([key, value]))
            .collect::<Vec<_>>()
    ])
}

fn projected_status(value_text: Option<&str>, unique: Option<&str>) -> J {
    let mut status = json!({
        "acceptance":"accepted",
        "computation":{"status":"ok","truth":true,"value_text":value_text},
        "basis":"not_applicable",
        "falsifier":{"status":"not_applicable","holds":null},
        "contention":"none_detected",
        "integrity":"assessed",
        "coverage":"complete",
        "coverage_included":true,
        "assurance":"computed",
        "recorded_evidence_kinds":[],
        "support":{"status":"clear","states":[]}
    });
    if let Some(unique) = unique {
        status["unique_assessment_field"] = json!(unique);
    }
    status
}

fn status_text(status: &J) -> String {
    render_projected_status(&V::from_json(status).unwrap()).unwrap()
}

fn text_display(text: &str) -> String {
    render_value(&V::Map(BTreeMap::from([
        ("type".into(), V::Text("text".into())),
        ("value".into(), V::Text(text.into())),
    ])))
    .unwrap()
}

fn row(
    reference: &str,
    id: &str,
    body: J,
    status: J,
    status_text: Option<J>,
    uncertainty: Option<J>,
    extra: J,
) -> J {
    let mut attributes = json!({"status":status,"dependencies":null,"custom":extra});
    attributes["source_fixture_id"] = json!(id);
    if let Some(status_text) = status_text {
        attributes["status_text"] = status_text;
    }
    if let Some(uncertainty) = uncertainty {
        attributes["uncertainty"] = uncertainty;
    }
    json!([reference, body, attributes])
}

fn packet(rows: Vec<J>) -> J {
    let dictionary = rows
        .iter()
        .map(|row| {
            (
                row[0].as_str().unwrap().to_owned(),
                json!({"kind":"node","original":row[0]}),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let ids = dictionary
        .values()
        .map(|entry| entry["original"].clone())
        .collect::<Vec<_>>();
    json!({
        "schema":"kpopper.canonical-graph-view/v2",
        "project":"attribute-fixture",
        "project_identity":["map",[["project",["text","attribute-fixture"]]]],
        "revision":"r1",
        "scope":"scope-a",
        "mode":"expand",
        "fields":{"node":["ref","body","attributes"],"group":["ref","count","label","description"]},
        "dictionary":dictionary,
        "nodes":rows,
        "groups":[],"links":[],"expanded_edges":[],
        "coverage":{"count":ids.len(),"source_ids":ids,"source_ids_sha256":"fixture","direct_count":rows.len(),"folded_count":0},
        "navigation_facets":{"groups":[],"nodes":[]},
        "custom_envelope":{"keep":true}
    })
}

fn by_ref<'a>(packet: &'a J, reference: &str) -> &'a J {
    packet["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row[0] == reference)
        .unwrap()
}

fn decoded_status(packet: &J, row: &J) -> J {
    let reference = row[0].as_str().unwrap();
    let rule = &packet["attribute_compaction"]["nodes"][reference]["status"];
    if let Some(alias) = rule.as_str() {
        assert_eq!(row[2]["status_ref"].as_str(), Some(alias));
        packet["shared_assessments"][alias].clone()
    } else {
        row[2]["status"].clone()
    }
}

fn synthetic_packet() -> J {
    let unicode = "Ω / שלום\nline two 🧭";
    let unicode_rendered = text_display(unicode);
    let repeated_status = projected_status(Some(&unicode_rendered), Some("repeatable assessment"));
    let first = row(
        "n0",
        "unicode.first",
        body(&[
            ("quoted", text_value(unicode)),
            ("stamp", text_value("source-a")),
        ]),
        repeated_status.clone(),
        Some(json!(status_text(&repeated_status))),
        Some(json!([])),
        json!({"custom_null":null,"nested":{"unicode":"שלום"}}),
    );
    let second = row(
        "n1",
        "unicode.second",
        body(&[
            ("quoted", text_value(unicode)),
            ("stamp", text_value("source-b")),
        ]),
        repeated_status.clone(),
        Some(json!(status_text(&repeated_status))),
        Some(json!([])),
        json!({"unique_source_stamp":"b"}),
    );

    let integer_status = projected_status(Some("42"), None);
    let integer = row(
        "n2",
        "numeric.integer",
        body(&[("value", json!(["int", "42"]))]),
        integer_status.clone(),
        Some(json!(status_text(&integer_status))),
        Some(json!([])),
        json!({"exact_integer":true}),
    );
    let null_status = projected_status(Some("null"), None);
    let null = row(
        "n3",
        "numeric.null",
        body(&[("v", json!(["null"]))]),
        null_status.clone(),
        Some(json!(status_text(&null_status))),
        Some(json!([])),
        json!({"null_is_present":true}),
    );

    let alias_states = json!([
        "declared uncertainty: premise is unmeasured",
        "review follows source change"
    ]);
    let mut alias = row(
        "n4",
        "array.alias",
        body(&[("name", text_value("array alias"))]),
        J::Null,
        Some(alias_states.clone()),
        Some(alias_states),
        json!({"keep":"unrelated metadata"}),
    );
    alias[2]["status_ref"] = json!("this is an original field");
    let arbitrary = row(
        "n5",
        "text.unique",
        body(&[("quoted", text_value("actual source quote"))]),
        projected_status(Some(&text_display("actual source quote")), None),
        Some(json!("human-authored status note")),
        None,
        json!({"status_text_ref":"unknown user data","private_flag":null}),
    );
    let mut preexisting_display_ref =
        projected_status(Some(&text_display("actual source quote")), None);
    preexisting_display_ref["computation"]["value_text_ref"] = json!({"custom":"original field"});
    let mut user_ref_collision = row(
        "n8",
        "source.status-ref",
        body(&[("quoted", text_value("actual source quote"))]),
        preexisting_display_ref,
        Some(json!("custom source status")),
        Some(json!([])),
        json!({"keep":"unrelated metadata"}),
    );
    user_ref_collision[2]["status_ref"] = json!("user-defined status pointer");
    let float_status = projected_status(Some("1"), None);
    let float = row(
        "n6",
        "numeric.float",
        body(&[("v", json!(["float", "0x1.0000000000000p+0"]))]),
        float_status.clone(),
        Some(json!(status_text(&float_status))),
        Some(json!([])),
        json!({"float_must_remain_typed":true}),
    );
    let mut missing_null = row(
        "n7",
        "missing.null",
        body(&[("quoted", text_value("field present"))]),
        J::Null,
        None,
        Some(J::Null),
        json!({"missing_status_text":true}),
    );
    missing_null[2]
        .as_object_mut()
        .unwrap()
        .insert("status_text".into(), J::Null);
    let missing_status_text = row(
        "n9",
        "missing.status-text",
        body(&[("name", text_value("field present"))]),
        J::Null,
        None,
        Some(json!([])),
        json!({"status_text_is_missing":true}),
    );
    let mut missing_value_text_status = projected_status(None, None);
    missing_value_text_status["computation"]
        .as_object_mut()
        .unwrap()
        .remove("value_text");
    let missing_value_text = row(
        "n10",
        "missing.value-text",
        body(&[("quoted", text_value("field present"))]),
        missing_value_text_status,
        None,
        Some(json!([])),
        json!({"value_text_is_missing":true}),
    );
    let null_value_text = row(
        "n11",
        "null.value-text",
        body(&[("quoted", text_value("field present"))]),
        projected_status(None, None),
        None,
        Some(json!([])),
        json!({"value_text_is_null":true}),
    );

    packet(vec![
        first,
        second,
        integer,
        null,
        alias,
        arbitrary,
        float,
        missing_null,
        user_ref_collision,
        missing_status_text,
        missing_value_text,
        null_value_text,
    ])
}

#[test]
fn frozen_d_f01_rows_compact_and_restore_exactly() {
    let mut packet: J =
        serde_json::from_str(include_str!("fixtures/attribute-compaction-df01.json")).unwrap();
    let original = packet["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| (row[0].as_str().unwrap().to_owned(), row[2].clone()))
        .collect::<BTreeMap<_, _>>();

    view_attributes::compact(&mut packet).unwrap();
    assert_eq!(
        packet["fields"]["attribute_compaction"]["schema"],
        ENCODING_SCHEMA
    );
    assert!(
        packet["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row[2].get("status_text").is_none())
    );
    assert_eq!(
        decoded_status(&packet, by_ref(&packet, "n0"))["computation"]["value_text_ref"],
        "quoted"
    );
    assert_eq!(
        by_ref(&packet, "n6")[2]["status_ref"],
        by_ref(&packet, "n8")[2]["status_ref"]
    );
    assert!(
        packet["shared_assessments"]
            .as_object()
            .is_some_and(|pool| !pool.is_empty())
    );

    for row in packet["nodes"].as_array().unwrap() {
        let reference = row[0].as_str().unwrap();
        assert_eq!(
            view_attributes::restore_attributes(&packet, row).unwrap(),
            original[reference]
        );
    }
}

#[test]
fn synthetic_display_aliases_pool_only_worthwhile_status_and_roundtrip_unknown_types() {
    let mut packet = synthetic_packet();
    let original = packet["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| (row[0].as_str().unwrap().to_owned(), row[2].clone()))
        .collect::<BTreeMap<_, _>>();
    view_attributes::compact(&mut packet).unwrap();

    assert!(by_ref(&packet, "n0")[2]["status_text"].is_null());
    assert_eq!(
        decoded_status(&packet, by_ref(&packet, "n0"))["computation"]["value_text_ref"],
        "quoted"
    );
    assert_eq!(
        decoded_status(&packet, by_ref(&packet, "n2"))["computation"]["value_text_ref"],
        "value"
    );
    assert_eq!(
        decoded_status(&packet, by_ref(&packet, "n3"))["computation"]["value_text_ref"],
        "v"
    );
    assert!(by_ref(&packet, "n4")[2]["status_text"].is_null());
    assert_eq!(
        by_ref(&packet, "n4")[2]["status_ref"],
        "this is an original field"
    );
    assert_eq!(
        by_ref(&packet, "n5")[2]["status_text"],
        "human-authored status note"
    );
    assert_eq!(
        decoded_status(&packet, by_ref(&packet, "n5"))["computation"]["value_text_ref"],
        "quoted"
    );
    assert_eq!(
        by_ref(&packet, "n8")[2]["status_ref"],
        "user-defined status pointer"
    );
    assert!(
        decoded_status(&packet, by_ref(&packet, "n8"))["computation"]["value_text_ref"]["custom"]
            .is_string()
    );
    assert_eq!(
        decoded_status(&packet, by_ref(&packet, "n6"))["computation"]["value_text"],
        "1"
    );
    assert_eq!(by_ref(&packet, "n7")[2]["status_text"], J::Null);
    assert!(
        by_ref(&packet, "n7")[2]
            .get("uncertainty")
            .is_some_and(J::is_null)
    );
    assert!(by_ref(&packet, "n9")[2].get("status_text").is_none());
    assert!(
        decoded_status(&packet, by_ref(&packet, "n10"))["computation"]
            .get("value_text")
            .is_none()
    );
    assert!(
        decoded_status(&packet, by_ref(&packet, "n10"))["computation"]
            .get("value_text_ref")
            .is_none()
    );
    assert!(decoded_status(&packet, by_ref(&packet, "n11"))["computation"]["value_text"].is_null());
    assert!(
        decoded_status(&packet, by_ref(&packet, "n11"))["computation"]
            .get("value_text_ref")
            .is_none()
    );
    assert_eq!(packet["custom_envelope"]["keep"], true);

    for row in packet["nodes"].as_array().unwrap() {
        let reference = row[0].as_str().unwrap();
        assert_eq!(
            view_attributes::restore_attributes(&packet, row).unwrap(),
            original[reference]
        );
    }
}

#[test]
fn missing_or_wrong_node_and_display_references_refuse_restoration() {
    let mut packet = synthetic_packet();
    view_attributes::compact(&mut packet).unwrap();
    let pooled_ref = packet["attribute_compaction"]["nodes"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, rule)| rule["status"].is_string())
        .unwrap()
        .0
        .clone();
    let index = packet["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .position(|row| row[0] == pooled_ref)
        .unwrap();

    let mut bad_ref = packet.clone();
    bad_ref["nodes"][index][2]["status_ref"] = json!("missing-pool-entry");
    assert!(view_attributes::restore_attributes(&bad_ref, &bad_ref["nodes"][index]).is_err());

    let alias = packet["nodes"][index][2]["status_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut missing_pool = packet.clone();
    missing_pool["shared_assessments"]
        .as_object_mut()
        .unwrap()
        .remove(&alias);
    assert!(
        view_attributes::restore_attributes(&missing_pool, &missing_pool["nodes"][index]).is_err()
    );

    let value_row = packet["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .position(|row| {
            packet["attribute_compaction"]["nodes"][row[0].as_str().unwrap()]
                .get("value_text")
                .is_some()
        })
        .unwrap();
    let mut wrong_field = packet.clone();
    let ref_alias = wrong_field["nodes"][value_row][2]["status_ref"]
        .as_str()
        .map(str::to_owned);
    if let Some(alias) = ref_alias {
        wrong_field["shared_assessments"][alias]["computation"]["value_text_ref"] =
            json!("missing");
    } else {
        wrong_field["nodes"][value_row][2]["status"]["computation"]["value_text_ref"] =
            json!("missing");
    }
    assert!(
        view_attributes::restore_attributes(&wrong_field, &wrong_field["nodes"][value_row])
            .is_err()
    );

    let reference = packet["nodes"][index][0].as_str().unwrap().to_owned();
    let mut partial_manifest = packet.clone();
    partial_manifest["attribute_compaction"]["nodes"]
        .as_object_mut()
        .unwrap()
        .remove(&reference);
    assert!(
        view_attributes::restore_attributes(&partial_manifest, &partial_manifest["nodes"][index])
            .is_err()
    );

    let mut missing_dictionary = packet.clone();
    missing_dictionary["dictionary"]
        .as_object_mut()
        .unwrap()
        .remove(&reference);
    assert!(
        view_attributes::restore_attributes(
            &missing_dictionary,
            &missing_dictionary["nodes"][index]
        )
        .is_err()
    );
}

#[test]
fn generated_status_text_helper_matches_node_wrapper() {
    let source = V::from_json(&json!({
        "acceptance":"accepted",
        "computation":{"status":"ok","value":{"type":"text","value":"computed value"}},
        "state":{"basis":"not_applicable","falsifier":{"status":"not_applicable"},
            "contention":"none_detected","integrity":"assessed","coverage":"complete",
            "support":{"status":"clear","states":[]}}
    }))
    .unwrap();
    let projected = reasoning_projection::project_node_status(&source).unwrap();
    assert_eq!(
        reasoning_projection::render_node_status(&source).unwrap(),
        render_projected_status(&projected).unwrap()
    );
}

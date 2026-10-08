use kpop_native::{
    canonical_view,
    view_format::{self, ViewFormat},
};
use serde_json::{Value as J, json};

#[test]
fn checked_text_reconstructs_the_native_structural_packet_and_rejects_missing_rows() {
    let full: J = serde_json::from_str(include_str!("fixtures/compact-structural.json")).unwrap();
    let packet = canonical_view::compact(&full, &[]).unwrap();
    let text = view_format::render(&packet, ViewFormat::CheckedText).unwrap();
    assert_eq!(view_format::decode_checked_text(&text).unwrap(), packet);
    let missing = text
        .lines()
        .filter(|line| !line.starts_with("@item \"nodes\" 0 "))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(view_format::decode_checked_text(&missing).is_err());
    let missing_id = text
        .lines()
        .filter(|line| !line.starts_with("@field \"view_id\" "))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(view_format::decode_checked_text(&missing_id).is_err());
    let rows = view_format::render(&packet, ViewFormat::CheckedTextRows).unwrap();
    assert_eq!(view_format::decode_checked_text(&rows).unwrap(), packet);
    assert!(rows.len() < text.len());
    // A v3 header must not permit v2 directives to bypass declared row counts.
    let mixed = text.replacen("kpopper.view-codec/v2", "kpopper.view-codec/v3", 1);
    assert!(view_format::decode_checked_text(&mixed).is_err());
    let mut lines = rows.lines().map(str::to_owned).collect::<Vec<_>>();
    let row = lines.iter().position(|line| line.starts_with('[')).unwrap();
    lines.remove(row);
    assert!(view_format::decode_checked_text(&lines.join("\n")).is_err());
    let tagged = view_format::render(&packet, ViewFormat::CheckedTextTagged).unwrap();
    assert_eq!(view_format::decode_checked_text(&tagged).unwrap(), packet);
    assert!(tagged.len() < text.len());
    assert!(view_format::decode_checked_text(&tagged.replacen("node [", "link [", 1)).is_err());
    let missing = tagged
        .lines()
        .filter(|line| !line.starts_with("link ["))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(view_format::decode_checked_text(&missing).is_err());
}

#[test]
fn text_values_do_not_create_protocol_rows_and_empty_sections_survive() {
    let mut full: J =
        serde_json::from_str(include_str!("fixtures/compact-structural.json")).unwrap();
    full["nodes"] = json!([]);
    full["groups"] = json!([]);
    full["links"] = json!([]);
    full["coverage"] = json!({"count":0,"source_ids":[]});
    full["navigation_membership"] = json!({});
    let mut packet = canonical_view::compact(&full, &[]).unwrap();
    packet["scope"] = json!("שלום\n@field \"schema\" \"spoof\"\u{2028}data");
    packet["missing_is_distinct"] = J::Null;
    packet["typed"] = json!(["int", "9007199254740993"]);
    let text = view_format::render(&packet, ViewFormat::CheckedText).unwrap();
    assert_eq!(view_format::decode_checked_text(&text).unwrap(), packet);
    assert!(view_format::decode_checked_text(&(text + "@field \"schema\" \"spoof\"\n")).is_err());
}

use kpop_native::{core_page, reasoning_context::CapturedAssessment};
use serde_json::Value;
use std::path::Path;

#[test]
fn page_wrapper_rebinds_secondary_revision_without_changing_canonical_identity() {
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/public-core-readers.json")).unwrap();
    let context = CapturedAssessment::from_data(&corpus["contexts"][0]).unwrap();
    let plain = core_page::project(&context, None, true).unwrap()["page_assessment"].clone();
    let delivered = core_page::project_for_page(
        &context,
        None,
        Path::new("/tmp/record/GROUNDING.yaml"),
        Path::new("/tmp/output/pages/record.html"),
    )
    .unwrap()["page_assessment"]
        .clone();
    assert_eq!(plain["snapshot_id"], delivered["snapshot_id"]);
    assert_eq!(plain["findings_revision"], delivered["findings_revision"]);
    assert!(delivered["page_inputs"]["revision"].as_str().is_some());
    assert!(delivered["page_assessment_revision"].as_str().is_some());
    assert!(delivered["page_inputs"]["values"]["nodes"].is_array());
}

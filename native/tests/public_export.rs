use kpop_native::{
    public_export::{self, Direction, Format, Options},
    reasoning_context::CapturedAssessment,
};
use serde_json::Value as J;

fn context(name: &str, oracle: &J) -> CapturedAssessment {
    if let Some(value) = oracle["custom_contexts"].get(name) {
        return CapturedAssessment::from_data(value).unwrap();
    }
    let corpus: J = serde_json::from_str(include_str!("fixtures/reasoning-context.json")).unwrap();
    let value = corpus["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["name"] == name)
        .unwrap();
    CapturedAssessment::from_data(&value["serialized"]).unwrap()
}

fn options(case: &J, format: Format, details: bool) -> Options {
    Options {
        direction: if case["direction"] == "impact" {
            Direction::Impact
        } else {
            Direction::Support
        },
        depth: case["depth"].as_u64().unwrap() as usize,
        max_nodes: case["max_nodes"].as_u64().unwrap() as usize,
        details,
        format,
    }
}

#[test]
fn defaults_match_public_export_contract() {
    let options = Options::default();
    assert_eq!(options.direction, Direction::Support);
    assert_eq!(options.depth, 1);
    assert_eq!(options.max_nodes, 12);
    assert!(!options.details);
    assert_eq!(options.format, Format::Markdown);
}

#[test]
fn core_projection_and_all_renderers_match_the_frozen_python_oracle() {
    let oracle: J =
        serde_json::from_str(include_str!("fixtures/public-export-oracle.json")).unwrap();
    for case in oracle["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let captured = context(case["context"].as_str().unwrap(), &oracle);
        let seeds = case["seeds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let projected =
            public_export::project(&captured, &seeds, &options(case, Format::Markdown, false))
                .unwrap();
        assert_eq!(projected, case["packet"], "projection: {name}");
        assert_eq!(
            public_export::render_markdown(&projected, false),
            case["markdown"],
            "markdown: {name}"
        );
        assert_eq!(
            public_export::render_markdown(&projected, true),
            case["markdown_details"],
            "markdown details: {name}"
        );
        assert_eq!(
            public_export::render_mermaid(&projected),
            case["mermaid"],
            "mermaid: {name}"
        );
        assert_eq!(
            public_export::render(
                &captured,
                &seeds,
                &options(case, Format::MarkdownMermaid, false)
            )
            .unwrap(),
            case["markdown_mermaid"],
            "combined: {name}"
        );
    }
}

#[test]
fn validates_bounds_after_seed_deduplication() {
    let oracle: J =
        serde_json::from_str(include_str!("fixtures/public-export-oracle.json")).unwrap();
    let captured = context("all-None", &oracle);
    let options = Options {
        max_nodes: 1,
        ..Options::default()
    };
    let duplicate = vec!["d.a".to_owned(), "d.a".to_owned()];
    let packet = public_export::project(&captured, &duplicate, &options).unwrap();
    assert_eq!(packet["seeds"], serde_json::json!(["d.a"]));

    let unknown =
        public_export::project(&captured, &["D.A".into()], &Options::default()).unwrap_err();
    assert_eq!(unknown.0, "unknown exact ID(s): D.A; use kpop open or pull");
    let too_deep = Options {
        depth: 5,
        ..Options::default()
    };
    assert_eq!(
        public_export::project(&captured, &["d.a".into()], &too_deep)
            .unwrap_err()
            .0,
        "depth must be 0..4"
    );
    let details_mermaid = Options {
        details: true,
        format: Format::Mermaid,
        ..Options::default()
    };
    assert_eq!(
        public_export::render(&captured, &["d.a".into()], &details_mermaid)
            .unwrap_err()
            .0,
        "--details needs a text format: markdown or markdown-mermaid"
    );
}

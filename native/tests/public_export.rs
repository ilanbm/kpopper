use kpop_native::{
    ordinary_runtime::Program,
    public_export::{self, Direction, Format, Options},
    reasoning_context::CapturedAssessment,
    reasoning_runtime::{OperationalBounds, Runtime},
    source_capture::{self, ReadMode},
};
use serde_json::Value as J;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn install_runtime(root: &Path) -> Runtime {
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    let reasoning = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/reasoning/native")
        .join(format!("{target}.kpopper-runtime"));
    let runtime = Runtime::open(
        &reasoning,
        &root.join("cache"),
        OperationalBounds::default(),
    )
    .unwrap();
    let os = if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(windows) {
        "windows"
    } else {
        std::env::consts::OS
    };
    let arch = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "arm64"
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        "amd64"
    } else {
        std::env::consts::ARCH
    };
    let ordinary = PathBuf::from(
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .unwrap(),
    )
    .join(".cache/kpopper/lean")
    .join(format!("{os}-{arch}"))
    .join(env!("KPOP_ORDINARY_SOURCE_SHA256"));
    runtime.with_ordinary_program(Program::open(&ordinary).unwrap())
}

fn write_ordinary(root: &Path, case: &J) -> PathBuf {
    let record = root.join("GROUNDING.yaml");
    fs::write(&record, case["yaml"].as_str().unwrap()).unwrap();
    for (name, value) in case["hypotheses"].as_object().into_iter().flatten() {
        let path = root.join(".kpopper/hypotheses").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value.as_str().unwrap()).unwrap();
    }
    record
}

/// Run `kpop --frozen export` in `root` with the core runtime as its only
/// resource.
fn core_export(root: &Path, args: &[&str]) -> std::process::Output {
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    let resources = root.join("resources/reasoning");
    if !resources.exists() {
        fs::create_dir_all(&resources).unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/reasoning/native")
                .join(format!("{target}.kpopper-runtime")),
            resources.join(format!("{target}.zip")),
        )
        .unwrap();
    }
    std::process::Command::new(env!("CARGO_BIN_EXE_kpop"))
        .args(["--workspace", root.to_str().unwrap(), "--frozen", "export"])
        .args(args)
        .env("KPOPPER_NATIVE_RESOURCES", root.join("resources"))
        .env("KPOPPER_NATIVE_CACHE", root.join("cache"))
        .output()
        .unwrap()
}

#[test]
fn public_cli_exports_core_findings_and_wraps_the_reference_exit_contract() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let record = "meta: {reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}}\nknown:\n  p.a: {v: 7}\njudgments:\n  d.keep: {verdict: Keep, rests_on: [p.a], seen: {p.a: 7}, wrong_if: 'p.a > 8'}\n";
    fs::write(root.join("GROUNDING.yaml"), record).unwrap();
    let run = |args: &[&str]| core_export(root, args);
    let text = run(&["d.keep"]);
    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("p.a") && text.contains("d.keep"));
    let wrapped = run(&["d.keep", "--json"]);
    assert!(wrapped.status.success());
    let value: J = serde_json::from_slice(&wrapped.stdout).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"command":"export","exit_code":0,"output":text,"error":""})
    );
    let invalid = run(&["d.keep", "--format", "mermaid", "--details", "--json"]);
    assert_eq!(invalid.status.code(), Some(2));
    let value: J = serde_json::from_slice(&invalid.stdout).unwrap();
    assert!(
        value["error"]
            .as_str()
            .unwrap()
            .contains("--details needs a text format")
    );
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        record
    );
}

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

fn first_difference(left: &J, right: &J, path: &str) -> Option<String> {
    match (left, right) {
        (J::Object(left), J::Object(right)) => {
            for key in left.keys().chain(right.keys()) {
                if left.get(key) != right.get(key) {
                    return first_difference(
                        left.get(key).unwrap_or(&J::Null),
                        right.get(key).unwrap_or(&J::Null),
                        &format!("{path}.{key}"),
                    );
                }
            }
            None
        }
        (J::Array(left), J::Array(right)) => left
            .iter()
            .zip(right)
            .enumerate()
            .find_map(|(index, (left, right))| {
                (left != right)
                    .then(|| first_difference(left, right, &format!("{path}[{index}]")))
                    .flatten()
            })
            .or_else(|| {
                (left.len() != right.len())
                    .then(|| format!("{path}.length: {} != {}", left.len(), right.len()))
            }),
        _ => Some(format!("{path}: {left:?} != {right:?}")),
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
fn ordinary_projection_and_renderers_match_the_frozen_python_oracle() {
    let oracle: J =
        serde_json::from_str(include_str!("fixtures/ordinary-export-oracle.json")).unwrap();
    for case in oracle["cases"].as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let record = write_ordinary(temp.path(), case);
        let runtime = install_runtime(temp.path());
        let capture = source_capture::capture_source_with_runtime(
            std::slice::from_ref(&record),
            temp.path(),
            ReadMode::Frozen,
            None,
            Some(&runtime),
        )
        .unwrap();
        let seeds = case["seeds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let options = options(case, Format::Markdown, false);
        let packet =
            public_export::project_ordinary(&capture, Some(&runtime), &seeds, &options).unwrap();
        assert_eq!(
            packet,
            case["packet"],
            "projection: {}: {}",
            case["name"],
            first_difference(&packet, &case["packet"], "$").unwrap_or_default()
        );
        assert_eq!(
            public_export::render_ordinary(&capture, Some(&runtime), &seeds, &options).unwrap(),
            case["markdown"],
            "markdown: {}",
            case["name"]
        );
        let details = Options {
            details: true,
            ..options.clone()
        };
        assert_eq!(
            public_export::render_ordinary(&capture, Some(&runtime), &seeds, &details).unwrap(),
            case["markdown_details"],
            "details: {}",
            case["name"]
        );
        let mermaid = Options {
            format: Format::Mermaid,
            ..options
        };
        assert_eq!(
            public_export::render_ordinary(&capture, Some(&runtime), &seeds, &mermaid).unwrap(),
            case["mermaid"],
            "mermaid: {}",
            case["name"]
        );
    }
}

/// The expected outputs below are Python's: `scripts/cli.py --frozen export ...`
/// run beside the fixture record.
#[test]
fn ordinary_export_matches_python_on_undecided_conditions_and_labels() {
    // v.heating_tab reads page.spill, which only the page counts: its
    // condition cannot be evaluated here, so it is UNKNOWN. The computed
    // names list their name, count and origin in that order. c.coupling's
    // label breaks after the hyphen of "baryon-acceleration", and
    // d.greeting's Hebrew points are escaped for Mermaid, as Python escapes
    // what `str.isalnum` rejects, and its U+001C separates words.
    let temp = tempfile::tempdir().unwrap();
    let record = temp.path().join("GROUNDING.yaml");
    fs::write(&record, include_str!("fixtures/export-unknown.yaml")).unwrap();
    let runtime = install_runtime(temp.path());
    let capture = source_capture::capture_source_with_runtime(
        std::slice::from_ref(&record),
        temp.path(),
        ReadMode::Frozen,
        None,
        Some(&runtime),
    )
    .unwrap();
    let seeds = ["v.heating_tab", "c.coupling", "d.greeting"].map(str::to_owned);
    let both = Options {
        format: Format::MarkdownMermaid,
        ..Options::default()
    };
    assert_eq!(
        public_export::render_ordinary(&capture, Some(&runtime), &seeds, &both).unwrap(),
        include_str!("fixtures/export-unknown.stdout")
    );
    let details = Options {
        details: true,
        format: Format::Mermaid,
        ..Options::default()
    };
    assert_eq!(
        public_export::render_ordinary(&capture, Some(&runtime), &seeds, &details)
            .unwrap_err()
            .0,
        "--details needs a text format: markdown or markdown-mermaid"
    );
    // The selection is read before the format is refused, as in Python.
    assert_eq!(
        public_export::render_ordinary(&capture, Some(&runtime), &["nope".into()], &details)
            .unwrap_err()
            .0,
        "unknown exact ID(s): nope; use kpop open or pull"
    );
}

#[test]
fn core_export_keeps_the_record_order_of_dependencies_and_fields() {
    // d.budget rests on [p.rate, p.hours, p.cost] and its fields, like the
    // known entries', are not in name order. The shared assessment's own
    // revision names the evaluator, so only the identity line is masked.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(
        root.join("GROUNDING.yaml"),
        include_str!("fixtures/export-core-order.yaml"),
    )
    .unwrap();
    let output = core_export(root, &["d.budget"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let identity =
        regex::Regex::new("Shared assessment [0-9a-f]{64}; findings [0-9a-f]{64}").unwrap();
    assert_eq!(
        identity.replace_all(
            &String::from_utf8(output.stdout).unwrap(),
            "Shared assessment <snapshot>; findings <findings>"
        ),
        include_str!("fixtures/export-core-order.stdout")
    );
}

#[test]
fn ordinary_export_refuses_unsupported_reference_shapes_without_surrogate_leaks() {
    let oracle: J =
        serde_json::from_str(include_str!("fixtures/ordinary-export-oracle.json")).unwrap();
    for case in oracle["refusals"].as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let record = write_ordinary(temp.path(), case);
        let runtime = install_runtime(temp.path());
        let capture = source_capture::capture_source_with_runtime(
            std::slice::from_ref(&record),
            temp.path(),
            ReadMode::Frozen,
            None,
            Some(&runtime),
        );
        let seeds = case["seeds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let error = match capture {
            Ok(capture) => public_export::project_ordinary(
                &capture,
                Some(&runtime),
                &seeds,
                &Options::default(),
            )
            .unwrap_err(),
            Err(error) => {
                assert_eq!(error.0, "invalid_ordinary_structural_key");
                let output = std::process::Command::new(env!("CARGO_BIN_EXE_kpop"))
                    .current_dir(temp.path())
                    .args(["--frozen", "export", "--json"])
                    .args(&seeds)
                    .arg("--record")
                    .arg(&record)
                    .output()
                    .unwrap();
                assert_eq!(output.status.code(), Some(2));
                assert!(output.stderr.is_empty());
                let packet: J = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(
                    packet,
                    serde_json::json!({"command":"export","exit_code":2,
                        "output":"","error":case["cli_error"]})
                );
                kpop_native::Error(case["error"].as_str().unwrap().to_owned())
            }
        };
        assert_eq!(error.0, case["error"], "{}", case["name"]);
        assert!(!error.0.contains("\0kpopper:ordinary-key:"));
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
    assert_eq!(
        public_export::render(&captured, &["D.A".into()], &details_mermaid)
            .unwrap_err()
            .0,
        "unknown exact ID(s): D.A; use kpop open or pull"
    );
}

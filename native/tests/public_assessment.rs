use kpop_native::{
    history_yaml, ordinary_assessment as A,
    public_assessment::{self, Options},
    source_capture::{self, ReadMode},
    value::TypedValue as V,
};
use serde_json::Value as J;
use std::{fs, path::Path, process::Command};
fn map(v: &V) -> Result<&std::collections::BTreeMap<String, V>, &'static str> {
    if let V::Map(m) = v {
        Ok(m)
    } else {
        Err("not a map")
    }
}
const RECORD: &str = "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.load: {v: 61}\n  p.backup: {v: false}\njudgments:\n  d.work:\n    verdict: continue\n    rests_on: [p.load, p.backup]\n    seen: {p.load: 44, p.backup: true}\n    wrong_if: 'p.load > 80'\n";
fn options() -> Options {
    Options {
        ids: vec!["d.work".into()],
        records: vec![],
        policy: A::POLICY.into(),
        profile: None,
        as_of: None,
        history: false,
        attention_only: false,
    }
}
fn cli(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_kpop-native"))
        .current_dir(root)
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn actual_cli_discovers_pointer_record_and_retains_selection_and_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("GROUNDING.yaml"), "record: facts.yaml\n").unwrap();
    fs::write(root.join("facts.yaml"), RECORD).unwrap();
    fs::create_dir(root.join("nested")).unwrap();
    let out = cli(
        &root.join("nested"),
        &["--frozen", "assess", "d.work", "--json"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let data: J = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(data["assessment_profile"], A::PROFILE);
    assert_eq!(data["selection"], serde_json::json!(["d.work"]));
    assert_eq!(data["nodes"].as_object().unwrap().len(), 1);
    assert_eq!(
        data["nodes"]["d.work"]["state"]["falsifier"]["status"],
        "does_not_hold"
    );
    assert_eq!(
        data["nodes"]["d.work"]["attention"][0]["reasons"][0]["related_ids"],
        serde_json::json!(["p.backup"])
    );
    assert_eq!(fs::read_to_string(root.join("facts.yaml")).unwrap(), RECORD);
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        "record: facts.yaml\n"
    );
    let focused = cli(root, &["assess", "d.work", "--attention-only", "--frozen"]);
    assert!(
        focused.status.success(),
        "{}",
        String::from_utf8_lossy(&focused.stderr)
    );
    let focused: J = serde_json::from_slice(&focused.stdout).unwrap();
    assert_eq!(focused["record_revision"], data["record_revision"]);
    assert!(focused.get("nodes").is_none());
    assert!(focused["attention"]["d.work"].is_array());
    let value = V::from_json(&focused).unwrap();
    assert_eq!(
        A::selected_attention(&value, Some(&["d.work".into()]), Some(&["review".into()]))
            .unwrap()
            .to_json()
            .unwrap(),
        focused["attention"]
    );
    assert!(A::selected_attention(&value, Some(&["p.load".into()]), None).is_err());
}
#[test]
fn actual_capture_reads_hypotheses_and_refuses_changed_inputs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("GROUNDING.yaml"), RECORD).unwrap();
    let folder = root.join(".kpopper/hypotheses");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("a.yaml"), "known:\n  p.load: {v: 70}\n").unwrap();
    fs::write(folder.join("b.yaml"), "known:\n  p.load: {v: 90}\n").unwrap();
    let capture = source_capture::capture_source(
        &[root.join("GROUNDING.yaml")],
        root,
        ReadMode::Frozen,
        None,
    )
    .unwrap();
    let report = A::from_capture(&capture, None, A::POLICY)
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(
        report["scope"]["hypotheses_checked"],
        serde_json::json!(["a", "b"])
    );
    assert_eq!(
        report["nodes"]["p.load"]["state"]["contention"]["status"],
        "detected"
    );
    assert_eq!(
        report["nodes"]["p.load"]["state"]["contention"]["witnesses"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    fs::write(folder.join("b.yaml"), "known:\n  p.load: {v: 70}\n").unwrap();
    assert!(A::from_capture(&capture, None, A::POLICY).is_err());
}
#[test]
fn unavailable_computation_and_profile_rejections_remain_explicit() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let doc = RECORD.replace("p.load: {v: 61}", "p.load: {rule: {expr: 'p.backup + 1'}}");
    fs::write(root.join("GROUNDING.yaml"), &doc).unwrap();
    let r = public_assessment::report(&options(), root, ReadMode::Frozen, None)
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(
        r["nodes"]["d.work"]["state"]["falsifier"]["status"],
        "error"
    );
    assert_eq!(
        r["nodes"]["d.work"]["state"]["basis"]["dependencies"]["p.load"]["current"]["status"],
        "unavailable"
    );
    let mut opts = options();
    opts.history = true;
    assert!(
        public_assessment::report(&opts, root, ReadMode::Frozen, None)
            .unwrap_err()
            .0
            .contains("--history requires")
    );
    opts.history = false;
    opts.as_of = Some("2026-01-01".into());
    assert!(
        public_assessment::report(&opts, root, ReadMode::Frozen, None)
            .unwrap_err()
            .0
            .contains("--as-of is available")
    );
    opts.as_of = None;
    opts.ids = vec!["not.here".into()];
    assert!(
        public_assessment::report(&opts, root, ReadMode::Frozen, None)
            .unwrap_err()
            .0
            .contains("unknown assessment ID")
    );
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        doc
    );
}
#[test]
fn ordinary_identity_preserves_types_and_attention_does_not_read_again() {
    let fixture: J =
        serde_json::from_str(include_str!("fixtures/ordinary-assessment.json")).unwrap();
    let case = &fixture["cases"][0];
    let report = V::from_tagged(&case["reports"][A::POLICY]).unwrap();
    let state = &map(&map(&map(&report).unwrap()["nodes"]).unwrap()["d.work"]).unwrap()["state"];
    let before = report.clone();
    assert_eq!(
        A::attention(state, A::POLICY).unwrap(),
        V::from_json(&serde_json::json!([])).unwrap()
    );
    assert!(A::attention(state, "invented-policy").is_err());
    assert_eq!(report, before);
    let a = history_yaml::decode_document(b"v: 2026-01-02\n").unwrap();
    let b = history_yaml::decode_document(b"v: '2026-01-02'\n").unwrap();
    assert_eq!(A::legacy_digest(&a).unwrap(), A::legacy_digest(&b).unwrap());
    assert_ne!(a.digest().unwrap(), b.digest().unwrap());
}

#[test]
fn actual_cli_runs_bundled_ordinary_and_core_lean_programs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    let resources = root.join("resources");
    let reasoning = resources.join("reasoning");
    let ordinary = resources.join("ordinary").join(&target);
    fs::create_dir_all(&reasoning).unwrap();
    fs::create_dir_all(&ordinary).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")),
        reasoning.join(format!("{target}.zip")),
    )
    .unwrap();
    let program = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let target = if cfg!(windows) {
                "windows-amd64"
            } else {
                &target
            };
            std::path::PathBuf::from(
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .unwrap(),
            )
            .join(".cache/kpopper/lean")
            .join(target)
            .join(env!("KPOP_ORDINARY_SOURCE_SHA256"))
        });
    for name in [
        "build.json",
        if cfg!(windows) {
            "epistemic-core.exe"
        } else {
            "epistemic-core"
        },
    ] {
        fs::copy(program.join(name), ordinary.join(name)).unwrap();
    }
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_kpop-native"))
            .current_dir(root)
            .env("KPOPPER_NATIVE_RESOURCES", &resources)
            .env("KPOPPER_NATIVE_CACHE", root.join("cache"))
            .args(["--frozen", "assess", "d.work"])
            .args(extra)
            .output()
            .unwrap()
    };
    let body = RECORD.replace("p.load: {v: 61}", "p.load: {rule: {expr: '21 * 3'}}");
    fs::write(root.join("GROUNDING.yaml"), &body).unwrap();
    let out = run(&[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let data: J = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(data["assessment_profile"], A::PROFILE);
    assert_eq!(
        data["nodes"]["d.work"]["state"]["basis"]["dependencies"]["p.load"]["current"]["value"],
        63
    );
    assert_eq!(
        data["nodes"]["d.work"]["state"]["falsifier"]["status"],
        "does_not_hold"
    );
    fs::write(root.join("GROUNDING.yaml"), format!("meta:\n  reasoning: {{version: 1, profile: core/v1, requires: [arithmetic/v1]}}\n{body}")).unwrap();
    let out = run(&["--as-of", "2026-01-01"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let data: J = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(data["assessment_profile"], "core/v1");
    // A declaration alone does not translate legacy executable text.
    assert_eq!(
        data["nodes"]["d.work"]["state"]["falsifier"]["status"],
        "unknown"
    );
    let core_body = body.replace("wrong_if: 'p.load > 80'", "wrong_if: {expr: 'p.load > 80'}");
    fs::write(root.join("GROUNDING.yaml"), format!("meta:\n  reasoning: {{version: 1, profile: core/v1, requires: [arithmetic/v1]}}\n{core_body}")).unwrap();
    let out = run(&["--as-of", "2026-01-01"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let data: J = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        data["nodes"]["d.work"]["state"]["falsifier"]["status"],
        "does_not_hold"
    );
    let out = run(&["--history"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let data: J = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(data["schema_version"], 3);
    fs::remove_file(ordinary.join("build.json")).unwrap();
    // The unavailable optional ordinary program cannot poison core reads.
    assert!(run(&[]).status.success());
    fs::write(root.join("GROUNDING.yaml"), &body).unwrap();
    let out = run(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
}

//! The actual `assess` command over captured records and versioned findings.
use crate::{
    Result, ordinary_findings as A,
    ordinary_value::{Value as V, map, map_mut, string_is},
    reasoning_runtime::OperationalBounds,
    source_capture::ReadMode,
    value::TypedValue as CV,
};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, clap::Args)]
pub struct Options {
    #[arg(required=true,num_args=1..)]
    pub ids: Vec<String>,
    #[arg(long = "record")]
    pub records: Vec<PathBuf>,
    #[arg(long,default_value=A::POLICY,value_parser=[A::POLICY,"falsifiers-only/v1"])]
    pub policy: String,
    #[arg(long,value_parser=[A::PROFILE,"core/v1"])]
    pub profile: Option<String>,
    #[arg(long)]
    pub as_of: Option<String>,
    #[arg(long)]
    pub history: bool,
    #[arg(long)]
    pub attention_only: bool,
}
pub fn report_value(
    options: &Options,
    cwd: &Path,
    mode: ReadMode,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<V> {
    crate::require(
        !(options.history && options.attention_only),
        "--attention-only is not yet a versioned schema-v3 projection",
    )?;
    let paths = if options.records.is_empty() {
        crate::public_workspace::records(cwd)?
    } else {
        options.records.clone()
    };
    let capture = crate::source_capture::capture_ordinary_source_with_runtime(
        &paths,
        cwd,
        mode,
        options.as_of.as_ref().map(|s| CV::Text(s.clone())),
        runtime,
    )?;
    let cap = crate::ordinary_fields::capabilities(
        capture.ordinary_document(),
        options.profile.as_deref(),
    )?;
    let core = string_is(&map(&cap)?["profile"], "core/v1");
    crate::require(
        core || !options.history,
        "--history requires --profile core/v1",
    )?;
    crate::require(
        core || options.as_of.is_none(),
        "--as-of is available on the explicit core/v1 assessment profile",
    )?;
    let bounds = OperationalBounds::default();
    let mut report = if core {
        if options.history {
            V::from_typed(&crate::reasoning_history_assessment::assess(
                capture.snapshot()?,
                Some(&options.ids),
                &options.policy,
                runtime,
                bounds.clone(),
                Some(&options.ids),
            )?)
        } else {
            V::from_typed(&crate::reasoning_assessment::assess(
                capture.snapshot()?,
                Some(&options.ids),
                &options.policy,
                runtime,
                bounds.clone(),
            )?)
        }
    } else {
        crate::ordinary_report::assess(
            capture.ordinary_document(),
            map(capture.hypotheses())?,
            &V::from_typed(&capture.ordinary_context()),
            runtime,
            &options.policy,
        )?
    };
    let nodes = map(&map(&report)?["nodes"])?;
    crate::require(
        options.ids.iter().all(|id| nodes.contains_key(id)),
        "unknown assessment ID; use open or pull to find an entry",
    )?;
    if !options.history {
        let selected = V::Map(
            options
                .ids
                .iter()
                .map(|id| (id.clone(), nodes[id].clone()))
                .collect(),
        );
        map_mut(&mut report)?.insert(
            "selection".into(),
            V::List(options.ids.iter().cloned().map(V::Text).collect()),
        );
        if options.attention_only {
            let attention = A::selected_attention(&report, Some(&options.ids), None)?;
            let r = map_mut(&mut report)?;
            r.insert("attention".into(), attention);
            r.remove("nodes");
        } else {
            map_mut(&mut report)?.insert("nodes".into(), selected);
        }
    }
    if core {
        crate::require(
            crate::history_yaml::compact_json_size(&report.try_typed()?) < bounds.output_bytes,
            "output_limit",
        )?;
    }
    capture.verify()?;
    Ok(report)
}
pub fn report(
    options: &Options,
    cwd: &Path,
    mode: ReadMode,
    runtime: Option<&crate::reasoning_runtime::Runtime>,
) -> Result<CV> {
    report_value(options, cwd, mode, runtime)?.try_typed()
}
pub fn run(options: &Options, cwd: &Path, mode: ReadMode) -> Result<String> {
    let paths = if options.records.is_empty() {
        crate::public_workspace::records(cwd)?
    } else {
        options.records.clone()
    };
    let runtime =
        crate::public_workspace::runtime_for_paths(&paths, cwd, options.profile.as_deref())?;
    let report = report_value(options, cwd, mode, runtime.as_ref())?;
    if string_is(&map(&report)?["assessment_profile"], A::PROFILE) {
        report.python_pretty_json()
    } else {
        report.python_json(false)
    }
}

#[cfg(test)]
mod ordinary_stdout_tests {
    use super::*;
    #[test]
    fn complete_nonfinite_assessment_stdout_matches_python() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/ordinary-nonfinite-cli.json"
        ))
        .unwrap();
        let cache = tempfile::tempdir().unwrap();
        let runtime = crate::history_authoring::tests::runtime(cache.path())
            .with_ordinary_program(crate::ordinary_reader::tests::program());
        for case in fixture["cases"].as_array().unwrap() {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let entry = root.join("GROUNDING.yaml");
            let source = case["source"].as_str().unwrap();
            std::fs::write(&entry, source).unwrap();
            let options = Options {
                ids: vec!["p.value".into()],
                records: vec![entry.clone()],
                policy: A::POLICY.into(),
                profile: None,
                as_of: None,
                history: false,
                attention_only: false,
            };
            let output = report_value(&options, &root, ReadMode::Frozen, Some(&runtime))
                .unwrap()
                .python_pretty_json()
                .unwrap()
                + "\n";
            assert_eq!(output, case["stdout"].as_str().unwrap(), "{}", case["name"]);
            assert_eq!(std::fs::read_to_string(&entry).unwrap(), source);
        }
    }
}

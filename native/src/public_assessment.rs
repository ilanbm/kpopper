//! The actual `assess` command over captured records and versioned findings.
use crate::{
    Result, history_contract::*, history_view::map_mut, ordinary_assessment as A,
    reasoning_runtime::OperationalBounds, source_capture::ReadMode, value::TypedValue as V,
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
pub fn report(
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
    let capture = crate::source_capture::capture_source_with_runtime(
        &paths,
        cwd,
        mode,
        options.as_of.as_ref().map(|s| V::Text(s.clone())),
        runtime,
    )?;
    let cap =
        crate::reasoning_fields::capabilities(&capture.document(), options.profile.as_deref())?;
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
            crate::reasoning_history_assessment::assess(
                capture.snapshot(),
                Some(&options.ids),
                &options.policy,
                runtime,
                bounds.clone(),
                Some(&options.ids),
            )?
        } else {
            crate::reasoning_assessment::assess(
                capture.snapshot(),
                Some(&options.ids),
                &options.policy,
                runtime,
                bounds.clone(),
            )?
        }
    } else {
        A::from_capture(&capture, runtime, &options.policy)?
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
            crate::history_yaml::compact_json_size(&report) < bounds.output_bytes,
            "output_limit",
        )?;
    }
    capture.verify()?;
    Ok(report)
}
pub fn run(options: &Options, cwd: &Path, mode: ReadMode) -> Result<String> {
    let runtime = crate::public_workspace::runtime()?;
    let report = report(options, cwd, mode, runtime.as_ref())?;
    crate::ordinary_assessment_report::compact_json(&report)
}

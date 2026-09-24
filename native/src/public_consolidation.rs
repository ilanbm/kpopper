//! Public named-hypothesis consolidation over ordinary records and active history.
#[path = "public_branch_consolidation.rs"]
mod branch;
#[path = "ordinary_consolidation_full.rs"]
mod full;
pub(crate) use full::branch_differences;
#[path = "consolidation_preview_facts.rs"]
mod preview_facts;
pub use preview_facts::{
    PreviewDecision, PreviewEvidence, PreviewFacts, PreviewJudgment, PreviewPageBound,
};
#[path = "ordinary_consolidation.rs"]
mod ordinary;
#[path = "consolidation_supersession.rs"]
pub(crate) mod supersession;
use crate::{
    Result, direct_history,
    history_authoring::{empty, obj, s},
    history_contract::*,
    history_hypothesis_authoring as HA,
    history_paths::Scheme,
    history_store::Store,
    history_transaction::PreparedMutation,
    history_transaction_fs as F,
    history_view::{list, map_mut},
    project_modes::WriteRoute,
    public_history::fresh_id,
    public_workspace,
    reasoning_runtime::Runtime,
    recording_privacy as Privacy, require,
    value::TypedValue as V,
};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

fn finding_lines(value: &V, key: &str) -> Result<Vec<String>> {
    list(field(map(value)?, key)?)?
        .iter()
        .map(|value| text(value).map(str::to_owned))
        .collect()
}

fn preview_output(
    names: &[String],
    assessment: &crate::history_prospective::Assessment,
) -> Result<String> {
    let mut out = vec![
        format!("prepared: {}", names.join(", ")),
        "captured history preview:".into(),
    ];
    for (stage, findings) in [
        ("base", &assessment.before_findings),
        ("candidate", &assessment.after_findings),
    ] {
        let counts = ["falsified", "holes", "moved", "notes"]
            .iter()
            .map(|kind| Ok(format!("{} {kind}", finding_lines(findings, kind)?.len())))
            .collect::<Result<Vec<_>>>()?;
        out.push(format!("  {stage}: {}", counts.join(", ")));
    }
    for kind in ["falsified", "holes", "moved", "notes"] {
        let before = finding_lines(&assessment.before_findings, kind)?
            .into_iter()
            .collect::<BTreeSet<_>>();
        for line in finding_lines(&assessment.after_findings, kind)? {
            if !before.contains(&line) {
                out.push(format!("  candidate {kind}: {line}"));
            }
        }
    }
    Ok(out.join("\n") + "\n")
}

#[derive(Clone, Default)]
pub struct Options {
    pub names: Vec<String>,
    pub dry_run: bool,
    pub refute: Option<String>,
    pub why: Option<String>,
    pub source: Option<String>,
    pub as_of: Option<String>,
    pub take: Vec<String>,
    pub drops: Vec<String>,
    pub record: Option<PathBuf>,
    pub from_refs: Vec<String>,
    pub by: Option<String>,
    pub choices: Vec<String>,
    pub source_revision: Option<String>,
    /// Read committed files only: the dry run then has no pending findings to test.
    pub frozen: bool,
}

fn validate(options: &Options) -> Result<()> {
    if let Some(day) = &options.as_of {
        require(
            regex::Regex::new(r"^\d{4}-\d{2}-\d{2}$")
                .unwrap()
                .is_match(day),
            "--as-of takes a date, YYYY-MM-DD",
        )?;
        if let Ok(day) = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d") {
            let today = chrono::Utc::now()
                .date_naive()
                .max(chrono::Local::now().date_naive());
            require(
                day <= today,
                &format!(
                    "--as-of {} is after today ({today}) - a day is the record's clock, and a fold is not dated ahead",
                    options.as_of.as_ref().unwrap()
                ),
            )?;
        }
    }
    if options.refute.is_some() {
        require(
            !options.dry_run
                && options.names.is_empty()
                && options.take.is_empty()
                && options.drops.is_empty(),
            "--refute takes one hypothesis and its why, and nothing else",
        )?;
        require(
            options
                .why
                .as_ref()
                .is_some_and(|why| !why.trim().is_empty()),
            "refused - a refutation says why: consolidate --refute <hypothesis> \"<why>\"",
        )?;
    } else {
        require(
            options.source.is_none(),
            "--as names the session source of a refutation: it goes with --refute",
        )?;
        require(options.why.is_none(), "refutation reason requires --refute")?;
    }
    Ok(())
}

fn drops(values: &[String]) -> Result<V> {
    let mut result = Map::new();
    for value in values {
        let (id, why) = value.split_once(':').ok_or_else(|| {
            error("--drop takes \"<id>: <why>\" - the dependency the replacement no longer rests on, and the reason")
        })?;
        require(
            !id.trim().is_empty() && !why.trim().is_empty(),
            "--drop takes \"<id>: <why>\" - the dependency the replacement no longer rests on, and the reason",
        )?;
        result.insert(id.trim().into(), s(why.trim()));
    }
    Ok(V::Map(result))
}

/// Consolidation over a record that declares core/v1. A hypothesis beside such a record is
/// read with its own semantics, by a core/v1 consolidation this command does not hold, so
/// laying one over the record is refused. A hypothesis that cannot be read, or a name
/// nothing holds, is refused first, as over any record; with none beside the record there
/// is nothing to lay over it. Pending contributions are not tested here: a core/v1 record
/// keeps its own account of what contests it.
fn core_record(
    hypotheses: &crate::ordinary_value::Value,
    names: &[String],
) -> Result<CommandOutput> {
    use crate::ordinary_value::{map, py, string_is, truth};
    let mut pool = BTreeSet::new();
    for (name, hypothesis) in map(hypotheses)? {
        let hypothesis = map(hypothesis)?;
        if hypothesis
            .get("kind")
            .is_some_and(|kind| string_is(kind, "contribution"))
        {
            continue;
        }
        if let Some(why) = hypothesis.get("error").filter(|why| truth(why)) {
            return Err(error(&format!(
                "refused - hypothesis {name} could not be read: {}",
                py(why)
            )));
        }
        pool.insert(name.as_str());
    }
    let missing = names
        .iter()
        .filter(|name| !pool.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    require(
        missing.is_empty(),
        &format!(
            "refused - no hypothesis named {} beside the record (there: {})",
            missing.join(", "),
            if pool.is_empty() {
                "none".into()
            } else {
                pool.iter().copied().collect::<Vec<_>>().join(", ")
            }
        ),
    )?;
    require(pool.is_empty(), crate::source_capture::CORE_CONSUMER)?;
    Ok(CommandOutput {
        stdout: "no hypotheses beside the record - nothing to consolidate\n".into(),
        stderr: String::new(),
        code: 0,
    })
}

fn authoring_options(prefix: &str, by: V) -> Result<crate::history_authoring::Options> {
    let now = chrono::Utc::now();
    Ok(crate::history_authoring::Options {
        operation: fresh_id(prefix)?,
        recorded_at: now.to_rfc3339(),
        recording_day: chrono::Local::now().date_naive().to_string(),
        by,
        strict: true,
        paths: Scheme::Hashed,
        receipt_version: None,
    })
}

fn selected_names(context: &HA::Context, requested: &[String]) -> Result<Vec<String>> {
    let names = if requested.is_empty() {
        map(&context.groups)?.keys().cloned().collect()
    } else {
        requested.to_vec()
    };
    if !names.is_empty() {
        HA::guard_names(&names, &context.groups, &context.physical, true)?;
    }
    Ok(names)
}

fn privacy_guard(
    route: &WriteRoute,
    context: &HA::Context,
    names: &[String],
    source: Option<&str>,
    action: &V,
) -> Result<()> {
    if let Some((name, retained)) = private_selection(context, names, source)? {
        let mut intent = action.clone();
        map_mut(&mut intent)?.insert("hypothesis".into(), s(&name));
        let receipt = Privacy::draft(
            route.project(),
            &intent,
            &retained,
            "private hypothesis or source permission; original retained without publication",
        )?;
        return Err(error(&format!(
            "private draft retained at {}; original hypothesis retained",
            text(field(map(&receipt)?, "path")?)?
        )));
    }
    Ok(())
}

pub(crate) fn private_selection(
    context: &HA::Context,
    names: &[String],
    source: Option<&str>,
) -> Result<Option<(String, V)>> {
    let combined = HA::layer(&context.base, &context.groups, names)?;
    let original_entries = crate::reasoning_snapshot::entries(&context.base)?;
    for name in names {
        let group = field(map(&context.groups)?, name)?;
        let group_fields = map(group)?;
        let mut roots = crate::reasoning_snapshot::entries(field(group_fields, "doc")?)?
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        if let Some(source) = source {
            roots.insert(source.to_owned());
        }
        let roots = roots.into_iter().collect::<Vec<_>>();
        let selected = crate::pending_bundle::closure(&combined, &roots).map_err(|_| {
            error("refused - hypothesis source closure could not be checked; original retained")
        })?;
        let original_roots = crate::reasoning_snapshot::entries(&selected)?
            .keys()
            .filter(|id| original_entries.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        let original = if original_roots.is_empty() {
            empty()
        } else {
            crate::pending_bundle::closure(&context.base, &original_roots).map_err(|_| {
                error("refused - hypothesis source closure could not be checked; original retained")
            })?
        };
        let record_meta = map(&combined)?.get("meta").cloned().unwrap_or_else(empty);
        let original_meta = map(&context.base)?
            .get("meta")
            .cloned()
            .unwrap_or_else(empty);
        let retained = obj([
            ("selected", selected),
            (
                "hypothesis",
                group_fields.get("head").cloned().unwrap_or_else(empty),
            ),
            ("record_metadata", record_meta),
            ("original_record_metadata", original_meta),
            ("original_record_closure", original),
        ]);
        if Privacy::private_marker(group) || Privacy::private_marker(&retained) {
            return Ok(Some((name.clone(), retained)));
        }
    }
    Ok(None)
}

/// A supplied ordinary proposal; previewing it never reads or writes source files.
pub struct PreviewHypothesis {
    pub name: String,
    pub document: V,
    pub head: V,
}
pub struct PreviewRequest<'a> {
    pub document: &'a V,
    pub hypotheses: &'a Map,
    pub proposals: &'a [PreviewHypothesis],
    pub context: Option<&'a V>,
    pub as_of: Option<&'a str>,
    pub runtime: Option<&'a Runtime>,
}
pub struct Preview {
    pub report: String,
    /// The ordinary dry-run status; movement alone does not make a dry-run red.
    pub exit_code: i32,
    /// Includes movement and unnamed dropped dependencies that prevent a fold.
    pub blocked: bool,
    pub candidate_document: Option<V>,
    pub facts: PreviewFacts,
}
pub fn preview(request: &PreviewRequest<'_>) -> Result<Preview> {
    preview_with_evidence(request, &PreviewEvidence::default())
}
pub fn preview_with_evidence(
    request: &PreviewRequest<'_>,
    evidence: &PreviewEvidence<'_>,
) -> Result<Preview> {
    ordinary::preview(request, evidence)
}

/// Read-only ordinary preview. Its source values never enter a finite writer.
pub struct OrdinaryPreviewHypothesis {
    pub name: String,
    pub document: crate::ordinary_value::Value,
    pub head: crate::ordinary_value::Value,
}
pub struct OrdinaryPreviewRequest<'a> {
    pub document: &'a crate::ordinary_value::Value,
    pub hypotheses: &'a crate::ordinary_value::Map,
    pub proposals: &'a [OrdinaryPreviewHypothesis],
    pub context: Option<&'a crate::ordinary_value::Value>,
    pub as_of: Option<&'a str>,
    pub runtime: Option<&'a Runtime>,
}
pub struct OrdinaryPreview {
    pub report: String,
    pub exit_code: i32,
    pub blocked: bool,
    pub candidate_document: Option<crate::ordinary_value::Value>,
    pub facts: PreviewFacts,
}
pub fn preview_ordinary(request: &OrdinaryPreviewRequest<'_>) -> Result<OrdinaryPreview> {
    preview_ordinary_with_evidence(request, &PreviewEvidence::default())
}
pub fn preview_ordinary_with_evidence(
    request: &OrdinaryPreviewRequest<'_>,
    evidence: &PreviewEvidence<'_>,
) -> Result<OrdinaryPreview> {
    full::preview(request, evidence)
}

pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}
pub fn dispatch(options: &Options, cwd: &Path) -> CommandOutput {
    match dispatch_with_runtime(options, cwd, None, &mut |_| Ok(())) {
        Ok(output) => output,
        Err(error) => CommandOutput {
            stdout: String::new(),
            stderr: format!(
                "{}\n",
                crate::public_readers::explain_missing(
                    error,
                    options.record.iter().filter_map(|p| p.to_str()),
                    cwd,
                    true
                )
            ),
            code: 1,
        },
    }
}
fn active_history(entry: &Path) -> Result<bool> {
    Ok(crate::legacy_authoring::authority_route(entry)?
        == crate::legacy_authoring::AuthorityRoute::History)
}
fn dispatch_with_runtime(
    options: &Options,
    cwd: &Path,
    runtime: Option<&Runtime>,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<CommandOutput> {
    let mut validation = options.clone();
    if validation.refute.is_some() {
        validation.take.clear();
        validation.drops.clear();
    }
    validate(&validation)?;
    let cwd = cwd.canonicalize()?;
    let paths = options
        .record
        .as_ref()
        .map(|p| Ok(vec![cwd.join(p)]))
        .unwrap_or_else(|| public_workspace::records(&cwd))?;
    let route = WriteRoute::capture(&paths, &cwd)?;
    require(route.paths().len() == 1, "choose one logical record entry")?;
    if !options.from_refs.is_empty() {
        return branch::run(options, &route, &paths, runtime, probe);
    }
    let lock = F::DirectoryGuard::acquire(
        route.paths()[0]
            .parent()
            .ok_or_else(|| error("invalid_path"))?,
        true,
    )?;
    if !active_history(&route.paths()[0])? {
        drops(&options.drops)?;
        if options.dry_run {
            return full::run(options, &route, runtime);
        }
        return ordinary::run(options, &route, runtime, probe);
    }
    drop(lock);
    drop(route);
    run_with_runtime(options, &cwd, runtime, probe).map(|stdout| CommandOutput {
        stdout,
        stderr: String::new(),
        code: 0,
    })
}
pub fn run(options: &Options, cwd: &Path) -> Result<String> {
    let output = dispatch_with_runtime(options, cwd, None, &mut |_| Ok(()))?;
    if output.code == 0 {
        Ok(output.stdout)
    } else {
        Err(error(output.stderr.trim_end()))
    }
}

fn run_with_runtime(
    options: &Options,
    cwd: &Path,
    runtime_override: Option<&Runtime>,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<String> {
    validate(options)?;
    let cwd = cwd.canonicalize()?;
    let original = options
        .record
        .as_ref()
        .map(|path| vec![cwd.join(path)])
        .map(Ok)
        .unwrap_or_else(|| public_workspace::records(&cwd))?;
    let route = WriteRoute::capture(&original, &cwd)?;
    require(route.paths().len() == 1, "choose one logical record entry")?;
    let store = Store::new(&route.paths()[0])?;
    let _lock = F::DirectoryGuard::acquire(&store.root, true)?;
    let journal = format!("{}.history", store.layout.journal);
    require(
        F::read(&F::target(&store.root, &journal)?)?.is_none(),
        "recovery_required",
    )?;
    if crate::history_node_publication::selected(&store.entry)? {
        return node_run(options, &route, &original, runtime_override, probe);
    }
    let captured = store.capture()?;
    let by = options.source.as_deref().map(s).unwrap_or(V::Null);
    let write_options = authoring_options(
        if options.refute.is_some() {
            "hypothesis-refute"
        } else {
            "hypothesis-fold"
        },
        by,
    )?;
    let context = HA::capture(&store, &captured, &write_options)?;
    let requested = options
        .refute
        .as_ref()
        .map(|name| vec![name.clone()])
        .unwrap_or_else(|| options.names.clone());
    let names = selected_names(&context, &requested)?;
    if names.is_empty() {
        route.verify()?;
        return Ok("unchanged: \n".into());
    }
    let action = obj([
        (
            "kind",
            s(if options.refute.is_some() {
                "refute"
            } else {
                "consolidate"
            }),
        ),
        ("why", options.why.as_deref().map(s).unwrap_or(V::Null)),
        (
            "source",
            options.source.as_deref().map(s).unwrap_or(V::Null),
        ),
    ]);
    if !options.dry_run {
        privacy_guard(&route, &context, &names, options.source.as_deref(), &action)?;
    }
    let loaded_runtime;
    let runtime = if let Some(runtime) = runtime_override {
        Some(runtime)
    } else {
        loaded_runtime = public_workspace::core_runtime()?;
        loaded_runtime.as_ref()
    };
    let mutation: PreparedMutation = if options.refute.is_some() {
        HA::prepare_refute(
            &store,
            &captured,
            &names,
            options.why.as_deref().unwrap(),
            &write_options,
            runtime,
        )?
    } else {
        HA::prepare_fold(
            &store,
            &captured,
            &HA::Fold {
                names: names.clone(),
                because: if options.dry_run {
                    "consolidation preview".into()
                } else {
                    "explicit consolidation".into()
                },
                take: options.take.clone(),
                drops: drops(&options.drops)?,
                assessment_version: 1,
            },
            &write_options,
            runtime,
        )?
    };
    route.verify()?;
    if options.dry_run {
        let assessment = HA::assess_prepared_fold(&store, &captured, &mutation, runtime)?;
        route.verify()?;
        return preview_output(&names, &assessment);
    }
    direct_history::publish_prepared(&store, &mutation, &route, &original, runtime, probe)?;
    Ok(format!(
        "{}: {}\n",
        if options.refute.is_some() {
            "refuted"
        } else {
            "folded"
        },
        names.join(", ")
    ))
}

fn node_run(
    options: &Options,
    route: &WriteRoute,
    original: &[PathBuf],
    runtime_override: Option<&Runtime>,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<String> {
    use crate::{
        history_node_capture::Capture, history_node_publication as P, history_node_writer as W,
        public_node_history as Public,
    };
    Public::scope(route)?;
    let root = route.paths()[0].parent().unwrap();
    crate::history_node_hypothesis::sources(root)?;
    let captured = Capture::read(root)?;
    let context = crate::history_node_hypothesis::context(&captured)?;
    let requested = options
        .refute
        .as_ref()
        .map(|name| vec![name.clone()])
        .unwrap_or_else(|| options.names.clone());
    let names = selected_names(&context, &requested)?;
    if names.is_empty() {
        route.verify()?;
        return Ok("unchanged: \n".into());
    }
    let source = crate::source_capture::capture_source(
        route.paths(),
        &route.project().root,
        crate::source_capture::ReadMode::Frozen,
        None,
    )?;
    let write_options = authoring_options(
        if options.refute.is_some() {
            "hypothesis-refute"
        } else {
            "hypothesis-fold"
        },
        options.source.as_deref().map(s).unwrap_or(V::Null),
    )?;
    let action = obj([
        (
            "kind",
            s(if options.refute.is_some() {
                "refute"
            } else {
                "fold"
            }),
        ),
        ("names", crate::history_authoring::strings(names.clone())),
        (
            "because",
            s(options.why.as_deref().unwrap_or(if options.dry_run {
                "consolidation preview"
            } else {
                "explicit consolidation"
            })),
        ),
        (
            "take",
            crate::history_authoring::strings(options.take.clone()),
        ),
        ("drops", drops(&options.drops)?),
    ]);
    let request = obj([("kind", s("hypothesis")), ("action", action)]);
    if !options.dry_run {
        privacy_guard(route, &context, &names, options.source.as_deref(), &request)?;
    }
    let loaded;
    let runtime = if let Some(runtime) = runtime_override {
        Some(runtime)
    } else {
        loaded = public_workspace::core_runtime()?;
        loaded.as_ref()
    };
    let prepared = W::prepare(root, &request, &write_options, runtime)?
        .with_guard(&Public::guard(route, original)?)?;
    let (_, after, _, objects) = Public::candidate(root, &prepared)?;
    if options.dry_run {
        let assessment = crate::history_node_hypothesis::assess(
            &captured,
            &after,
            write_options
                .recorded_at
                .get(..10)
                .ok_or_else(|| error("missing_recording_time"))?,
            runtime,
        )?;
        source.verify()?;
        route.verify()?;
        return preview_output(&names, &assessment);
    }
    require(
        !Privacy::private_marker(&objects),
        "private_proposal_requires_draft",
    )?;
    P::publish(
        root,
        &prepared,
        |p| {
            Public::verify_guard(route, original, p)?;
            source.verify()?;
            W::verify(root, p, runtime)?;
            Public::candidate(root, p)?;
            source.verify()?;
            route.verify()
        },
        |phase| {
            route.verify()?;
            W::verify_sources(root, &prepared)?;
            probe(match phase {
                P::Phase::Journal => "journal",
                P::Phase::Append(_) => "append",
                P::Phase::Evidence(_) => "evidence",
                P::Phase::Commit => "committed",
                P::Phase::View => "view",
            })?;
            W::verify_sources(root, &prepared)?;
            route.verify()
        },
    )?;
    let ids = names
        .iter()
        .map(|name| {
            crate::reasoning_snapshot::entries(&map(&map(&context.groups)?[name])?["doc"])
                .map(|e| e.into_keys().collect::<Vec<_>>())
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    crate::session_activity::published(
        root,
        &[crate::history_transaction::FileImage {
            path: "GROUNDING.yaml".into(),
            role: "record".into(),
            before: prepared.before_view()?,
            after: Some(prepared.after_view()?),
        }],
        Some(&ids),
    );
    Ok(format!(
        "{}: {}\n",
        if options.refute.is_some() {
            "refuted"
        } else {
            "folded"
        },
        names.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as J;
    use std::{fs, process::Command};

    struct Oracle {
        python: PathBuf,
        root: PathBuf,
        core_cache: PathBuf,
        runtime: Runtime,
    }
    fn oracle() -> Oracle {
        let required = |name: &str| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .unwrap_or_else(|| panic!("set explicit oracle input {name}"))
        };
        let cache = tempfile::tempdir().unwrap().keep();
        Oracle {
            python: required("KPOP_CONSOLIDATION_ORACLE_PYTHON"),
            root: required("KPOP_CONSOLIDATION_ORACLE_ROOT"),
            core_cache: required("KPOP_CONSOLIDATION_ORACLE_CORE_CACHE"),
            runtime: Runtime::open(
                &required("KPOP_CONSOLIDATION_NATIVE_ARCHIVE"),
                &cache,
                crate::reasoning_runtime::OperationalBounds::default(),
            )
            .unwrap(),
        }
    }
    fn case(name: &str) -> J {
        serde_json::from_str::<J>(include_str!(
            "../tests/fixtures/history-fold-candidate.json"
        ))
        .unwrap()["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == name)
            .unwrap()
            .clone()
    }
    fn materialize(case: &J) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for (path, raw) in case["files"].as_object().unwrap() {
            let path = root.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, raw.as_str().unwrap()).unwrap();
        }
        root
    }
    fn python(oracle: &Oracle, root: &Path, args: &[&str]) -> std::process::Output {
        Command::new(&oracle.python)
            .arg(oracle.root.join("scripts/cli.py"))
            .arg("consolidate")
            .args(args)
            .arg(root.join("GROUNDING.yaml"))
            .env("KPOPPER_CORE_CACHE", &oracle.core_cache)
            .current_dir(root)
            .output()
            .unwrap()
    }
    fn run_native(
        oracle: &Oracle,
        root: &Path,
        options: &Options,
    ) -> std::result::Result<String, String> {
        run_with_runtime(options, root, Some(&oracle.runtime), &mut |_| Ok(()))
            .map_err(|error| error.0)
    }
    fn document_without_history(path: &Path) -> V {
        let mut document = crate::history_yaml::decode_document(&fs::read(path).unwrap()).unwrap();
        if let Some(meta) = map_mut(&mut document).unwrap().get_mut("meta") {
            map_mut(meta).unwrap().remove("history");
        }
        document
    }
    fn tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        fn visit(root: &Path, at: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
            let mut entries = fs::read_dir(at)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            entries.sort();
            for path in entries {
                if path.is_dir() {
                    visit(root, &path, out);
                } else if path.extension().and_then(|value| value.to_str()) == Some("lock") {
                    continue;
                } else {
                    out.push((
                        path.strip_prefix(root).unwrap().to_owned(),
                        fs::read(path).unwrap(),
                    ));
                }
            }
        }
        let mut out = vec![];
        visit(root, root, &mut out);
        out
    }

    #[test]
    #[ignore = "requires explicit immutable Python oracle and core runtime"]
    fn dry_run_fold_and_refute_match_the_python_18_public_contract() {
        let oracle = oracle();
        for action in ["dry-run", "fold", "refute"] {
            let py_root = materialize(&case("0-new-value"));
            let rs_root = materialize(&case("0-new-value"));
            let (py_args, options) = match action {
                "dry-run" => (
                    vec!["--dry-run", "trial"],
                    Options {
                        names: vec!["trial".into()],
                        dry_run: true,
                        ..Default::default()
                    },
                ),
                "fold" => (
                    vec!["trial"],
                    Options {
                        names: vec!["trial".into()],
                        ..Default::default()
                    },
                ),
                _ => (
                    vec!["--refute", "trial", "tested and refuted"],
                    Options {
                        refute: Some("trial".into()),
                        why: Some("tested and refuted".into()),
                        ..Default::default()
                    },
                ),
            };
            let before = tree(rs_root.path());
            let expected = python(&oracle, py_root.path(), &py_args);
            assert!(
                expected.status.success(),
                "{action}: {}",
                String::from_utf8_lossy(&expected.stderr)
            );
            let actual = run_native(&oracle, rs_root.path(), &options).unwrap();
            assert_eq!(
                actual,
                String::from_utf8(expected.stdout).unwrap(),
                "{action}"
            );
            if action == "dry-run" {
                assert_eq!(tree(rs_root.path()), before, "dry-run changed the record");
            } else {
                assert_eq!(
                    document_without_history(&py_root.path().join("GROUNDING.yaml")),
                    document_without_history(&rs_root.path().join("GROUNDING.yaml")),
                    "{action}: accepted document"
                );
            }
        }
    }

    #[test]
    #[ignore = "requires explicit immutable Python oracle and core runtime"]
    fn conflicts_and_interruptions_retain_the_python_refusal_and_exact_recovery() {
        let oracle = oracle();
        let py_root = materialize(&case("0-overlapping"));
        let expected = python(&oracle, py_root.path(), &["--dry-run", "trial", "other"]);
        assert!(!expected.status.success());
        let rs_root = materialize(&case("0-overlapping"));
        let before = tree(rs_root.path());
        let refusal = run_native(
            &oracle,
            rs_root.path(),
            &Options {
                names: vec!["trial".into(), "other".into()],
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(String::from_utf8_lossy(&expected.stderr).contains(&refusal));
        assert_eq!(tree(rs_root.path()), before, "refusal changed evidence");

        for stage in ["journal", "committed"] {
            let root = materialize(&case("0-new-value"));
            let entry = root.path().join("GROUNDING.yaml");
            let stopped = run_with_runtime(
                &Options {
                    names: vec!["trial".into()],
                    ..Default::default()
                },
                root.path(),
                Some(&oracle.runtime),
                &mut |at| require(at != stage, "interrupted"),
            );
            assert_eq!(stopped.unwrap_err().0, "interrupted");
            let store = Store::new(&entry).unwrap();
            let journal = root
                .path()
                .join(format!("{}.history", store.layout.journal));
            let bytes = fs::read(&journal).unwrap();
            let recovered = direct_history::recover_with_runtime(
                std::slice::from_ref(&entry),
                root.path(),
                false,
                Some(&oracle.runtime),
            )
            .unwrap();
            assert!(string_is(&map(&recovered).unwrap()["state"], "recovered"));
            assert!(!journal.exists());
            assert!(!bytes.is_empty());
        }
    }
}

#[cfg(test)]
#[path = "ordinary_consolidation_tests.rs"]
mod ordinary_tests;

#[cfg(test)]
mod node_tests {
    use super::*;
    use crate::{
        history_node_capture::Capture, history_node_publication as P, history_node_writer as W,
        history_yaml as Y,
    };
    use serde_json::json;
    fn v(value: serde_json::Value) -> V {
        V::from_json(&value).unwrap()
    }
    fn fixture(private: bool) -> (tempfile::TempDir, tempfile::TempDir, Runtime) {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".kpopper")).unwrap();
        std::fs::write(root.path().join(".kpopper/history.yaml"),"version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: fixture\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
        let doc = v(
            json!({"meta":{"purpose":"Fixture","reasoning":{"version":2,"profile":"core/v1","requires":["arithmetic/v1"]}},"schema":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"},"known":{}}),
        );
        std::fs::write(
            root.path().join("GROUNDING.yaml"),
            Y::encode_document(&doc).unwrap(),
        )
        .unwrap();
        let cache = tempfile::tempdir().unwrap();
        let archive = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!(
                "{}.kpopper-runtime",
                crate::reasoning_runtime::target_name().unwrap()
            ));
        let runtime = Runtime::open(
            &archive,
            cache.path(),
            crate::reasoning_runtime::OperationalBounds::default(),
        )
        .unwrap();
        let mut body = json!({"v":2});
        if private {
            body["private"] = json!(true);
        }
        let action = v(
            json!({"kind":"hypothesis","name":"alpha","action":{"kind":"add","id":"p.new","body":body}}),
        );
        let p = W::prepare(
            root.path(),
            &action,
            &authoring_options("proposal", V::Null).unwrap(),
            Some(&runtime),
        )
        .unwrap();
        W::publish(root.path(), &p, Some(&runtime), |_| Ok(())).unwrap();
        (root, cache, runtime)
    }
    #[test]
    fn public_named_fold_recovers_each_durable_decision() {
        for phase in ["journal", "committed"] {
            let (root, _cache, runtime) = fixture(false);
            let options = Options {
                names: vec!["alpha".into()],
                ..Default::default()
            };
            assert!(
                run_with_runtime(
                    &options,
                    root.path(),
                    Some(&runtime),
                    &mut |at| if at == phase {
                        Err(error("crash"))
                    } else {
                        Ok(())
                    }
                )
                .is_err()
            );
            let original = vec![root.path().join("GROUNDING.yaml")];
            let route = WriteRoute::capture(&original, root.path()).unwrap();
            let result =
                crate::public_node_history::recover(&route, &original, false, Some(&runtime))
                    .unwrap();
            assert!(string_is(
                &map(&result).unwrap()["state"],
                if phase == "journal" {
                    "rolled_back"
                } else {
                    "committed"
                }
            ));
            let c = Capture::read(root.path()).unwrap();
            assert_eq!(
                crate::reasoning_snapshot::entries(c.document())
                    .unwrap()
                    .contains_key("p.new"),
                phase == "committed"
            );
        }
    }
    #[test]
    fn recovery_refuses_private_named_fold_even_when_only_accept_acts_are_written() {
        let (root, _cache, runtime) = fixture(true);
        let original = vec![root.path().join("GROUNDING.yaml")];
        let route = WriteRoute::capture(&original, root.path()).unwrap();
        let request = v(
            json!({"kind":"hypothesis","action":{"kind":"fold","names":["alpha"],"because":"test"}}),
        );
        let p = W::prepare(
            root.path(),
            &request,
            &authoring_options("fold", V::Null).unwrap(),
            Some(&runtime),
        )
        .unwrap()
        .with_guard(&crate::public_node_history::guard(&route, &original).unwrap())
        .unwrap();
        assert!(
            P::publish(
                root.path(),
                &p,
                |p| W::verify(root.path(), p, Some(&runtime)),
                |phase| if phase == P::Phase::Commit {
                    Err(error("crash"))
                } else {
                    Ok(())
                }
            )
            .is_err()
        );
        let before = std::fs::read(&original[0]).unwrap();
        assert!(
            crate::public_node_history::recover(&route, &original, false, Some(&runtime))
                .unwrap_err()
                .0
                .contains("private")
        );
        assert_eq!(std::fs::read(&original[0]).unwrap(), before);
        assert!(
            root.path()
                .join(".kpopper/.history-node-publication.json")
                .exists()
        );
    }
}

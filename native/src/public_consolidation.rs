//! Public named-hypothesis consolidation over ordinary records and active history.
#[path = "public_branch_consolidation.rs"]
mod branch;
#[path = "ordinary_consolidation_full.rs"]
mod full;
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
            let mut intent = action.clone();
            map_mut(&mut intent)?.insert("hypothesis".into(), s(name));
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
    }
    Ok(())
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
            stderr: format!("{error}\n"),
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

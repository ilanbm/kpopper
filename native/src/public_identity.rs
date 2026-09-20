//! Public `same` and `distinct` commands over an active history record.
#[path = "ordinary_sameness.rs"]
pub(crate) mod ordinary_sameness;
use crate::{
    Result, direct_history,
    history_authoring::{self as A, obj, s},
    history_contract::*,
    history_hypothesis_authoring as HA, history_identity as I,
    history_paths::Scheme,
    history_store::Store,
    history_transaction_fs as F,
    project_modes::WriteRoute,
    public_history::fresh_id,
    public_workspace,
    reasoning_runtime::Runtime,
    recording_privacy as Privacy, require,
    value::TypedValue as V,
};
use std::path::{Path, PathBuf};

#[derive(Clone, Default, clap::Args)]
pub struct SameOptions {
    pub a: String,
    pub b: String,
    #[arg(long)]
    pub keep: Option<String>,
    #[arg(long)]
    pub as_of: Option<String>,
    /// Optional record YAML path.
    pub record: Option<PathBuf>,
}

#[derive(Clone, Default, clap::Args)]
pub struct DistinctOptions {
    pub a: String,
    pub b: String,
    /// Why these similar subjects denote different things.
    pub why: String,
    #[arg(long)]
    pub as_of: Option<String>,
    /// Optional record YAML path.
    pub record: Option<PathBuf>,
}

#[derive(Clone)]
struct Request {
    a: String,
    b: String,
    action: I::Action,
    as_of: Option<String>,
    record: Option<PathBuf>,
}

fn authoring_options(prefix: &str) -> Result<A::Options> {
    let now = chrono::Utc::now();
    Ok(A::Options {
        operation: fresh_id(prefix)?,
        recorded_at: now.to_rfc3339(),
        recording_day: chrono::Local::now().date_naive().to_string(),
        by: V::Null,
        strict: true,
        paths: Scheme::Hashed,
        receipt_version: None,
    })
}

fn action(request: &Request) -> V {
    obj([
        (
            "kind",
            s(if matches!(request.action, I::Action::Same { .. }) {
                "same"
            } else {
                "distinct"
            }),
        ),
        ("ids", V::List(vec![s(&request.a), s(&request.b)])),
    ])
}

fn selected_privacy(context: &HA::Context, request: &Request) -> Result<V> {
    let roots = [request.a.clone(), request.b.clone()];
    let mut selected = vec![];
    let mut worlds = vec![context.base.clone()];
    for name in map(&context.groups)?.keys() {
        worlds.push(HA::layer(
            &context.base,
            &context.groups,
            std::slice::from_ref(name),
        )?);
    }
    for world in worlds {
        let present = crate::reasoning_snapshot::entries(&world)?;
        let roots = roots
            .iter()
            .filter(|id| present.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        if !roots.is_empty() {
            selected.push(crate::pending_bundle::closure(&world, &roots).map_err(|_| {
                error("identity source closure could not be checked; original retained")
            })?);
        }
    }
    let controls = V::Map(
        map(&context.base)?
            .iter()
            .filter(|(key, _)| {
                ["meta", "privacy", "visibility", "private", "shareability"].contains(&key.as_str())
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    );
    Ok(obj([
        ("selected", V::List(selected)),
        ("record_metadata", controls),
    ]))
}

pub fn run_same(options: &SameOptions, cwd: &Path) -> Result<String> {
    run(
        Request {
            a: options.a.clone(),
            b: options.b.clone(),
            action: I::Action::Same {
                keep: options.keep.clone(),
            },
            as_of: options.as_of.clone(),
            record: options.record.clone(),
        },
        cwd,
    )
}

pub fn run_distinct(options: &DistinctOptions, cwd: &Path) -> Result<String> {
    require(
        !options.why.contains('\n'),
        "the why is one line: a second line would be a line of the record",
    )?;
    run(
        Request {
            a: options.a.clone(),
            b: options.b.clone(),
            action: I::Action::Distinct {
                because: options.why.clone(),
            },
            as_of: options.as_of.clone(),
            record: options.record.clone(),
        },
        cwd,
    )
}

fn run(request: Request, cwd: &Path) -> Result<String> {
    run_with_runtime(request, cwd, None, &mut |_| Ok(()))
}

fn run_with_runtime(
    request: Request,
    cwd: &Path,
    runtime_override: Option<&Runtime>,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<String> {
    let cwd = cwd.canonicalize()?;
    let original = request
        .record
        .as_ref()
        .map(|path| vec![cwd.join(path)])
        .map(Ok)
        .unwrap_or_else(|| public_workspace::records(&cwd))?;
    let route = WriteRoute::capture(&original, &cwd)?;
    require(route.paths().len() == 1, "choose one logical record entry")?;
    let entry = &route.paths()[0];
    let _lock =
        F::DirectoryGuard::acquire(entry.parent().ok_or_else(|| error("invalid_path"))?, true)?;
    require(
        crate::legacy_authoring::route(entry, route.config())?
            == crate::legacy_authoring::AuthorityRoute::History,
        "legacy identity commands require the Python adapter",
    )?;
    let store = Store::new(entry)?;
    let journal = format!("{}.history", store.layout.journal);
    require(
        F::read(&F::target(&store.root, &journal)?)?.is_none(),
        "recovery_required",
    )?;
    let captured = store.capture()?;
    let write_options = authoring_options(if matches!(request.action, I::Action::Same { .. }) {
        "history-same"
    } else {
        "history-distinct"
    })?;
    let context = HA::capture(&store, &captured, &write_options)?;
    let retained = selected_privacy(&context, &request)?;
    let command = action(&request);
    if Privacy::private_marker(&retained) {
        let draft = Privacy::draft(
            route.project(),
            &command,
            &retained,
            "private identity change retained for review",
        )?;
        return Ok(format!(
            "{}\n",
            crate::public_core_readers::json_value(&draft)?
        ));
    }
    let loaded_runtime;
    let runtime = if let Some(runtime) = runtime_override {
        Some(runtime)
    } else {
        loaded_runtime = public_workspace::runtime_for_document(&context.base)?;
        loaded_runtime.as_ref()
    };
    let intent = I::Intent {
        a: request.a.clone(),
        b: request.b.clone(),
        action: request.action,
        as_of: request.as_of,
    };
    let mutation = I::prepare(&store, &captured, &intent, &write_options, runtime)?;
    let authored = V::List(
        mutation
            .files()
            .iter()
            .filter(|file| file.role == "history_object")
            .map(|file| {
                crate::history_yaml::decode_document(
                    file.after
                        .as_ref()
                        .ok_or_else(|| error("invalid_history_object"))?,
                )
            })
            .collect::<Result<_>>()?,
    );
    if Privacy::private_marker(&authored) {
        let draft = Privacy::draft(
            route.project(),
            &command,
            &obj([("history", authored)]),
            "private identity change retained for review",
        )?;
        return Ok(format!(
            "{}\n",
            crate::public_core_readers::json_value(&draft)?
        ));
    }
    direct_history::publish_prepared(&store, &mutation, &route, &original, runtime, probe)?;
    crate::session_activity::published(
        &store.root,
        mutation.files(),
        Some(&[request.a.clone(), request.b.clone()].into_iter().collect()),
    );
    Ok(format!(
        "history committed: {} ({} {}, {})\n",
        text(&map(&mutation.to_data())?["operation"])?,
        text(&map(&command)?["kind"])?,
        request.a,
        request.b
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_view::map_mut;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde_json::Value as J;
    use std::{fs, process::Command};

    fn corpus() -> J {
        crate::json_ingress::parse_slice(
            include_bytes!("../tests/fixtures/history-identity.json"),
            crate::json_ingress::DuplicateKeys::Reject,
        )
        .unwrap()
    }
    fn case(prefix: &str, kind: &str) -> J {
        corpus()["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"].as_str().unwrap().starts_with(prefix) && case["kind"] == kind)
            .unwrap()
            .clone()
    }
    fn materialize(case: &J) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for (path, raw) in case["files"].as_object().unwrap() {
            let path = root.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, STANDARD.decode(raw.as_str().unwrap()).unwrap()).unwrap();
        }
        root
    }
    fn runtime(cache: &Path) -> Runtime {
        Runtime::open(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/reasoning/native")
                .join(format!(
                    "{}.kpopper-runtime",
                    crate::reasoning_runtime::target_name().unwrap()
                )),
            cache,
            crate::reasoning_runtime::OperationalBounds::default(),
        )
        .unwrap()
    }
    fn request(case: &J) -> Request {
        Request {
            a: case["a"].as_str().unwrap().into(),
            b: case["b"].as_str().unwrap().into(),
            action: if case["kind"] == "same" {
                I::Action::Same {
                    keep: case["keep"].as_str().map(str::to_owned),
                }
            } else {
                I::Action::Distinct {
                    because: case["because"].as_str().unwrap().into(),
                }
            },
            as_of: case["as_of"].as_str().map(str::to_owned),
            record: Some("GROUNDING.yaml".into()),
        }
    }
    fn tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        fn visit(root: &Path, at: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
            let mut paths = fs::read_dir(at)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            paths.sort();
            for path in paths {
                if path.is_dir() {
                    visit(root, &path, out);
                } else if path.extension().and_then(|v| v.to_str()) != Some("lock") {
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
    fn document(path: &Path) -> V {
        let mut value = crate::history_yaml::decode_document(&fs::read(path).unwrap()).unwrap();
        if let Some(meta) = map_mut(&mut value).unwrap().get_mut("meta") {
            map_mut(meta).unwrap().remove("history");
        }
        value
    }

    #[test]
    fn privacy_selection_follows_the_two_subjects_source_closure() {
        let context = HA::Context {
            base: V::from_json(&serde_json::json!({
                "known": {
                    "p.a": {"v": 1, "from": "p.source"},
                    "p.b": {"v": 1},
                    "p.source": {"v": "sensitive", "privacy": "private"},
                    "p.unrelated": {"v": "also sensitive", "privacy": "private"}
                }
            }))
            .unwrap(),
            groups: obj([]),
            index: obj([]),
            physical: Default::default(),
        };
        let request = Request {
            a: "p.a".into(),
            b: "p.b".into(),
            action: I::Action::Same { keep: None },
            as_of: None,
            record: None,
        };
        let selected = selected_privacy(&context, &request).unwrap();
        assert!(Privacy::private_marker(&selected));
        let encoded = selected.canonical_bytes().unwrap();
        assert!(encoded.windows(b"p.source".len()).any(|w| w == b"p.source"));
        assert!(
            !encoded
                .windows(b"p.unrelated".len())
                .any(|w| w == b"p.unrelated")
        );
    }

    #[test]
    fn public_identity_publishes_once_and_retains_preexisting_objects() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        for (prefix, kind) in [
            ("same_rewrites_current", "same"),
            ("distinct_keeps_original", "distinct"),
        ] {
            let root = materialize(&case(prefix, kind));
            let entry = root.path().join("GROUNDING.yaml");
            let store = Store::new(&entry).unwrap();
            let before = store.capture().unwrap();
            let output = run_with_runtime(
                request(&case(prefix, kind)),
                root.path(),
                Some(&runtime),
                &mut |_| Ok(()),
            )
            .unwrap();
            assert!(output.starts_with(&format!("history committed: history-{kind}-")));
            assert!(output.ends_with(&format!(" ({kind} p.input, p.other)\n")));
            let after = Store::new(&entry).unwrap().capture().unwrap();
            assert_eq!(after.commits.len(), before.commits.len() + 1);
            for (key, raw) in before.object_bytes {
                assert_eq!(after.object_bytes.get(&key), Some(&raw));
            }
            assert!(
                !root
                    .path()
                    .join(format!("{}.history", store.layout.journal))
                    .exists()
            );
        }
    }

    #[test]
    fn identity_refusals_do_not_change_any_record_bytes() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        for (prefix, expected) in [
            (
                "unordered_conflicting_values",
                "identity_conflicting_readings",
            ),
            ("alias_self_dependency_refuses", "identity_pin_cycle"),
            ("pin_context_collapse_refuses", "identity_pin_collision"),
        ] {
            let selected = case(prefix, "same");
            let root = materialize(&selected);
            let before = tree(root.path());
            let error =
                run_with_runtime(request(&selected), root.path(), Some(&runtime), &mut |_| {
                    Ok(())
                })
                .unwrap_err();
            assert!(error.0.contains(expected), "{prefix}: {error}");
            assert_eq!(tree(root.path()), before, "{prefix}");
        }
    }

    #[test]
    fn identity_journal_recovers_forward_or_rolls_back_without_losing_evidence() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        for (stage, rollback) in [("journal", true), ("journal", false), ("committed", false)] {
            let selected = case("distinct_keeps_original", "distinct");
            let root = materialize(&selected);
            let entry = root.path().join("GROUNDING.yaml");
            let before = Store::new(&entry).unwrap().capture().unwrap();
            let stopped =
                run_with_runtime(request(&selected), root.path(), Some(&runtime), &mut |at| {
                    require(at != stage, "interrupted")
                });
            assert_eq!(stopped.unwrap_err().0, "interrupted");
            let store = Store::new(&entry).unwrap();
            let journal = root
                .path()
                .join(format!("{}.history", store.layout.journal));
            assert!(journal.is_file());
            let result = direct_history::recover_with_runtime(
                std::slice::from_ref(&entry),
                root.path(),
                rollback,
                Some(&runtime),
            )
            .unwrap();
            assert_eq!(
                text(&map(&result).unwrap()["state"]).unwrap(),
                if rollback { "restored" } else { "recovered" }
            );
            assert!(!journal.exists());
            let after = Store::new(&entry).unwrap().capture().unwrap();
            for (key, raw) in &before.object_bytes {
                assert_eq!(after.object_bytes.get(key), Some(raw));
            }
            assert_eq!(
                after.commits.len(),
                before.commits.len() + usize::from(!rollback)
            );
        }
    }

    #[test]
    #[ignore = "requires the explicit immutable Python oracle"]
    fn public_output_and_documents_match_python() {
        let python = std::env::var_os("KPOP_IDENTITY_ORACLE_PYTHON").unwrap();
        let oracle = PathBuf::from(std::env::var_os("KPOP_IDENTITY_ORACLE_ROOT").unwrap());
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let shape = regex::Regex::new(r"history-(same|distinct)-[0-9a-f]+").unwrap();
        for (prefix, kind, why) in [
            ("same_rewrites_current", "same", None),
            (
                "distinct_keeps_original",
                "distinct",
                Some("separate observations"),
            ),
        ] {
            let selected = case(prefix, kind);
            let py = materialize(&selected);
            let rs = materialize(&selected);
            let mut args = vec![kind, "p.input", "p.other"];
            if let Some(why) = why {
                args.push(why);
            }
            args.extend(["GROUNDING.yaml", "--as-of", "2026-09-19"]);
            let expected = Command::new(&python)
                .arg(oracle.join("scripts/provenance.py"))
                .args(&args)
                .current_dir(py.path())
                .output()
                .unwrap();
            assert!(
                expected.status.success(),
                "{}",
                String::from_utf8_lossy(&expected.stderr)
            );
            let mut native = request(&selected);
            native.as_of = Some("2026-09-19".into());
            if let I::Action::Distinct { because } = &mut native.action {
                *because = "separate observations".into();
            }
            let actual =
                run_with_runtime(native, rs.path(), Some(&runtime), &mut |_| Ok(())).unwrap();
            assert_eq!(
                shape.replace_all(&actual, "<operation>"),
                shape.replace_all(&String::from_utf8(expected.stdout).unwrap(), "<operation>")
            );
            assert_eq!(
                document(&rs.path().join("GROUNDING.yaml")),
                document(&py.path().join("GROUNDING.yaml"))
            );
        }
    }
}

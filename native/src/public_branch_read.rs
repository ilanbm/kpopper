//! Read-only comparison with one immutable Git commit. The resolved commit is
//! data, never a checkout or source of write authority.
use super::C;
use crate::{
    Result,
    history_capture::Capture,
    history_contract::{Map, error, map, string_is, text},
    ordinary_value::{Map as OMap, Value as OV},
    reasoning_runtime::Runtime,
    require,
    source_capture::OrdinaryCapture,
    value::TypedValue as V,
};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

const OUTPUT_LIMIT: usize = 1024 * 1024;

fn s(value: &str) -> V {
    V::Text(value.into())
}
fn os(value: &str) -> OV {
    OV::Text(value.into())
}

fn git(root: &Path, args: &[&str], missing: bool) -> Result<Option<Vec<u8>>> {
    crate::pending_state::git(root, args, OUTPUT_LIMIT, missing)
}

fn resolve(root: &Path, reference: &str) -> Result<(String, String)> {
    require(
        !reference.is_empty() && !reference.contains('\0'),
        "invalid_branch_ref",
    )?;
    let expression = format!("{reference}^{{commit}}");
    let Some(raw) = git(
        root,
        &["rev-parse", "--verify", "--end-of-options", &expression],
        true,
    )?
    else {
        return Err(error(&format!(
            "refused - {reference} is not a commit this checkout knows"
        )));
    };
    let oid = std::str::from_utf8(&raw)
        .map_err(|_| error("invalid_branch_ref"))?
        .trim()
        .to_owned();
    require(
        [40, 64].contains(&oid.len())
            && oid
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "invalid_branch_ref",
    )?;
    let raw = git(root, &["log", "-1", "--format=%as", &oid], false)?
        .ok_or_else(|| error("branch_git_unavailable"))?;
    let day = std::str::from_utf8(&raw)
        .map_err(|_| error("invalid_branch_ref"))?
        .trim()
        .to_owned();
    Ok((oid, day))
}

pub(crate) fn context(
    paths: &[PathBuf],
    cwd: &Path,
    reference: &str,
) -> Result<(PathBuf, PathBuf, String, String, String)> {
    let entry = paths
        .first()
        .ok_or_else(|| error("record_required"))?
        .canonicalize()?;
    let project = crate::project_modes::project_for(paths, cwd)?;
    require(
        project.is_git(),
        &format!(
            "refused - --from reads a committed record, and {} is not in a git checkout",
            entry.display()
        ),
    )?;
    let relative = entry
        .strip_prefix(&project.root)
        .map_err(|_| {
            error(&format!(
                "refused - --from reads a committed record, and {} is not in the tree at {}",
                entry.display(),
                project.root.display()
            ))
        })?
        .to_str()
        .ok_or_else(|| error("nonportable_project_path"))?
        .replace('\\', "/");
    let (oid, day) = resolve(&project.root, reference)?;
    Ok((entry, project.root, relative, oid, day))
}

fn indent_yaml(value: &V, lines: &mut Vec<String>, bytes: &mut usize) -> Result<()> {
    let raw = crate::public_ordinary_readers::python_safe_dump(
        &crate::history_yaml::OrdinaryValue::from_typed(value),
    )?;
    *bytes += raw.len() + 3 * (raw.iter().filter(|b| **b == b'\n').count() + 1);
    require(*bytes <= OUTPUT_LIMIT, "branch_pull_output_limit")?;
    let rendered = std::str::from_utf8(&raw).map_err(|_| error("invalid_branch_output"))?;
    lines.extend(rendered.trim_end().lines().map(|line| format!("  {line}")));
    Ok(())
}

fn entries(document: &V) -> Result<std::collections::BTreeMap<String, (String, V)>> {
    crate::reasoning_snapshot::entries(document)
}

fn history_side(subject: &str, capture: &Capture, snapshot: &V) -> Result<V> {
    let states = map(&map(&capture.state)?["subjects"])?;
    let state = states.get(subject).map(map).transpose()?;
    let mut originals = Vec::new();
    if let Some(state) = state {
        let heads = crate::history_view::list(&state["heads"])?;
        let proposals = crate::history_view::list(&state["proposals"])?;
        let versions = heads
            .iter()
            .chain(proposals)
            .map(text)
            .collect::<Result<BTreeSet<_>>>()?;
        for version in versions {
            let object = map(capture
                .objects
                .get(version)
                .ok_or_else(|| error("missing_history_object"))?)?;
            originals.push(V::Map(Map::from([
                ("version".into(), s(version)),
                (
                    "disposition".into(),
                    s(if heads.iter().any(|v| string_is(v, version)) {
                        "head"
                    } else {
                        "proposal"
                    }),
                ),
                ("kind".into(), object["kind"].clone()),
                ("by".into(), object["by"].clone()),
                ("on".into(), object["on"].clone()),
                ("operation".into(), object["op"].clone()),
                (
                    "authored".into(),
                    object.get("authored").cloned().unwrap_or(V::Null),
                ),
                (
                    "pins".into(),
                    object
                        .get("pins")
                        .cloned()
                        .unwrap_or_else(|| V::Map(Map::new())),
                ),
                (
                    "pin_gaps".into(),
                    object
                        .get("pin_gaps")
                        .cloned()
                        .unwrap_or_else(|| V::Map(Map::new())),
                ),
                ("body".into(), object["body"].clone()),
            ])));
        }
    }
    let mut named = Map::new();
    for (name, hypothesis) in map(&map(snapshot)?["hypotheses"])? {
        let hypothesis = map(hypothesis)?;
        if let Some((collection, body)) = entries(&hypothesis["document"])?.get(subject) {
            named.insert(
                name.clone(),
                V::Map(Map::from([
                    ("standing".into(), s("proposal")),
                    ("head".into(), hypothesis["head"].clone()),
                    ("collection".into(), s(collection)),
                    ("body".into(), body.clone()),
                    (
                        "error".into(),
                        hypothesis.get("error").cloned().unwrap_or(V::Null),
                    ),
                ])),
            );
        }
    }
    Ok(V::Map(Map::from([
        (
            "standing".into(),
            s(state
                .map(|v| text(&v["acceptance"]))
                .transpose()?
                .unwrap_or("absent")),
        ),
        (
            "heads".into(),
            state
                .map(|v| v["heads"].clone())
                .unwrap_or_else(|| V::List(vec![])),
        ),
        ("original_claims".into(), V::List(originals)),
        ("named_proposals".into(), V::Map(named)),
    ])))
}

fn history_pull(
    reference: &str,
    seeds: &[String],
    budget: i64,
    entry: &Path,
    root: &Path,
    relative: &str,
    oid: &str,
    current: &OrdinaryCapture,
) -> Result<C::Output> {
    let observed = crate::history_branch_git::capture(root, oid, relative, None)?;
    let branch = crate::history_branch::validate(&observed.envelope, &observed.files)?;
    let branch_snapshot =
        crate::history_branch::snapshot(&observed.envelope, &observed.files)?.to_data();
    let current_capture = current
        .history_capture()
        .ok_or_else(|| error("history_capture_required"))?;
    let current_snapshot = current.snapshot()?.to_data();
    let mut candidates = map(&map(&current_capture.state)?["subjects"])?
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    candidates.extend(map(&map(&branch.state)?["subjects"])?.keys().cloned());
    for snapshot in [&current_snapshot, &branch_snapshot] {
        for hypothesis in map(&map(snapshot)?["hypotheses"])?.values() {
            candidates.extend(entries(&map(hypothesis)?["document"])?.into_keys());
        }
    }
    let mut selected = BTreeSet::new();
    let mut unmatched = Vec::new();
    for seed in seeds {
        let prefix = format!("{}.", seed.trim_end_matches('.'));
        let found = candidates
            .iter()
            .filter(|id| *id == seed || id.starts_with(&prefix))
            .cloned()
            .collect::<Vec<_>>();
        if found.is_empty() {
            unmatched.push(seed.clone());
        }
        selected.extend(found);
    }
    let ordered = selected.into_iter().collect::<Vec<_>>();
    let visible = ordered
        .iter()
        .take(budget as usize)
        .cloned()
        .collect::<Vec<_>>();
    let manifest = map(&map(&observed.envelope)?["manifest"])?;
    let source = map(&manifest["source"])?;
    let authority = |capture: &Capture| -> Result<String> {
        let marker = map(&capture.marker)?;
        Ok(format!(
            "{} / generation {}",
            text(&marker["record_id"])?,
            crate::ordinary_value::py(&OV::from_typed(&marker["generation"]))
        ))
    };
    let digest = |capture: &Capture| -> Result<String> {
        Ok(text(&map(&capture.baseline)?["committed_set_digest"])?.to_owned())
    };
    let mut lines = vec![
        "BRANCH OBSERVATION — no target acceptance or adoption".into(),
        format!("source ref: {reference}"),
        format!("source commit: {}", text(&source["commit"])?),
        format!("source entry: {}", text(&source["entry"])?),
        format!("branch authority: {}", authority(&branch)?),
        format!("branch committed set: {}", digest(&branch)?),
        format!("current record: {}", entry.display()),
        format!("current authority: {}", authority(current_capture)?),
        format!("current committed set: {}", digest(current_capture)?),
        format!(
            "subjects shown: {}; omitted by budget: {}",
            visible.len(),
            ordered.len() - visible.len()
        ),
    ];
    if !unmatched.is_empty() {
        lines.push(format!("unmatched seeds: {}", unmatched.join(", ")));
    }
    let mut bytes = lines.iter().map(|line| line.len() + 1).sum();
    for (label, snapshot) in [("current", &current_snapshot), ("branch", &branch_snapshot)] {
        let mut metadata = map(&map(snapshot)?["document"])?
            .get("meta")
            .cloned()
            .unwrap_or_else(|| V::Map(Map::new()));
        if let V::Map(m) = &mut metadata {
            m.remove("history");
        }
        if !map(&metadata)?.is_empty() {
            lines.push(format!(
                "{label} record metadata (generated baseline identified above):"
            ));
            indent_yaml(&metadata, &mut lines, &mut bytes)?;
        }
    }
    for subject in visible {
        lines.push(String::new());
        lines.push(format!("SUBJECT {subject}"));
        for (label, capture, snapshot) in [
            ("current", current_capture, &current_snapshot),
            ("branch", &branch, &branch_snapshot),
        ] {
            lines.push(format!("{label}:"));
            indent_yaml(
                &history_side(&subject, capture, snapshot)?,
                &mut lines,
                &mut bytes,
            )?;
        }
    }
    let output = lines.join("\n") + "\n";
    require(output.len() <= OUTPUT_LIMIT, "branch_pull_output_limit")?;
    Ok(C::Output {
        text: output,
        code: 0,
    })
}

fn ordinary_hypothesis(name: &str, document: OV, head: OV) -> OV {
    OV::Map(OMap::from([
        ("name".into(), os(name)),
        ("doc".into(), document.clone()),
        ("document".into(), document),
        ("head".into(), head),
    ]))
}

fn ordinary_pull(
    reference: &str,
    seeds: &[String],
    budget: i64,
    root: &Path,
    relative: &str,
    oid: &str,
    day: &str,
    current: &OrdinaryCapture,
    runtime: Option<&Runtime>,
) -> Result<C::Output> {
    let branch =
        crate::source_target::records_ordinary(root, relative, oid, runtime).map_err(|e| {
            if e.0 == "target_record_unavailable" {
                error(&format!("refused - {reference} holds no {relative}"))
            } else {
                e
            }
        })?;
    let branch_doc = branch.document;
    let mut hypotheses = match current.hypotheses() {
        OV::Map(m) => m.clone(),
        _ => OMap::new(),
    };
    let head = OV::Map(OMap::from([
        (
            "claim".into(),
            os(&format!(
                "what {reference} committed ({}), read as a hypothesis",
                &oid[..7]
            )),
        ),
        ("born".into(), os(day)),
    ]));
    hypotheses.insert(
        reference.into(),
        ordinary_hypothesis(reference, branch_doc, head),
    );
    for (name, item) in &branch.hypotheses {
        let item = crate::ordinary_value::map(item)?;
        let qualified = format!("{reference}:{name}");
        let document = item
            .get("doc")
            .or_else(|| item.get("document"))
            .cloned()
            .ok_or_else(|| error("invalid_target_hypothesis"))?;
        let head = item
            .get("head")
            .cloned()
            .unwrap_or_else(|| OV::Map(OMap::new()));
        let same = hypotheses
            .get(name)
            .and_then(|v| crate::ordinary_value::map(v).ok())
            .is_some_and(|mine| {
                mine.get("doc").or_else(|| mine.get("document")) == Some(&document)
                    && mine.get("head") == Some(&head)
            });
        if !same {
            hypotheses.insert(
                qualified.clone(),
                ordinary_hypothesis(&qualified, document, head),
            );
        }
    }
    let conflicts = OMap::new();
    let projection = crate::ordinary_views::Projection::new(
        current.ordinary_document(),
        &hypotheses,
        &conflicts,
        vec![],
        runtime,
    )?;
    Ok(C::Output {
        text: projection.pull(seeds, budget)?,
        code: 0,
    })
}

pub(super) fn pull(
    reference: &str,
    seeds: &[String],
    budget: i64,
    cwd: &Path,
    paths: &[PathBuf],
    current: &OrdinaryCapture,
    runtime: Option<&Runtime>,
) -> Result<C::Output> {
    require((1..=1000).contains(&budget), "invalid_pull_budget")?;
    let (entry, root, relative, oid, day) = context(paths, cwd, reference)?;
    let capabilities = crate::ordinary_fields::capabilities(current.ordinary_document(), None)?;
    if crate::ordinary_value::map(&capabilities)?
        .get("profile")
        .is_some_and(|value| crate::ordinary_value::string_is(value, "core/v1"))
    {
        return Err(error("unsupported_capability: use core/v1 consumer"));
    }
    if current.history_capture().is_some() {
        history_pull(
            reference, seeds, budget, &entry, &root, &relative, &oid, current,
        )
    } else {
        ordinary_pull(
            reference, seeds, budget, &root, &relative, &oid, &day, current, runtime,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn command(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn option_like_and_missing_refs_are_argv_data_and_never_commands() {
        let temp = tempfile::tempdir().unwrap();
        command(temp.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(temp.path().join("GROUNDING.yaml"), "known: {p.x: {v: 1}}\n").unwrap();
        command(temp.path(), &["add", "."]);
        command(
            temp.path(),
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=f@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "fixture",
            ],
        );
        let before = Command::new("git")
            .arg("-C")
            .arg(temp.path())
            .args(["status", "--porcelain=v1"])
            .output()
            .unwrap()
            .stdout;
        for reference in ["--help", "$(touch injected)", "missing ref"] {
            let error = resolve(temp.path(), reference).unwrap_err();
            assert_eq!(
                error.0,
                format!("refused - {reference} is not a commit this checkout knows")
            );
        }
        assert!(!temp.path().join("injected").exists());
        let after = Command::new("git")
            .arg("-C")
            .arg(temp.path())
            .args(["status", "--porcelain=v1"])
            .output()
            .unwrap()
            .stdout;
        assert_eq!(after, before);
    }
}

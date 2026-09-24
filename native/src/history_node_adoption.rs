//! Explicit branch overlap choices, bound to pinned source frontiers.
use crate::{
    Result, history_authoring as A,
    history_contract::*,
    history_node_branch_source::{self as S, Observation},
    history_node_capture::Capture,
    history_node_publication as P,
    history_view::list,
    require,
    value::TypedValue as V,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub struct Options {
    pub operation: String,
    pub recorded_at: String,
    pub by: String,
}
fn closure(snapshot: &P::Snapshot, frontier: &Map) -> Result<BTreeSet<String>> {
    require(
        !frontier.is_empty() && frontier.len() <= 16,
        "invalid_branch_frontier",
    )?;
    let mut todo = Vec::new();
    for (op, digest) in frontier {
        require(
            snapshot
                .transactions
                .get(op)
                .is_some_and(|tx| string_is(digest, &tx.digest)),
            "branch_source_changed",
        )?;
        todo.push(op.clone());
    }
    let mut result = BTreeSet::new();
    while let Some(op) = todo.pop() {
        if result.insert(op.clone()) {
            todo.extend(
                snapshot
                    .transactions
                    .get(&op)
                    .ok_or_else(|| error("incomplete_commit"))?
                    .parents
                    .iter()
                    .cloned(),
            );
            require(
                result.len() <= crate::history_node_codec::MAX_EVENTS,
                "node_transaction_limit",
            )?;
        }
    }
    Ok(result)
}
fn source_counts(
    target: &P::Snapshot,
    capture: &Capture,
    sources: &V,
) -> Result<BTreeMap<String, usize>> {
    let sources = list(sources)?;
    require(
        !sources.is_empty() && sources.len() <= 16,
        "invalid_branch_source_set",
    )?;
    let mut count = BTreeMap::new();
    let mut coverage = target.transactions.keys().cloned().collect::<BTreeSet<_>>();
    let mut unique = BTreeSet::new();
    for source in sources {
        let s = schema(
            source,
            &[
                "kind",
                "commit",
                "entry",
                "object_format",
                "association",
                "frontier",
                "bundle_sha256",
            ],
            &[],
        )?;
        require(
            string_is(&s["kind"], "node-git-observation/v1")
                && string_is(&s["association"], "local_git_capture"),
            "invalid_branch_source",
        )?;
        let commit = text(&s["commit"])?;
        require(
            crate::history_source_ancestry::oid(commit) && unique.insert(commit.to_owned()),
            "invalid_branch_source",
        )?;
        require(
            (string_is(&s["object_format"], "sha1") && commit.len() == 40)
                || (string_is(&s["object_format"], "sha256") && commit.len() == 64),
            "invalid_branch_source",
        )?;
        crate::history_branch::portable_path(text(&s["entry"])?)?;
        require(
            text(&s["bundle_sha256"])?.len() == 64
                && crate::history_paths::object_id(text(&s["bundle_sha256"])?),
            "invalid_branch_source",
        )?;
        let ops = closure(&capture.snapshot, map(&s["frontier"])?)?;
        for (subject, versions) in &capture.snapshot.versions {
            if versions.values().any(|v| ops.contains(v.operation())) {
                *count.entry(subject.clone()).or_insert(0) += 1;
            }
        }
        coverage.extend(ops);
    }
    require(
        coverage == capture.snapshot.transactions.keys().cloned().collect(),
        "branch_source_closure_mismatch",
    )?;
    Ok(count)
}
pub(crate) fn apply(
    target: &P::Snapshot,
    capture: &Capture,
    operation: &str,
    admission: &V,
) -> Result<(Capture, Vec<V>)> {
    let a = schema(admission, &["sources", "choices", "by", "recorded_at"], &[])?;
    require(
        !text(&a["by"])?.trim().is_empty() && !text(&a["recorded_at"])?.is_empty(),
        "invalid_adopter",
    )?;
    let count = source_counts(target, capture, &a["sources"])?;
    let choices = map(&a["choices"])?;
    for (subject, n) in &count {
        require(
            (*n == 1 && !target.versions.contains_key(subject)) || choices.contains_key(subject),
            "adoption_choice_required",
        )?;
    }
    let options = A::Options {
        operation: operation.into(),
        recorded_at: text(&a["recorded_at"])?.into(),
        recording_day: text(&a["recorded_at"])?.into(),
        by: a["by"].clone(),
        strict: false,
        paths: crate::history_paths::Scheme::Hashed,
        receipt_version: None,
    };
    let mut objects = Vec::new();
    for (subject, chosen) in choices {
        require(count.contains_key(subject), "invalid_adoption_choice")?;
        let claim = capture.object(subject, text(chosen)?)?;
        require_interpretable_claim(&claim)?;
        let st = map(&map(&map(capture.state())?["subjects"])?[subject])?;
        let over = list(&st["heads"])?
            .iter()
            .chain(list(&st["disputed_acts"])?)
            .filter(|id| *id != chosen)
            .map(|id| text(id).map(str::to_owned))
            .collect::<Result<BTreeSet<_>>>()?;
        let saw = capture
            .history
            .objects()
            .iter()
            .filter(|(_, v)| map(v).is_ok_and(|m| string_is(&m["subject"], subject)))
            .map(|(id, _)| id.clone());
        let act = A::make_object(
            subject,
            "act",
            A::obj([
                ("act", A::s("accept")),
                ("of", chosen.clone()),
                ("over", A::strings(over)),
                (
                    "because",
                    A::s(&format!(
                        "explicit branch adoption of {}",
                        a["sources"].digest()?
                    )),
                ),
            ]),
            A::strings(saw),
            None,
            A::empty(),
            A::empty(),
            &options,
        )?;
        objects.push(act);
    }
    let candidate = capture.candidate(&objects)?;
    for (subject, chosen) in choices {
        let st = map(&map(&map(candidate.state())?["subjects"])?[subject])?;
        require(
            string_is(&st["acceptance"], "accepted") && list(&st["heads"])?.contains(chosen),
            "unresolved_adoption_choice",
        )?;
    }
    Ok((candidate, objects))
}
pub fn prepare(
    root: &Path,
    sources: &[Observation],
    choices: &V,
    options: &Options,
) -> Result<P::Prepared> {
    let sources = S::ordered(sources)?;
    let admission = A::obj([
        (
            "sources",
            V::List(sources.iter().map(|s| s.binding().clone()).collect()),
        ),
        ("choices", choices.clone()),
        ("by", A::s(&options.by)),
        ("recorded_at", A::s(&options.recorded_at)),
    ]);
    let bundles = sources.iter().map(|s| s.bundle.clone()).collect::<Vec<_>>();
    crate::history_node_branch::prepare_adoption(root, &bundles, &options.operation, &admission)
}

pub fn preview(root: &Path, observations: &[Observation]) -> Result<V> {
    let sources = S::ordered(observations)?;
    let bundles = sources.iter().map(|s| s.bundle.clone()).collect::<Vec<_>>();
    let prepared = crate::history_node_branch::prepare(
        root,
        &bundles,
        &format!("branch-preview-{}", uuid::Uuid::new_v4().simple()),
    );
    let (before, after) = match prepared {
        Ok(prepared) => {
            let (before, after) = prepared.snapshots(root)?;
            (
                Capture::from_snapshot(before)?,
                Capture::from_snapshot(after)?,
            )
        }
        Err(e) if e.0 == "node_branch_no_new_history" => {
            let capture = Capture::read(root)?;
            (capture.clone(), capture)
        }
        Err(e) => return Err(e),
    };
    let mut count = BTreeMap::<String, usize>::new();
    for source in &sources {
        for subject in map(&map(source.capture()?.state())?["subjects"])?.keys() {
            *count.entry(subject.clone()).or_default() += 1;
        }
    }
    let mut subjects = Map::new();
    for (subject, n) in count {
        let target = map(&map(before.state())?["subjects"])?.get(&subject);
        let union = &map(&map(after.state())?["subjects"])?[&subject];
        subjects.insert(
            subject,
            A::obj([
                ("requires_choice", V::Bool(n > 1 || target.is_some())),
                (
                    "target_heads",
                    target
                        .map(map)
                        .transpose()?
                        .and_then(|s| s.get("heads"))
                        .cloned()
                        .unwrap_or(V::List(vec![])),
                ),
                ("union", union.clone()),
            ]),
        );
    }
    let bindings = V::List(sources.iter().map(|s| s.binding().clone()).collect());
    let mut result = Map::from([
        ("state".into(), A::s("prepared")),
        ("admission".into(), A::s("requires_explicit_choices")),
        ("sources".into(), bindings.clone()),
        ("subjects".into(), V::Map(subjects)),
    ]);
    result.insert(
        if sources.len() == 1 {
            "source_revision"
        } else {
            "source_set_revision"
        }
        .into(),
        A::s(&if sources.len() == 1 {
            sources[0].revision()?
        } else {
            bindings.digest()?
        }),
    );
    Ok(V::Map(result))
}

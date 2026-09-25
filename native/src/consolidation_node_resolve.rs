//! A real Git merge of one compact node-history record. Each side is the exact
//! record history pinned in its merge commit and must equal its staged view; the
//! resolution is their existing deterministic branch union, which selects no head
//! and records no knowledge act. The staged tree must already hold every history
//! file of that union: only the union manifest and the rendered view are new.
use super::{Item, blobs, git};
use crate::{
    Result,
    history_contract::{error, map, text},
    history_node_capture::Capture,
    history_node_publication as P, history_node_writer as W, history_transaction_fs as F,
    identity::sha256,
    require,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

const VIEW: &str = "GROUNDING.yaml";
const MARKER: &str = ".kpopper/history.yaml";
const HISTORY_DIRS: [&str; 5] = [
    ".kpopper/history-commits",
    ".kpopper/history",
    ".kpopper/hypotheses",
    ".kpopper-history-migration",
    "evidence",
];

pub(super) struct Resolution {
    pub(super) view: Vec<u8>,
    /// New immutable files, relative to the checkout root.
    pub(super) files: BTreeMap<String, Vec<u8>>,
    /// Every staged record-history file, relative to the checkout root.
    staged: BTreeMap<String, Vec<u8>>,
    prefix: String,
    transactions: BTreeMap<String, String>,
}

/// Paths a compact record owns beside its view. Other files are not history.
fn owned(path: &str) -> bool {
    let within = |dir: &str, suffix: &str| {
        path.strip_prefix(dir)
            .and_then(|rest| rest.strip_suffix(suffix))
            .is_some_and(|name| !name.is_empty() && !name.contains('/'))
    };
    path == VIEW
        || path == MARKER
        || within(".kpopper/history/", ".jsonl")
        || within(".kpopper/history-commits/", ".json")
        || P::evidence_path(path).is_ok()
}

/// The record's history files in one merge commit, keyed by record-relative path.
fn side(root: &Path, revision: &str, prefix: &str) -> Result<BTreeMap<String, Vec<u8>>> {
    let raw = git(
        root,
        &["ls-tree", "-r", "-z", "--full-tree", revision],
        vec![],
        16 * 1024 * 1024,
    )?;
    let mut items = vec![];
    for row in raw.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let row = std::str::from_utf8(row).map_err(|_| error("resolve_non_utf8_git_path"))?;
        let (info, path) = row
            .split_once('\t')
            .ok_or_else(|| error("resolve_invalid_tree"))?;
        let Some(relative) = path.strip_prefix(prefix) else {
            continue;
        };
        if !owned(relative) {
            continue;
        }
        let fields: Vec<_> = info.split(' ').collect();
        require(
            fields.len() == 3 && fields[1] == "blob" && fields[0] == "100644",
            &format!("resolve_history_file_mode: {path}; history files must be regular files"),
        )?;
        items.push(Item {
            mode: fields[0].into(),
            oid: fields[2].into(),
            stage: 0,
            path: relative.into(),
        });
    }
    let bytes = blobs(root, &items)?;
    Ok(items
        .into_iter()
        .map(|item| (item.path, bytes[&item.oid].clone()))
        .collect())
}

fn frames(raw: &[u8]) -> Vec<&[u8]> {
    let mut frames = raw.split_inclusive(|b| *b == b'\n').collect::<Vec<_>>();
    frames.sort();
    frames
}

/// Build and verify the union of both pinned sides in private temporary copies.
pub(super) fn prepare(
    root: &Path,
    relative: &str,
    head: &str,
    other: &str,
    items: &[Item],
    staged_blobs: &BTreeMap<String, Vec<u8>>,
    conflict: &[&Item],
) -> Result<Resolution> {
    let prefix = match relative.rsplit_once('/') {
        Some((dir, name)) => {
            require(name == VIEW, "node_history_entry_unsupported")?;
            format!("{dir}/")
        }
        None => {
            require(relative == VIEW, "node_history_entry_unsupported")?;
            String::new()
        }
    };
    let ours = side(root, head, &prefix)?;
    let theirs = side(root, other, &prefix)?;
    // The staged sides are the committed views exactly; lazy originals live there.
    for (files, stage) in [(&ours, conflict[1]), (&theirs, conflict[2])] {
        require(
            files.get(VIEW) == Some(&staged_blobs[&stage.oid]),
            "resolve_history_stage_mismatch: a staged side differs from its merge commit",
        )?;
    }
    let bundle = |files: &BTreeMap<String, Vec<u8>>, name: &str| {
        let bundle = P::Bundle::from_files(files.clone())?;
        // Reconstruction verifies the complete side, its view and any lazy originals,
        // and rejects a history file the side's own export would not contain.
        let copy = bundle.reconstruct().map_err(|e| {
            error(&format!(
                "resolve_history_side_invalid: {name}: {e}; a hand-edited or incomplete side needs an explicit decision"
            ))
        })?;
        Ok::<_, crate::Error>((bundle, copy))
    };
    let (_, target) = bundle(&ours, "HEAD")?;
    let (source, _) = bundle(&theirs, "MERGE_HEAD")?;
    let identity = serde_json::to_vec(&serde_json::json!({
        "kind": "consolidate-resolve/v1",
        "head": head,
        "merge_head": other,
        "entry": relative,
        "staged": items
            .iter()
            .filter(|i| i.path.starts_with(&prefix))
            .map(|i| [i.stage.to_string(), i.path.clone(), i.oid.clone()])
            .collect::<Vec<_>>(),
    }))?;
    // Deterministic, so an interrupted resolution rebuilds the same union bytes.
    let operation = format!("resolve-{}", &sha256(&identity)[..48]);
    let prepared = crate::history_node_branch::prepare(
        target.path(),
        std::slice::from_ref(&source),
        &operation,
    )
    .map_err(|e| error(&format!("resolve_history_union_refused: {e}")))?;
    let (_, after) = prepared.snapshots(target.path())?;
    let merged = Capture::from_snapshot(after.clone())?;
    for (id, state) in map(&map(merged.state())?["subjects"])? {
        require(
            text(&map(state)?["acceptance"])? != "contested",
            &format!("resolve_contested: {id}; history needs an explicit decision"),
        )?;
    }
    let joined = after
        .versions
        .iter()
        .filter(|(_, versions)| versions.values().any(|v| v.operation() == operation))
        .map(|(subject, _)| subject.as_str())
        .collect::<Vec<_>>();
    require(
        joined.is_empty(),
        &format!(
            "resolve_history_stream_merge: {}; both sides changed this history and the staged streams cannot hold the join",
            joined.join(", ")
        ),
    )?;
    W::publish(target.path(), &prepared, None, |_| Ok(()))?;
    let union = P::export(target.path())?.files()?;
    let manifest = format!(".kpopper/history-commits/{operation}.json");
    let expected = union
        .iter()
        .filter(|(p, _)| *p != VIEW && **p != manifest && owned(p))
        .map(|(p, raw)| (p.clone(), raw))
        .collect::<BTreeMap<_, _>>();
    let staged = items
        .iter()
        .filter(|i| i.stage == 0)
        .filter_map(|i| {
            i.path
                .strip_prefix(&prefix)
                .filter(|p| owned(p))
                .map(|p| (p.to_owned(), &staged_blobs[&i.oid]))
        })
        .collect::<BTreeMap<_, _>>();
    let differing = expected
        .keys()
        .chain(staged.keys())
        .filter(
            |p| match (expected.get(p.as_str()), staged.get(p.as_str())) {
                (Some(want), Some(have)) if p.starts_with(".kpopper/history/") => {
                    frames(want) != frames(have)
                }
                (Some(want), Some(have)) => want != have,
                _ => true,
            },
        )
        .map(|p| format!("{prefix}{p}"))
        .collect::<BTreeSet<_>>();
    require(
        differing.is_empty(),
        &format!(
            "resolve_history_staged_mismatch: {}; the staged history is not the union of both merge commits",
            differing.into_iter().collect::<Vec<_>>().join(", ")
        ),
    )?;
    Ok(Resolution {
        view: union[VIEW].clone(),
        files: BTreeMap::from([(format!("{prefix}{manifest}"), union[&manifest].clone())]),
        staged: staged
            .into_iter()
            .map(|(p, raw)| (format!("{prefix}{p}"), raw.clone()))
            .collect(),
        prefix,
        transactions: after
            .transactions
            .iter()
            .map(|(op, tx)| (op.clone(), tx.digest.clone()))
            .collect(),
    })
}

impl Resolution {
    /// The staged tree plus this resolution must read as exactly the prepared union.
    pub(super) fn verify_candidate(&self, scratch: &Path) -> Result<()> {
        let captured = Capture::read(&scratch.join(&self.prefix))?;
        require(
            captured
                .snapshot
                .transactions
                .iter()
                .map(|(op, tx)| (op.clone(), tx.digest.clone()))
                .collect::<BTreeMap<_, _>>()
                == self.transactions
                && captured.entry_bytes() == self.view.as_slice(),
            "resolve_history_projection_mismatch",
        )
    }

    /// Working history files still equal the staged ones. The only untracked
    /// history file allowed is this resolution's own manifest, left by an interruption.
    pub(super) fn verify_worktree(&self, root: &Path) -> Result<()> {
        for (path, raw) in &self.staged {
            require(
                F::read(&F::target(root, path)?)?.as_deref() == Some(raw.as_slice()),
                &format!("resolve_history_worktree_changed: {path} differs from the staged merge"),
            )?;
        }
        let mut args = vec!["ls-files", "--others", "-z", "--"];
        let dirs = HISTORY_DIRS
            .iter()
            .map(|d| format!("{}{d}", self.prefix))
            .collect::<Vec<_>>();
        args.extend(dirs.iter().map(String::as_str));
        let raw = git(root, &args, vec![], 16 * 1024 * 1024)?;
        for path in raw.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let path = std::str::from_utf8(path).map_err(|_| error("resolve_non_utf8_git_path"))?;
            let Some(relative) = path.strip_prefix(&self.prefix) else {
                continue;
            };
            if !owned(relative) {
                continue;
            }
            let ours = match self.files.get(path) {
                Some(raw) => F::read(&F::target(root, path)?)?.as_deref() == Some(raw.as_slice()),
                None => false,
            };
            require(
                ours,
                &format!("resolve_history_untracked: {path} is not part of the staged merge"),
            )?;
        }
        Ok(())
    }

    /// Add the union manifest before the view that names it. An identical manifest
    /// left by an interrupted resolution is kept; any other bytes refuse.
    pub(super) fn publish(&self, root: &Path) -> Result<()> {
        for (path, raw) in &self.files {
            match F::read(&F::target(root, path)?)? {
                Some(existing) => require(
                    existing == *raw,
                    &format!("resolve_history_untracked: {path} is not this resolution"),
                )?,
                None => F::publish_immutable(root, path, raw)?,
            }
        }
        Ok(())
    }

    pub(super) fn paths(&self) -> Vec<&str> {
        self.files.keys().map(String::as_str).collect()
    }
}

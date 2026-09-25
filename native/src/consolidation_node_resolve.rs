//! A real Git merge of one compact node-history record. Each side is the exact
//! record history pinned in its merge commit and must equal its staged view; the
//! resolution is their existing deterministic branch union, which selects no head
//! and records no knowledge act. The staged tree must already hold every history
//! file of that union: only the union manifest and the rendered view are new. The
//! manifest is staged before it is written, so an abandoned merge never orphans it.
use super::{Item, blobs, git, git_with, inventory, string};
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
    /// Generated files already staged with exactly their bytes by an earlier run.
    already_staged: BTreeSet<String>,
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
    // Only the pinned merge and this record's conflict stages name the operation:
    // unrelated staging, and the generated manifest itself, never change it.
    let identity = serde_json::to_vec(&serde_json::json!({
        "kind": "consolidate-resolve/v1",
        "head": head,
        "merge_head": other,
        "entry": relative,
        "stages": conflict
            .iter()
            .map(|i| [i.stage.to_string(), i.mode.clone(), i.oid.clone()])
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
    // Staging hashes these exact bytes; Git must see the written file identically,
    // or abort and reset could not recognize it. Checked before anything is written.
    let generated = format!("{prefix}{manifest}");
    let exact = string(git(
        root,
        &["hash-object", "--no-filters", "--stdin"],
        union[&manifest].clone(),
        1024,
    )?)?;
    let as_file = string(git(
        root,
        &["hash-object", &format!("--path={generated}"), "--stdin"],
        union[&manifest].clone(),
        1024,
    )?)?;
    require(
        exact == as_file,
        &format!(
            "resolve_history_manifest_conversion: {generated}; Git attributes would change its bytes"
        ),
    )?;
    // A rerun may find this exact manifest already staged; any other bytes are not ours.
    let mut already_staged = BTreeSet::new();
    for item in items.iter().filter(|i| i.path == format!("{prefix}{manifest}")) {
        require(
            item.stage == 0
                && item.mode == "100644"
                && staged_blobs[&item.oid] == union[&manifest],
            &format!(
                "resolve_history_staged_manifest: {} is staged with bytes this resolution did not generate",
                item.path
            ),
        )?;
        already_staged.insert(item.path.clone());
    }
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
                .filter(|p| owned(p) && **p != manifest)
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
        already_staged,
        prefix,
        transactions: after
            .transactions
            .iter()
            .map(|(op, tx)| (op.clone(), tx.digest.clone()))
            .collect(),
    })
}

type Entry = (String, u8, String, String);
fn entry(item: &Item) -> Entry {
    (item.path.clone(), item.stage, item.mode.clone(), item.oid.clone())
}

/// Update a private alternate index beside the real one, starting from `before`,
/// then atomically replace the real index. The caller holds the real `index.lock`
/// throughout; this never takes, releases or bypasses it. Split index mode is
/// written out as one complete index; Git may split it again on its next write.
fn replace_index(
    root: &Path,
    index: &Path,
    before: &[u8],
    expected: &BTreeSet<Entry>,
    update: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    let directory = index.parent().ok_or_else(|| error("resolve_missing_index"))?;
    let alternate = tempfile::Builder::new()
        .prefix(".kpopper-resolve-index-")
        .tempfile_in(directory)?;
    std::fs::write(alternate.path(), before)?;
    update(alternate.path())?;
    let listing = git_with(
        root,
        Some(alternate.path()),
        &["ls-files", "--stage", "-z"],
        vec![],
        16 * 1024 * 1024,
    )?;
    let actual = inventory(&listing)?.iter().map(entry).collect::<BTreeSet<_>>();
    require(
        actual == *expected,
        "resolve_index_update_mismatch: the prepared index differs beyond the generated manifest",
    )?;
    let prepared = F::read(alternate.path())?.ok_or_else(|| error("resolve_missing_index"))?;
    require(
        F::read(index)?.as_deref() == Some(before),
        "resolve_stale_merge: the merge changed during validation; retry",
    )?;
    // The established atomic replacement keeps the index's permissions and syncs
    // its directory portably. The caller still holds the real `index.lock`.
    F::replace(index, Some(&prepared))
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
        for (path, raw) in &self.files {
            require(
                F::read(&F::target(root, path)?)?.is_none_or(|existing| existing == *raw),
                &format!("resolve_history_worktree_changed: {path} is not this resolution"),
            )?;
        }
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

    /// Stage exactly the generated manifests under the caller's real `index.lock`,
    /// before any working file names them. Every other entry, stage and path stays
    /// as the user staged it. Returns the resulting index entries.
    pub(super) fn stage(
        &self,
        root: &Path,
        index: &Path,
        index_before: &[u8],
        items: &[Item],
    ) -> Result<BTreeSet<Entry>> {
        let mut expected = items.iter().map(entry).collect::<BTreeSet<_>>();
        let pending = self
            .files
            .iter()
            .filter(|(path, _)| !self.already_staged.contains(*path))
            .collect::<Vec<_>>();
        if pending.is_empty() {
            return Ok(expected);
        }
        let mut added = vec![];
        for (path, raw) in pending {
            // Exact bytes: no clean filters, attributes or hooks.
            let oid = string(git_with(
                root,
                None,
                &["hash-object", "-w", "--no-filters", "--stdin"],
                raw.clone(),
                1024,
            )?)?;
            crate::pending_state::verify_blob(&oid, raw)?;
            expected.insert((path.clone(), 0, "100644".into(), oid.clone()));
            added.push(format!("100644,{oid},{path}"));
        }
        replace_index(root, index, index_before, &expected, |alternate| {
            for info in &added {
                git_with(
                    root,
                    Some(alternate),
                    &["update-index", "--no-split-index", "--add", "--cacheinfo", info],
                    vec![],
                    1024 * 1024,
                )?;
            }
            Ok(())
        })?;
        Ok(expected)
    }

    /// After the manifest is written, record its file status so abort and reset can
    /// remove it. Only the generated entries are reread; no other entry, stage or
    /// unstaged edit is examined, and `expected` must remain exactly.
    pub(super) fn refresh(&self, root: &Path, index: &Path, expected: &BTreeSet<Entry>) -> Result<()> {
        let before = F::read(index)?.ok_or_else(|| error("resolve_missing_index"))?;
        let mut args = vec!["update-index", "--no-split-index", "--"];
        args.extend(self.files.keys().map(String::as_str));
        replace_index(root, index, &before, expected, |alternate| {
            git_with(root, Some(alternate), &args, vec![], 1024 * 1024).map(|_| ())
        })
    }

    pub(super) fn paths(&self) -> Vec<&str> {
        self.files.keys().map(String::as_str).collect()
    }
}

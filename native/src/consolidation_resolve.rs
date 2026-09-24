//! Resolve one conflicted record against the staged merge, without staging or committing it.
#[path = "consolidation_resolve_text.rs"]
mod text_merge;
use super::{CommandOutput, Options};
use crate::{
    Result,
    history_contract::{error, map, text},
    history_store::Store,
    history_transaction_fs as F, history_yaml as Y, require,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const MAX_BYTES: usize = 256 * 1024 * 1024;
const MAX_FILES: usize = 100_000;

fn git(root: &Path, args: &[&str], input: Vec<u8>, limit: usize) -> Result<Vec<u8>> {
    let mut cmd = Command::new("git");
    cmd.args([
        "--no-pager",
        "--no-replace-objects",
        "--no-lazy-fetch",
        "-C",
    ])
    .arg(root)
    .args(args)
    .env("GIT_TERMINAL_PROMPT", "0")
    .env("GIT_OPTIONAL_LOCKS", "0");
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        cmd.env_remove(name);
    }
    crate::reasoning_runtime::run_command_bounded(&mut cmd, input, Duration::from_secs(30), limit)
}
fn string(raw: Vec<u8>) -> Result<String> {
    let mut value = String::from_utf8(raw).map_err(|_| error("resolve_non_utf8_git_path"))?;
    require(value.ends_with('\n'), "resolve_invalid_git_output")?;
    value.pop();
    require(
        !value.contains(['\n', '\r']),
        "resolve_multiline_git_output",
    )?;
    Ok(value)
}
fn query(root: &Path, args: &[&str]) -> Result<String> {
    string(git(root, args, vec![], 1024 * 1024)?)
}
#[derive(Clone)]
struct Item {
    mode: String,
    oid: String,
    stage: u8,
    path: String,
}
fn inventory(raw: &[u8]) -> Result<Vec<Item>> {
    let mut items = vec![];
    for row in raw.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let row = std::str::from_utf8(row).map_err(|_| error("resolve_non_utf8_git_path"))?;
        let (info, path) = row
            .split_once('\t')
            .ok_or_else(|| error("resolve_invalid_index"))?;
        crate::history_branch::portable_path(path)?;
        let fields: Vec<_> = info.split(' ').collect();
        require(fields.len() == 3, "resolve_invalid_index")?;
        require(
            ["100644", "100755"].contains(&fields[0]),
            &format!(
                "resolve_unsupported_index_file: {path}; symbolic links and submodules require explicit review"
            ),
        )?;
        require(
            [40, 64].contains(&fields[1].len()) && fields[1].bytes().all(|b| b.is_ascii_hexdigit()),
            "resolve_invalid_index",
        )?;
        let stage = fields[2]
            .parse::<u8>()
            .map_err(|_| error("resolve_invalid_index"))?;
        require(stage <= 3, "resolve_invalid_index")?;
        items.push(Item {
            mode: fields[0].into(),
            oid: fields[1].into(),
            stage,
            path: path.into(),
        });
        require(items.len() <= MAX_FILES, "resolve_snapshot_limit")?;
    }
    Ok(items)
}
fn blobs(root: &Path, items: &[Item]) -> Result<BTreeMap<String, Vec<u8>>> {
    let ids: std::collections::BTreeSet<_> = items.iter().map(|i| i.oid.as_str()).collect();
    let input = (ids.iter().copied().collect::<Vec<_>>().join("\n") + "\n").into_bytes();
    let raw = git(root, &["cat-file", "--batch"], input, MAX_BYTES)?;
    let mut cursor = 0;
    let mut files = BTreeMap::new();
    for id in ids {
        let end = raw[cursor..]
            .iter()
            .position(|b| *b == b'\n')
            .ok_or_else(|| error("resolve_invalid_blob"))?
            + cursor;
        let line =
            std::str::from_utf8(&raw[cursor..end]).map_err(|_| error("resolve_invalid_blob"))?;
        let parts: Vec<_> = line.split(' ').collect();
        require(
            parts.len() == 3 && parts[0] == id && parts[1] == "blob",
            "resolve_invalid_blob",
        )?;
        let size: usize = parts[2]
            .parse()
            .map_err(|_| error("resolve_invalid_blob"))?;
        cursor = end + 1;
        require(
            size <= MAX_BYTES && cursor.checked_add(size).is_some_and(|e| e < raw.len()),
            "resolve_snapshot_limit",
        )?;
        let bytes = &raw[cursor..cursor + size];
        crate::pending_state::verify_blob(id, bytes)?;
        files.insert(id.into(), bytes.to_vec());
        cursor += size;
        require(raw[cursor] == b'\n', "resolve_invalid_blob")?;
        cursor += 1;
    }
    require(cursor == raw.len(), "resolve_invalid_blob")?;
    Ok(files)
}

struct IndexLock(PathBuf);
impl Drop for IndexLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn unchanged(
    root: &Path,
    index: &Path,
    index_before: &[u8],
    entry: &Path,
    entry_before: &[u8],
    head: &str,
    other: &str,
) -> Result<()> {
    require(
        F::read(index)?.as_deref() == Some(index_before)
            && F::read(entry)?.as_deref() == Some(entry_before)
            && query(root, &["rev-parse", "HEAD"])? == head
            && query(root, &["rev-parse", "--verify", "MERGE_HEAD"])? == other,
        "resolve_stale_merge: the merge changed during validation; retry",
    )
}

pub(super) fn run(options: &Options, cwd: &Path) -> Result<CommandOutput> {
    require(
        options.names.is_empty()
            && options.from_refs.is_empty()
            && options.refute.is_none()
            && options.take.is_empty()
            && options.drops.is_empty()
            && options.choices.is_empty()
            && options.by.is_none()
            && options.source_revision.is_none()
            && options.as_of.is_none()
            && options.source.is_none()
            && options.why.is_none(),
        "--resolve accepts only an optional record path and --dry-run; it never chooses a competing claim",
    )?;
    require(
        std::env::var_os("GIT_INDEX_FILE").is_none(),
        "resolve_alternate_index_unsupported",
    )?;
    let root = PathBuf::from(query(cwd, &["rev-parse", "--show-toplevel"])?).canonicalize()?;
    let entry = options
        .record
        .as_ref()
        .map(|p| cwd.join(p))
        .map(Ok)
        .unwrap_or_else(|| crate::public_workspace::records(cwd).map(|p| p[0].clone()))?;
    // target rejects symlinks, traversal and paths outside the checkout.
    let relative = entry
        .strip_prefix(&root)
        .map_err(|_| error("resolve_external_record: choose the conflicted in-tree record"))?
        .to_str()
        .ok_or_else(|| error("resolve_non_utf8_git_path"))?
        .replace('\\', "/");
    crate::history_branch::portable_path(&relative)?;
    let entry = F::target(&root, &relative)?;
    let head = query(&root, &["rev-parse", "HEAD"])?;
    let other=query(&root,&["rev-parse","--verify","MERGE_HEAD"])
        .map_err(|_|error("resolve_no_merge: start a normal Git merge first; rebase and cherry-pick are not supported"))?;
    require(!other.contains('\n'), "resolve_octopus_unsupported")?;
    let index = PathBuf::from(query(
        &root,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    )?);
    let index_before = F::read(&index)?.ok_or_else(|| error("resolve_missing_index"))?;
    let entry_before = F::read(&entry)?.ok_or_else(|| error("resolve_missing_record"))?;
    require(
        entry_before.len() <= Y::MAX_DOCUMENT_BYTES,
        "resolve_record_limit",
    )?;
    let listing = git(
        &root,
        &["ls-files", "--stage", "-z"],
        vec![],
        16 * 1024 * 1024,
    )?;
    let items = inventory(&listing)?;
    let conflicts: Vec<_> = items.iter().filter(|i| i.stage != 0).collect();
    require(
        !conflicts.is_empty(),
        "resolve_no_conflict: no unresolved Git paths",
    )?;
    let others: std::collections::BTreeSet<_> = conflicts
        .iter()
        .filter(|i| i.path != relative)
        .map(|i| i.path.as_str())
        .collect();
    require(
        others.is_empty(),
        &format!(
            "resolve_other_conflicts: resolve and stage these paths first: {}",
            others.into_iter().collect::<Vec<_>>().join(", ")
        ),
    )?;
    require(
        conflicts.len() == 3 && conflicts.iter().map(|i| i.stage).collect::<Vec<_>>() == [1, 2, 3],
        "resolve_missing_merge_side: add/add and delete/edit of the record require explicit review",
    )?;
    require(
        conflicts.iter().all(|i| i.mode == conflicts[0].mode),
        "resolve_record_mode_conflict",
    )?;
    // ort retains the original merge output. Do not overwrite a user's partial resolution.
    let original = git(
        &root,
        &["show", &format!("AUTO_MERGE:{relative}")],
        vec![],
        Y::MAX_DOCUMENT_BYTES,
    )
    .map_err(|_| {
        error("resolve_original_merge_unavailable: this command requires Git's AUTO_MERGE snapshot")
    })?;
    require(
        original == entry_before,
        "resolve_record_edited: record differs from Git's original conflict; preserve or finish your manual resolution",
    )?;
    let files = blobs(&root, &items)?;
    let materialized_bytes =
        items
            .iter()
            .filter(|i| i.stage == 0)
            .try_fold(0usize, |total, item| {
                let size = total
                    .checked_add(files[&item.oid].len())
                    .ok_or_else(|| error("resolve_snapshot_limit"))?;
                require(size <= MAX_BYTES, "resolve_snapshot_limit")?;
                Ok::<_, crate::Error>(size)
            })?;
    require(materialized_bytes <= MAX_BYTES, "resolve_snapshot_limit")?;
    let sides: Vec<_> = conflicts.iter().map(|i| files[&i.oid].as_slice()).collect();
    let temp = tempfile::tempdir()?;
    let scratch = temp.path().canonicalize()?;
    for item in items.iter().filter(|i| i.stage == 0) {
        let path = F::target(&scratch, &item.path)?;
        fs::create_dir_all(path.parent().unwrap())?;
        require(!path.exists(), "resolve_snapshot_path_collision")?;
        fs::write(path, &files[&item.oid])?;
    }
    let scratch_entry = scratch.join(&relative);
    fs::create_dir_all(scratch_entry.parent().unwrap())?;
    let candidate = match crate::legacy_authoring::authority_route(&scratch_entry)? {
        crate::legacy_authoring::AuthorityRoute::Legacy => {
            text_merge::merge(sides[0], sides[1], sides[2])?
        }
        crate::legacy_authoring::AuthorityRoute::History => {
            let store = Store::new(&scratch_entry)?;
            let mut rebuilt = None;
            for side in [sides[1], sides[2]] {
                fs::write(&scratch_entry, side)?;
                let capture = store.capture()?;
                require(
                    Store::known_view(&capture)?,
                    "resolve_history_hand_edit: reconcile authored changes explicitly",
                )?;
                for (id, state) in map(&map(&capture.state)?["subjects"])? {
                    let acceptance = text(&map(state)?["acceptance"])?;
                    require(
                        acceptance != "contested",
                        &format!("resolve_contested: {id}; history needs an explicit decision"),
                    )?;
                }
                let raw = store.rebuild(Some(&capture), false, false, &[])?;
                require(
                    rebuilt.as_ref().is_none_or(|old| old == &raw),
                    "resolve_history_projection_mismatch",
                )?;
                rebuilt = Some(raw);
            }
            rebuilt.unwrap()
        }
    };
    fs::write(&scratch_entry, &candidate)?;
    let options_read = crate::public_readers::Options {
        subjects: vec![relative.clone()],
        ..Default::default()
    };
    let checked = crate::public_readers::run_auto(
        "check",
        &options_read,
        &scratch,
        crate::source_capture::ReadMode::Frozen,
        crate::public_readers::Reply::Text,
    )?;
    require(
        checked.code == 0,
        &format!(
            "resolve_check_failed: candidate left unapplied\n{}",
            checked.text
        ),
    )?;
    let record_report = checked.text;
    let checked = super::dispatch(
        &Options {
            dry_run: true,
            frozen: true,
            record: Some(relative.clone().into()),
            ..Default::default()
        },
        &scratch,
    );
    require(
        checked.code == 0,
        &format!(
            "resolve_consolidation_failed: candidate left unapplied\n{}{}",
            checked.stdout, checked.stderr
        ),
    )?;
    let _guard = F::DirectoryGuard::acquire(entry.parent().unwrap(), true)?;
    require(
        F::target(&root, &relative)? == entry,
        "resolve_record_location_changed",
    )?;
    unchanged(
        &root,
        &index,
        &index_before,
        &entry,
        &entry_before,
        &head,
        &other,
    )?;
    let verb = if options.dry_run {
        "preview"
    } else {
        "resolved"
    };
    if !options.dry_run {
        let lock = PathBuf::from(format!("{}.lock", index.display()));
        let handle = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .map_err(|_| error("resolve_index_locked: another Git operation is active"))?;
        let _lock = IndexLock(lock);
        unchanged(
            &root,
            &index,
            &index_before,
            &entry,
            &entry_before,
            &head,
            &other,
        )?;
        F::replace(&entry, Some(&candidate))?;
        drop(handle);
    }
    Ok(CommandOutput {
        stdout: format!(
            "{verb}: {relative}\nCandidate record check:\n{record_report}\n{}\nThe candidate and its hypotheses passed against the staged tree. No recipes or code tests were run.\n{}\n",
            checked.stdout,
            if options.dry_run {
                "No files or Git index entries changed.".into()
            } else {
                format!(
                    "Review the resolved record, then git add -- {relative:?} and finish your Git merge. The index, commits and remote were not changed."
                )
            }
        ),
        stderr: String::new(),
        code: 0,
    })
}

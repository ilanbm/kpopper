//! Where a record lives when this branch has none: the default branch, or a sibling
//! worktree of the same repository. The probe only asks Git which files a ref holds and
//! looks for entry files on disk; it never reads another record, and it enumerates
//! nothing outside Git. Any failure along the way leaves the line out.
use crate::project_modes::Project;
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const ENTRIES: [&str; 2] = ["GROUNDING.yaml", "PROVENANCE.yaml"];

/// One line naming where a record exists for a workspace that has none, with how to
/// bring it in. A branch read needs a record of its own here, so the way in is a merge. None outside Git, for a configured or registered record, or when no
/// other branch or worktree holds one.
pub fn line(cwd: &Path) -> Option<String> {
    let project = Project::open(cwd).ok()?;
    let common = project.common.as_ref()?;
    if project.config_path.exists() || common.join("kpopper-record").symlink_metadata().is_ok() {
        return None;
    }
    let root = &project.root;
    if ENTRIES
        .iter()
        .any(|name| root.join(name).symlink_metadata().is_ok())
    {
        return None;
    }
    let current = text(root, &["symbolic-ref", "-q", "--short", "HEAD"]);
    if let Some(default) = default_branch(root)
        && current.as_deref() != Some(default.as_str())
        && committed(root, &default)
        && let Some(commit) = text(
            root,
            &["rev-parse", "--short", &format!("{default}^{{commit}}")],
        )
    {
        return Some(format!(
            "No record on this branch; one exists on {default} at {commit}. Bring it in with `git merge {default}`, or check out that branch, rather than starting a second record here."
        ));
    }
    let own = root.canonicalize().ok()?;
    for (path, branch) in worktrees(root) {
        if path.canonicalize().ok().as_ref() == Some(&own)
            || !ENTRIES.iter().any(|name| path.join(name).is_file())
        {
            continue;
        }
        let place = path.display();
        return Some(match branch {
            Some(branch) if committed(root, &branch) => format!(
                "No record on this branch; one exists on {branch} in worktree {place}. Bring it in with `git merge {branch}`, or work in that worktree, rather than starting a second record here."
            ),
            Some(branch) => format!(
                "No record on this branch; one exists uncommitted in worktree {place} (branch {branch}). Commit it there and bring it in with `git merge {branch}`, or work in that worktree, rather than starting a second record here."
            ),
            None => format!(
                "No record on this branch; one exists in worktree {place} (detached HEAD). Work in that worktree rather than starting a second record here."
            ),
        });
    }
    None
}

/// The remote's default branch when one is known, else a local main or master.
fn default_branch(root: &Path) -> Option<String> {
    if let Some(remote) = text(
        root,
        &["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"],
    ) {
        return Some(remote);
    }
    ["main", "master"]
        .into_iter()
        .map(str::to_owned)
        .find(|name| {
            text(
                root,
                &[
                    "rev-parse",
                    "--verify",
                    "-q",
                    &format!("refs/heads/{name}^{{commit}}"),
                ],
            )
            .is_some()
        })
}

/// Whether the ref's tree holds a record entry at the repository's root.
fn committed(root: &Path, reference: &str) -> bool {
    let mut args = vec!["ls-tree", "--name-only", reference, "--"];
    args.extend(ENTRIES);
    text(root, &args).is_some()
}

/// Every worktree of the repository with its branch, None when detached.
fn worktrees(root: &Path) -> Vec<(PathBuf, Option<String>)> {
    let Some(raw) = git(root, &["worktree", "list", "--porcelain", "-z"]) else {
        return vec![];
    };
    let mut found = vec![];
    let mut path = None;
    let mut branch = None;
    for field in raw.split(|b| *b == 0) {
        let field = String::from_utf8_lossy(field);
        if let Some(rest) = field.strip_prefix("worktree ") {
            path = Some(PathBuf::from(rest));
            branch = None;
        } else if let Some(rest) = field.strip_prefix("branch ") {
            branch = Some(rest.strip_prefix("refs/heads/").unwrap_or(rest).to_owned());
        } else if field.is_empty()
            && let Some(path) = path.take()
        {
            found.push((path, branch.take()));
        }
    }
    if let Some(path) = path {
        found.push((path, branch));
    }
    found
}

/// A Git answer as trimmed text, None on failure or when empty.
fn text(root: &Path, args: &[&str]) -> Option<String> {
    let raw = git(root, args)?;
    let text = String::from_utf8(raw).ok()?.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// A read-only Git query for this repository alone: no inherited repository variables,
/// no prompts, no optional locks and no fetches.
fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let mut command = Command::new("git");
    command
        .args([
            "--no-pager",
            "--no-replace-objects",
            "--no-lazy-fetch",
            "-C",
        ])
        .arg(root)
        .args(args);
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(name);
    }
    for (name, value) in [
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("GIT_NO_LAZY_FETCH", "1"),
        ("GIT_NO_REPLACE_OBJECTS", "1"),
    ] {
        command.env(name, value);
    }
    crate::reasoning_runtime::run_command_bounded(
        &mut command,
        Vec::new(),
        Duration::from_secs(10),
        1024 * 1024,
    )
    .ok()
}

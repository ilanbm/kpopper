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

/// A branch named both for the probe and for the person: the full ref Git is asked about,
/// and the short name shown.
struct Branch {
    full: String,
    short: String,
}

/// One line naming where a record exists for a workspace that has none, with how to
/// bring it in. The way in is a merge: reading another branch needs a record here first.
/// None outside Git, for a registered record or one configured outside the branch, or
/// when no other branch or worktree holds one.
pub fn line(cwd: &Path) -> Option<String> {
    let project = Project::open(cwd).ok()?;
    let common = project.common.as_ref()?;
    if common.join("kpopper-record").symlink_metadata().is_ok() {
        return None;
    }
    // A configured record is looked for only where it lives on the branch: the entry
    // file at the root. One kept elsewhere is shared, and no branch can hold it.
    let names: Vec<&str> = if project.config_path.exists() {
        let record = project.record(None).ok()?;
        vec![
            *ENTRIES
                .iter()
                .find(|name| project.root.join(name) == record)?,
        ]
    } else {
        ENTRIES.to_vec()
    };
    let root = &project.root;
    if names
        .iter()
        .any(|name| root.join(name).symlink_metadata().is_ok())
    {
        return None;
    }
    let current = text(root, &["symbolic-ref", "-q", "HEAD"]);
    for default in default_branches(root) {
        if current.as_deref() != Some(default.full.as_str())
            && committed(root, &default.full, &names)
            && let Some(commit) = text(
                root,
                &[
                    "rev-parse",
                    "--short",
                    &format!("{}^{{commit}}", default.full),
                ],
            )
        {
            return Some(format!(
                "No record on this branch; one exists on {} at {commit}. Bring it in with `git merge {}`, or check out that branch, rather than starting a second record here.",
                default.short,
                merge_argument(root, &default)
            ));
        }
    }
    let own = root.canonicalize().ok()?;
    for (path, branch) in worktrees(root) {
        if path.canonicalize().ok().as_ref() == Some(&own)
            || !names.iter().any(|name| path.join(name).is_file())
        {
            continue;
        }
        let place = path.display();
        return Some(match branch {
            Some(branch) if committed(root, &branch.full, &names) => format!(
                "No record on this branch; one exists on {} in worktree {place}. Bring it in with `git merge {}`, or work in that worktree, rather than starting a second record here.",
                branch.short,
                merge_argument(root, &branch)
            ),
            Some(branch) => format!(
                "No record on this branch; one exists uncommitted in worktree {place} (branch {}). Commit it there and bring it in with `git merge {}`, or work in that worktree, rather than starting a second record here.",
                branch.short,
                merge_argument(root, &branch)
            ),
            None => format!(
                "No record on this branch; one exists in worktree {place} (detached HEAD). Work in that worktree rather than starting a second record here."
            ),
        });
    }
    None
}

/// The default branch, local first: the branch the remote's HEAD names, then the remote
/// branch itself, or a local main or master when the remote names none.
fn default_branches(root: &Path) -> Vec<Branch> {
    let local = |name: &str| {
        let full = format!("refs/heads/{name}");
        text(
            root,
            &["rev-parse", "--verify", "-q", &format!("{full}^{{commit}}")],
        )
        .map(|_| Branch {
            full,
            short: name.to_owned(),
        })
    };
    if let Some(full) = text(root, &["symbolic-ref", "-q", "refs/remotes/origin/HEAD"])
        && let Some(short) = full.strip_prefix("refs/remotes/")
    {
        let remote = Branch {
            short: short.to_owned(),
            full: full.clone(),
        };
        let name = short.strip_prefix("origin/").unwrap_or(short);
        return local(name).into_iter().chain([remote]).collect();
    }
    ["main", "master"]
        .into_iter()
        .find_map(local)
        .into_iter()
        .collect()
}

/// Whether the ref's tree holds a record entry at the repository's root as a regular
/// file. Only the tree's listing is read, never the file.
fn committed(root: &Path, reference: &str, names: &[&str]) -> bool {
    let mut args = vec!["ls-tree", "-z", reference, "--"];
    args.extend(names);
    git(root, &args).is_some_and(|raw| {
        raw.split(|b| *b == 0).any(|entry| {
            let entry = String::from_utf8_lossy(entry);
            let mut fields = entry.split_whitespace();
            matches!(fields.next(), Some("100644" | "100755")) && fields.next() == Some("blob")
        })
    })
}

/// The branch as an argument to paste into a shell: the short name when Git reads it as
/// this branch, the full ref when a tag shares the name, quoted unless every character is
/// plain.
fn merge_argument(root: &Path, branch: &Branch) -> String {
    let ambiguous = text(
        root,
        &[
            "rev-parse",
            "--verify",
            "-q",
            &format!("refs/tags/{}", branch.short),
        ],
    )
    .is_some();
    let name = if ambiguous {
        &branch.full
    } else {
        &branch.short
    };
    if name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "._/-+@=,:".contains(c))
    {
        name.clone()
    } else {
        format!("'{}'", name.replace('\'', "'\\''"))
    }
}

/// Every worktree of the repository with its branch, None when detached.
fn worktrees(root: &Path) -> Vec<(PathBuf, Option<Branch>)> {
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
            branch = Some(Branch {
                full: rest.to_owned(),
                short: rest.strip_prefix("refs/heads/").unwrap_or(rest).to_owned(),
            });
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

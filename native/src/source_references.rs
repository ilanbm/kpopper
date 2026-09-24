//! Advisory checks of file locators, separate from the truth of their claims.
use crate::{
    Result,
    ordinary_value::{Value, map, text},
    project_modes::Project,
    source_inventory::{Inventory, absolute},
};
use std::{collections::BTreeSet, path::Path};

fn is_line_suffix(suffix: &str) -> bool {
    let (first, last) = suffix.split_once('-').unwrap_or((suffix, ""));
    !first.is_empty()
        && first.bytes().all(|byte| byte.is_ascii_digit())
        && (last.is_empty() || last.bytes().all(|byte| byte.is_ascii_digit()))
}

fn strip_line_suffix(locator: &str) -> &str {
    let mut path = locator;
    loop {
        let Some((prefix, suffix)) = path.rsplit_once(':') else {
            return path;
        };
        if is_line_suffix(suffix) {
            path = prefix;
        } else {
            return path;
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PinnedFileStatus {
    Available,
    Missing,
    RevisionUnavailable,
    Unresolved,
    Unavailable,
}

fn pinned_file_status(root: &Path, revision: &str, path: &str) -> PinnedFileStatus {
    if revision.is_empty() || path.is_empty() || revision.contains('\0') || path.contains('\0') {
        return PinnedFileStatus::Unresolved;
    }
    let reference = format!("{revision}^{{commit}}");
    let commit = match crate::pending_state::git(
        root,
        &["rev-parse", "--verify", "--end-of-options", &reference],
        1024,
        true,
    ) {
        Ok(Some(commit)) => commit,
        Ok(None) => return PinnedFileStatus::Unresolved,
        Err(_) => return PinnedFileStatus::RevisionUnavailable,
    };
    let commit = String::from_utf8_lossy(&commit);
    let commit = commit.trim();
    if ![40, 64].contains(&commit.len()) || !commit.bytes().all(|c| c.is_ascii_hexdigit()) {
        return PinnedFileStatus::Unresolved;
    }
    // The resolved object ID keeps authored option-like revisions out of cat-file's arguments.
    let object = format!("{commit}:{path}");
    match crate::pending_state::git(root, &["cat-file", "-t", &object], 1024, true) {
        Ok(Some(kind)) if kind == b"blob\n" => PinnedFileStatus::Available,
        Ok(_) => PinnedFileStatus::Missing,
        Err(_) => PinnedFileStatus::Unavailable,
    }
}

pub(crate) fn notes(
    document: &Value,
    workspace: &Path,
    record_root: &Path,
    inventory: &mut Inventory,
) -> Result<Vec<String>> {
    let project = Project::open(workspace)?;
    if !project.is_git() {
        return Ok(vec![]);
    }
    let collections = crate::ordinary_fields::collections(document)?;
    let ids = collections
        .values()
        .flat_map(|members| members.keys())
        .collect::<BTreeSet<_>>();
    let mut notes = vec![];
    for (id, body) in collections.values().flat_map(|members| members.iter()) {
        let Ok(fields) = map(body) else {
            continue;
        };
        for field in ["file", "from"] {
            let Some(raw) = fields.get(field).and_then(|v| text(v).ok()) else {
                continue;
            };
            let locator = raw.trim();
            let raw = locator;
            // from can be an entry ID or prose. Slashes, pins and common file extensions
            // identify path tokens; file makes an unfamiliar root filename explicit.
            let file_extension = Path::new(raw)
                .extension()
                .and_then(|v| v.to_str())
                .is_some_and(|v| {
                    [
                        "md", "rst", "txt", "toml", "json", "yaml", "yml", "py", "rs", "js", "ts",
                        "tsx", "jsx", "sh", "ps1", "lean", "c", "h", "cpp", "hpp", "html", "css",
                        "csv", "tsv", "pdf", "docx", "xlsx", "xml", "ini", "cfg", "sql",
                    ]
                    .contains(&v.to_ascii_lowercase().as_str())
                });
            if raw.is_empty()
                || raw.chars().any(char::is_control)
                || raw.contains("://")
                || raw.starts_with("mailto:")
                || (raw.starts_with("\\\\") && !Path::new(raw).starts_with(&project.root))
                || raw.contains(['*', '?', '[', '<', '>'])
                || (field == "from"
                    && (ids.contains(&raw.to_owned())
                        || (!raw.contains('/') && !raw.contains(':') && !file_extension)
                        || raw.chars().any(char::is_whitespace)))
            {
                continue;
            }
            // A Windows absolute path is external when read on another platform too.
            if raw.as_bytes().get(1) == Some(&b':')
                && raw.as_bytes()[0].is_ascii_alphabetic()
                && (raw.as_bytes().get(2) == Some(&b'\\') || raw.as_bytes().get(2) == Some(&b'/'))
                && !cfg!(windows)
            {
                continue;
            }
            let path = Path::new(locator);
            // Match the hub's existing file links: relative to the primary record,
            // while Git's revision:path locator stays relative to the repository.
            let path = absolute(&record_root.join(path))?;
            let resolved = crate::project_modes::resolved(&path)?;
            if !resolved.starts_with(&project.root) {
                continue;
            }
            if inventory.exists(&path)? {
                continue;
            }
            if let Some((revision, pinned_path)) = locator.split_once(':')
                && !revision.is_empty()
                && !pinned_path.contains(':')
                && is_line_suffix(pinned_path)
            {
                match pinned_file_status(&project.root, revision, pinned_path) {
                    PinnedFileStatus::Available => continue,
                    PinnedFileStatus::Missing => {
                        notes.push(format!("{id}: pinned file {locator} is unavailable in this repository; verify the revision and path with git show {locator}, or re-read the source"));
                        continue;
                    }
                    PinnedFileStatus::RevisionUnavailable => {
                        notes.push(format!(
                            "{id}: could not check locator {locator} because Git's revision probe was unavailable; retry the check or re-read the source"
                        ));
                        continue;
                    }
                    PinnedFileStatus::Unavailable => {
                        notes.push(format!(
                            "{id}: could not check pinned file {locator} because Git's object probe was unavailable; retry the check or re-read the source"
                        ));
                        continue;
                    }
                    PinnedFileStatus::Unresolved => {}
                }
            }
            let raw = strip_line_suffix(locator);
            if raw != locator {
                let path = Path::new(raw);
                let path = absolute(&record_root.join(path))?;
                let resolved = crate::project_modes::resolved(&path)?;
                if !resolved.starts_with(&project.root) {
                    continue;
                }
                if inventory.exists(&path)? {
                    continue;
                }
            }
            if !Path::new(raw).is_absolute()
                && let Some((revision, pinned_path)) = raw.split_once(':')
            {
                // Existing local paths were handled above. Any revision that resolves is a pin,
                // including refs with slashes; an unresolved slash-free revision with a
                // repository path suffix remains an advisory pin candidate.
                let pin = (!revision.is_empty() && !pinned_path.is_empty())
                    .then(|| pinned_file_status(&project.root, revision, pinned_path));
                if pin == Some(PinnedFileStatus::RevisionUnavailable) {
                    notes.push(format!(
                        "{id}: could not check locator {locator} because Git's revision probe was unavailable; retry the check or re-read the source"
                    ));
                    continue;
                }
                if pin == Some(PinnedFileStatus::Unavailable) {
                    notes.push(format!(
                        "{id}: could not check pinned file {locator} because Git's object probe was unavailable; retry the check or re-read the source"
                    ));
                    continue;
                }
                let explicit_pin = matches!(
                    pin,
                    Some(
                        PinnedFileStatus::Available | PinnedFileStatus::Missing
                    )
                )
                    || (!revision.is_empty()
                        && !revision.contains(['/', '\\'])
                        && !pinned_path.is_empty()
                        && pinned_path.contains('/'));
                if explicit_pin {
                    if pin != Some(PinnedFileStatus::Available) {
                        notes.push(format!("{id}: pinned file {locator} is unavailable in this repository; verify the revision and path with git show {raw}, or re-read the source"));
                    }
                    continue;
                }
            }
            notes.push(format!("{id}: file {locator} is absent from this repository; re-read the surviving source, or pin the historical file as revision:path (open it with git show revision:path)"));
        }
    }
    Ok(notes)
}

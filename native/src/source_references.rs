//! Advisory checks of file locators, separate from the truth of their claims.
use crate::{
    Result,
    ordinary_value::{Value, map, text},
    project_modes::Project,
    source_inventory::{Inventory, absolute},
};
use std::{collections::BTreeSet, path::Path};

fn pinned_file_exists(root: &Path, revision: &str, path: &str) -> Result<bool> {
    if revision.is_empty() || path.is_empty() || revision.contains('\0') || path.contains('\0') {
        return Ok(false);
    }
    let reference = format!("{revision}^{{commit}}");
    let Some(commit) = crate::pending_state::git(
        root,
        &["rev-parse", "--verify", "--end-of-options", &reference],
        1024,
        true,
    )?
    else {
        return Ok(false);
    };
    let commit = String::from_utf8_lossy(&commit);
    let commit = commit.trim();
    if ![40, 64].contains(&commit.len()) || !commit.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Ok(false);
    }
    // The resolved object ID keeps authored option-like revisions out of cat-file's arguments.
    let object = format!("{commit}:{path}");
    Ok(
        crate::pending_state::git(root, &["cat-file", "-t", &object], 1024, true)?
            .is_some_and(|kind| kind == b"blob\n"),
    )
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
            let raw = raw.trim();
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
            let path = Path::new(raw);
            if !path.is_absolute()
                && let Some((revision, path)) = raw.split_once(':')
            {
                if !pinned_file_exists(&project.root, revision, path)? {
                    notes.push(format!("{id}: pinned file {raw} is unavailable in this repository; verify the revision and path with git show {raw}, or re-read the source"));
                }
                continue;
            }
            // Match the hub's existing file links: relative to the primary record,
            // while Git's revision:path locator stays relative to the repository.
            let path = absolute(&record_root.join(path))?;
            let resolved = crate::project_modes::resolved(&path)?;
            if !resolved.starts_with(&project.root) {
                continue;
            }
            if !inventory.exists(&path)? {
                notes.push(format!("{id}: file {raw} is absent from this repository; re-read the surviving source, or pin the historical file as revision:path (open it with git show revision:path)"));
            }
        }
    }
    Ok(notes)
}

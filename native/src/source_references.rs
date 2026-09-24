//! Advisory checks of file locators, separate from the truth of their claims.
use crate::{
    Result,
    ordinary_value::{Value, map, text},
    project_modes::Project,
    source_inventory::{Inventory, absolute},
};
use std::{collections::BTreeSet, path::Path};

const PIN_PROBE_OUTPUT_LIMIT: usize = 8 * 1024;

fn is_line_suffix(suffix: &str) -> bool {
    let (first, last) = suffix.split_once('-').unwrap_or((suffix, ""));
    !first.is_empty()
        && first.bytes().all(|byte| byte.is_ascii_digit())
        && (last.is_empty() || last.bytes().all(|byte| byte.is_ascii_digit()))
}

fn locator_candidates(locator: &str) -> Vec<&str> {
    let mut candidates = vec![locator];
    while candidates.len() < 9 {
        let path = *candidates.last().expect("full locator is first");
        let Some((prefix, suffix)) = path.rsplit_once(':') else {
            break;
        };
        if !is_line_suffix(suffix) {
            break;
        }
        candidates.push(prefix);
    }
    candidates
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PinnedFileStatus {
    Available,
    Missing,
    RevisionUnavailable,
    Unresolved,
    Unavailable,
}

fn repo_relative_pin_path(
    project_root: &Path,
    record_root: &Path,
    path: &str,
) -> Result<Option<String>> {
    let path = Path::new(path);
    let dot_relative = matches!(
        path.components().next(),
        Some(std::path::Component::CurDir | std::path::Component::ParentDir)
    );
    if !dot_relative {
        return Ok(Some(path.to_string_lossy().into_owned()));
    }
    let resolved_path = absolute(&record_root.join(path))?;
    let target = crate::project_modes::resolved(&resolved_path)?;
    if !target.starts_with(project_root) {
        return Ok(None);
    }
    Ok(resolved_path
        .strip_prefix(project_root)
        .ok()
        .map(|relative| relative.to_string_lossy().into_owned()))
}

fn tree_blob_status(output: &[u8], path: &str) -> PinnedFileStatus {
    for entry in output.split(|byte| *byte == 0).filter(|entry| !entry.is_empty()) {
        let Some(separator) = entry.iter().position(|byte| *byte == b'\t') else {
            return PinnedFileStatus::Unavailable;
        };
        let metadata = entry[..separator].split(|byte| byte.is_ascii_whitespace());
        let kind = metadata
            .skip(1)
            .next()
            .filter(|field| !field.is_empty());
        let Some(kind) = kind else {
            return PinnedFileStatus::Unavailable;
        };
        if &entry[separator + 1..] == path.as_bytes() {
            return if kind == b"blob" {
                PinnedFileStatus::Available
            } else {
                PinnedFileStatus::Missing
            };
        }
    }
    PinnedFileStatus::Missing
}

fn pinned_file_status(root: &Path, revision: &str, path: &str) -> PinnedFileStatus {
    if revision.is_empty() || path.is_empty() || revision.contains('\0') || path.contains('\0') {
        return PinnedFileStatus::Unresolved;
    }
    let reference = format!("{revision}^{{commit}}");
    let commit = match crate::pending_state::git(
        root,
        &["rev-parse", "--verify", "--end-of-options", &reference],
        PIN_PROBE_OUTPUT_LIMIT,
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
    // The resolved object ID keeps authored option-like revisions out of Git's arguments.
    // ls-tree reads tree metadata without requiring the blob, which matters in blobless clones.
    let pathspec = format!(":(literal){path}");
    match crate::pending_state::git(
        root,
        &["ls-tree", "--full-tree", "-z", &commit, "--", &pathspec],
        PIN_PROBE_OUTPUT_LIMIT,
        true,
    ) {
        Ok(Some(output)) => tree_blob_status(&output, path),
        Ok(None) => PinnedFileStatus::Missing,
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
            // Try the full locator and each line/column-trimmed form. Check every
            // local candidate before Git so existing citations never depend on Git.
            let candidates = locator_candidates(locator);
            let mut inside_candidates = Vec::new();
            let mut local_exists = false;
            for candidate in &candidates {
                let path = absolute(&record_root.join(Path::new(candidate)))?;
                let resolved = crate::project_modes::resolved(&path)?;
                if !resolved.starts_with(&project.root) {
                    continue;
                }
                inside_candidates.push(*candidate);
                if inventory.exists(&path)? {
                    local_exists = true;
                    break;
                }
            }
            if local_exists {
                continue;
            }
            if inside_candidates.is_empty() {
                continue;
            }

            let mut missing_pin: Option<&str> = None;
            let mut ambiguous_pin: Option<(&str, &str)> = None;
            let mut unavailable_pin: Option<PinnedFileStatus> = None;
            let mut pin_available = false;
            if !Path::new(locator).is_absolute() {
                for candidate in &candidates {
                    let Some((revision, pinned_path)) = candidate.split_once(':') else {
                        continue;
                    };
                    if revision.is_empty() || pinned_path.is_empty() {
                        continue;
                    }
                    let Some(repository_path) =
                        repo_relative_pin_path(&project.root, record_root, pinned_path)?
                    else {
                        continue;
                    };
                    // Resolved refs may contain slashes; retain the older slash-free
                    // revision:path heuristic when Git confirms no revision exists.
                    match pinned_file_status(&project.root, revision, &repository_path) {
                        PinnedFileStatus::Available => {
                            pin_available = true;
                            break;
                        }
                        PinnedFileStatus::Missing => missing_pin = Some(candidate),
                        status @ (PinnedFileStatus::RevisionUnavailable
                        | PinnedFileStatus::Unavailable) => {
                            unavailable_pin = Some(status);
                            break;
                        }
                        PinnedFileStatus::Unresolved
                            if !revision.contains(['/', '\\'])
                                && repository_path.contains('/') =>
                        {
                            missing_pin = Some(candidate);
                        }
                        PinnedFileStatus::Unresolved
                            if revision.contains('/') && repository_path.contains('/') =>
                        {
                            ambiguous_pin = Some((revision, candidate));
                        }
                        PinnedFileStatus::Unresolved => {}
                    }
                }
            }
            if pin_available {
                continue;
            }
            if let Some(status) = unavailable_pin {
                let reason = match status {
                    PinnedFileStatus::RevisionUnavailable => "revision",
                    PinnedFileStatus::Unavailable => "object",
                    _ => unreachable!("only unavailable probe states are stored"),
                };
                notes.push(format!(
                    "{id}: could not check locator {locator} because Git's {reason} probe was unavailable; retry the check or re-read the source"
                ));
                continue;
            }
            if let Some(candidate) = missing_pin {
                notes.push(format!("{id}: pinned file {locator} is unavailable in this repository; verify the revision and path with git show {candidate}, or re-read the source"));
                continue;
            }
            if let Some((revision, candidate)) = ambiguous_pin {
                notes.push(format!(
                    "{id}: no local file matches {locator}, and Git does not know revision {revision}; if this is a pin, verify it with git show {candidate}, or re-read the source"
                ));
                continue;
            }
            notes.push(format!("{id}: file {locator} is absent from this repository; re-read the surviving source, or pin the historical file as revision:path (open it with git show revision:path)"));
        }
    }
    Ok(notes)
}

//! Portable physical paths bind exact logical subjects; subjects are never paths.
use crate::{Error, Result, identity::sha256, require};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Hashed,
    Legacy,
}
pub const CAPABILITY: &str = "subject-paths/v2";
pub fn object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub fn subject(value: &str) -> Result<()> {
    require(value.len() <= 1_048_576, "history_subject_limit")
}
fn legacy(value: &str) -> bool {
    value
        .bytes()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
}
pub fn object_path(name: &str, id: &str, scheme: Scheme) -> Result<String> {
    subject(name)?;
    require(object_id(id), "invalid_history_object_id")?;
    let directory = match scheme {
        Scheme::Legacy => {
            require(legacy(name), "invalid_legacy_subject_path")?;
            name.into()
        }
        Scheme::Hashed => {
            let mut bytes = b"kpopper-history-subject-path/v1\0".to_vec();
            bytes.extend_from_slice(name.as_bytes());
            format!("~{}", sha256(&bytes))
        }
    };
    Ok(format!("{directory}/{id}.yaml"))
}
pub fn parse_object_path(path: &str) -> Result<(Scheme, &str, &str)> {
    require(!path.contains(['\\', '\0']), "invalid_history_object_path")?;
    let (directory, filename) = path
        .split_once('/')
        .ok_or_else(|| Error("invalid_history_object_path".into()))?;
    let id = filename
        .strip_suffix(".yaml")
        .ok_or_else(|| Error("invalid_history_object_path".into()))?;
    require(
        !directory.is_empty() && object_id(id),
        "invalid_history_object_path",
    )?;
    let scheme = if directory
        .strip_prefix('~')
        .is_some_and(|s| s.len() == 64 && object_id(s))
    {
        Scheme::Hashed
    } else {
        require(legacy(directory), "invalid_history_object_path")?;
        Scheme::Legacy
    };
    Ok((scheme, directory, id))
}
pub fn validate_object_path(path: &str, name: &str, id: &str) -> Result<Scheme> {
    let (scheme, _, _) = parse_object_path(path)?;
    require(
        path == object_path(name, id, scheme)?,
        "history_object_path_mismatch",
    )?;
    Ok(scheme)
}
pub fn resolve_object_path(paths: &BTreeSet<String>, name: &str, id: &str) -> Result<String> {
    let mut candidates = vec![object_path(name, id, Scheme::Hashed)?];
    if legacy(name) {
        candidates.push(object_path(name, id, Scheme::Legacy)?);
    }
    let mut matches = candidates.into_iter().filter(|path| paths.contains(path));
    let path = matches
        .next()
        .ok_or_else(|| Error("missing_history_object_path".into()))?;
    require(matches.next().is_none(), "duplicate_history_object_path")?;
    Ok(path)
}
pub fn validate_path_capability(paths: &BTreeSet<String>, requires: &[String]) -> Result<()> {
    for path in paths {
        if parse_object_path(path)?.0 == Scheme::Hashed {
            require(
                requires.iter().any(|s| s == CAPABILITY),
                "subject_path_capability_required",
            )?;
        }
    }
    Ok(())
}

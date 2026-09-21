//! Optional private evidence of successful record publication by this session.

use crate::{
    Error, Result,
    history_transaction::FileImage,
    history_yaml::{self, OrdinaryValue, SourceValue},
    identity::sha256,
    source_capture::CapturedSource,
    value::TypedValue,
};
use fs2::FileExt;
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_STATE: usize = 1024 * 1024;

/// Empty TMPDIR must never turn optional private state into workspace files.
pub fn temporary_directory() -> PathBuf {
    let candidates = ["TMPDIR", "TEMP", "TMP"].map(|name| env::var_os(name).map(PathBuf::from));
    choose_temporary_directory(&candidates, env::temp_dir())
}
fn choose_temporary_directory(candidates: &[Option<PathBuf>], system: PathBuf) -> PathBuf {
    for path in candidates.iter().flatten() {
        if !path.as_os_str().is_empty() && path.is_absolute() && path.is_dir() {
            return path.clone();
        }
    }
    if system.is_absolute() && system.is_dir() {
        return system;
    }
    #[cfg(unix)]
    {
        PathBuf::from("/tmp")
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
            .join("Temp")
    }
}

pub fn published(root: &Path, files: &[FileImage], subjects: Option<&BTreeSet<String>>) {
    let sid = env::var("KPOPPER_AGENT_SESSION")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| env::var("CODEX_THREAD_ID").ok().filter(|s| !s.is_empty()));
    let tmp = temporary_directory();
    if let Some(sid) = sid {
        let _ = published_with(&sid, &tmp, root, files, subjects);
    }
}

fn valid_session(sid: &str) -> bool {
    !sid.is_empty()
        && sid.len() <= 200
        && sid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn session_home(tmp: &Path, sid: &str) -> Result<PathBuf> {
    if !valid_session(sid) {
        return Err(Error("invalid_session".into()));
    }
    Ok(tmp.join(format!("kpopper-session-{sid}")))
}
fn safe_home(tmp: &Path, sid: &str) -> Result<PathBuf> {
    let home = session_home(tmp, sid)?;
    let mut directory = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory.mode(0o700);
    }
    match directory.create(&home) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let meta = fs::symlink_metadata(&home)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(Error("session state must not be a symlink".into()));
    }
    Ok(home)
}
fn read_state(path: &Path) -> Map<String, Value> {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Map::new();
    };
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() as usize > MAX_STATE {
        return Map::new();
    }
    let Ok(file) = fs::File::open(path) else {
        return Map::new();
    };
    let mut raw = Vec::new();
    if file
        .take((MAX_STATE + 1) as u64)
        .read_to_end(&mut raw)
        .is_err()
        || raw.len() > MAX_STATE
    {
        return Map::new();
    }
    let Ok(Value::Object(object)) =
        crate::json_ingress::parse_slice(&raw, crate::json_ingress::DuplicateKeys::LastWins)
    else {
        return Map::new();
    };
    object
}
fn state_path(home: &Path, key: &str) -> Result<PathBuf> {
    state_file(home, &format!("writes-{key}.json"))
}
fn state_file(home: &Path, name: &str) -> Result<PathBuf> {
    let path = home.join(name);
    if let Ok(meta) = fs::symlink_metadata(&path)
        && meta.file_type().is_symlink()
    {
        return Err(Error("session state member must not be a symlink".into()));
    }
    Ok(path)
}
fn update(
    home: &Path,
    key: &str,
    changed: &BTreeMap<String, String>,
    removed: &BTreeSet<String>,
) -> Result<()> {
    let lock_path = home.join("state.lock");
    if let Ok(meta) = fs::symlink_metadata(&lock_path)
        && meta.file_type().is_symlink()
    {
        return Err(Error("session lock must not be a symlink".into()));
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)?;
    lock.lock_exclusive()?;
    let path = state_path(home, key)?;
    let mut state = read_state(&path);
    for subject in removed {
        state.remove(subject);
    }
    state.extend(
        changed
            .iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone()))),
    );
    let data = serde_json::to_vec(&Value::Object(state))?;
    if data.len() > MAX_STATE {
        return Err(Error("session state limit exceeded".into()));
    }
    let mut temporary = tempfile::NamedTempFile::new_in(home)?;
    temporary.write_all(&data)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(&path)
        .map_err(|e| Error(format!("persist: {}", e.error)))?;
    Ok(())
}

/// Return subjects whose captured body and exact origin have a matching private
/// publication receipt for this session.  Attribution is deliberately best
/// effort: malformed, unavailable, or unsupported state produces no subjects.
pub fn owned(tmp: &Path, sid: &str, capture: &CapturedSource) -> BTreeSet<String> {
    let Ok(home) = session_home(tmp, sid) else {
        return BTreeSet::new();
    };
    let Ok(meta) = fs::symlink_metadata(&home) else {
        return BTreeSet::new();
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return BTreeSet::new();
    }
    let mut bodies = match capture.source() {
        OrdinaryValue::Map(collections) => collections
            .iter()
            .filter(|(section, _)| {
                !matches!(section.text(), Some("meta" | "schema" | "record" | "also"))
            })
            .filter_map(|(_, members)| match members {
                OrdinaryValue::Map(members) => Some(members),
                _ => None,
            })
            .filter_map(|members| {
                members
                    .iter()
                    .map(|(id, body)| Some((id.text()?.to_owned(), body.projected())))
                    .collect::<Option<Vec<_>>>()
            })
            .flatten()
            .collect::<BTreeMap<_, _>>(),
        _ => BTreeMap::new(),
    };
    let mut origins = BTreeMap::new();
    if let OrdinaryValue::Map(collections) = capture.source() {
        for (section, members) in collections {
            let Some(section) = section.text() else {
                continue;
            };
            if matches!(section, "meta" | "schema" | "record" | "also") {
                continue;
            }
            let OrdinaryValue::Map(members) = members else {
                continue;
            };
            for (id, _) in members {
                let Some(id) = id.text() else { continue };
                if let Some(path) = capture.origins().get(section).and_then(|m| m.get(id)) {
                    origins.insert(id.to_owned(), path.clone());
                } else {
                    origins.remove(id);
                }
            }
        }
    }
    // Hypothesis-only IDs participate in ownership exactly like ordinary IDs.
    if let Ok(hypotheses) = crate::history_contract::map(capture.hypotheses()) {
        for hypothesis in hypotheses.values() {
            let Ok(hypothesis) = crate::history_contract::map(hypothesis) else {
                continue;
            };
            let Some(path) = hypothesis
                .get("path")
                .and_then(|v| crate::history_contract::text(v).ok())
            else {
                continue;
            };
            let origin = PathBuf::from(path);
            let origin = if origin.is_absolute() {
                origin
            } else if let Some(member) = capture.members().first() {
                member
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(origin)
            } else {
                origin
            };
            let Ok(resolved_origin) = crate::project_modes::resolved(&origin) else {
                continue;
            };
            let physical = capture.files().iter().find(|(file, _)| {
                crate::project_modes::resolved(file).ok().as_ref() == Some(&resolved_origin)
            });
            let physical =
                physical.and_then(|(_, bytes)| history_yaml::decode_source_value(bytes).ok());
            // Virtual history layers have no physical publication image here.
            // Only captured physical bytes can bind a hypothesis write receipt.
            let Some(SourceValue::Map(sections)) = physical else {
                continue;
            };
            let mut raw = BTreeMap::new();
            for (section, members) in sections {
                if section == "hypothesis"
                    || matches!(section.as_str(), "meta" | "schema" | "record" | "also")
                {
                    continue;
                }
                let SourceValue::Map(members) = members else {
                    continue;
                };
                raw.extend(members.into_iter().map(|(id, body)| (id, body.typed())));
            }
            for (id, body) in raw {
                if !bodies.contains_key(&id) {
                    bodies.insert(id.clone(), body);
                    origins.insert(id.clone(), origin.clone());
                }
            }
        }
    }
    let mut receipts = BTreeMap::<String, Map<String, Value>>::new();
    let mut result = BTreeSet::new();
    for (id, body) in bodies {
        let Some(origin) = origins.get(&id) else {
            continue;
        };
        let Ok(origin) = crate::project_modes::resolved(origin) else {
            continue;
        };
        let Some(origin) = origin.to_str() else {
            continue;
        };
        let key = sha256(origin.as_bytes());
        let state = receipts
            .entry(key.clone())
            .or_insert_with(|| read_state(&home.join(format!("writes-{key}.json"))));
        if let Ok(digest) = body.digest()
            && state.get(&id).and_then(Value::as_str) == Some(digest.as_str())
        {
            result.insert(id);
        }
    }
    result
}

/// Atomically select fresh assessed findings for a session and resolved record.
/// Receipt failures suppress delivery so an unavailable private store cannot
/// trap the host in a repeated Stop loop.
pub fn deliver_stop(
    tmp: &Path,
    sid: &str,
    issues: &[(String, String, String)],
    paths: &[PathBuf],
) -> Vec<String> {
    if !valid_session(sid) || paths.is_empty() || issues.is_empty() {
        return Vec::new();
    }
    // Bound caller data before cloning/hashing it, as well as the persisted JSON.
    let bytes = issues
        .iter()
        .try_fold(0usize, |total, (kind, condition, message)| {
            total
                .checked_add(kind.len())?
                .checked_add(condition.len())?
                .checked_add(message.len())?
                .checked_add(70)
        });
    if bytes.is_none_or(|n| n > MAX_STATE) || paths.len() > MAX_STATE / 70 {
        return Vec::new();
    }
    let mut resolved = Vec::with_capacity(paths.len());
    let mut path_bytes = 0usize;
    for path in paths {
        let Ok(path) = crate::project_modes::resolved(path) else {
            return Vec::new();
        };
        let Some(path) = path.to_str() else {
            return Vec::new();
        };
        path_bytes = path_bytes.saturating_add(path.len());
        if path_bytes > MAX_STATE {
            return Vec::new();
        }
        resolved.push(path.to_owned());
    }
    resolved.sort();
    let Ok(record) =
        TypedValue::List(resolved.into_iter().map(TypedValue::Text).collect()).digest()
    else {
        return Vec::new();
    };
    let Ok(home) = safe_home(tmp, sid) else {
        return Vec::new();
    };
    let Ok(lock_path) = state_file(&home, &format!("delivery-{record}.json")) else {
        return Vec::new();
    };
    let state_lock = home.join("state.lock");
    if fs::symlink_metadata(&state_lock).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Vec::new();
    }
    let Ok(lock) = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(state_lock)
    else {
        return Vec::new();
    };
    if lock.lock_exclusive().is_err() {
        return Vec::new();
    }
    let mut state = read_state(&lock_path);
    let mut fresh = Vec::new();
    for (kind, condition, message) in issues {
        let key = match TypedValue::List(vec![
            TypedValue::Text(kind.clone()),
            TypedValue::Text(condition.clone()),
        ])
        .digest()
        {
            Ok(key) => key,
            Err(_) => continue,
        };
        if !state.contains_key(&key) {
            state.insert(key, Value::Bool(true));
            fresh.push(message.clone());
        }
    }
    let Ok(data) = serde_json::to_vec(&Value::Object(state)) else {
        return Vec::new();
    };
    if data.len() > MAX_STATE {
        return Vec::new();
    }
    let Ok(mut temporary) = tempfile::NamedTempFile::new_in(&home) else {
        return Vec::new();
    };
    if temporary.write_all(&data).is_err() || temporary.as_file().sync_all().is_err() {
        return Vec::new();
    }
    if temporary.persist(&lock_path).is_err() {
        return Vec::new();
    }
    fresh
}
fn bodies(raw: Option<&[u8]>) -> Result<BTreeMap<String, TypedValue>> {
    let Some(raw) = raw else {
        return Ok(BTreeMap::new());
    };
    let source = history_yaml::decode_source_value(raw)?;
    if !crate::history_view::truth(&source.typed()) {
        return Ok(BTreeMap::new());
    }
    let SourceValue::Map(collections) = source else {
        return Err(Error("invalid_schema".into()));
    };
    let mut result = BTreeMap::new();
    for (collection, members) in collections {
        if matches!(collection.as_str(), "meta" | "schema" | "record" | "also") {
            continue;
        }
        let SourceValue::Map(members) = members else {
            continue;
        };
        // `provenance.bodies`, used by publication receipts, includes every
        // mapping collection. It is deliberately broader than collections_of.
        for (subject, body) in members {
            result.insert(subject, body.typed());
        }
    }
    Ok(result)
}
fn published_with(
    sid: &str,
    tmp: &Path,
    root: &Path,
    files: &[FileImage],
    subjects: Option<&BTreeSet<String>>,
) -> Result<()> {
    for item in files {
        if !matches!(
            item.role.as_str(),
            "record" | "record_member" | "hypothesis"
        ) {
            continue;
        }
        let before = bodies(item.before.as_deref())?;
        let after = bodies(item.after.as_deref())?;
        let mut changed = BTreeMap::new();
        for (subject, body) in &after {
            if subjects.is_some_and(|allowed| !allowed.contains(subject)) {
                continue;
            }
            let digest = body.digest()?;
            if before.get(subject).map(|old| old.digest()).transpose()? != Some(digest.clone()) {
                changed.insert(subject.clone(), digest);
            }
        }
        let removed: BTreeSet<_> = before
            .keys()
            .filter(|subject| {
                !after.contains_key(*subject)
                    && subjects.is_none_or(|allowed| allowed.contains(*subject))
            })
            .cloned()
            .collect();
        if changed.is_empty() && removed.is_empty() {
            continue;
        }
        let resolved = crate::project_modes::resolved(&root.join(&item.path))?;
        let path = resolved
            .to_str()
            .ok_or_else(|| Error("nonportable_session_path".into()))?;
        let home = safe_home(tmp, sid)?;
        update(&home, &sha256(path.as_bytes()), &changed, &removed)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_capture::{ReadMode, capture_source};
    use tempfile::tempdir;
    fn image(before: Option<&str>, after: Option<&str>, role: &str) -> FileImage {
        image_path("GROUNDING.yaml", before, after, role)
    }

    #[test]
    fn temporary_directory_ignores_blank_relative_and_missing_candidates() {
        let valid = tempdir().unwrap();
        let choices = [
            Some(PathBuf::new()),
            Some(PathBuf::from("relative")),
            Some(valid.path().join("missing")),
            Some(valid.path().to_owned()),
        ];
        assert_eq!(
            choose_temporary_directory(&choices, PathBuf::from("also-relative")),
            valid.path()
        );
    }
    fn image_path(path: &str, before: Option<&str>, after: Option<&str>, role: &str) -> FileImage {
        FileImage {
            path: path.into(),
            role: role.into(),
            before: before.map(str::as_bytes).map(Vec::from),
            after: after.map(str::as_bytes).map(Vec::from),
        }
    }

    #[test]
    fn owned_requires_exact_body_and_origin_and_keeps_authoritative_shadow() {
        let tmp = tempdir().unwrap();
        let root = tempdir().unwrap();
        let record = root.path().join("GROUNDING.yaml");
        fs::write(&record, "facts:\n  a: {v: 1}\n").unwrap();
        published_with(
            "unit",
            tmp.path(),
            root.path(),
            &[image(None, Some("facts:\n  a: {v: 1}\n"), "record")],
            None,
        )
        .unwrap();
        let capture = capture_source(
            std::slice::from_ref(&record),
            root.path(),
            ReadMode::Frozen,
            None,
        )
        .unwrap();
        assert_eq!(
            owned(tmp.path(), "unit", &capture),
            BTreeSet::from(["a".into()])
        );
        fs::write(&record, "facts:\n  a: {v: 2}\n").unwrap();
        let changed = capture_source(
            std::slice::from_ref(&record),
            root.path(),
            ReadMode::Frozen,
            None,
        )
        .unwrap();
        assert!(owned(tmp.path(), "unit", &changed).is_empty());

        let other = root.path().join("OTHER.yaml");
        fs::write(&other, "facts:\n  a: {v: 1}\n").unwrap();
        let moved = capture_source(
            std::slice::from_ref(&other),
            root.path(),
            ReadMode::Frozen,
            None,
        )
        .unwrap();
        assert!(owned(tmp.path(), "unit", &moved).is_empty());
        // A shadowed ID remains tied to the authoritative ordinary origin.
        fs::write(&record, "facts:\n  a: {v: 1}\n").unwrap();
        let shadowed = capture_source(
            std::slice::from_ref(&record),
            root.path(),
            ReadMode::Frozen,
            None,
        )
        .unwrap();
        assert_eq!(
            owned(tmp.path(), "unit", &shadowed),
            BTreeSet::from(["a".into()])
        );
    }

    #[test]
    fn owned_includes_physical_hypothesis_only_ids_and_shadows_base() {
        let tmp = tempdir().unwrap();
        let root = tempdir().unwrap();
        let record = root.path().join("GROUNDING.yaml");
        let hyp = root.path().join(".kpopper/hypotheses/try.yaml");
        fs::write(&record, "facts:\n  a: {v: 1}\n").unwrap();
        fs::create_dir_all(hyp.parent().unwrap()).unwrap();
        fs::write(
            &hyp,
            "hypothesis: {claim: test}\nideas:\n  h: {v: 3}\n  a: {v: 9}\n",
        )
        .unwrap();
        published_with(
            "unit",
            tmp.path(),
            root.path(),
            &[
                image(None, Some("facts:\n  a: {v: 1}\n"), "record"),
                image_path(
                    ".kpopper/hypotheses/try.yaml",
                    None,
                    Some("hypothesis: {claim: test}\nideas:\n  h: {v: 3}\n  a: {v: 9}\n"),
                    "hypothesis",
                ),
            ],
            None,
        )
        .unwrap();
        let capture = capture_source(
            std::slice::from_ref(&record),
            root.path(),
            ReadMode::Frozen,
            None,
        )
        .unwrap();
        assert_eq!(
            owned(tmp.path(), "unit", &capture),
            BTreeSet::from(["a".into(), "h".into()])
        );
    }

    fn receipt(tmp: &Path, root: &Path) -> Value {
        let key = sha256(
            crate::project_modes::resolved(&root.join("GROUNDING.yaml"))
                .unwrap()
                .to_string_lossy()
                .as_bytes(),
        );
        serde_json::from_slice(
            &fs::read(
                tmp.join("kpopper-session-unit")
                    .join(format!("writes-{key}.json")),
            )
            .unwrap(),
        )
        .unwrap()
    }
    #[test]
    fn changed_noop_removed_and_allowlist() {
        let tmp = tempdir().unwrap();
        let root = tempdir().unwrap();
        fs::write(root.path().join("GROUNDING.yaml"), "x: 1\n").unwrap();
        published_with(
            "unit",
            tmp.path(),
            root.path(),
            &[image(None, Some("facts:\n  a: {v: 1}\n"), "record")],
            None,
        )
        .unwrap();
        assert!(receipt(tmp.path(), root.path())["a"].is_string());
        published_with(
            "unit",
            tmp.path(),
            root.path(),
            &[image(
                Some("facts:\n  a: {v: 1}\n"),
                Some("facts:\n  a: {v: 1}\n"),
                "record",
            )],
            None,
        )
        .unwrap();
        published_with(
            "unit",
            tmp.path(),
            root.path(),
            &[image(
                Some("facts:\n  a: {v: 1}\n"),
                Some("facts:\n  b: {v: 2}\n"),
                "record",
            )],
            Some(&BTreeSet::from(["b".into()])),
        )
        .unwrap();
        assert!(
            receipt(tmp.path(), root.path())["a"].is_string()
                && receipt(tmp.path(), root.path())["b"].is_string()
        );
        published_with(
            "unit",
            tmp.path(),
            root.path(),
            &[image(
                Some("facts:\n  a: {v: 1}\n  b: {v: 2}\n"),
                Some("facts:\n  a: {v: 1}\n"),
                "record",
            )],
            None,
        )
        .unwrap();
        assert!(receipt(tmp.path(), root.path()).get("b").is_none());
    }
    #[test]
    fn typed_scalars_and_source_order_are_distinct() {
        let a = bodies(Some(b"one:\n  x: {v: 1}\ntwo:\n  x: {v: true}\n")).unwrap();
        let b = bodies(Some(b"one:\n  x: {v: true}\ntwo:\n  x: {v: 1}\n")).unwrap();
        assert_ne!(a["x"].digest().unwrap(), b["x"].digest().unwrap());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_home_refused() {
        use std::os::unix::fs::symlink;
        let tmp = tempdir().unwrap();
        let target = tempdir().unwrap();
        symlink(target.path(), tmp.path().join("kpopper-session-unit")).unwrap();
        let root = tempdir().unwrap();
        fs::write(root.path().join("GROUNDING.yaml"), "x: 1\n").unwrap();
        assert!(
            published_with(
                "unit",
                tmp.path(),
                root.path(),
                &[image(None, Some("facts:\n  a: {v: 1}\n"), "record")],
                None,
            )
            .is_err()
        );
    }
    #[test]
    fn malformed_and_oversized_receipts_are_ignored() {
        let tmp = tempdir().unwrap();
        let root = tempdir().unwrap();
        let home = tmp.path().join("kpopper-session-unit");
        fs::create_dir(&home).unwrap();
        fs::write(home.join("writes-bad.json"), b"not json").unwrap();
        assert!(read_state(&home.join("writes-bad.json")).is_empty());
        fs::write(home.join("writes-big.json"), vec![b'x'; MAX_STATE + 1]).unwrap();
        assert!(read_state(&home.join("writes-big.json")).is_empty());
        fs::write(root.path().join("GROUNDING.yaml"), "x: 1\n").unwrap();
        published_with(
            "unit",
            tmp.path(),
            root.path(),
            &[image(None, Some("facts:\n  a: {v: 1}\n"), "record")],
            None,
        )
        .unwrap();
        assert!(receipt(tmp.path(), root.path())["a"].is_string());
    }
    #[test]
    fn no_op_does_not_create_session_state() {
        let tmp = tempdir().unwrap();
        let root = tempdir().unwrap();
        published_with(
            "unit",
            tmp.path(),
            root.path(),
            &[image(None, None, "other")],
            None,
        )
        .unwrap();
        assert!(!tmp.path().join("kpopper-session-unit").exists());
    }

    #[test]
    fn stop_delivery_is_once_per_finding_and_record() {
        let tmp = tempdir().unwrap();
        let root = tempdir().unwrap();
        let path = root.path().join("GROUNDING.yaml");
        let issues = vec![
            ("missing".into(), "a".into(), "A".into()),
            ("missing".into(), "a".into(), "A duplicate".into()),
            ("changed".into(), "a".into(), "B".into()),
        ];
        assert_eq!(
            deliver_stop(tmp.path(), "s1", &issues, std::slice::from_ref(&path)),
            vec!["A", "B"]
        );
        assert!(deliver_stop(tmp.path(), "s1", &issues, std::slice::from_ref(&path)).is_empty());
        assert_eq!(
            deliver_stop(tmp.path(), "s2", &issues, std::slice::from_ref(&path)),
            vec!["A", "B"]
        );
        let other = root.path().join("OTHER.yaml");
        assert_eq!(
            deliver_stop(tmp.path(), "s1", &issues, std::slice::from_ref(&other)),
            vec!["A", "B"]
        );
    }

    #[test]
    fn empty_stop_issues_do_not_create_private_state() {
        let temp = tempdir().unwrap();
        assert!(
            deliver_stop(
                temp.path(),
                "empty",
                &[],
                &[temp.path().join("GROUNDING.yaml")]
            )
            .is_empty()
        );
        assert!(!temp.path().join("kpopper-session-empty").exists());
    }

    #[test]
    fn stop_delivery_rejects_invalid_session_and_unavailable_state() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("GROUNDING.yaml");
        let issue = vec![("kind".into(), "condition".into(), "message".into())];
        assert!(
            deliver_stop(
                tmp.path(),
                "bad/session",
                &issue,
                std::slice::from_ref(&path)
            )
            .is_empty()
        );
        fs::create_dir(tmp.path().join("kpopper-session-ok")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(tmp.path(), tmp.path().join("kpopper-session-ok/state.lock"))
            .unwrap();
        #[cfg(windows)]
        fs::create_dir(tmp.path().join("kpopper-session-ok/state.lock")).unwrap();
        assert!(deliver_stop(tmp.path(), "ok", &issue, std::slice::from_ref(&path)).is_empty());
    }

    #[test]
    fn oversized_stop_input_is_rejected_before_private_state_is_created() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("GROUNDING.yaml");
        let issues = vec![(
            "failure".into(),
            "condition".into(),
            "x".repeat(MAX_STATE + 1),
        )];
        assert!(deliver_stop(tmp.path(), "bounded", &issues, &[path]).is_empty());
        assert!(!tmp.path().join("kpopper-session-bounded").exists());
    }
    #[test]
    fn deleted_file_can_remove_stale_receipt() {
        let tmp = tempdir().unwrap();
        let root = tempdir().unwrap();
        let record = root.path().join("GROUNDING.yaml");
        fs::write(&record, "facts:\n  a: {v: 1}\n").unwrap();
        published_with(
            "unit",
            tmp.path(),
            root.path(),
            &[image(None, Some("facts:\n  a: {v: 1}\n"), "record")],
            None,
        )
        .unwrap();
        fs::remove_file(record).unwrap();
        published_with(
            "unit",
            tmp.path(),
            root.path(),
            &[image(Some("facts:\n  a: {v: 1}\n"), None, "record")],
            None,
        )
        .unwrap();
        assert!(receipt(tmp.path(), root.path()).get("a").is_none());
    }
    #[test]
    fn collections_match_python_golden_vectors() {
        let ordinary = bodies(Some(b"plain:\n  x: [a]\nmap:\n  x: {v: 1}\n")).unwrap();
        assert_eq!(
            ordinary["x"].digest().unwrap(),
            "f3f7fd0a1b2fa11b0883497d0f08e47086fbda9fb7f6aa08f90a8187cfb43844"
        );
        let core = bodies(Some(
            b"meta:\n  reasoning:\n    profile: core/v1\nplain:\n  x: [a]\nmap:\n  x: {v: 1}\n",
        ))
        .unwrap();
        assert_eq!(
            core["x"].digest().unwrap(),
            "f3f7fd0a1b2fa11b0883497d0f08e47086fbda9fb7f6aa08f90a8187cfb43844"
        );
        let mixed = bodies(Some(b"facts:\n  x: [1, 2]\n  y: {v: 1}\n")).unwrap();
        assert_eq!(
            mixed["x"].digest().unwrap(),
            "4fc8da64970cc349eef04c7ac16fde89a5daf73f00e53fde20bc9927093708a4"
        );
        assert_eq!(
            mixed["y"].digest().unwrap(),
            "f3f7fd0a1b2fa11b0883497d0f08e47086fbda9fb7f6aa08f90a8187cfb43844"
        );
        assert!(bodies(Some(b"null\n")).unwrap().is_empty());
        assert!(bodies(Some(b"\n")).unwrap().is_empty());
    }
}

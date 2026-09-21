//! Serialized local control of publication policy and cached decisions.
//! These operations never contact a remote or publish a branch.
use crate::{
    Error, Result,
    history_contract::{field, is_int, map, text},
    history_view::map_mut,
    pending_state::{self, Ledger},
    project_modes::Project,
    require,
    source_capture::{ReadMode, capture_source},
    value::{FiniteFloat, Integer, TypedValue as V},
};
use fs2::FileExt;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn s(value: &str) -> V {
    V::Text(value.into())
}
fn n(value: &str) -> V {
    V::Integer(Integer::new(value).unwrap())
}
fn empty() -> V {
    V::Map(BTreeMap::new())
}
fn object(items: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(
        items
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    )
}

pub(crate) fn git(root: &Path, args: &[&str]) -> Result<(bool, Vec<u8>)> {
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
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(name);
    }
    command.env("GIT_TERMINAL_PROMPT", "0");
    command.env("GIT_OPTIONAL_LOCKS", "0");
    let output = crate::reasoning_runtime::run_command_bounded(
        &mut command,
        vec![],
        Duration::from_secs(10),
        16 * 1024 * 1024,
    );
    match output {
        Ok(stdout) => Ok((true, stdout)),
        Err(error) if error.0 == "native reasoning process failed" => Ok((false, vec![])),
        Err(error) => Err(error),
    }
}

pub(crate) fn save(path: &Path, value: &V) -> Result<()> {
    let parent = path.parent().ok_or_else(|| Error("invalid_path".into()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700).create(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        let mut temporary = tempfile::Builder::new()
            .prefix(&format!(
                ".{}.",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("state")
            ))
            .tempfile_in(parent)?;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
        temporary.write_all(&canonical_json(value)?)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(path)
            .map_err(|error| Error(format!("io: {}", error.error)))?;
        File::open(parent)?.sync_all()?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::Builder::new()
            .prefix(".pending-")
            .tempfile_in(parent)?;
        temporary.write_all(&canonical_json(value)?)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(path)
            .map_err(|error| Error(format!("io: {}", error.error)))?;
        Ok(())
    }
}

pub(crate) fn canonical_json(value: &V) -> Result<Vec<u8>> {
    let mut raw = serde_json::to_vec(&value.to_json()?)?;
    raw.push(b'\n');
    Ok(raw)
}

fn default_state() -> V {
    object([
        ("version", n("1")),
        ("scope", V::Null),
        ("decisions", empty()),
        ("receipts", V::List(vec![])),
        ("expected_head", V::Null),
        ("pr", V::Null),
        ("cycle", n("0")),
        ("proposed", V::List(vec![])),
        ("intent", V::Null),
        ("failures", n("0")),
        ("retry_at", n("0")),
        ("paused", V::Bool(false)),
        ("states", empty()),
    ])
}

pub(crate) fn load_state(path: &Path) -> Result<V> {
    let Some(raw) = pending_state::read_private(path)? else {
        return Ok(default_state());
    };
    let value =
        crate::json_ingress::parse_slice(&raw, crate::json_ingress::DuplicateKeys::LastWins)?;
    let value = V::from_json(&value)?;
    require(
        map(&value)?
            .get("version")
            .is_some_and(|value| is_int(value, "1")),
        "unknown publication receipt version",
    )?;
    Ok(value)
}

pub(crate) struct PublisherLock {
    _file: File,
}
impl Drop for PublisherLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self._file);
    }
}
impl PublisherLock {
    pub(crate) fn acquire(project: &Project) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true).mode(0o700).create(&project.state)?;
        }
        #[cfg(not(unix))]
        fs::create_dir_all(&project.state)?;
        let path = project.state.join("publisher.lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options.open(path)?;
        file.try_lock_exclusive().map_err(|error| {
            if lock_contended(&error) {
                Error("another local publisher is active".into())
            } else {
                error.into()
            }
        })?;
        Ok(Self { _file: file })
    }
}
fn lock_contended(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || (cfg!(windows) && error.raw_os_error() == Some(33))
}

pub struct Configure<'a> {
    pub remote: Option<&'a str>,
    pub target: Option<&'a str>,
    pub branch: Option<&'a str>,
    pub grant: bool,
    pub revoke: bool,
}

pub fn configure(project: &Project, options: &Configure<'_>) -> Result<V> {
    require(project.is_git(), "publication needs Git")?;
    let policy = project.lock()?;
    let mut config = policy.config().clone();
    let previous = map(&config)?
        .get("publication")
        .filter(|value| **value != V::Null)
        .cloned()
        .unwrap_or_else(empty);
    let previous = map(&previous)?;
    let remote = options
        .remote
        .or_else(|| previous.get("remote").and_then(|value| text(value).ok()))
        .ok_or_else(|| {
            Error("first publication configuration needs --remote and --target".into())
        })?;
    let target = options
        .target
        .or_else(|| previous.get("target").and_then(|value| text(value).ok()))
        .ok_or_else(|| {
            Error("first publication configuration needs --remote and --target".into())
        })?;
    let branch = options
        .branch
        .or_else(|| previous.get("branch").and_then(|value| text(value).ok()))
        .unwrap_or("pending_grounding");
    for branch_name in [target, branch] {
        require(
            git(
                &project.root,
                &["check-ref-format", "--branch", branch_name],
            )?
            .0,
            "invalid publication branch",
        )?;
    }
    require(
        target != branch,
        "publication branch must differ from its target",
    )?;
    let (_, remotes) = git(&project.root, &["remote"])?;
    let remotes = String::from_utf8(remotes).map_err(|_| Error("invalid remote name".into()))?;
    require(
        remotes.lines().any(|name| name == remote),
        "publication remote is not configured in this repository",
    )?;
    let (success, urls) = git(
        &project.root,
        &["remote", "get-url", "--push", "--all", remote],
    )?;
    let urls =
        String::from_utf8(urls).map_err(|_| Error("invalid publication destination".into()))?;
    let urls = urls.lines().collect::<Vec<_>>();
    require(
        success && urls.len() == 1,
        "publication needs exactly one push destination",
    )?;
    let same = previous.get("remote") == Some(&s(remote))
        && previous.get("repository") == Some(&s(urls[0]))
        && previous.get("target") == Some(&s(target))
        && previous.get("branch") == Some(&s(branch));
    let standing = options.grant
        || same && previous.get("standing_permission") == Some(&V::Bool(true)) && !options.revoke;
    let scope = object([
        ("remote", s(remote)),
        ("repository", s(urls[0])),
        ("target", s(target)),
        ("branch", s(branch)),
        ("standing_permission", V::Bool(standing)),
    ]);
    if !previous.is_empty()
        && V::Map(previous.clone()) != scope
        && Ledger::capture(project)?.head.is_some()
    {
        let scope_fields = map(&scope)?;
        let changed = ["remote", "repository", "target", "branch"]
            .iter()
            .any(|key| previous.get(*key) != scope_fields.get(*key));
        require(
            !changed,
            "pending store belongs to the existing publication scope",
        )?;
    }
    let generation: num_bigint::BigInt = match field(map(&config)?, "generation")? {
        V::Integer(value) => {
            value
                .as_str()
                .parse::<num_bigint::BigInt>()
                .map_err(|_| Error("invalid_project_config".into()))?
                + 1
        }
        _ => return Err(Error("invalid_project_config".into())),
    };
    let fields = map_mut(&mut config)?;
    fields.insert("publication".into(), scope.clone());
    fields.insert(
        "generation".into(),
        V::Integer(Integer::new(&generation.to_string())?),
    );
    policy.verify()?;
    save(&project.config_path, &config)?;
    Ok(object([
        ("publication", scope),
        ("published", V::Bool(false)),
    ]))
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

pub fn decision(project: &Project, action: &str, revisions: &[String], reason: &str) -> Result<V> {
    action_at(project, action, revisions, reason, None, now())
}

pub fn decision_at(
    project: &Project,
    action: &str,
    revisions: &[String],
    reason: &str,
    at: f64,
) -> Result<V> {
    action_at(project, action, revisions, reason, None, at)
}

pub fn action(
    project: &Project,
    action: &str,
    revisions: &[String],
    reason: &str,
    replacement: Option<&str>,
) -> Result<V> {
    action_at(project, action, revisions, reason, replacement, now())
}

pub fn action_at(
    project: &Project,
    action: &str,
    revisions: &[String],
    reason: &str,
    replacement: Option<&str>,
    at: f64,
) -> Result<V> {
    require(
        project.is_git(),
        "Simple projects without Git have no publication queue",
    )?;
    require(
        matches!(
            action,
            "pause" | "resume" | "withdraw" | "reject" | "supersede" | "retry"
        ),
        "unknown publication action",
    )?;
    let _publisher = PublisherLock::acquire(project)?;
    let policy = project.lock()?;
    let path = project.state.join("publication.json");
    let mut state = load_state(&path)?;
    let ledger = Ledger::capture(project)?;
    require(
        revisions
            .iter()
            .all(|revision| ledger.bundles.contains_key(revision)),
        "decision must name a captured immutable revision",
    )?;
    if matches!(action, "withdraw" | "reject" | "supersede") {
        require(
            !revisions.is_empty() && !reason.trim().is_empty(),
            "terminal decisions need exact revisions and a reason",
        )?;
        if action == "supersede" {
            require(
                replacement.is_some_and(|revision| {
                    ledger.bundles.contains_key(revision)
                        && !revisions.iter().any(|selected| selected == revision)
                }),
                "supersession must name a different captured revision",
            )?;
        }
        let state_name = match action {
            "withdraw" => "withdrawn",
            "reject" => "rejected",
            _ => "superseded",
        };
        let fields = map_mut(&mut state)?;
        for revision in revisions {
            map_mut(
                fields
                    .get_mut("decisions")
                    .ok_or_else(|| Error("invalid_publication_state".into()))?,
            )?
            .insert(
                revision.clone(),
                object([
                    ("state", s(state_name)),
                    ("reason", s(reason)),
                    ("replacement", replacement.map(s).unwrap_or(V::Null)),
                ]),
            );
            map_mut(
                fields
                    .get_mut("states")
                    .ok_or_else(|| Error("invalid_publication_state".into()))?,
            )?
            .insert(revision.clone(), s(state_name));
        }
    } else if action == "pause" {
        map_mut(&mut state)?.insert("paused".into(), V::Bool(true));
    } else if action == "resume" {
        if !revisions.is_empty() {
            let record = project.record(Some(policy.config()))?;
            let destination = if record.exists() {
                capture_source(&[record], &project.root, ReadMode::Frozen, None)?
                    .strict_document()?
            } else {
                empty()
            };
            let bundles = ledger
                .bundles
                .iter()
                .map(|(revision, bundle)| (revision.clone(), bundle.value.clone()))
                .collect();
            let files = ledger
                .bundles
                .iter()
                .map(|(revision, bundle)| (revision.clone(), bundle.files.clone()))
                .collect();
            crate::pending_bundle::compatible(
                &destination,
                &bundles,
                &files,
                field(map(&state)?, "decisions")?,
                revisions,
            )?;
            let fields = map_mut(&mut state)?;
            for revision in revisions {
                map_mut(
                    fields
                        .get_mut("decisions")
                        .ok_or_else(|| Error("invalid_publication_state".into()))?,
                )?
                .remove(revision);
                map_mut(
                    fields
                        .get_mut("states")
                        .ok_or_else(|| Error("invalid_publication_state".into()))?,
                )?
                .insert(revision.clone(), s("captured"));
            }
        }
        map_mut(&mut state)?.insert("paused".into(), V::Bool(false));
    } else {
        let fields = map_mut(&mut state)?;
        fields.insert("failures".into(), n("0"));
        fields.insert("retry_at".into(), n("0"));
    }
    let receipt = object([
        ("event", s("decision")),
        ("at", V::Float(FiniteFloat::new(at)?)),
        ("action", s(action)),
        (
            "revisions",
            V::List(revisions.iter().map(|value| s(value)).collect()),
        ),
        ("reason", s(reason)),
        ("replacement", replacement.map(s).unwrap_or(V::Null)),
    ]);
    let receipts = map_mut(&mut state)?
        .get_mut("receipts")
        .ok_or_else(|| Error("invalid_publication_state".into()))?;
    let V::List(receipts) = receipts else {
        return Err(Error("invalid_publication_state".into()));
    };
    receipts.push(receipt);
    policy.verify()?;
    save(&path, &state)?;
    pending_state::publication_status(Some(&canonical_json(&state)?), &ledger)
}

#[cfg(test)]
mod locking_tests {
    use super::*;

    #[test]
    fn would_block_is_publisher_contention() {
        assert!(lock_contended(&std::io::Error::from(
            std::io::ErrorKind::WouldBlock
        )));
    }

    #[cfg(windows)]
    #[test]
    fn windows_lock_violation_is_publisher_contention() {
        assert!(lock_contended(&std::io::Error::from_raw_os_error(33)));
    }
}

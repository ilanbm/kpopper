//! Frozen legacy-record import and its durable local pending acknowledgement.
use super::ImportOptions;
use crate::{
    Error, Result,
    history_authority::Files,
    history_contract::{Map, field, map, text, token},
    history_transaction_fs as guarded_fs,
    history_view::map_mut,
    pending_bundle,
    pending_state::{self, Ledger},
    project_modes::Project,
    recording_privacy as privacy, require,
    source_capture::{CapturedSource, ReadMode, capture_source},
    value::{Integer, TypedValue as V},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::Path,
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_GIT_OUTPUT: usize = 64 * 1024 * 1024;
const MAX_EVIDENCE_BYTES: usize = 64 * 1024 * 1024;

fn s(value: &str) -> V {
    V::Text(value.into())
}
fn n(value: u128) -> Result<V> {
    Ok(V::Integer(Integer::new(&value.to_string())?))
}
fn object(items: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(
        items
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    )
}

fn private_locator(value: &V) -> bool {
    match value {
        V::Map(fields) => fields.iter().any(|(key, value)| {
            [
                "file",
                "path",
                "location",
                "source_file",
                "uri",
                "url",
                "from",
            ]
            .contains(&key.as_str())
                && matches!(value, V::Text(value) if value.starts_with("file:")
                    || value.starts_with('/') || value.starts_with("~/")
                    || value.as_bytes().get(1) == Some(&b':')
                        && matches!(value.as_bytes().get(2), Some(b'/') | Some(b'\\')))
                || private_locator(value)
        }),
        V::List(values) => values.iter().any(private_locator),
        _ => false,
    }
}

fn roots(document: &V) -> Result<Vec<String>> {
    let entries = crate::reasoning_snapshot::entries(document)?;
    let mut cited = BTreeSet::new();
    for (_, body) in entries.values() {
        let V::Map(body) = body else { continue };
        let Some(source) = body.get("from") else {
            continue;
        };
        match source {
            V::Text(source) if entries.contains_key(source) => {
                cited.insert(source.clone());
            }
            V::List(sources) => {
                for source in sources {
                    if let V::Text(source) = source
                        && entries.contains_key(source)
                    {
                        cited.insert(source.clone());
                    }
                }
            }
            _ => {}
        }
    }
    Ok(entries
        .keys()
        .filter(|entry| !cited.contains(*entry))
        .cloned()
        .collect())
}

fn evidence(
    document: &V,
    root: Option<&Path>,
) -> Result<(Files, BTreeMap<std::path::PathBuf, Vec<u8>>)> {
    let required = pending_bundle::required_files(document)?;
    if required.is_empty() {
        return Ok((Files::new(), BTreeMap::new()));
    }
    let root = root.ok_or_else(|| {
        Error("import needs an explicit --evidence root for referenced files".into())
    })?;
    let root = root.canonicalize()?;
    require(root.is_dir(), "invalid_evidence_root")?;
    let mut files = Files::new();
    let mut observations = BTreeMap::new();
    let mut total = 0usize;
    for name in required {
        crate::history_branch::portable_path(&name)?;
        let path = root.join(&name).canonicalize()?;
        require(
            path.starts_with(&root) && path.is_file(),
            "invalid_evidence_path",
        )?;
        let bytes = read_evidence(&path)?;
        total = total
            .checked_add(bytes.len())
            .ok_or_else(|| Error("evidence_limit".into()))?;
        require(total <= MAX_EVIDENCE_BYTES, "evidence_limit")?;
        observations.insert(path, bytes.clone());
        files.insert(name, bytes);
    }
    Ok((files, observations))
}

fn read_evidence(path: &Path) -> Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    let size =
        usize::try_from(file.metadata()?.len()).map_err(|_| Error("evidence_limit".into()))?;
    require(size <= MAX_EVIDENCE_BYTES, "evidence_limit")?;
    let mut bytes = Vec::with_capacity(size);
    file.take((MAX_EVIDENCE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    require(bytes.len() <= MAX_EVIDENCE_BYTES, "evidence_limit")?;
    Ok(bytes)
}

fn freeze_defaults(project: &Project, source: &CapturedSource) -> Result<bool> {
    if project.config_path.exists() {
        return Ok(false);
    }
    let expected = project.config()?;
    let guard = project.lock()?;
    if project.config_path.exists() {
        guard.verify()?;
        require(
            project.config()? == expected,
            "project policy or destination changed before capture; retry",
        )?;
        // Another importer published the same defaults after our source
        // capture. The caller must recapture against that now-stable policy.
        return Ok(true);
    }
    source.verify()?;
    guard.verify()?;
    require(
        !project.config_path.exists() && project.config()? == expected,
        "project policy or destination changed before capture; retry",
    )?;
    require(
        !crate::history_contract::string_is(&map(&expected)?["mode"], "simple"),
        "Simple uses its shared record, not a Git contribution queue",
    )?;
    let parent = project
        .config_path
        .parent()
        .ok_or_else(|| Error("invalid_path".into()))?;
    fs::create_dir_all(parent)?;
    let mut bytes = serde_json::to_vec(&expected.to_json()?)?;
    bytes.push(b'\n');
    guarded_fs::publish_immutable(
        parent,
        project
            .config_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| Error("nonportable_project_path".into()))?,
        &bytes,
    )?;
    Ok(true)
}

fn fresh_event_id() -> Result<String> {
    let token = tempfile::Builder::new()
        .prefix("kpop-import-")
        .rand_bytes(24)
        .tempdir()?;
    Ok(crate::identity::sha256(token.path().as_os_str().as_encoded_bytes())[..32].into())
}

fn git(root: &Path, args: &[&str], input: &[u8], env: &[(&str, &str)]) -> Result<Vec<u8>> {
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
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(key);
    }
    for (key, value) in [
        ("GIT_NO_LAZY_FETCH", "1"),
        ("GIT_NO_REPLACE_OBJECTS", "1"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_OPTIONAL_LOCKS", "0"),
    ] {
        command.env(key, value);
    }
    command.envs(env.iter().copied());
    crate::reasoning_runtime::run_command_bounded(
        &mut command,
        input.to_vec(),
        Duration::from_secs(30),
        MAX_GIT_OUTPUT,
    )
    .map_err(|error| {
        if ["runtime_timeout", "output_limit"].contains(&error.0.as_str()) {
            error
        } else {
            Error("pending_git_unavailable".into())
        }
    })
}

fn git_text(root: &Path, args: &[&str], input: &[u8], env: &[(&str, &str)]) -> Result<String> {
    String::from_utf8(git(root, args, input, env)?)
        .map(|value| value.trim().to_owned())
        .map_err(|_| Error("invalid_pending_ledger".into()))
}

fn tree(root: &Path, head: Option<&str>) -> Result<BTreeMap<String, String>> {
    let Some(head) = head else {
        return Ok(BTreeMap::new());
    };
    let raw = pending_state::git(root, &["ls-tree", "-r", "-z", head], MAX_GIT_OUTPUT, false)?
        .ok_or_else(|| Error("invalid_pending_ledger".into()))?;
    let mut files = BTreeMap::new();
    for row in raw.split(|byte| *byte == 0).filter(|row| !row.is_empty()) {
        let split = row
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or_else(|| Error("invalid_pending_ledger".into()))?;
        let info = std::str::from_utf8(&row[..split])
            .map_err(|_| Error("invalid_pending_ledger".into()))?
            .split(' ')
            .collect::<Vec<_>>();
        require(
            info.len() == 3 && info[0] == "100644" && info[1] == "blob",
            "invalid_pending_ledger",
        )?;
        let path = std::str::from_utf8(&row[split + 1..])
            .map_err(|_| Error("invalid_pending_ledger".into()))?;
        crate::history_branch::portable_path(path)?;
        require(
            files.insert(path.into(), info[2].into()).is_none(),
            "invalid_pending_ledger",
        )?;
    }
    Ok(files)
}

fn blob(root: &Path, oid: &str) -> Result<Vec<u8>> {
    pending_state::git(root, &["cat-file", "blob", oid], MAX_GIT_OUTPUT, false)?
        .ok_or_else(|| Error("invalid_pending_ledger".into()))
}

fn write_commit(
    root: &Path,
    old: Option<&str>,
    existing: &BTreeMap<String, String>,
    additions: &BTreeMap<String, Vec<u8>>,
) -> Result<String> {
    let index = tempfile::NamedTempFile::new()?.into_temp_path();
    fs::remove_file(&index)?;
    let index_name = index
        .to_str()
        .ok_or_else(|| Error("nonportable_project_path".into()))?;
    let env = [("GIT_INDEX_FILE", index_name)];
    if let Some(old) = old {
        git(root, &["read-tree", old], &[], &env)?;
    } else {
        git(root, &["read-tree", "--empty"], &[], &env)?;
    }
    let mut expected = existing.clone();
    for (path, bytes) in additions {
        crate::history_branch::portable_path(path)?;
        if let Some(oid) = existing.get(path) {
            require(
                blob(root, oid)? == *bytes,
                "pending ledger contains conflicting immutable content",
            )?;
            continue;
        }
        let oid = git_text(root, &["hash-object", "-w", "--stdin"], bytes, &[])?;
        pending_state::verify_blob(&oid, bytes)?;
        expected.insert(path.clone(), oid.clone());
        let cache = format!("100644,{oid},{path}");
        git(
            root,
            &["update-index", "--add", "--cacheinfo", &cache],
            &[],
            &env,
        )?;
    }
    let tree_id = git_text(root, &["write-tree"], &[], &env)?;
    require(
        tree(root, Some(&tree_id))? == expected,
        "pending tree verification failed before publication",
    )?;
    for (path, bytes) in additions {
        require(
            blob(root, &expected[path])? == *bytes,
            "pending tree verification failed before publication",
        )?;
    }
    let author = [
        ("GIT_AUTHOR_NAME", "Knowledge record"),
        ("GIT_AUTHOR_EMAIL", "knowledge@localhost"),
        ("GIT_COMMITTER_NAME", "Knowledge record"),
        ("GIT_COMMITTER_EMAIL", "knowledge@localhost"),
    ];
    let mut args = vec!["commit-tree", "--no-gpg-sign", tree_id.as_str()];
    if let Some(old) = old {
        args.extend(["-p", old]);
    }
    git_text(
        root,
        &args,
        b"Retain project knowledge contribution\n",
        &author,
    )
}

fn update_ref(root: &Path, old: Option<&str>, new: &str) -> Result<bool> {
    let zero = "0".repeat(new.len());
    let old = old.unwrap_or(&zero);
    let mut command = Command::new("git");
    command
        .args([
            "--no-pager",
            "--no-replace-objects",
            "--no-lazy-fetch",
            "-C",
        ])
        .arg(root)
        .args(["update-ref", pending_state::REF, new, old])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0");
    let (status, _) = crate::reasoning_runtime::run_command_bounded_with_status(
        &mut command,
        Vec::new(),
        Duration::from_secs(30),
        MAX_GIT_OUTPUT,
    )?;
    Ok(status.success())
}

fn publication_attempt(config: &V) -> Result<V> {
    let permission = map(config)?
        .get("publication")
        .and_then(|value| map(value).ok())
        .and_then(|value| value.get("standing_permission"))
        == Some(&V::Bool(true));
    Ok(object([
        ("started", V::Bool(false)),
        (
            "reason",
            s(if permission {
                "native capture does not start publication"
            } else {
                "no standing publication permission"
            }),
        ),
    ]))
}

fn receipt(project: &Project, event: &V, head: &str, replay: bool, config: &V) -> Result<V> {
    let event_id = text(field(map(event)?, "event_id")?)?;
    let path = format!("events/{event_id}.json");
    let first = git_text(
        &project.root,
        &["log", "--diff-filter=A", "--format=%H", head, "--", &path],
        &[],
        &[],
    )?
    .lines()
    .last()
    .ok_or_else(|| Error("invalid_pending_event".into()))?
    .to_owned();
    let mut out = event.clone();
    map_mut(&mut out)?.extend(Map::from([
        ("state".into(), s("captured")),
        ("commit".into(), s(&first)),
        ("ledger_commit".into(), s(head)),
        ("ref".into(), s(pending_state::REF)),
        ("replay".into(), V::Bool(replay)),
        ("publication_attempt".into(), publication_attempt(config)?),
    ]));
    Ok(out)
}

/// Capture a fully prepared public bundle behind the project policy lock.
/// `verify_source` must recheck every source/evidence observation immediately
/// before the compare-and-swap. The callback may be reused by other importers.
pub(crate) fn capture_pending(
    project: &Project,
    bundle: &V,
    files: &Files,
    event_id: &str,
    contribution_id: &str,
    verify_source: &mut dyn FnMut() -> Result<()>,
) -> Result<V> {
    capture_pending_with_hooks(
        project,
        bundle,
        files,
        event_id,
        contribution_id,
        verify_source,
        &mut |_, _, _| Ok(()),
        &mut || Ok(()),
    )
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn capture_pending_with_hooks(
    project: &Project,
    bundle: &V,
    files: &Files,
    event_id: &str,
    contribution_id: &str,
    verify_source: &mut dyn FnMut() -> Result<()>,
    before_cas: &mut dyn FnMut(usize, Option<&str>, &str) -> Result<()>,
    after_cas: &mut dyn FnMut() -> Result<()>,
) -> Result<V> {
    require(project.is_git(), "durable contribution store needs Git")?;
    token(&s(event_id))?;
    token(&s(contribution_id))?;
    pending_bundle::validate(bundle, files)?;
    let revision = text(field(map(bundle)?, "revision")?)?.to_owned();
    let expected_config = project.config()?;
    let guard = project.lock()?;
    verify_source()?;
    guard.verify()?;
    require(
        project.config()? == expected_config,
        "project policy or destination changed before capture; retry",
    )?;
    require(
        !crate::history_contract::string_is(&map(&expected_config)?["mode"], "simple"),
        "Simple uses its shared record, not a Git contribution queue",
    )?;
    let event_path = format!("events/{event_id}.json");
    for attempt in 0..8 {
        let old = pending_state::resolve(&project.root, pending_state::REF)?;
        let ledger = Ledger::at(&project.root, old.clone())?;
        if let Some(event) = ledger.events.iter().find(|event| {
            map(event).ok().and_then(|fields| fields.get("event_id")) == Some(&s(event_id))
        }) {
            let fields = map(event)?;
            require(
                fields.get("contribution_id") == Some(&s(contribution_id))
                    && fields.get("revision") == Some(&s(&revision)),
                "event ID was already used with different content or target",
            )?;
            return receipt(
                project,
                event,
                old.as_deref().unwrap(),
                true,
                &expected_config,
            );
        }
        let existing = tree(&project.root, old.as_deref())?;
        let sequence = existing
            .keys()
            .filter(|path| path.starts_with("events/"))
            .count()
            + 1;
        let captured_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error("clock_unavailable".into()))?
            .as_nanos();
        let event = object([
            ("event_id", s(event_id)),
            ("contribution_id", s(contribution_id)),
            ("revision", s(&revision)),
            ("sequence", n(sequence as u128)?),
            ("captured_at_ns", n(captured_at)?),
        ]);
        let prefix = format!("contributions/{revision}/");
        let manifest = field(map(bundle)?, "manifest")?;
        let mut additions = BTreeMap::from([
            (event_path.clone(), serde_json::to_vec(&event.to_json()?)?),
            (
                format!("{prefix}manifest.json"),
                manifest.canonical_bytes()?,
            ),
        ]);
        for (path, bytes) in files {
            additions.insert(format!("{prefix}evidence/{path}"), bytes.clone());
        }
        let commit = write_commit(&project.root, old.as_deref(), &existing, &additions)?;
        verify_source()?;
        guard.verify()?;
        before_cas(attempt, old.as_deref(), &commit)?;
        verify_source()?;
        guard.verify()?;
        require(
            project.config()? == expected_config,
            "project policy or destination changed before capture; retry",
        )?;
        if update_ref(&project.root, old.as_deref(), &commit)? {
            after_cas()?;
            return receipt(project, &event, &commit, false, &expected_config);
        }
    }
    Err(Error(
        "pending ref kept changing; retry capture with the same event ID".into(),
    ))
}

fn import_with_hook(
    workspace: &Path,
    options: &ImportOptions,
    before_capture: &mut dyn FnMut() -> Result<()>,
) -> Result<V> {
    let project = Project::open(workspace)?;
    let mut source = capture_source(
        std::slice::from_ref(&options.file),
        workspace,
        ReadMode::Frozen,
        None,
    )?;
    let document = source.strict_document()?;
    let event_id = options
        .event_id
        .clone()
        .map(Ok)
        .unwrap_or_else(fresh_event_id)?;
    token(&s(&event_id))?;
    let action = object([
        ("kind", s("import")),
        ("shareability", s(&options.shareability)),
        (
            "scope",
            object([
                ("kind", s(&options.scope)),
                ("environment", s(&options.environment)),
            ]),
        ),
        ("event_id", s(&event_id)),
    ]);
    if options.shareability != "project"
        || privacy::private_marker(&document)
        || private_locator(&document)
    {
        source.verify()?;
        return privacy::draft(
            &project,
            &action,
            &document,
            "private or unclear import permission",
        );
    }
    let (files, evidence_observations) = evidence(&document, options.evidence.as_deref())?;
    let scope = map(&action)?["scope"].clone();
    let bundle = pending_bundle::prepare(&document, &roots(&document)?, &scope, "project", &files)?;
    pending_bundle::validate(&bundle, &files)?;
    let revision = text(field(map(&bundle)?, "revision")?)?.to_owned();
    let contribution_id = format!("import-{revision}");
    before_capture()?;
    if freeze_defaults(&project, &source)? {
        source = capture_source(
            std::slice::from_ref(&options.file),
            workspace,
            ReadMode::Frozen,
            None,
        )?;
        require(source.strict_document()? == document, "snapshot_changed")?;
    }
    capture_pending(
        &project,
        &bundle,
        &files,
        &event_id,
        &contribution_id,
        &mut || {
            source.verify()?;
            require(
                evidence_observations.iter().all(|(path, bytes)| {
                    path.is_file() && read_evidence(path).ok().as_ref() == Some(bytes)
                }),
                "snapshot_changed",
            )
        },
    )
}

pub(crate) fn run(workspace: &Path, options: &ImportOptions) -> Result<V> {
    import_with_hook(workspace, options, &mut || Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(root: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, ImportOptions) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir(&root).unwrap();
        command(&root, &["init", "-b", "main"]);
        command(&root, &["config", "user.name", "Fixture"]);
        command(&root, &["config", "user.email", "fixture@example.test"]);
        fs::write(root.join("GROUNDING.yaml"), "known: {}\n").unwrap();
        command(&root, &["add", "GROUNDING.yaml"]);
        command(
            &root,
            &["-c", "commit.gpgsign=false", "commit", "-m", "fixture"],
        );
        let source = root.join("legacy.yaml");
        fs::write(&source, "sources:\n  s.vendor: {name: Vendor}\nknown:\n  fact.import:\n    v: 10\n    from: s.vendor\n    scope: {kind: external, environment: vendor}\n").unwrap();
        let options = ImportOptions {
            file: source,
            shareability: "project".into(),
            scope: "external".into(),
            environment: "vendor".into(),
            evidence: None,
            event_id: Some("event-fixed".into()),
        };
        (temp, root, options)
    }

    #[test]
    fn source_change_before_capture_leaves_no_pending_ref() {
        let (_temp, root, options) = fixture();
        let source = options.file.clone();
        let error = import_with_hook(&root, &options, &mut || {
            fs::write(&source, "known: {changed: true}\n")?;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.0, "snapshot_changed");
        assert!(
            pending_state::resolve(&root, pending_state::REF)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            fs::read(root.join("GROUNDING.yaml")).unwrap(),
            b"known: {}\n"
        );
    }

    #[test]
    fn successful_capture_replays_and_conflicting_event_refuses() {
        let (_temp, root, options) = fixture();
        let first = run(&root, &options).unwrap();
        assert_eq!(map(&first).unwrap()["replay"], V::Bool(false));
        let head = pending_state::resolve(&root, pending_state::REF).unwrap();
        let replay = run(&root, &options).unwrap();
        assert_eq!(map(&replay).unwrap()["replay"], V::Bool(true));
        assert_eq!(
            pending_state::resolve(&root, pending_state::REF).unwrap(),
            head
        );
        fs::write(
            &options.file,
            fs::read_to_string(&options.file)
                .unwrap()
                .replace("v: 10", "v: 11"),
        )
        .unwrap();
        assert_eq!(
            run(&root, &options).unwrap_err().0,
            "event ID was already used with different content or target"
        );
        assert_eq!(
            fs::read(root.join("GROUNDING.yaml")).unwrap(),
            b"known: {}\n"
        );
    }

    fn prepared(
        root: &Path,
        options: &ImportOptions,
    ) -> (Project, CapturedSource, V, Files, String) {
        let project = Project::open(root).unwrap();
        let source = capture_source(
            std::slice::from_ref(&options.file),
            root,
            ReadMode::Frozen,
            None,
        )
        .unwrap();
        let document = source.strict_document().unwrap();
        let scope = object([("kind", s("external")), ("environment", s("vendor"))]);
        let bundle = pending_bundle::prepare(
            &document,
            &roots(&document).unwrap(),
            &scope,
            "project",
            &Files::new(),
        )
        .unwrap();
        let revision = text(&map(&bundle).unwrap()["revision"]).unwrap().to_owned();
        freeze_defaults(&project, &source).unwrap();
        let source = capture_source(
            std::slice::from_ref(&options.file),
            root,
            ReadMode::Frozen,
            None,
        )
        .unwrap();
        (project, source, bundle, Files::new(), revision)
    }

    #[test]
    fn competing_pending_tip_is_retried_without_overwrite() {
        let (_temp, root, options) = fixture();
        let (project, source, bundle, files, revision) = prepared(&root, &options);
        let mut once = true;
        let value = capture_pending_with_hooks(
            &project,
            &bundle,
            &files,
            "event-fixed",
            &format!("import-{revision}"),
            &mut || source.verify(),
            &mut |_, old, candidate| {
                if once {
                    once = false;
                    let current = tree(&root, old)?;
                    let competing = write_commit(&root, old, &current, &BTreeMap::new())?;
                    assert!(update_ref(&root, old, &competing)?);
                    assert_ne!(candidate, competing);
                }
                Ok(())
            },
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(map(&value).unwrap()["sequence"], n(1).unwrap());
        let head = text(&map(&value).unwrap()["ledger_commit"]).unwrap();
        assert_eq!(
            git_text(&root, &["rev-list", "--count", head], &[], &[]).unwrap(),
            "2"
        );
        assert_eq!(Ledger::capture(&project).unwrap().events.len(), 1);
    }

    #[test]
    fn failure_after_cas_is_recovered_as_replay() {
        let (_temp, root, options) = fixture();
        let (project, source, bundle, files, revision) = prepared(&root, &options);
        let error = capture_pending_with_hooks(
            &project,
            &bundle,
            &files,
            "event-fixed",
            &format!("import-{revision}"),
            &mut || source.verify(),
            &mut |_, _, _| Ok(()),
            &mut || Err(Error("simulated_after_cas".into())),
        )
        .unwrap_err();
        assert_eq!(error.0, "simulated_after_cas");
        assert!(
            pending_state::resolve(&root, pending_state::REF)
                .unwrap()
                .is_some()
        );
        let replay = capture_pending(
            &project,
            &bundle,
            &files,
            "event-fixed",
            &format!("import-{revision}"),
            &mut || source.verify(),
        )
        .unwrap();
        assert_eq!(map(&replay).unwrap()["replay"], V::Bool(true));
    }

    #[test]
    fn evidence_change_before_cas_leaves_no_pending_ref() {
        let (_temp, root, mut options) = fixture();
        fs::write(
            &options.file,
            "sources:\n  s.vendor: {name: Vendor, file: proof.txt}\nknown:\n  fact.import: {v: 10, from: s.vendor, scope: {kind: external, environment: vendor}}\n",
        )
        .unwrap();
        let evidence = root.join("evidence");
        fs::create_dir(&evidence).unwrap();
        let proof = evidence.join("proof.txt");
        fs::write(&proof, "before\n").unwrap();
        options.evidence = Some(evidence);
        let error = import_with_hook(&root, &options, &mut || {
            fs::write(&proof, "after\n")?;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.0, "snapshot_changed");
        assert!(
            pending_state::resolve(&root, pending_state::REF)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn config_change_at_cas_leaves_no_pending_ref() {
        let (_temp, root, options) = fixture();
        let (project, source, bundle, files, revision) = prepared(&root, &options);
        let config = project.config_path.clone();
        let error = capture_pending_with_hooks(
            &project,
            &bundle,
            &files,
            "event-fixed",
            &format!("import-{revision}"),
            &mut || source.verify(),
            &mut |_, _, _| {
                fs::write(
                    &config,
                    b"{\"version\":1,\"mode\":\"advanced\",\"generation\":1}",
                )?;
                Ok(())
            },
            &mut || Ok(()),
        )
        .unwrap_err();
        assert!(
            error.0.contains("changed") || error.0.contains("config"),
            "{}",
            error.0
        );
        assert!(
            pending_state::resolve(&root, pending_state::REF)
                .unwrap()
                .is_none()
        );
    }
}

//! Recoverable publication of immutable pending contributions.
//!
//! Remote observations are pinned before use. Every push is protected by the
//! configured managed-head lease, and provider writes require explicit or
//! standing authority immediately before the operation.
use crate::{
    Error, Result,
    history_authority::Files,
    history_contract::{field, is_int, map, string_is, text},
    history_view::{list, map_mut},
    pending_control::{self, PublisherLock},
    pending_state::{self, Ledger},
    project_modes::Project,
    publication_provider::{GitHubProvider, Provider, PullRequest},
    require,
    value::{FiniteFloat, Integer, TypedValue as V},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_FAILURES: u64 = 5;
const TERMINAL: [&str; 3] = ["withdrawn", "rejected", "superseded"];

fn s(value: &str) -> V {
    V::Text(value.into())
}
fn n(value: u64) -> V {
    V::Integer(Integer::new(&value.to_string()).unwrap())
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
fn truth(value: Option<&V>) -> bool {
    matches!(value, Some(V::Bool(true)))
}
fn integer(value: &V) -> Result<u64> {
    match value {
        V::Integer(value) => value
            .as_str()
            .parse()
            .map_err(|_| Error("invalid_publication_state".into())),
        _ => Err(Error("invalid_publication_state".into())),
    }
}

#[derive(Debug)]
enum Failure {
    Attention(String),
    Unknown(String),
}
impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self::Attention(error.0)
    }
}
type PResult<T> = std::result::Result<T, Failure>;
type Reconciled = (
    Target,
    Option<String>,
    Option<PullRequest>,
    BTreeSet<String>,
);

fn command(
    root: &Path,
    args: &[&str],
    input: Vec<u8>,
    environment: &[(&str, &Path)],
) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    #[cfg(windows)]
    command.args(["-c", "core.longpaths=true"]);
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
    for (name, value) in environment {
        #[cfg(windows)]
        if *name == "GIT_INDEX_FILE" {
            let value = value.to_str().ok_or_else(|| Error("invalid git index path".into()))?;
            command.env(name, windows_index_path(value));
            continue;
        }
        command.env(name, value);
    }
    command.env("GIT_TERMINAL_PROMPT", "0");
    command.env("GIT_OPTIONAL_LOCKS", "0");
    crate::reasoning_runtime::run_command_bounded(
        &mut command,
        input,
        Duration::from_secs(30),
        64 * 1024 * 1024,
    )
}

#[cfg(any(windows, test))]
fn windows_index_path(path: &str) -> String {
    // Git accepts ordinary drive/UNC paths in GIT_INDEX_FILE, but not the
    // verbatim prefix returned by Windows canonicalize. Git adds its own long
    // path prefix when core.longpaths is enabled for this invocation.
    if let Some(path) = path.strip_prefix(r"\\?\UNC\") {
        format!("//{}", path.replace('\\', "/"))
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(path).replace('\\', "/")
    }
}

fn scope(project: &Project) -> Result<V> {
    let config = project.config()?;
    require(
        string_is(field(map(&config)?, "mode")?, "advanced"),
        "publication is inactive in Simple mode",
    )?;
    let scope = map(&config)?
        .get("publication")
        .filter(|value| **value != V::Null)
        .cloned()
        .ok_or_else(|| Error("publication destination is not configured".into()))?;
    let fields = map(&scope)?;
    let remote = text(field(fields, "remote")?)?;
    let repository = text(field(fields, "repository")?)?;
    let (success, urls) = pending_control::git(
        &project.root,
        &["remote", "get-url", "--push", "--all", remote],
    )?;
    let urls =
        String::from_utf8(urls).map_err(|_| Error("invalid publication destination".into()))?;
    require(
        success && !repository.starts_with('-') && urls.lines().eq([repository]),
        "configured push destination changed; reconcile the publication scope",
    )?;
    for key in ["target", "branch"] {
        require(
            pending_control::git(
                &project.root,
                &["check-ref-format", "--branch", text(field(fields, key)?)?],
            )?
            .0,
            "invalid configured publication branch",
        )?;
    }
    require(
        fields["target"] != fields["branch"],
        "managed branch cannot be the target",
    )?;
    crate::history_branch::portable_path(text(field(map(&config)?, "record")?)?)?;
    Ok(scope)
}

fn authority_check(project: &Project, expected: &V, authorized: bool) -> Result<()> {
    let current = scope(project)?;
    require(
        pending_state::scope_identity(&current)? == pending_state::scope_identity(expected)?
            && (authorized || truth(map(&current)?.get("standing_permission"))),
        "publication authority was revoked or its destination changed",
    )
}

fn merge_capabilities(graph: &mut V, incoming: &V, ledger: &Ledger, state: &V) -> Result<()> {
    let current = crate::reasoning_capabilities::document_capabilities(graph)?;
    let added = crate::reasoning_capabilities::document_capabilities(incoming)?;
    let current_fields = map(&current)?;
    let added_fields = map(&added)?;
    if current_fields["profile"] != added_fields["profile"] {
        require(
            string_is(&added_fields["profile"], "core/v1"),
            "pending_profile_reconciliation_required: legacy content cannot inherit the target profile",
        )?;
        require(
            crate::reasoning_authoring_preparation::promotion_blockers(graph, Some(graph))?
                .is_empty(),
            "requires explicit target migration: existing executable legacy fields",
        )?;
        let decisions = map(field(map(state)?, "decisions")?)?;
        for (revision, bundle) in &ledger.bundles {
            if decisions
                .get(revision)
                .and_then(|value| map(value).ok())
                .and_then(|value| value.get("state"))
                .is_some_and(|value| TERMINAL.iter().any(|name| string_is(value, name)))
            {
                continue;
            }
            let manifest = map(field(map(&bundle.value)?, "manifest")?)?;
            let other =
                crate::reasoning_capabilities::document_capabilities(field(manifest, "document")?)?;
            if map(&other)?["profile"] != added_fields["profile"]
                || map(&other)?["requires"] != added_fields["requires"]
            {
                return Err(Error(format!(
                    "pending_profile_reconciliation_required: {revision}"
                )));
            }
        }
    }
    if string_is(&added_fields["profile"], "core/v1") {
        let mut declaration = added.clone();
        if string_is(&current_fields["profile"], "core/v1") {
            let current_version = integer(field(current_fields, "version")?)?;
            let added_version = integer(field(added_fields, "version")?)?;
            map_mut(&mut declaration)?
                .insert("version".into(), n(current_version.max(added_version)));
            let mut requires = list(field(current_fields, "requires")?)?
                .iter()
                .chain(list(field(added_fields, "requires")?)?)
                .map(text)
                .collect::<Result<BTreeSet<_>>>()?
                .into_iter()
                .map(s)
                .collect::<Vec<_>>();
            requires.sort_by(|a, b| text(a).unwrap().cmp(text(b).unwrap()));
            map_mut(&mut declaration)?.insert("requires".into(), V::List(requires));
        }
        let metadata = map_mut(graph)?.entry("meta".into()).or_insert_with(empty);
        map_mut(metadata)?.insert("reasoning".into(), declaration);
    }
    Ok(())
}

#[derive(Clone)]
struct TreeEntry {
    mode: String,
    kind: String,
    oid: String,
}

#[derive(Clone)]
struct Target {
    revision: String,
    files: Files,
    document: V,
    history: Option<crate::history_capture::Capture>,
    /// A compact target, read only through its verified node capture.
    node: Option<crate::history_node_capture::Capture>,
}

struct TemporaryIndex(PathBuf);
impl Drop for TemporaryIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
        let mut lock = self.0.as_os_str().to_owned();
        lock.push(".lock");
        let _ = fs::remove_file(PathBuf::from(lock));
    }
}

pub struct Publisher<'a, P: Provider> {
    project: Project,
    provider: &'a mut P,
    clock: fn() -> f64,
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

impl<'a, P: Provider> Publisher<'a, P> {
    pub fn new(project: Project, provider: &'a mut P) -> Self {
        Self {
            project,
            provider,
            clock: now,
        }
    }
    pub fn with_clock(project: Project, provider: &'a mut P, clock: fn() -> f64) -> Self {
        Self {
            project,
            provider,
            clock,
        }
    }
    fn state_path(&self) -> PathBuf {
        self.project.state.join("publication.json")
    }
    fn save<K: Into<String>>(
        &self,
        state: &mut V,
        event: &str,
        details: impl IntoIterator<Item = (K, V)>,
    ) -> Result<()> {
        let mut receipt = object([
            ("event", s(event)),
            ("at", V::Float(FiniteFloat::new((self.clock)())?)),
        ]);
        map_mut(&mut receipt)?.extend(details.into_iter().map(|(key, value)| (key.into(), value)));
        let receipts = map_mut(state)?
            .get_mut("receipts")
            .ok_or_else(|| Error("invalid_publication_state".into()))?;
        let V::List(receipts) = receipts else {
            return Err(Error("invalid_publication_state".into()));
        };
        receipts.push(receipt);
        pending_control::save(&self.state_path(), state)
    }
    fn remote(&self, scope: &V) -> PResult<(String, Option<String>)> {
        let fields = map(scope)?;
        let repository = text(&fields["repository"])?;
        let target_branch = text(&fields["target"])?;
        let managed_branch = text(&fields["branch"])?;
        let output = command(
            &self.project.root,
            &[
                "ls-remote",
                "--refs",
                repository,
                &format!("refs/heads/{target_branch}"),
                &format!("refs/heads/{managed_branch}"),
            ],
            vec![],
            &[],
        )
        .map_err(|_| Failure::Unknown("remote heads are unavailable".into()))?;
        let mut refs = BTreeMap::new();
        for line in String::from_utf8(output)
            .map_err(|_| Failure::Unknown("remote heads are unavailable".into()))?
            .lines()
        {
            let mut parts = line.split_whitespace();
            if let (Some(oid), Some(reference), None) = (parts.next(), parts.next(), parts.next()) {
                refs.insert(reference.to_owned(), oid.to_owned());
            }
        }
        let target = refs
            .get(&format!("refs/heads/{target_branch}"))
            .cloned()
            .ok_or_else(|| Failure::Attention("configured target branch is unavailable".into()))?;
        let head = refs.get(&format!("refs/heads/{managed_branch}")).cloned();
        for (label, oid) in [("target", Some(&target)), ("head", head.as_ref())] {
            if let Some(oid) = oid {
                command(
                    &self.project.root,
                    &["fetch", "--no-tags", repository, oid],
                    vec![],
                    &[],
                )
                .and_then(|_| {
                    command(
                        &self.project.root,
                        &[
                            "update-ref",
                            &format!("refs/kpopper/publication/observed/{label}/{oid}"),
                            oid,
                        ],
                        vec![],
                        &[],
                    )
                })
                .map_err(|_| {
                    Failure::Unknown("remote observation changed or could not be fetched".into())
                })?;
            }
        }
        Ok((target, head))
    }

    fn tree(&self, revision: &str) -> Result<(BTreeMap<String, TreeEntry>, Files)> {
        let raw = command(
            &self.project.root,
            &["ls-tree", "-r", "-z", revision],
            vec![],
            &[],
        )?;
        let mut tree = BTreeMap::new();
        let mut files = Files::new();
        let mut total = 0usize;
        for row in raw.split(|byte| *byte == 0).filter(|row| !row.is_empty()) {
            let split = row
                .iter()
                .position(|byte| *byte == b'\t')
                .ok_or_else(|| Error("invalid target tree".into()))?;
            let info = std::str::from_utf8(&row[..split])
                .map_err(|_| Error("invalid target tree".into()))?
                .split_whitespace()
                .collect::<Vec<_>>();
            require(info.len() == 3, "invalid target tree")?;
            let path = std::str::from_utf8(&row[split + 1..])
                .map_err(|_| Error("invalid target path".into()))?;
            crate::history_branch::portable_path(path)?;
            let entry = TreeEntry {
                mode: info[0].into(),
                kind: info[1].into(),
                oid: info[2].into(),
            };
            if entry.kind == "blob" && matches!(entry.mode.as_str(), "100644" | "100755") {
                let bytes = command(
                    &self.project.root,
                    &["cat-file", "blob", &entry.oid],
                    vec![],
                    &[],
                )?;
                total = total.saturating_add(bytes.len());
                require(
                    bytes.len() <= 16 * 1024 * 1024 && total <= 64 * 1024 * 1024,
                    "target_limit",
                )?;
                pending_state::verify_blob(&entry.oid, &bytes)?;
                files.insert(path.into(), bytes);
            }
            tree.insert(path.into(), entry);
            require(tree.len() <= 100_000, "target_limit")?;
        }
        Ok((tree, files))
    }

    fn target(&self, revision: &str) -> Result<Target> {
        let (tree, files) = self.tree(revision)?;
        let config = self.project.config()?;
        let record = text(field(map(&config)?, "record")?)?;
        if let Some(entry) = tree.get(record) {
            require(
                entry.kind == "blob" && matches!(entry.mode.as_str(), "100644" | "100755"),
                &format!("record or evidence path is not a regular file: {record}"),
            )?;
        }
        let temp = tempfile::tempdir()?;
        for (path, raw) in &files {
            let target = crate::history_transaction_fs::target(temp.path(), path)?;
            fs::create_dir_all(target.parent().unwrap())?;
            fs::write(target, raw)?;
        }
        let entry = temp.path().join(record);
        if !entry.exists() {
            fs::create_dir_all(entry.parent().unwrap())?;
            fs::write(&entry, b"{}")?;
        }
        let captured = crate::source_capture::capture_source(
            &[entry],
            temp.path(),
            crate::source_capture::ReadMode::Frozen,
            None,
        )?;
        let history = captured.history_capture().cloned();
        let node = captured.node_history_capture().cloned();
        let document = if let Some(history) = &history {
            crate::history_adapter::from_store_capture(history)?
                .document()
                .clone()
        } else {
            captured.strict_document()?
        };
        Ok(Target {
            revision: revision.into(),
            files,
            document,
            history,
            node,
        })
    }

    fn evidence(&self, target: &Target, bundle: &pending_state::Bundle) -> Result<Files> {
        let config = self.project.config()?;
        let record = text(field(map(&config)?, "record")?)?;
        let parent = Path::new(record).parent().unwrap_or(Path::new(""));
        let manifest = map(field(map(&bundle.value)?, "manifest")?)?;
        Ok(map(field(manifest, "evidence")?)?
            .keys()
            .filter_map(|name| {
                target
                    .files
                    .get(&parent.join(name).to_string_lossy().replace('\\', "/"))
                    .map(|raw| (name.clone(), raw.clone()))
            })
            .collect())
    }

    fn accepted(&self, ledger: &Ledger, state: &V, target: &Target) -> Result<BTreeSet<String>> {
        let decisions = map(field(map(state)?, "decisions")?)?;
        let mut accepted = BTreeSet::new();
        for (revision, bundle) in &ledger.bundles {
            if decisions
                .get(revision)
                .and_then(|value| map(value).ok())
                .and_then(|value| value.get("state"))
                .is_some_and(|value| TERMINAL.iter().any(|state| string_is(value, state)))
            {
                continue;
            }
            let manifest = map(field(map(&bundle.value)?, "manifest")?)?;
            let version = field(manifest, "version")?;
            require(
                ["1", "2", "3", "4"].iter().any(|v| is_int(version, v)),
                "unsupported_contribution_version",
            )?;
            if let Some(node) = &target.node {
                let mut document = target.document.clone();
                if let Some(meta) = map_mut(&mut document)?.get_mut("meta")
                    && let Ok(meta) = map_mut(meta)
                {
                    meta.remove("history");
                }
                if crate::pending_bundle::equivalent_node(
                    &bundle.value,
                    &bundle.files,
                    node,
                    &document,
                    &self.evidence(target, bundle)?,
                )? {
                    accepted.insert(revision.clone());
                }
                continue;
            }
            let mut document = target.document.clone();
            let history = if is_int(version, "3") {
                target.history.as_ref()
            } else {
                if let Some(meta) = map_mut(&mut document)?.get_mut("meta")
                    && let Ok(meta) = map_mut(meta)
                {
                    meta.remove("history");
                }
                None
            };
            if crate::pending_bundle::equivalent(
                &bundle.value,
                &bundle.files,
                &document,
                &self.evidence(target, bundle)?,
                history,
            )? {
                accepted.insert(revision.clone());
            }
        }
        Ok(accepted)
    }

    fn provider_rows(&mut self, scope: &V) -> PResult<(Vec<PullRequest>, Option<PullRequest>)> {
        let rows = self
            .provider
            .list(&scope.to_json()?)
            .map_err(provider_failure)?;
        if rows.iter().any(|row| {
            !matches!(
                row.state.as_str(),
                "open" | "closed" | "merged" | "rejected"
            )
        }) {
            return Err(Failure::Unknown(
                "provider returned an incomplete publication state".into(),
            ));
        }
        let active = rows
            .iter()
            .filter(|row| row.state == "open")
            .cloned()
            .collect::<Vec<_>>();
        if active.len() > 1 {
            return Err(Failure::Attention(
                "multiple pull requests use the configured managed branch".into(),
            ));
        }
        if active
            .first()
            .is_some_and(|row| !row.body.contains(&marker(scope).unwrap_or_default()))
        {
            return Err(Failure::Attention(
                "the managed branch belongs to an unrecognized pull request".into(),
            ));
        }
        Ok((rows, active.into_iter().next()))
    }

    fn reconcile(&mut self, state: &mut V, ledger: &Ledger, scope: &V) -> PResult<Reconciled> {
        let (target_revision, head) = self.remote(scope)?;
        let target = self.target(&target_revision)?;
        let accepted = self.accepted(ledger, state, &target)?;
        let (rows, active) = self.provider_rows(scope)?;
        if let Some(intent) = map(state)?
            .get("intent")
            .filter(|value| **value != V::Null)
            .cloned()
        {
            let intent_fields = map(&intent)?;
            if string_is(field(intent_fields, "kind")?, "create") {
                let intended = text(field(intent_fields, "marker")?)?;
                let matches = rows
                    .iter()
                    .filter(|row| row.body.contains(intended))
                    .collect::<Vec<_>>();
                if matches.len() > 1 {
                    return Err(Failure::Attention(
                        "multiple pull requests match an uncertain creation receipt".into(),
                    ));
                }
                let Some(found) = matches.first() else {
                    return Err(Failure::Unknown(
                        "pull-request creation remains uncertain; no second create was sent".into(),
                    ));
                };
                let fields = map_mut(state)?;
                fields.insert("pr".into(), s(&found.id));
                fields.insert(
                    "proposed".into(),
                    field(intent_fields, "revisions")?.clone(),
                );
                fields.insert("intent".into(), V::Null);
                self.save(state, "create_reconciled", [("pr", s(&found.id))])?;
                if found.state != "open" {
                    return Err(Failure::Unknown(
                        "recovered a completed pull request; reconcile its contributions on the next run".into(),
                    ));
                }
            } else if string_is(field(intent_fields, "kind")?, "push") {
                let commit = text(field(intent_fields, "commit")?)?;
                let expected = intent_fields
                    .get("expected")
                    .and_then(|value| text(value).ok());
                if head.as_deref() == Some(commit) {
                    let fields = map_mut(state)?;
                    fields.insert("expected_head".into(), s(commit));
                    fields.insert(
                        "proposed".into(),
                        field(intent_fields, "revisions")?.clone(),
                    );
                    fields.insert("intent".into(), V::Null);
                    self.save(state, "push_reconciled", [("head", s(commit))])?;
                } else if head.as_deref() != expected {
                    return Err(Failure::Attention(
                        "remote branch changed during an uncertain push".into(),
                    ));
                }
            } else if string_is(field(intent_fields, "kind")?, "update") {
                let pr = field(intent_fields, "pr")?.clone();
                map_mut(state)?.insert("intent".into(), V::Null);
                self.save(state, "update_reconciled", [("pr", pr)])?;
            }
        }
        let prior_id = map(state)?
            .get("pr")
            .and_then(|value| text(value).ok())
            .map(str::to_owned);
        let previous = prior_id
            .as_ref()
            .and_then(|id| rows.iter().find(|row| &row.id == id));
        if prior_id.is_some() && previous.is_none() {
            return Err(Failure::Unknown(
                "previous pull request is missing from the complete provider listing".into(),
            ));
        }
        if active
            .as_ref()
            .is_some_and(|row| prior_id.as_deref() != Some(row.id.as_str()))
        {
            return Err(Failure::Attention(
                "another publisher owns the active pull request".into(),
            ));
        }
        if let Some(previous) =
            previous.filter(|row| matches!(row.state.as_str(), "closed" | "rejected"))
        {
            let proposed = list(field(map(state)?, "proposed")?)?
                .iter()
                .map(|value| text(value).map(str::to_owned))
                .collect::<Result<Vec<_>>>()?;
            for revision in proposed {
                if !accepted.contains(&revision)
                    && !map(field(map(state)?, "decisions")?)?.contains_key(&revision)
                {
                    map_mut(map_mut(state)?.get_mut("decisions").unwrap())?.insert(
                        revision,
                        object([
                            ("state", s(&previous.state)),
                            ("pr", s(&previous.id)),
                            (
                                "reason",
                                s(&format!("provider reported {}", previous.state)),
                            ),
                        ]),
                    );
                }
            }
        }
        let ended = previous
            .is_some_and(|row| matches!(row.state.as_str(), "merged" | "closed" | "rejected"));
        let expected_head = map(state)?
            .get("expected_head")
            .and_then(|value| text(value).ok());
        if head.as_deref() != expected_head && !(ended && head.is_none()) {
            return Err(Failure::Attention(
                "remote managed head changed; it was not overwritten".into(),
            ));
        }
        if ended {
            let fields = map_mut(state)?;
            fields.insert("pr".into(), V::Null);
            fields.insert(
                "expected_head".into(),
                head.clone().map(|value| s(&value)).unwrap_or(V::Null),
            );
            fields.insert("proposed".into(), V::List(vec![]));
        }
        let paused = truth(map(state)?.get("paused"));
        let proposed = list(field(map(state)?, "proposed")?)?
            .iter()
            .filter_map(|value| text(value).ok())
            .collect::<BTreeSet<_>>();
        let decisions = map(field(map(state)?, "decisions")?)?.clone();
        let states = ledger
            .bundles
            .keys()
            .map(|revision| {
                let decision = decisions
                    .get(revision)
                    .and_then(|value| map(value).ok())
                    .and_then(|value| value.get("state"))
                    .cloned();
                let value = if accepted.contains(revision) {
                    s("accepted")
                } else if let Some(value) = decision {
                    value
                } else if paused {
                    s("paused")
                } else if active.is_some() && proposed.contains(revision.as_str()) {
                    s("proposed")
                } else {
                    s("captured")
                };
                (revision.clone(), value)
            })
            .collect();
        let scope_id = pending_state::scope_identity(scope)?;
        let verified = object([
            ("target", s(&target_revision)),
            (
                "head",
                head.clone().map(|value| s(&value)).unwrap_or(V::Null),
            ),
            (
                "ledger_ref",
                ledger
                    .head
                    .clone()
                    .map(|value| s(&value))
                    .unwrap_or(V::Null),
            ),
            ("scope", s(&scope_id)),
            ("at", V::Float(FiniteFloat::new((self.clock)())?)),
        ]);
        let fields = map_mut(state)?;
        fields.insert("states".into(), V::Map(states));
        fields.insert("verified".into(), verified.clone());
        self.save(
            state,
            "reconciled",
            [
                ("target", s(&target_revision)),
                (
                    "head",
                    head.clone().map(|value| s(&value)).unwrap_or(V::Null),
                ),
                (
                    "ledger_ref",
                    ledger
                        .head
                        .clone()
                        .map(|value| s(&value))
                        .unwrap_or(V::Null),
                ),
                ("scope", s(&scope_id)),
                ("at", map(&verified)?["at"].clone()),
            ],
        )?;
        Ok((target, head, active, accepted))
    }

    fn status(
        &self,
        state: &V,
        _ledger: &Ledger,
        extra: impl IntoIterator<Item = (&'static str, V)>,
    ) -> Result<V> {
        let ledger = Ledger::capture(&self.project)?;
        let raw = pending_control::canonical_json(state)?;
        let mut status = pending_state::publication_status(Some(&raw), &ledger)?;
        map_mut(&mut status)?.extend(extra.into_iter().map(|(key, value)| (key.into(), value)));
        Ok(status)
    }

    pub fn verify(&mut self) -> Result<V> {
        let _publisher = PublisherLock::acquire(&self.project)?;
        let mut state = pending_control::load_state(&self.state_path())?;
        let ledger = Ledger::capture(&self.project)?;
        let config = self.project.config()?;
        let configured = map(&config)?
            .get("publication")
            .filter(|value| **value != V::Null);
        let scope_id = configured.map(pending_state::scope_identity).transpose()?;
        let stored_scope = map(&state)?.get("scope").and_then(|value| text(value).ok());
        require(
            stored_scope.is_none() || stored_scope == scope_id.as_deref(),
            "publication scope differs from its receipts",
        )?;
        let mut terminal = BTreeMap::new();
        for (revision, decision) in map(field(map(&state)?, "decisions")?)? {
            let decision = map(decision)?;
            if ledger.bundles.contains_key(revision)
                && decision
                    .get("state")
                    .is_some_and(|value| TERMINAL.iter().any(|name| string_is(value, name)))
                && !decision.contains_key("pr")
            {
                terminal.insert(revision.clone(), decision["state"].clone());
            }
        }
        let mut unresolved = ledger
            .bundles
            .keys()
            .filter(|revision| !terminal.contains_key(*revision))
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut target = V::Null;
        if !unresolved.is_empty()
            || map(&state)?
                .get("intent")
                .is_some_and(|value| *value != V::Null)
        {
            let scope = scope(&self.project)?;
            let (observed, _, _, _) = self
                .reconcile(&mut state, &ledger, &scope)
                .map_err(failure)?;
            let states = map(field(map(&state)?, "states")?)?;
            for (revision, value) in states {
                if string_is(value, "accepted")
                    || string_is(value, "closed")
                    || TERMINAL.iter().any(|name| string_is(value, name))
                {
                    terminal.insert(revision.clone(), value.clone());
                }
            }
            unresolved.retain(|revision| !terminal.contains_key(revision));
            target = s(&observed.revision);
        }
        Ok(object([
            ("verified", V::Bool(true)),
            (
                "ledger_ref",
                ledger
                    .head
                    .clone()
                    .map(|value| s(&value))
                    .unwrap_or(V::Null),
            ),
            ("scope", scope_id.map(|value| s(&value)).unwrap_or(V::Null)),
            ("generation", field(map(&config)?, "generation")?.clone()),
            ("target", target),
            ("terminal", V::Map(terminal)),
            (
                "unresolved",
                V::List(unresolved.into_iter().map(|value| s(&value)).collect()),
            ),
            ("checked_at", V::Float(FiniteFloat::new((self.clock)())?)),
        ]))
    }

    fn write_commit(&self, target: &str, additions: &Files) -> Result<String> {
        let index = tempfile::NamedTempFile::new_in(&self.project.state)?;
        let index_path = TemporaryIndex(index.path().to_owned());
        drop(index);
        let environment = [("GIT_INDEX_FILE", index_path.0.as_path())];
        command(
            &self.project.root,
            &["read-tree", target],
            vec![],
            &environment,
        )?;
        for (path, raw) in additions {
            let oid = String::from_utf8(command(
                &self.project.root,
                &["hash-object", "-w", "--stdin"],
                raw.clone(),
                &[],
            )?)
            .map_err(|_| Error("invalid git object identity".into()))?;
            command(
                &self.project.root,
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("100644,{},{}", oid.trim(), path),
                ],
                vec![],
                &environment,
            )?;
        }
        let tree = String::from_utf8(command(
            &self.project.root,
            &["write-tree"],
            vec![],
            &environment,
        )?)
        .map_err(|_| Error("invalid git tree identity".into()))?;
        let name = Path::new("Knowledge record");
        let email = Path::new("knowledge@localhost");
        let env = [
            ("GIT_INDEX_FILE", index_path.0.as_path()),
            ("GIT_AUTHOR_NAME", name),
            ("GIT_AUTHOR_EMAIL", email),
            ("GIT_COMMITTER_NAME", name),
            ("GIT_COMMITTER_EMAIL", email),
        ];
        let commit = String::from_utf8(command(
            &self.project.root,
            &["commit-tree", "--no-gpg-sign", tree.trim(), "-p", target],
            b"Propose pending project knowledge\n".to_vec(),
            &env,
        )?)
        .map_err(|_| Error("invalid git commit identity".into()))?;
        let commit = commit.trim().to_owned();
        command(
            &self.project.root,
            &[
                "update-ref",
                &format!("refs/kpopper/publication/prepared/{commit}"),
                &commit,
            ],
            vec![],
            &[],
        )?;
        Ok(commit)
    }

    fn prepare_commit(
        &self,
        target: &Target,
        ledger: &Ledger,
        state: &V,
        revisions: &[String],
    ) -> Result<String> {
        for revision in revisions {
            let version = map(&ledger.bundles[revision].value)
                .ok()
                .and_then(|value| value.get("manifest"))
                .and_then(|value| map(value).ok())
                .and_then(|value| value.get("version"))
                .cloned()
                .unwrap_or(V::Null);
            require(
                ["1", "2", "3"].iter().any(|v| is_int(&version, v)),
                if is_int(&version, "4") {
                    "scoped node contribution requires explicit adoption choices"
                } else {
                    "unsupported_contribution_version"
                },
            )?;
        }
        // A compact record's view is bound to its node storage; publication never
        // rewrites it as a document. Contributions land only by explicit adoption.
        require(
            target.node.is_none(),
            "compact target publication requires explicit adoption",
        )?;
        if target.history.is_some()
            || revisions.iter().any(|revision| {
                map(&ledger.bundles[revision].value)
                    .ok()
                    .and_then(|value| value.get("manifest"))
                    .and_then(|value| map(value).ok())
                    .and_then(|value| value.get("version"))
                    .is_some_and(|value| is_int(value, "3"))
            })
        {
            return self.prepare_history_commit(target, ledger, state, revisions);
        }
        let mut graph = target.document.clone();
        let mut current = crate::reasoning_snapshot::entries(&graph)?;
        let config = self.project.config()?;
        let record = text(field(map(&config)?, "record")?)?;
        let parent = Path::new(record).parent().unwrap_or(Path::new(""));
        let mut additions = Files::new();
        let decisions = map(field(map(state)?, "decisions")?)?;
        let mut replaceable: BTreeMap<String, Vec<&pending_state::Bundle>> = BTreeMap::new();
        for (old, decision) in decisions {
            let decision = map(decision)?;
            let Some(replacement) = decision
                .get("replacement")
                .and_then(|value| text(value).ok())
            else {
                continue;
            };
            if !string_is(field(decision, "state")?, "superseded")
                || !revisions.iter().any(|revision| revision == replacement)
            {
                continue;
            }
            let Some(prior) = ledger.bundles.get(old) else {
                continue;
            };
            if crate::pending_bundle::equivalent(
                &prior.value,
                &prior.files,
                &target.document,
                &self.evidence(target, prior)?,
                None,
            )? {
                replaceable
                    .entry(replacement.into())
                    .or_default()
                    .push(prior);
            }
        }
        for revision in revisions {
            let bundle = &ledger.bundles[revision];
            crate::pending_bundle::validate(&bundle.value, &bundle.files)?;
            let manifest = map(field(map(&bundle.value)?, "manifest")?)?;
            let incoming = field(manifest, "document")?;
            merge_capabilities(&mut graph, incoming, ledger, state)?;
            for (collection, members) in map(incoming)? {
                if matches!(members, V::Map(values) if values.is_empty())
                    && !["meta", "schema", "record", "also"].contains(&collection.as_str())
                {
                    map_mut(&mut graph)?
                        .entry(collection.clone())
                        .or_insert_with(empty);
                }
            }
            if let Some(schema) = map(incoming)?.get("schema") {
                if map(&graph)?
                    .get("schema")
                    .is_some_and(|value| value.digest().ok() != schema.digest().ok())
                {
                    return Err(Error(
                        "pending contribution conflicts with target schema".into(),
                    ));
                }
                map_mut(&mut graph)?.insert("schema".into(), schema.clone());
            }
            for (name, (collection, body)) in crate::reasoning_snapshot::entries(incoming)? {
                if let Some(existing) = current.get(&name)
                    && V::List(vec![s(&existing.0), existing.1.clone()]).digest()?
                        != V::List(vec![s(&collection), body.clone()]).digest()?
                    && !replaceable.get(revision).is_some_and(|priors| {
                        priors.iter().any(|prior| {
                            let Ok(manifest) = map(&prior.value)
                                .and_then(|value| field(value, "manifest"))
                                .and_then(map)
                            else {
                                return false;
                            };
                            let Ok(entries) =
                                crate::reasoning_snapshot::entries(&manifest["document"])
                            else {
                                return false;
                            };
                            entries.get(&name).is_some_and(|prior_value| {
                                V::List(vec![s(&prior_value.0), prior_value.1.clone()])
                                    .digest()
                                    .ok()
                                    == V::List(vec![s(&existing.0), existing.1.clone()])
                                        .digest()
                                        .ok()
                            })
                        })
                    })
                {
                    return Err(Error(format!(
                        "pending revisions conflict; explicitly reconcile entry {name}"
                    )));
                }
                let collection_map = map_mut(&mut graph)?
                    .entry(collection.clone())
                    .or_insert_with(empty);
                map_mut(collection_map)?.insert(name.clone(), body.clone());
                current.insert(name, (collection, body));
            }
            for (name, raw) in &bundle.files {
                let path = parent.join(name).to_string_lossy().replace('\\', "/");
                require(path != record, "evidence collides with the target record")?;
                if additions
                    .get(&path)
                    .or_else(|| target.files.get(&path))
                    .is_some_and(|value| value != raw)
                    && !replaceable.get(revision).is_some_and(|priors| {
                        additions
                            .get(&path)
                            .or_else(|| target.files.get(&path))
                            .is_some_and(|existing| {
                                priors
                                    .iter()
                                    .any(|prior| prior.files.get(name) == Some(existing))
                            })
                    })
                {
                    return Err(Error(format!(
                        "pending evidence conflicts with target file: {path}"
                    )));
                }
                additions.insert(path, raw.clone());
            }
        }
        additions.insert(record.into(), crate::history_emit::encode_document(&graph)?);
        for revision in revisions {
            let bundle = &ledger.bundles[revision];
            let evidence = self.proposal_evidence(&additions, target, bundle)?;
            if !crate::pending_bundle::equivalent(
                &bundle.value,
                &bundle.files,
                &graph,
                &evidence,
                None,
            )? {
                return Err(Error(
                    "combined graph changes a contribution meaning; reconcile before publishing"
                        .into(),
                ));
            }
        }
        for (revision, prior) in &ledger.bundles {
            if revisions.contains(revision)
                || decisions
                    .get(revision)
                    .and_then(|value| map(value).ok())
                    .and_then(|value| value.get("state"))
                    .is_some_and(|value| TERMINAL.iter().any(|name| string_is(value, name)))
            {
                continue;
            }
            let prior_evidence = self.evidence(target, prior)?;
            if crate::pending_bundle::equivalent(
                &prior.value,
                &prior.files,
                &target.document,
                &prior_evidence,
                None,
            )? && !crate::pending_bundle::equivalent(
                &prior.value,
                &prior.files,
                &graph,
                &self.proposal_evidence(&additions, target, prior)?,
                None,
            )? {
                return Err(Error("replacement would change another accepted contribution; reconcile its revision explicitly".into()));
            }
        }
        self.write_commit(&target.revision, &additions)
    }

    fn prepare_history_commit(
        &self,
        target: &Target,
        ledger: &Ledger,
        state: &V,
        revisions: &[String],
    ) -> Result<String> {
        let mut merged = target.history.clone().ok_or_else(|| {
            Error("history contribution requires explicit target history migration".into())
        })?;
        let config = self.project.config()?;
        let record = text(field(map(&config)?, "record")?)?;
        let parent = Path::new(record).parent().unwrap_or(Path::new(""));
        let mut additions = Files::new();
        let mut add = |path: String, raw: Vec<u8>| -> Result<()> {
            if additions
                .get(&path)
                .or_else(|| target.files.get(&path))
                .is_some_and(|existing| existing != &raw)
            {
                return Err(Error(format!(
                    "immutable history or evidence collision: {path}"
                )));
            }
            additions.insert(path, raw);
            Ok(())
        };
        for revision in revisions {
            let bundle = &ledger.bundles[revision];
            let manifest = map(field(map(&bundle.value)?, "manifest")?)?;
            require(
                is_int(field(manifest, "version")?, "3"),
                "history publication requires versioned history contributions",
            )?;
            let incoming =
                crate::pending_bundle::contribution_history(&bundle.value, &bundle.files)?;
            let binding = map(field(manifest, "history")?)?;
            let artifact = map(field(binding, "manifest")?)?;
            require(
                !is_int(field(artifact, "version")?, "2"),
                "scoped history contribution requires explicit adoption choices",
            )?;
            for (generation, evidence) in &incoming.inactive_generations {
                require(
                    merged
                        .inactive_generations
                        .get(generation)
                        .is_some_and(|current| current.digest == evidence.digest),
                    "retained generation evidence requires explicit target reconciliation",
                )?;
            }
            require(
                incoming.marker.digest()? == merged.marker.digest()?,
                "different history authority requires explicit adoption",
            )?;
            require(
                map(&incoming.state)?["rules"].digest()?
                    == map(&merged.state)?["rules"].digest()?,
                "history contribution rules disagree with target",
            )?;
            for (operation, raw) in &incoming.commits {
                if merged
                    .commits
                    .get(operation)
                    .is_some_and(|existing| existing != raw)
                {
                    return Err(Error(format!(
                        "immutable history operation collision: {operation}"
                    )));
                }
                merged.commits.insert(operation.clone(), raw.clone());
            }
            for (key, raw) in &incoming.object_bytes {
                if merged
                    .object_bytes
                    .get(key)
                    .is_some_and(|existing| existing != raw)
                {
                    return Err(Error(format!(
                        "immutable history object collision: {}",
                        key.1
                    )));
                }
                let path = incoming.object_paths.get(key).ok_or_else(|| {
                    Error(format!("invalid incoming history object path: {}", key.1))
                })?;
                require(
                    incoming.storage_bytes.get(path) == Some(raw),
                    &format!("invalid incoming history object path: {}", key.1),
                )?;
                if merged
                    .object_paths
                    .get(key)
                    .is_some_and(|existing| existing != path)
                {
                    return Err(Error(format!(
                        "incompatible immutable history object representation: {}",
                        key.1
                    )));
                }
                if merged
                    .object_paths
                    .iter()
                    .any(|(existing_key, existing)| existing == path && existing_key != key)
                {
                    return Err(Error(format!(
                        "incompatible immutable history storage path: {path}"
                    )));
                }
                if merged
                    .storage_bytes
                    .get(path)
                    .is_some_and(|existing| existing != &incoming.storage_bytes[path])
                {
                    return Err(Error(format!(
                        "immutable history storage collision: {path}"
                    )));
                }
                merged.object_bytes.insert(key.clone(), raw.clone());
                merged
                    .storage_bytes
                    .insert(path.clone(), incoming.storage_bytes[path].clone());
                merged.object_paths.insert(key.clone(), path.clone());
            }
            let archive = parent.join(".kpopper-contributions").join(revision);
            let manifest_bytes = serde_json::to_vec(&V::Map(manifest.clone()).to_tagged()?)?;
            add(
                archive
                    .join("manifest.json")
                    .to_string_lossy()
                    .replace('\\', "/"),
                manifest_bytes,
            )?;
            for (name, raw) in &bundle.files {
                add(
                    archive
                        .join("evidence")
                        .join(name)
                        .to_string_lossy()
                        .replace('\\', "/"),
                    raw.clone(),
                )?;
                if !name.starts_with("history-closure/") {
                    add(
                        parent.join(name).to_string_lossy().replace('\\', "/"),
                        raw.clone(),
                    )?;
                }
            }
        }
        merged.objects = crate::history_authority::committed_objects(
            &merged.marker,
            &merged.commits,
            &merged.object_bytes,
        )?;
        merged.state = crate::history_reduce::reduce_bytes(
            &merged
                .object_bytes
                .iter()
                .filter(|((_, id), _)| merged.objects.contains_key(id))
                .map(|(key, raw)| (key.clone(), raw.clone()))
                .collect(),
            Some(map(&map(&merged.state)?["rules"])?),
            None,
        )?;
        merged.baseline =
            crate::history_capture::baseline(&merged.marker, &merged.commits, &merged.state)?;
        let rendered = crate::history_view::render(
            &merged,
            &merged.objects,
            &merged.object_bytes,
            &merged.commits,
        )?;
        merged.entry_bytes = rendered.clone();
        merged.document = crate::history_yaml::decode_document(&rendered)?;
        let layout = crate::history_capture::Layout::for_entry(
            Path::new(record)
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| Error("invalid_path".into()))?,
        )?;
        for (operation, raw) in &merged.commits {
            add(
                parent
                    .join(&layout.commits)
                    .join(format!("{operation}.yaml"))
                    .to_string_lossy()
                    .replace('\\', "/"),
                raw.clone(),
            )?;
        }
        for (key, raw) in &merged.object_bytes {
            let path = merged
                .object_paths
                .get(key)
                .ok_or_else(|| Error("invalid merged history object path".into()))?;
            add(
                parent
                    .join(&layout.objects)
                    .join(path)
                    .to_string_lossy()
                    .replace('\\', "/"),
                raw.clone(),
            )?;
        }
        require(
            !additions.contains_key(record),
            "evidence collides with history target entry",
        )?;
        additions.insert(record.into(), rendered);
        let adapted = crate::history_adapter::from_store_capture(&merged)?
            .document()
            .clone();
        for revision in revisions {
            let bundle = &ledger.bundles[revision];
            if !crate::pending_bundle::equivalent(
                &bundle.value,
                &bundle.files,
                &adapted,
                &self.proposal_evidence(&additions, target, bundle)?,
                Some(&merged),
            )? {
                return Err(Error(
                    "combined history does not retain a complete contribution".into(),
                ));
            }
        }
        let decisions = map(field(map(state)?, "decisions")?)?;
        for (revision, prior) in &ledger.bundles {
            if revisions.contains(revision)
                || decisions
                    .get(revision)
                    .and_then(|value| map(value).ok())
                    .and_then(|value| value.get("state"))
                    .is_some_and(|value| TERMINAL.iter().any(|name| string_is(value, name)))
            {
                continue;
            }
            let mut before = target.document.clone();
            let manifest = map(field(map(&prior.value)?, "manifest")?)?;
            let prior_history = is_int(field(manifest, "version")?, "3");
            if !prior_history
                && let Some(meta) = map_mut(&mut before)?.get_mut("meta")
                && let Ok(meta) = map_mut(meta)
            {
                meta.remove("history");
            }
            if crate::pending_bundle::equivalent(
                &prior.value,
                &prior.files,
                &before,
                &self.evidence(target, prior)?,
                prior_history.then_some(target.history.as_ref().unwrap()),
            )? {
                let mut after = adapted.clone();
                if !prior_history
                    && let Some(meta) = map_mut(&mut after)?.get_mut("meta")
                    && let Ok(meta) = map_mut(meta)
                {
                    meta.remove("history");
                }
                if !crate::pending_bundle::equivalent(
                    &prior.value,
                    &prior.files,
                    &after,
                    &self.proposal_evidence(&additions, target, prior)?,
                    prior_history.then_some(&merged),
                )? {
                    return Err(Error(
                        "history union changes an accepted contribution".into(),
                    ));
                }
            }
        }
        self.write_commit(&target.revision, &additions)
    }

    fn proposal_evidence(
        &self,
        additions: &Files,
        target: &Target,
        bundle: &pending_state::Bundle,
    ) -> Result<Files> {
        let config = self.project.config()?;
        let record = text(field(map(&config)?, "record")?)?;
        let parent = Path::new(record).parent().unwrap_or(Path::new(""));
        Ok(bundle
            .files
            .keys()
            .filter_map(|name| {
                let path = parent.join(name).to_string_lossy().replace('\\', "/");
                additions
                    .get(&path)
                    .or_else(|| target.files.get(&path))
                    .map(|raw| (name.clone(), raw.clone()))
            })
            .collect())
    }

    fn push(&self, state: &mut V, scope: &V, intent: &V, authorized: bool) -> PResult<()> {
        authority_check(&self.project, scope, authorized)?;
        let intent_fields = map(intent)?;
        self.save(
            state,
            "push_intended",
            intent_fields
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        )?;
        let scope_fields = map(scope)?;
        let expected = intent_fields
            .get("expected")
            .and_then(|value| text(value).ok())
            .unwrap_or("");
        let commit = text(&intent_fields["commit"])?;
        let branch = text(&scope_fields["branch"])?;
        let repository = text(&scope_fields["repository"])?;
        command(
            &self.project.root,
            &[
                "push",
                "--porcelain",
                &format!("--force-with-lease=refs/heads/{branch}:{expected}"),
                repository,
                &format!("{commit}:refs/heads/{branch}"),
            ],
            vec![],
            &[],
        )
        .map_err(|_| {
            Failure::Unknown(
                "push result is unknown; the expected remote head will be reconciled".into(),
            )
        })?;
        let revisions = intent_fields["revisions"].clone();
        let fields = map_mut(state)?;
        fields.insert("expected_head".into(), s(commit));
        fields.insert("proposed".into(), revisions.clone());
        fields.insert("intent".into(), V::Null);
        self.save(
            state,
            "pushed",
            [("head", s(commit)), ("revisions", revisions)],
        )?;
        Ok(())
    }

    pub fn run(&mut self, authorized: bool, force_retry: bool) -> Result<V> {
        let lock = match PublisherLock::acquire(&self.project) {
            Ok(lock) => lock,
            Err(error) if error.0 == "another local publisher is active" => {
                return Ok(object([
                    ("outcome", s("busy")),
                    ("verified", V::Bool(false)),
                ]));
            }
            Err(error) => return Err(error),
        };
        let _lock = lock;
        self.run_cycle(authorized, force_retry, false)
    }

    /// Captures are the durable queue. A concurrent child waits for the current
    /// publisher, then reads the current ledger rather than dropping its wakeup.
    /// The owner also drains arrivals during its own bounded batch.
    pub fn run_automatic(&mut self) -> Result<V> {
        let deadline = Instant::now() + Duration::from_secs(60);
        let _lock = loop {
            match PublisherLock::acquire(&self.project) {
                Ok(lock) => break lock,
                Err(error) if error.0 == "another local publisher is active" => {
                    if Instant::now() >= deadline {
                        return Ok(object([("outcome", s("queued")), ("verified", V::Bool(false))]));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => return Err(error),
            }
        };
        for _ in 0..4 {
            let before = Ledger::capture(&self.project)?.head;
            let result = self.run_cycle(false, false, true)?;
            let outcome = map(&result)?.get("outcome");
            if outcome != Some(&s("proposed")) && outcome != Some(&s("idle"))
                || before == Ledger::capture(&self.project)?.head {
                return Ok(result);
            }
        }
        Ok(object([("outcome", s("queued")), ("verified", V::Bool(false))]))
    }

    fn run_cycle(&mut self, authorized: bool, force_retry: bool, automatic: bool) -> Result<V> {
        let mut state = pending_control::load_state(&self.state_path())?;
        let failures = integer(field(map(&state)?, "failures")?)?;
        let retry_at = match field(map(&state)?, "retry_at")? {
            V::Integer(value) => value.as_str().parse::<f64>().unwrap_or(0.0),
            V::Float(value) => value.get(),
            _ => 0.0,
        };
        let ledger = Ledger::capture(&self.project)?;
        if !force_retry && ((!automatic && failures >= MAX_FAILURES) || (self.clock)() < retry_at) {
            return self.status(&state, &ledger, [("outcome", s("backoff"))]);
        }
        if automatic {
            let config = self.project.config()?;
            let granted = map(&config)?.get("publication").and_then(|v| map(v).ok())
                .and_then(|v| v.get("standing_permission")) == Some(&V::Bool(true));
            if !granted || !string_is(&map(&config)?["mode"], "advanced") || truth(map(&state)?.get("paused")) {
                return self.status(&state, &ledger, [("outcome", s("idle"))]);
            }
        }
        let attempted = (|| -> PResult<V> {
            let scope = scope(&self.project)?;
            let scope_id = pending_state::scope_identity(&scope)?;
            let old_scope = map(&state)?.get("scope").filter(|value| **value != V::Null);
            if old_scope.is_some_and(|value| value != &s(&scope_id)) {
                return Err(Failure::Attention(
                    "publication receipts belong to another configured scope".into(),
                ));
            }
            map_mut(&mut state)?.insert("scope".into(), s(&scope_id));
            let (target, head, active, accepted) = self.reconcile(&mut state, &ledger, &scope)?;
            if let Some(intent) = map(&state)?
                .get("intent")
                .filter(|value| **value != V::Null)
                .cloned()
                && string_is(field(map(&intent)?, "kind")?, "push")
            {
                self.push(&mut state, &scope, &intent, authorized)?;
            }
            let decisions = map(field(map(&state)?, "decisions")?)?;
            let pending = ledger
                .bundles
                .keys()
                .filter(|revision| {
                    !accepted.contains(*revision) && !decisions.contains_key(*revision)
                })
                .cloned()
                .collect::<Vec<_>>();
            let proposed = list(field(map(&state)?, "proposed")?)?
                .iter()
                .filter_map(|value| text(value).ok())
                .collect::<BTreeSet<_>>();
            let remove_decided = active.is_some()
                && proposed
                    .iter()
                    .any(|revision| decisions.contains_key(*revision));
            if truth(map(&state)?.get("paused"))
                || pending.is_empty() && !remove_decided
                || !(authorized || truth(map(&scope)?.get("standing_permission")))
            {
                let fields = map_mut(&mut state)?;
                fields.insert("failures".into(), n(0));
                fields.insert("retry_at".into(), n(0));
                self.save(&mut state, "idle", std::iter::empty::<(&str, V)>())?;
                return self
                    .status(
                        &state,
                        &ledger,
                        [(
                            "outcome",
                            s(if truth(map(&state)?.get("paused")) {
                                "paused"
                            } else {
                                "idle"
                            }),
                        )],
                    )
                    .map_err(Into::into);
            }
            let changed = proposed != pending.iter().map(String::as_str).collect()
                || head.is_none()
                || active.is_none();
            if changed {
                let commit = self.prepare_commit(&target, &ledger, &state, &pending)?;
                let intent = object([
                    ("kind", s("push")),
                    ("commit", s(&commit)),
                    (
                        "expected",
                        head.clone().map(|value| s(&value)).unwrap_or(V::Null),
                    ),
                    (
                        "revisions",
                        V::List(pending.iter().map(|value| s(value)).collect()),
                    ),
                ]);
                map_mut(&mut state)?.insert("intent".into(), intent.clone());
                self.push(&mut state, &scope, &intent, authorized)?;
            }
            let marker = marker(&scope)?;
            let title = "kpopper Board: shared findings";
            let mut body = format!("{marker}\n\nPortable contributions for review:\n");
            for revision in &pending {
                body.push_str(&format!("- `{revision}`\n"));
            }
            authority_check(&self.project, &scope, authorized)?;
            let scope_json = scope.to_json()?;
            if let Some(active) = active {
                if let Some(cycle) = active
                    .body
                    .lines()
                    .find(|line| line.starts_with("<!-- cycle:"))
                {
                    body = body.replacen(&marker, &format!("{marker}\n{cycle}"), 1);
                }
                let intent = object([("kind", s("update")), ("pr", s(&active.id))]);
                map_mut(&mut state)?.insert("intent".into(), intent);
                self.save(&mut state, "update_intended", [("pr", s(&active.id))])?;
                let row = self
                    .provider
                    .update(&scope_json, &active.id, title, &body)
                    .map_err(provider_failure)?;
                if row.state != "open" {
                    return Err(Failure::Unknown(
                        "pull request ended during update; verify target acceptance".into(),
                    ));
                }
                map_mut(&mut state)?.insert("intent".into(), V::Null);
                self.save(&mut state, "updated", [("pr", s(&active.id))])?;
            } else {
                let cycle = integer(field(map(&state)?, "cycle")?)? + 1;
                let cycle_marker = format!("{marker}\n<!-- cycle:{cycle} -->");
                body = body.replacen(&marker, &cycle_marker, 1);
                let request_id = fresh_id()?;
                let intent = object([
                    ("kind", s("create")),
                    ("marker", s(&cycle_marker)),
                    (
                        "revisions",
                        V::List(pending.iter().map(|value| s(value)).collect()),
                    ),
                    ("request_id", s(&request_id)),
                ]);
                let fields = map_mut(&mut state)?;
                fields.insert("cycle".into(), n(cycle));
                fields.insert("intent".into(), intent.clone());
                self.save(
                    &mut state,
                    "create_intended",
                    [
                        ("kind", s("create")),
                        ("marker", s(&cycle_marker)),
                        (
                            "revisions",
                            V::List(pending.iter().map(|value| s(value)).collect()),
                        ),
                        ("request_id", s(&request_id)),
                    ],
                )?;
                let row = self
                    .provider
                    .create(&scope_json, title, &body, &request_id)
                    .map_err(provider_failure)?;
                if row.state != "open" {
                    return Err(Failure::Unknown(
                        "created pull request is already terminal; reconcile before proceeding"
                            .into(),
                    ));
                }
                let fields = map_mut(&mut state)?;
                fields.insert("pr".into(), s(&row.id));
                fields.insert("intent".into(), V::Null);
                self.save(&mut state, "created", [("pr", s(&row.id))])?;
            }
            for revision in &pending {
                map_mut(map_mut(&mut state)?.get_mut("states").unwrap())?
                    .insert(revision.clone(), s("proposed"));
            }
            let fields = map_mut(&mut state)?;
            fields.insert("failures".into(), n(0));
            fields.insert("retry_at".into(), n(0));
            self.save(
                &mut state,
                "attempt_complete",
                std::iter::empty::<(&str, V)>(),
            )?;
            self.status(&state, &ledger, [("outcome", s("proposed"))])
                .map_err(Into::into)
        })();
        match attempted {
            Ok(value) => Ok(value),
            Err(Failure::Attention(detail)) => {
                self.save(&mut state, "attention", [("detail", s(&detail))])?;
                self.status(
                    &state,
                    &ledger,
                    [("outcome", s("attention")), ("detail", s(&detail))],
                )
            }
            Err(Failure::Unknown(detail)) => {
                let failures = integer(field(map(&state)?, "failures")?)? + 1;
                let delay = if failures >= MAX_FAILURES {
                    300
                } else {
                    2_u64.pow(failures as u32).min(300)
                };
                let retry = (self.clock)() + delay as f64;
                let fields = map_mut(&mut state)?;
                fields.insert("failures".into(), n(failures));
                fields.insert("retry_at".into(), V::Float(FiniteFloat::new(retry)?));
                self.save(&mut state, "unknown", [("detail", s(&detail))])?;
                self.status(
                    &state,
                    &ledger,
                    [
                        (
                            "outcome",
                            s(if failures >= MAX_FAILURES {
                                "attention"
                            } else {
                                "unknown"
                            }),
                        ),
                        ("detail", s(&detail)),
                    ],
                )
            }
        }
    }
}

fn marker(scope: &V) -> Result<String> {
    Ok(format!(
        "<!-- kpopper-publication:{} -->",
        pending_state::scope_identity(scope)?
    ))
}

fn fresh_id() -> Result<String> {
    let token = tempfile::Builder::new()
        .prefix("kpop-publication-")
        .rand_bytes(24)
        .tempdir()?;
    Ok(crate::identity::sha256(token.path().as_os_str().as_encoded_bytes())[..32].into())
}

fn failure(error: Failure) -> Error {
    match error {
        Failure::Attention(value) | Failure::Unknown(value) => Error(value),
    }
}

fn provider_failure(error: Error) -> Failure {
    if error.0 == "configure a supported GitHub repository URL"
        || error.0 == "repository must identify one GitHub owner and repository"
    {
        Failure::Attention(error.0)
    } else {
        Failure::Unknown(error.0)
    }
}

pub fn publish(project: Project, authorized: bool, force_retry: bool) -> Result<V> {
    let config = project.config()?;
    let repository = map(&config)?
        .get("publication")
        .and_then(|value| map(value).ok())
        .and_then(|value| value.get("repository"))
        .and_then(|value| text(value).ok())
        .unwrap_or("");
    let mut provider = GitHubProvider::new(repository);
    Publisher::new(project, &mut provider).run(authorized, force_retry)
}

pub fn publish_automatic(project: Project) -> Result<V> {
    let config = project.config()?;
    let repository = map(&config)?.get("publication").and_then(|v| map(v).ok())
        .and_then(|v| v.get("repository")).and_then(|v| text(v).ok()).unwrap_or("");
    let mut provider = GitHubProvider::new(repository);
    Publisher::new(project, &mut provider).run_automatic()
}

/// Schedule one ordinary publication cycle under existing standing authority.
/// The child rechecks authority, remote heads and publisher state itself. This
/// creates no schedule and never waits for a network operation in the caller.
pub fn trigger_after_capture(project: &Project) -> V {
    match trigger_with_executable(project, std::env::current_exe, now) {
        Ok(result) => result,
        Err(error) => object([("started", V::Bool(false)), ("reason", s(&error.0))]),
    }
}

fn trigger_with_executable(
    project: &Project,
    executable: impl FnOnce() -> std::io::Result<PathBuf>,
    clock: fn() -> f64,
) -> Result<V> {
    let skipped = |reason: &str| object([("started", V::Bool(false)), ("reason", s(reason))]);
    let config = project.config()?;
    let fields = map(&config)?;
    let granted = fields.get("publication")
        .and_then(|v| map(v).ok())
        .and_then(|v| v.get("standing_permission")) == Some(&V::Bool(true));
    if !project.is_git() || !string_is(&fields["mode"], "advanced") || !granted {
        return Ok(skipped("no standing publication permission"));
    }
    let state = pending_control::load_state(&project.state.join("publication.json"))?;
    let fields = map(&state)?;
    let retry_at = match field(fields, "retry_at")? {
        V::Integer(value) => value.as_str().parse::<f64>().unwrap_or(0.0),
        V::Float(value) => value.get(),
        _ => return Err(Error("invalid publication retry time".into())),
    };
    if truth(fields.get("paused")) || clock() < retry_at {
        return Ok(skipped("paused or bounded backoff"));
    }
    if pending_state::resolve(&project.root, pending_state::REF)?.is_none() {
        return Ok(skipped("no captured contributions"));
    }
    let mut command = Command::new(executable()?);
    command.arg("--workspace").arg(&project.root).args(["pending", "_publish"])
        .current_dir(&project.root)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
        command.creation_flags(0x00000008 | 0x00000200);
    }
    let mut child = command.spawn()?;
    let pid = child.id();
    // Reap in long-lived library callers; CLI exit does not wait on this thread.
    std::thread::spawn(move || { let _ = child.wait(); });
    Ok(object([("started", V::Bool(true)), ("pid", n(pid.into()))]))
}

pub fn verify(project: Project) -> Result<V> {
    let config = project.config()?;
    let repository = map(&config)?
        .get("publication")
        .and_then(|value| map(value).ok())
        .and_then(|value| value.get("repository"))
        .and_then(|value| text(value).ok())
        .unwrap_or("");
    let mut provider = GitHubProvider::new(repository);
    Publisher::new(project, &mut provider).verify()
}

#[cfg(test)]
mod index_path_tests {
    use super::*;

    #[test]
    fn windows_git_index_paths_preserve_drive_and_unc_locations() {
        assert_eq!(windows_index_path(r"\\?\C:\work\index"), "C:/work/index");
        assert_eq!(windows_index_path(r"\\?\UNC\server\share\index"), "//server/share/index");
        assert_eq!(windows_index_path(r"C:\work\index"), "C:/work/index");
    }
}

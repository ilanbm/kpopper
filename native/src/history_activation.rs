//! Guarded same-record history authority lifecycle. A retained mutation is only
//! evidence; the caller must hold deployment exclusion throughout each operation.
use crate::{
    Result, history_activation_verify as Verify,
    history_authoring::{empty, n, obj, s, strings},
    history_authority as A, history_capture as H,
    history_contract::*,
    history_emit as E,
    history_migration::{self as M, Plan},
    history_migration_source as MS,
    history_transaction::{self as T, FileImage, PreparedMutation},
    history_transaction_fs as FS,
    history_view::{list, map_mut},
    history_yaml as Y,
    identity::sha256,
    pending_state::Ledger,
    project_modes::{self as P, PolicyGuard, Project},
    reasoning_runtime::Runtime,
    require,
    source_capture::{self as Source, ReadMode},
    source_inventory::{Inventory, Observation, name},
    value::TypedValue as V,
};
use serde_json::Value as J;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Duration,
};
pub const KIND: &str = "history-authority-transition/v1";

/// Implemented by a deployment owner that excludes every managed writer and
/// replacement of the selected launcher sources until the returned guard drops.
pub trait Deployment {
    fn exclude<'a>(
        &'a self,
        inventory: &J,
        expected_digests: &J,
    ) -> Result<Box<dyn DeploymentGuard + 'a>>;
}
pub trait DeploymentGuard {
    fn verify(&self) -> Result<()>;
}
#[derive(Clone)]
pub struct Selection {
    pub inventory: J,
    pub expected_digests: J,
}
#[derive(Clone)]
pub struct Options {
    pub operation: String,
    pub recorded_at: String,
    pub record_id: Option<String>,
}
pub(crate) fn at<'a>(v: &'a V, key: &str) -> Result<&'a V> {
    map(v)?
        .get(key)
        .ok_or_else(|| error("invalid_authority_transition"))
}
pub(crate) fn base(m: &PreparedMutation) -> Result<V> {
    Ok(at(&m.to_data(), "baseline")?.clone())
}
pub(crate) fn selected(b: &V) -> Result<Selection> {
    let d = at(b, "deployment")?;
    Ok(Selection {
        inventory: at(d, "inventory")?.to_json()?,
        expected_digests: at(d, "expected_digests")?.to_json()?,
    })
}
pub(crate) fn file<'a>(m: &'a PreparedMutation, role: &str) -> Result<&'a FileImage> {
    let mut found = m.files().iter().filter(|i| i.role == role);
    let item = found
        .next()
        .ok_or_else(|| error("invalid_authority_transition"))?;
    require(found.next().is_none(), "invalid_authority_transition")?;
    Ok(item)
}
pub(crate) fn after(item: &FileImage) -> Result<&[u8]> {
    item.after
        .as_deref()
        .ok_or_else(|| error("invalid_authority_transition"))
}
pub(crate) fn layout(entry: &Path) -> Result<T::Layout> {
    T::Layout::for_entry(name(Path::new(
        entry.file_name().ok_or_else(|| error("invalid_path"))?,
    ))?)
}
pub(crate) fn members(entry: &Path) -> Result<Vec<PathBuf>> {
    crate::source_document::members(&[entry.to_path_buf()], &mut Inventory::default())
}
pub(crate) fn mutation_members(entry: &Path, mutation: &PreparedMutation) -> Result<Vec<PathBuf>> {
    map(at(&base(mutation)?, "record_members")?)?
        .keys()
        .map(|p| FS::target(entry.parent().unwrap(), p))
        .collect()
}
pub(crate) fn project_state(entry: &Path, project: &Project) -> Result<V> {
    let roots = project
        .worktrees()?
        .iter()
        .map(|p| P::resolved(p))
        .collect::<Result<BTreeSet<_>>>()?;
    let records = match entry.strip_prefix(&project.root) {
        Ok(relative) => roots
            .iter()
            .map(|root| P::resolved(&root.join(relative)))
            .collect::<Result<BTreeSet<_>>>()?,
        Err(_) => BTreeSet::from([entry.into()]),
    };
    // Non-Git is the only case with no ledger. Git always reads the actual ref,
    // immutable objects and bundle closure, including in Simple mode.
    let ledger = if project.is_git() {
        Ledger::capture(project)?.portable()
    } else {
        obj([
            ("ref", V::Null),
            ("events", V::List(vec![])),
            ("bundles", empty()),
        ])
    };
    let publication = crate::pending_state::read_private(&project.state.join("publication.json"))?;
    Ok(obj([
        ("root", s(name(&project.root)?)),
        (
            "worktrees",
            strings(
                roots
                    .iter()
                    .map(|p| name(p).map(str::to_owned))
                    .collect::<Result<Vec<_>>>()?,
            ),
        ),
        (
            "records",
            strings(
                records
                    .iter()
                    .map(|p| name(p).map(str::to_owned))
                    .collect::<Result<Vec<_>>>()?,
            ),
        ),
        ("config", project.config()?),
        (
            "pending",
            obj([
                ("ledger", ledger),
                ("publication", T::blob(publication.as_deref())),
            ]),
        ),
        (
            "observation",
            Source::routing_observation(&[entry.into()], &project.cwd)?,
        ),
    ]))
}

/// Private live lifecycle: owns exclusion, policy locks and sorted directory locks.
/// It cannot be constructed or deserialized by an external caller or transferred
/// to a detached replay thread (DirectoryGuard is thread-bound).
pub(crate) struct Guard<'a> {
    pub(crate) projects: BTreeMap<PathBuf, Project>,
    selection: Selection,
    receipt: Option<V>,
    directories: BTreeSet<PathBuf>,
    locks: Vec<FS::DirectoryGuard>,
    policies: Vec<PolicyGuard>,
    deployment: Box<dyn DeploymentGuard + 'a>,
}
impl<'a> Guard<'a> {
    pub(crate) fn open(
        entries: &[PathBuf],
        member_paths: &[PathBuf],
        selection: &Selection,
        deployment: &'a dyn Deployment,
        group_operation: Option<&str>,
    ) -> Result<Self> {
        let projects = entries
            .iter()
            .map(|entry| {
                Ok((
                    entry.clone(),
                    P::project_for(std::slice::from_ref(entry), entry.parent().unwrap())?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        if group_operation.is_some() {
            require(
                (2..=32).contains(&entries.len()) && projects.len() == entries.len(),
                "invalid_group_entries",
            )?;
            require(
                entries
                    .iter()
                    .map(|e| e.parent())
                    .collect::<BTreeSet<_>>()
                    .len()
                    == entries.len(),
                "overlapping_group_entries",
            )?;
            require(
                projects.values().all(Project::is_git)
                    && projects
                        .values()
                        .map(|p| &p.common)
                        .collect::<BTreeSet<_>>()
                        .len()
                        == 1,
                "group_requires_one_git_project",
            )?;
        } else {
            require(entries.len() == 1, "history_group_transition_required")?;
        }
        let excluded = deployment.exclude(&selection.inventory, &selection.expected_digests)?;
        excluded.verify()?;
        let mut policies = vec![];
        for project in projects
            .values()
            .map(|p| (p.state.clone(), p))
            .collect::<BTreeMap<_, _>>()
            .values()
        {
            policies.push(project.lock()?);
        }
        let mut directories = entries
            .iter()
            .chain(member_paths)
            .map(|p| P::resolved(p.parent().unwrap()))
            .collect::<Result<BTreeSet<_>>>()?;
        // The source reader also guards physical hypotheses. Include those
        // directories before taking any lock so an earlier worktree never tries
        // to acquire a new child lock behind a later participant's live lock.
        for entry in entries.iter().chain(member_paths) {
            let hypotheses = entry.parent().unwrap().join(layout(entry)?.hypotheses);
            if hypotheses.is_dir() {
                directories.insert(P::resolved(&hypotheses)?);
            }
        }
        for project in projects.values() {
            for root in project.worktrees()? {
                if root.is_dir() {
                    directories.insert(P::resolved(&root)?);
                }
            }
        }
        let locks = directories
            .iter()
            .map(|p| FS::DirectoryGuard::acquire(p, true))
            .collect::<Result<Vec<_>>>()?;
        let receipt = match group_operation {
            Some(op) => Some(obj([
                ("version", n("1")),
                ("operation", s(op)),
                (
                    "entries",
                    strings(
                        projects
                            .keys()
                            .map(|p| name(p).map(str::to_owned))
                            .collect::<Result<Vec<_>>>()?,
                    ),
                ),
            ])),
            None => None,
        };
        let guard = Self {
            projects,
            selection: selection.clone(),
            receipt,
            directories,
            locks,
            policies,
            deployment: excluded,
        };
        guard.verify()?;
        Ok(guard)
    }
    pub(crate) fn verify(&self) -> Result<()> {
        self.deployment.verify()?;
        for policy in &self.policies {
            policy.verify()?;
        }
        require(
            self.locks.len() == self.directories.len(),
            "group_locks_required",
        )
    }
    pub(crate) fn receipt(&self) -> Option<&V> {
        self.receipt.as_ref()
    }
    pub(crate) fn state(&self, entry: &Path) -> Result<V> {
        self.verify()?;
        let project = self
            .projects
            .get(entry)
            .ok_or_else(|| error("invalid_group_context"))?;
        let current = P::project_for(&[entry.into()], entry.parent().unwrap())?;
        require(
            current.root == project.root
                && current.common == project.common
                && current.state == project.state,
            "transition_project_changed",
        )?;
        let state = project_state(entry, project)?;
        let records = list(at(&state, "records")?)?;
        if let Some(receipt) = self.receipt() {
            require(
                at(receipt, "entries")? == at(&state, "records")?,
                "history_group_membership_mismatch",
            )?;
            require(
                at(at(&state, "observation")?, "record")? == &s(name(entry)?),
                "group_routing_mismatch",
            )?;
        } else {
            require(records.len() == 1, "history_group_transition_required")?;
        }
        Verify::pending(at(at(&state, "pending")?, "ledger")?)?;
        Ok(state)
    }
    pub(crate) fn check_selection(&self, b: &V) -> Result<()> {
        let selection = selected(b)?;
        require(
            selection.inventory == self.selection.inventory
                && selection.expected_digests == self.selection.expected_digests,
            "group_deployment_mismatch",
        )?;
        require(
            map(b)?.get("group") == self.receipt(),
            "group_recovery_required",
        )
    }
    pub(crate) fn check_members(&self, entry: &Path) -> Result<Vec<PathBuf>> {
        let members = members(entry)?;
        require(
            members
                .iter()
                .all(|p| self.directories.contains(p.parent().unwrap())),
            "group_member_locks_changed",
        )?;
        Ok(members)
    }
}
fn fresh_id(prefix: &str) -> Result<String> {
    let token = tempfile::Builder::new()
        .prefix("kpop-identity-")
        .rand_bytes(24)
        .tempdir()?;
    Ok(format!(
        "{prefix}-{}",
        sha256(name(token.path())?.as_bytes())
    ))
}
pub(crate) fn probe(selection: &Selection) -> Result<V> {
    let nonce = fresh_id("probe")?;
    let proof = crate::history_runtime::probe_launchers(
        &selection.inventory,
        &selection.expected_digests,
        &nonce,
        Duration::from_secs(10),
    )?;
    require(
        proof["complete"] == true
            && proof["launchers"].as_array().is_some_and(|a| {
                !a.is_empty()
                    && a.iter().all(|l| {
                        l["declaration"]["schemas"]["history"]["prepared_mutation"]
                            .as_array()
                            .is_some_and(|versions| versions.contains(&J::from(2)))
                    })
            }),
        "history_transition_runtime_unsupported",
    )?;
    for launcher in proof["launchers"].as_array().unwrap() {
        if launcher["declaration"]["version"] == 2 {
            crate::history_native_declaration::require_complete(&launcher["declaration"])?;
        }
    }
    V::from_json(&proof)
}
fn deployment(selection: &Selection) -> Result<V> {
    Ok(obj([
        ("inventory", V::from_json(&selection.inventory)?),
        (
            "expected_digests",
            V::from_json(&selection.expected_digests)?,
        ),
        ("prepared_probe", probe(selection)?),
        ("exclusion", s("caller_guard_required_through_publication")),
    ]))
}
pub(crate) fn isolated<T: Send>(call: impl FnOnce() -> Result<T> + Send) -> Result<T> {
    std::thread::scope(|scope| {
        scope
            .spawn(call)
            .join()
            .map_err(|_| error("transition_replay_failed"))?
    })
}
fn hashes(files: &A::Files) -> V {
    V::Map(
        files
            .iter()
            .map(|(p, b)| (p.clone(), s(&sha256(b))))
            .collect(),
    )
}
fn role(entry: &Path, relative: &str) -> Result<&'static str> {
    let l = layout(entry)?;
    Ok(if relative == l.entry {
        "record"
    } else if relative == l.authority {
        "history_authority"
    } else if relative.starts_with(&format!("{}/", l.commits)) {
        "history_commit"
    } else if relative.starts_with(&format!("{}/", l.objects)) {
        "history_object"
    } else {
        "history_retained"
    })
}
fn reads(inventory: &Inventory) -> Result<V> {
    Ok(V::List(
        inventory
            .events
            .iter()
            .map(|((kind, path), value)| {
                let value = match value {
                    Observation::Bytes(hash) => s(hash),
                    Observation::Unreadable(_) | Observation::File(_) => {
                        return Err(error("unsupported_transition_observation"));
                    }
                    Observation::Exists(b) | Observation::Directory(b) => V::Bool(*b),
                    Observation::Glob(paths) => strings(
                        paths
                            .iter()
                            .map(|p| name(p).map(str::to_owned))
                            .collect::<Result<Vec<_>>>()?,
                    ),
                };
                Ok(obj([
                    ("kind", s(kind)),
                    ("path", s(name(path)?)),
                    ("value", value),
                ]))
            })
            .collect::<Result<Vec<_>>>()?,
    ))
}
pub fn prepare_activation(
    entry: &Path,
    options: &Options,
    selection: &Selection,
    deployment_guard: &dyn Deployment,
    runtime: Option<&Runtime>,
) -> Result<PreparedMutation> {
    let entry = entry.canonicalize()?;
    let guard = Guard::open(
        std::slice::from_ref(&entry),
        &members(&entry)?,
        selection,
        deployment_guard,
        None,
    )?;
    prepare_guarded(&entry, options, selection, &guard, runtime)
}
pub(crate) fn prepare_guarded(
    entry: &Path,
    options: &Options,
    selection: &Selection,
    guard: &Guard<'_>,
    runtime: Option<&Runtime>,
) -> Result<PreparedMutation> {
    let state = guard.state(entry)?;
    let members = guard.check_members(entry)?;
    let root = entry.parent().unwrap();
    let l = layout(entry)?;
    require(
        FS::read(&FS::target(root, &l.journal)?)?.is_none(),
        "recovery_required",
    )?;
    let previous = FS::read(&FS::target(root, &l.authority)?)?
        .map(|b| Y::decode_document(&b))
        .transpose()?;
    if let Some(p) = &previous {
        A::validate_authority(p)?;
        require(
            string_is(at(p, "authority")?, "legacy"),
            "history_already_active",
        )?;
    }
    require(
        !root.join(M::ARTIFACTS).exists() || previous.is_some(),
        "existing_activation_evidence",
    )?;
    let record_id = options.record_id.clone().or_else(|| {
        previous
            .as_ref()
            .and_then(|v| at(v, "record_id").ok())
            .and_then(|v| text(v).ok())
            .map(str::to_owned)
    });
    let record_id = Some(match record_id {
        Some(id) => id,
        None => fresh_id("record")?,
    });
    let mode = if string_is(at(at(&state, "config")?, "mode")?, "advanced") {
        ReadMode::Live
    } else {
        ReadMode::Frozen
    };
    let plan = Plan::prepare(
        entry,
        root,
        M::Options {
            operation: options.operation.clone(),
            recorded_at: options.recorded_at.clone(),
            record_id,
            read_mode: mode,
            route: false,
            as_of: None,
        },
        runtime,
    )?;
    require(
        at(plan.manifest(), "complete")? == &V::Bool(true),
        "incomplete_history_import",
    )?;
    let marker = Y::decode_document(&plan.files()[&l.authority])?;
    let before = match previous {
        Some(p) => p,
        None => A::authority(
            text(at(&marker, "record_id")?)?,
            "legacy",
            &n("0"),
            Map::new(),
        )?,
    };
    let deploy = deployment(selection)?;
    isolated(|| {
        let temp = tempfile::tempdir()?;
        plan.publish(&temp.path().join("candidate")).map(|_| ())
    })?;
    let mut files = vec![];
    let mut retained = Map::new();
    for (relative, after) in plan.files() {
        let before = FS::read(&FS::target(root, relative)?)?;
        if before.as_ref() == Some(after) {
            continue;
        }
        let role = role(entry, relative)?;
        if role == "history_retained" {
            require(before.is_none(), "existing_activation_evidence")?;
            retained.insert(relative.clone(), s(&sha256(after)));
        }
        files.push(FileImage {
            path: relative.clone(),
            role: role.into(),
            before,
            after: Some(after.clone()),
        });
    }
    let commit = files
        .iter()
        .find(|i| i.role == "history_commit")
        .ok_or_else(|| error("invalid_authority_transition"))?;
    let manifest = Y::decode_document(after(commit)?)?;
    let source = plan
        .inventory()
        .files
        .iter()
        .map(|(path, raw)| {
            Ok((
                MS::posix(
                    path.strip_prefix(root)
                        .map_err(|_| error("transition_external_source"))?,
                )?,
                raw.clone(),
            ))
        })
        .collect::<Result<A::Files>>()?;
    let record_members = members
        .iter()
        .map(|p| {
            Ok((
                MS::posix(
                    p.strip_prefix(root)
                        .map_err(|_| error("transition_external_source"))?,
                )?,
                s(&sha256(&std::fs::read(p)?)),
            ))
        })
        .collect::<Result<Map>>()?;
    require(plan.members() == members, "transition_source_changed")?;
    let mut baseline = obj([
        ("kind", s(KIND)),
        ("direction", s("activate")),
        ("project", state.clone()),
        ("transaction_root", s(name(root)?)),
        ("deployment", deploy),
        (
            "history_baseline",
            M::empty_capture(root, &l.entry, &marker)?.baseline,
        ),
        ("record_members", V::Map(record_members)),
        ("retained_files", V::Map(retained)),
        ("source_files", hashes(&source)),
        ("source_storage", at(plan.manifest(), "originals")?.clone()),
        ("reads", reads(plan.inventory())?),
        ("managed_trees", Verify::trees(entry)?),
        (
            "activation",
            obj([
                ("operation", s(&options.operation)),
                ("recorded_at", s(&options.recorded_at)),
                ("record_id", at(&marker, "record_id")?.clone()),
                ("artifact_root", s(&plan.artifacts)),
                ("first_commit_sha256", s(&sha256(after(commit)?))),
            ]),
        ),
    ]);
    if let Some(receipt) = guard.receipt() {
        map_mut(&mut baseline)?.insert("group".into(), receipt.clone());
    }
    if let Some(observation) = map(plan.manifest())?.get("observation") {
        map_mut(&mut baseline)?.insert("live_observation".into(), observation.clone());
    }
    plan.verify_source()?;
    require(guard.state(entry)? == state, "transition_project_changed")?;
    let prepared = T::PreparedMutation::prepare(
        &options.operation,
        &before,
        &baseline,
        files,
        at(&manifest, "receipt")?,
        &l.entry,
        Some(&obj([("version", n("1")), ("after", marker)])),
    )?;
    Verify::candidate(entry, &prepared, runtime)?;
    Ok(prepared)
}
pub(crate) fn verify(
    entry: &Path,
    mutation: &PreparedMutation,
    guard: &Guard<'_>,
    runtime: Option<&Runtime>,
) -> Result<()> {
    let b = base(mutation)?;
    require(
        at(&mutation.to_data(), "entry")? == &s(&layout(entry)?.entry)
            && at(&b, "kind")? == &s(KIND)
            && at(&b, "transaction_root")? == &s(name(entry.parent().unwrap())?),
        "invalid_authority_transition",
    )?;
    guard.check_selection(&b)?;
    let state = guard.state(entry)?;
    require(&state == at(&b, "project")?, "transition_project_changed")?;
    Verify::read_witnesses(entry, mutation, None)?;
    Verify::candidate(entry, mutation, runtime)?;
    probe(&selected(&b)?)?;
    Verify::read_witnesses(entry, mutation, None)?;
    require(guard.state(entry)? == state, "transition_project_changed")
}
pub fn publish(
    entry: &Path,
    mutation: &PreparedMutation,
    deployment: &dyn Deployment,
    runtime: Option<&Runtime>,
) -> Result<V> {
    let entry = P::resolved(entry)?;
    let b = base(mutation)?;
    require(!map(&b)?.contains_key("group"), "group_recovery_required")?;
    let guard = Guard::open(
        std::slice::from_ref(&entry),
        &mutation_members(&entry, mutation)?,
        &selected(&b)?,
        deployment,
        None,
    )?;
    let l = layout(&entry)?;
    FS::publish_transition(
        entry.parent().unwrap(),
        &l.journal,
        mutation,
        &mut |_| verify(&entry, mutation, &guard, runtime),
        Some(&mut |_| {
            verify(&entry, mutation, &guard, runtime)
                .and_then(|()| Verify::terminal_images(&entry, mutation, FS::Direction::After))
                .map_err(|e| error(&format!("transition_unfinalized: {e}")))
        }),
    )?;
    Verify::live_result(&entry, mutation, runtime)?;
    let d = mutation.to_data();
    Ok(obj([
        (
            "state",
            s(if string_is(at(&b, "direction")?, "activate") {
                "activated"
            } else {
                "deactivated"
            }),
        ),
        ("operation", at(&d, "operation")?.clone()),
        ("authority", at(at(&d, "transition")?, "after")?.clone()),
    ]))
}

pub fn prepare_deactivation(
    entry: &Path,
    activation: &PreparedMutation,
    operation: &str,
    selection: &Selection,
    deployment: &dyn Deployment,
    runtime: Option<&Runtime>,
) -> Result<PreparedMutation> {
    let entry = entry.canonicalize()?;
    let guard = Guard::open(
        std::slice::from_ref(&entry),
        &mutation_members(&entry, activation)?,
        selection,
        deployment,
        None,
    )?;
    deactivate_guarded(&entry, activation, operation, selection, &guard, runtime)
}
pub(crate) fn deactivate_guarded(
    entry: &Path,
    activation: &PreparedMutation,
    operation: &str,
    selection: &Selection,
    guard: &Guard<'_>,
    runtime: Option<&Runtime>,
) -> Result<PreparedMutation> {
    let original = activation.to_data();
    let original_base = base(activation)?;
    require(
        at(&original_base, "direction")? == &s("activate"),
        "activation_receipt_required",
    )?;
    let state = guard.state(entry)?;
    require(
        &state == at(&original_base, "project")?,
        "transition_project_changed",
    )?;
    Verify::read_witnesses(entry, activation, None)?;
    Verify::candidate(entry, activation, runtime)?;
    let captured = H::capture(entry, None, None)?;
    let initial_operation = text(at(&original, "operation")?)?;
    require(
        &captured.marker == at(at(&original, "transition")?, "after")?
            && captured.commits.len() == 1
            && captured.commits.get(initial_operation).is_some_and(|b| {
                at(
                    at(&original_base, "activation").unwrap(),
                    "first_commit_sha256",
                )
                .ok()
                    == Some(&s(&sha256(b)))
            }),
        "newer_history_not_representable",
    )?;
    let original_entry = file(activation, "record")?
        .before
        .as_ref()
        .ok_or_else(|| error("transition_original_mismatch"))?;
    let activation_meta = at(&original_base, "activation")?;
    let artifacts = map(activation_meta)?
        .get("artifact_root")
        .map(text)
        .transpose()?
        .unwrap_or(M::ARTIFACTS);
    let original_snapshot = activation
        .files()
        .iter()
        .find(|i| i.path == format!("{artifacts}/original.json"))
        .ok_or_else(|| error("transition_original_mismatch"))?;
    let snapshot = crate::reasoning_snapshot::Snapshot::from_json(after(original_snapshot)?)?;
    let generation = at(&captured.marker, "generation")?;
    let V::Integer(generation) = generation else {
        return Err(error("invalid_generation"));
    };
    let generation = generation
        .as_str()
        .parse::<num_bigint::BigInt>()
        .map_err(|_| error("invalid_generation"))?
        + num_bigint::BigInt::from(1);
    let marker = A::authority(
        text(at(&captured.marker, "record_id")?)?,
        "legacy",
        &n(&generation.to_string()),
        map(&captured.marker)?
            .get("cancellations")
            .map(map)
            .transpose()?
            .cloned()
            .unwrap_or_default(),
    )?;
    let capabilities =
        crate::reasoning_fields::capabilities(at(&snapshot.to_data(), "document")?, None)?;
    let receipt = T::semantic_receipt(
        text(at(&capabilities, "profile")?)?,
        &capabilities,
        &obj([
            ("kind", s(KIND)),
            ("baseline", captured.baseline),
            ("activation_digest", at(&original, "digest")?.clone()),
        ]),
        &obj([
            ("kind", s("original-with-inactive-history/v1")),
            ("snapshot_id", s(snapshot.snapshot_id())),
        ]),
    )?;
    let l = layout(entry)?;
    let root = entry.parent().unwrap();
    let sources = map(at(&original_base, "source_files")?)?
        .keys()
        .filter(|p| **p != l.authority)
        .map(|p| {
            Ok((
                p.clone(),
                s(&sha256(
                    &FS::read(&FS::target(root, p)?)?
                        .ok_or_else(|| error("transition_source_changed"))?,
                )),
            ))
        })
        .collect::<Result<Map>>()?;
    let record_members = mutation_members(entry, activation)?
        .iter()
        .map(|p| {
            Ok((
                MS::posix(p.strip_prefix(root).map_err(|_| error("invalid_path"))?)?,
                s(&sha256(&std::fs::read(p)?)),
            ))
        })
        .collect::<Result<Map>>()?;
    let mut baseline = obj([
        ("kind", s(KIND)),
        ("direction", s("deactivate")),
        ("project", state.clone()),
        ("transaction_root", s(name(root)?)),
        ("deployment", deployment(selection)?),
        ("record_members", V::Map(record_members)),
        ("retained_files", empty()),
        ("source_files", V::Map(sources)),
        ("reads", at(&original_base, "reads")?.clone()),
        ("managed_trees", Verify::trees(entry)?),
        ("activation", activation_meta.clone()),
        (
            "original_snapshot",
            T::blob(Some(after(original_snapshot)?)),
        ),
        ("original_entry", T::blob(Some(original_entry))),
    ]);
    if let Some(receipt) = guard.receipt() {
        map_mut(&mut baseline)?.insert("group".into(), receipt.clone());
    }
    if let Some(observation) = map(&original_base)?.get("live_observation") {
        map_mut(&mut baseline)?.insert("live_observation".into(), observation.clone());
    }
    let files = vec![
        FileImage {
            path: l.entry.clone(),
            role: "record".into(),
            before: Some(captured.entry_bytes),
            after: Some(original_entry.clone()),
        },
        FileImage {
            path: l.authority,
            role: "history_authority".into(),
            before: Some(captured.authority_bytes),
            after: Some(E::encode_document(&marker)?),
        },
    ];
    require(guard.state(entry)? == state, "transition_project_changed")?;
    let prepared = PreparedMutation::prepare(
        operation,
        &captured.marker,
        &baseline,
        files,
        &receipt,
        &l.entry,
        Some(&obj([("version", n("1")), ("after", marker)])),
    )?;
    Verify::candidate(entry, &prepared, runtime)?;
    Ok(prepared)
}

pub(crate) fn verify_cancellation(
    entry: &Path,
    mutation: &PreparedMutation,
    guard: &Guard<'_>,
    plan: &crate::history_cancellation::CancellationPlan,
    completed: bool,
    runtime: Option<&Runtime>,
) -> Result<()> {
    require(
        plan == &crate::history_cancellation::cancellation_plan(entry, mutation)?,
        "cancellation_plan_mismatch",
    )?;
    let b = base(mutation)?;
    guard.check_selection(&b)?;
    require(
        at(&b, "transaction_root")? == &s(name(entry.parent().unwrap())?),
        "invalid_authority_transition",
    )?;
    let state = guard.state(entry)?;
    require(&state == at(&b, "project")?, "transition_project_changed")?;
    Verify::read_witnesses(entry, mutation, Some(plan))?;
    Verify::candidate(entry, mutation, runtime)?;
    probe(&selected(&b)?)?;
    let root = entry.parent().unwrap();
    let receipt = FS::read(&FS::target(root, &plan.receipt_path)?)?;
    require(
        receipt.is_none() || receipt.as_ref() == Some(&plan.receipt_bytes),
        "cancellation_receipt_collision",
    )?;
    for item in mutation.files() {
        let raw = FS::read(&FS::target(root, &item.path)?)?;
        let valid = match item.role.as_str() {
            "record" => {
                if completed {
                    raw == item.before
                } else {
                    raw == item.before || raw == item.after
                }
            }
            "history_authority" => {
                if completed {
                    raw.as_ref() == Some(&plan.marker_bytes)
                } else {
                    raw == item.before
                        || raw == item.after
                        || raw.as_ref() == Some(&plan.marker_bytes)
                }
            }
            _ => {
                if completed {
                    raw == item.after
                } else {
                    raw.is_none() || raw == item.after
                }
            }
        };
        require(valid, "cancellation_image_mismatch")?;
    }
    if completed
        || FS::read(&FS::target(root, &layout(entry)?.authority)?)?.as_ref()
            == Some(&plan.marker_bytes)
    {
        require(
            receipt.as_ref() == Some(&plan.receipt_bytes),
            "missing_cancellation_receipt",
        )?;
        let l = layout(entry)?;
        let load = |relative: &str, flat: bool| -> Result<A::Files> {
            let inventory = Verify::tree(root, relative)?;
            map(&inventory)?
                .keys()
                .map(|path| {
                    let local = path
                        .strip_prefix(&format!("{relative}/"))
                        .ok_or_else(|| error("invalid_path"))?;
                    require(
                        !flat || !local.contains('/'),
                        "cancellation_membership_mismatch",
                    )?;
                    require(local.ends_with(".yaml"), "cancellation_membership_mismatch")?;
                    Ok((
                        local.to_owned(),
                        FS::read(&FS::target(root, path)?)?
                            .ok_or_else(|| error("cancellation_image_mismatch"))?,
                    ))
                })
                .collect()
        };
        let commits = load(&l.commits, true)?
            .into_iter()
            .map(|(k, v)| (k.strip_suffix(".yaml").unwrap().to_owned(), v))
            .collect();
        let storage = load(&l.objects, false)?;
        let cancellations = load(&l.cancellations, true)?;
        let (objects, _) = A::objects_from_storage(&commits, &storage)?;
        A::committed_generations(
            &Y::decode_document(&plan.marker_bytes)?,
            &commits,
            &objects,
            &cancellations,
        )?;
        for (path, bytes) in &plan.immutable_images {
            require(
                FS::read(&FS::target(root, path)?)?.as_ref() == Some(bytes),
                "cancellation_immutable_mismatch",
            )?;
        }
    }
    Verify::read_witnesses(entry, mutation, Some(plan))?;
    require(guard.state(entry)? == state, "transition_project_changed")
}
pub(crate) fn apply_cancellation(
    entry: &Path,
    mutation: &PreparedMutation,
    plan: &crate::history_cancellation::CancellationPlan,
) -> Result<()> {
    require(
        plan == &crate::history_cancellation::cancellation_plan(entry, mutation)?,
        "cancellation_plan_mismatch",
    )?;
    let root = entry.parent().unwrap();
    for (path, raw) in &plan.immutable_images {
        FS::publish_immutable(root, path, raw)?;
    }
    FS::publish_immutable(root, &plan.receipt_path, &plan.receipt_bytes)?;
    FS::replace(entry, Some(&plan.original_entry))?;
    FS::replace(
        &FS::target(root, &layout(entry)?.authority)?,
        Some(&plan.marker_bytes),
    )
}
pub fn recover(
    entry: &Path,
    direction: FS::Direction,
    deployment: &dyn Deployment,
    runtime: Option<&Runtime>,
) -> Result<V> {
    let entry = P::resolved(entry)?;
    let l = layout(&entry)?;
    let root = entry.parent().unwrap();
    let raw =
        FS::read(&FS::target(root, &l.journal)?)?.ok_or_else(|| error("no_recovery_pending"))?;
    let mutation = PreparedMutation::from_bytes(&raw)?;
    let b = base(&mutation)?;
    require(!map(&b)?.contains_key("group"), "group_recovery_required")?;
    let guard = Guard::open(
        std::slice::from_ref(&entry),
        &mutation_members(&entry, &mutation)?,
        &selected(&b)?,
        deployment,
        None,
    )?;
    require(
        FS::read(&FS::target(root, &l.journal)?)?.as_ref() == Some(&raw),
        "concurrent_edit",
    )?;
    let mut result = obj([
        ("state", s("recovered")),
        (
            "direction",
            s(if direction == FS::Direction::After {
                "after"
            } else {
                "before"
            }),
        ),
        ("operation", at(&mutation.to_data(), "operation")?.clone()),
    ]);
    if direction == FS::Direction::Before && string_is(at(&b, "direction")?, "activate") {
        let plan = crate::history_cancellation::cancellation_plan(&entry, &mutation)?;
        verify_cancellation(&entry, &mutation, &guard, &plan, false, runtime)?;
        require(
            FS::read(&FS::target(root, &l.journal)?)?.as_ref() == Some(&raw),
            "concurrent_edit",
        )?;
        apply_cancellation(&entry, &mutation, &plan)?;
        verify_cancellation(&entry, &mutation, &guard, &plan, true, runtime)?;
        FS::finish_cancelled_transition(root, &l.journal, &mutation)?;
        map_mut(&mut result)?.extend(Map::from([
            ("authority".into(), Y::decode_document(&plan.marker_bytes)?),
            (
                "cancelled_generation".into(),
                at(at(&mutation.to_data(), "transition")?, "after")
                    .and_then(|m| at(m, "generation"))?
                    .clone(),
            ),
            ("cancellation_receipt".into(), s(&plan.receipt_path)),
        ]));
    } else {
        FS::recover_transition(
            root,
            &l.journal,
            direction,
            &mut |_| verify(&entry, &mutation, &guard, runtime),
            Some(&mut |_| {
                verify(&entry, &mutation, &guard, runtime)
                    .and_then(|()| Verify::terminal_images(&entry, &mutation, direction))
                    .map_err(|e| error(&format!("transition_unfinalized: {e}")))
            }),
        )?;
    }
    Verify::live_result(&entry, &mutation, runtime)?;
    Ok(result)
}

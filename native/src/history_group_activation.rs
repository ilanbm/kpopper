//! Explicit all-worktree lifecycle. Only a live private Guard owns exclusion;
//! GroupPrepared is a bounded retry envelope and grants no access or lock bypass.
use crate::{
    Result,
    history_activation::{self as A, Deployment, Guard, Selection, at, base, layout},
    history_activation_verify as Verify,
    history_authoring::{n, obj, s},
    history_cancellation::{self as C, CancellationPlan},
    history_contract::*,
    history_group::GroupPrepared,
    history_transaction_fs::{self as FS, Direction},
    identity::sha256,
    project_modes as P,
    reasoning_runtime::Runtime,
    require,
    source_inventory::name,
    value::TypedValue as V,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
fn entries(input: &[PathBuf]) -> Result<Vec<PathBuf>> {
    require((2..=32).contains(&input.len()), "invalid_group_entries")?;
    let mut entries = input
        .iter()
        .map(|p| P::resolved(p))
        .collect::<Result<Vec<_>>>()?;
    entries.sort();
    require(
        entries.windows(2).all(|w| w[0] != w[1]),
        "duplicate_group_entry",
    )?;
    require(
        entries
            .iter()
            .map(|p| p.parent())
            .collect::<BTreeSet<_>>()
            .len()
            == entries.len(),
        "overlapping_group_entries",
    )?;
    Ok(entries)
}
fn group_entries(group: &GroupPrepared) -> Vec<PathBuf> {
    group.mutations().keys().map(PathBuf::from).collect()
}
fn selection(group: &GroupPrepared) -> Result<Selection> {
    let data = group.to_data();
    Ok(Selection {
        inventory: at(&data, "inventory")?.to_json()?,
        expected_digests: at(&data, "expected_digests")?.to_json()?,
    })
}
fn guard<'a>(group: &GroupPrepared, deployment: &'a dyn Deployment) -> Result<Guard<'a>> {
    let members = group
        .mutations()
        .iter()
        .map(|(p, m)| A::mutation_members(Path::new(p), m))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    Guard::open(
        &group_entries(group),
        &members,
        &selection(group)?,
        deployment,
        Some(text(at(&group.to_data(), "operation")?)?),
    )
}
pub fn prepare(
    input: &[PathBuf],
    operation: &str,
    recorded_at: &str,
    selection: &Selection,
    deployment: &dyn Deployment,
    activations: Option<&GroupPrepared>,
    runtime: Option<&Runtime>,
) -> Result<GroupPrepared> {
    let entries = entries(input)?;
    token(&s(operation))?;
    if let Some(original) = activations {
        require(
            group_entries(original) == entries
                && string_is(at(&original.to_data(), "direction")?, "activate"),
            "group_activation_receipt_required",
        )?;
    }
    let mut members = vec![];
    for entry in &entries {
        members.extend(if let Some(original) = activations {
            A::mutation_members(entry, &original.mutations()[name(entry)?])?
        } else {
            A::members(entry)?
        });
    }
    let guard = Guard::open(&entries, &members, selection, deployment, Some(operation))?;
    let mut mutations = BTreeMap::new();
    for entry in &entries {
        let member_operation = format!(
            "group-member-{}",
            obj([("operation", s(operation)), ("entry", s(name(entry)?))]).digest()?
        );
        let mutation = if let Some(original) = activations {
            A::deactivate_guarded(
                entry,
                &original.mutations()[name(entry)?],
                &member_operation,
                selection,
                &guard,
                runtime,
            )?
        } else {
            A::prepare_guarded(
                entry,
                &A::Options {
                    operation: member_operation,
                    recorded_at: recorded_at.into(),
                    record_id: None,
                },
                selection,
                &guard,
                runtime,
            )?
        };
        mutations.insert(name(entry)?.to_owned(), mutation);
    }
    for (entry, mutation) in &mutations {
        A::verify(Path::new(entry), mutation, &guard, runtime)?;
    }
    GroupPrepared::prepare(
        operation,
        &entries
            .iter()
            .map(|p| name(p).map(str::to_owned))
            .collect::<Result<Vec<_>>>()?,
        &mutations,
        &V::from_json(&selection.inventory)?,
        &V::from_json(&selection.expected_digests)?,
    )
}
pub fn journal_for(group: &GroupPrepared) -> Result<PathBuf> {
    let (entry, mutation) = group
        .mutations()
        .first_key_value()
        .ok_or_else(|| error("invalid_group_journal"))?;
    let entry = Path::new(entry);
    let root = entry.parent().unwrap();
    let primary = FS::target(root, &layout(entry)?.journal)?;
    let coordinator = primary.parent().unwrap().join(format!(
        "group-{}.json",
        sha256(text(at(&group.to_data(), "operation")?)?.as_bytes())
    ));
    for path in [
        &coordinator,
        &coordinator.with_extension("ready"),
        &coordinator.with_extension("complete"),
        &coordinator.with_extension("cancel"),
    ] {
        FS::journal_path(
            root,
            name(path.strip_prefix(root).map_err(|_| error("invalid_path"))?)?,
            mutation,
        )?;
    }
    Ok(coordinator)
}
fn raw(value: &V) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&value.to_json()?)?)
}
fn publish_path(root: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    FS::publish_immutable(
        root,
        name(path.strip_prefix(root).map_err(|_| error("invalid_path"))?)?,
        bytes,
    )
}
fn first_root(group: &GroupPrepared) -> Result<&Path> {
    Ok(Path::new(
        group
            .mutations()
            .first_key_value()
            .ok_or_else(|| error("invalid_group_journal"))?
            .0,
    )
    .parent()
    .unwrap())
}
fn local_guards(group: &GroupPrepared) -> Result<BTreeMap<PathBuf, (PathBuf, Vec<u8>)>> {
    let coordinator = journal_for(group)?;
    let data = group.to_data();
    let mut guards = BTreeMap::new();
    for (entry, mutation) in group.mutations() {
        let path = Path::new(entry);
        let root = path.parent().unwrap();
        let journal = layout(path)?.journal;
        let primary = FS::journal_path(root, &journal, mutation)?;
        let guard = obj([
            ("version", n("1")),
            ("kind", s("history-authority-group-guard/v1")),
            ("operation", at(&data, "operation")?.clone()),
            ("digest", at(&data, "digest")?.clone()),
            ("entry", s(entry)),
            ("coordinator", s(name(&coordinator)?)),
        ]);
        require(
            guards
                .insert(primary, (root.into(), guard.canonical_bytes()?))
                .is_none(),
            "overlapping_group_guards",
        )?;
        for replica in FS::replicas(root, &journal, mutation)? {
            FS::journal_path(
                root,
                name(
                    replica
                        .strip_prefix(root)
                        .map_err(|_| error("invalid_path"))?,
                )?,
                mutation,
            )?;
            let raw =
                FS::member_guard(root, mutation, replica.parent().unwrap().parent().unwrap())?;
            require(
                guards.insert(replica, (root.into(), raw)).is_none(),
                "overlapping_group_guards",
            )?;
        }
    }
    Ok(guards)
}
fn ready_bytes(group: &GroupPrepared) -> Result<Vec<u8>> {
    raw(&obj([
        ("kind", s("history-authority-group-ready/v1")),
        ("digest", at(&group.to_data(), "digest")?.clone()),
        (
            "guards",
            V::Map(
                local_guards(group)?
                    .iter()
                    .map(|(p, (_, b))| Ok((name(p)?.to_owned(), s(&sha256(b)))))
                    .collect::<Result<Map>>()?,
            ),
        ),
    ]))
}
fn complete_bytes(group: &GroupPrepared, direction: Direction, phase: &str) -> Result<Vec<u8>> {
    raw(&obj([
        ("kind", s("history-authority-group-complete/v1")),
        ("digest", at(&group.to_data(), "digest")?.clone()),
        ("direction", s(direction_name(direction))),
        ("phase", s(phase)),
    ]))
}
fn direction_name(d: Direction) -> &'static str {
    if d == Direction::After {
        "after"
    } else {
        "before"
    }
}
fn validate_guards(group: &GroupPrepared, complete: bool) -> Result<()> {
    for (path, (_, expected)) in local_guards(group)? {
        let raw = FS::read(&path)?;
        require(
            raw.as_ref() == Some(&expected) || (complete && raw.is_none()),
            "group_guard_mismatch",
        )?;
    }
    Ok(())
}
fn install_guards(group: &GroupPrepared) -> Result<()> {
    for (path, (root, expected)) in local_guards(group)? {
        require(
            FS::read(&path)?.is_none_or(|b| b == expected),
            "group_guard_mismatch",
        )?;
        publish_path(&root, &path.parent().unwrap().join(".gitignore"), b"*\n")?;
        publish_path(&root, &path, &expected)?;
    }
    Ok(())
}
fn cleanup(group: &GroupPrepared) -> Result<()> {
    let coordinator = journal_for(group)?;
    let mut copies = local_guards(group)?.into_keys().collect::<Vec<_>>();
    copies.push(coordinator.with_extension("ready"));
    copies.push(coordinator.with_extension("cancel"));
    FS::remove_journals(&coordinator, &copies)
}
fn verify(group: &GroupPrepared, guard: &Guard<'_>, runtime: Option<&Runtime>) -> Result<()> {
    for (entry, mutation) in group.mutations() {
        A::verify(Path::new(entry), mutation, guard, runtime)?;
    }
    Ok(())
}
fn targets(group: &GroupPrepared, initial: bool) -> Result<Vec<Vec<PathBuf>>> {
    group
        .mutations()
        .iter()
        .map(|(entry, m)| {
            let paths = FS::transition_targets(Path::new(entry).parent().unwrap(), m)?;
            if initial {
                FS::mutable_before(m, &paths)?;
            }
            Ok(paths)
        })
        .collect()
}
fn final_images(group: &GroupPrepared, direction: Direction) -> Result<()> {
    targets(group, false)?;
    for (entry, mutation) in group.mutations() {
        Verify::terminal_images(Path::new(entry), mutation, direction)?;
    }
    Ok(())
}

fn plans(group: &GroupPrepared) -> Result<BTreeMap<String, CancellationPlan>> {
    require(
        string_is(at(&group.to_data(), "direction")?, "activate"),
        "invalid_group_cancellation",
    )?;
    group
        .mutations()
        .iter()
        .map(|(e, m)| Ok((e.clone(), C::cancellation_plan(Path::new(e), m)?)))
        .collect()
}
fn cancellation_bytes(
    group: &GroupPrepared,
    plans: &BTreeMap<String, CancellationPlan>,
) -> Result<Vec<u8>> {
    raw(&obj([
        ("version", n("1")),
        ("kind", s("history-authority-group-cancellation/v1")),
        ("digest", at(&group.to_data(), "digest")?.clone()),
        ("direction", s("before")),
        (
            "members",
            V::Map(
                plans
                    .iter()
                    .map(|(e, p)| {
                        (
                            e.clone(),
                            obj([
                                ("plan_digest", s(&p.digest)),
                                ("receipt_sha256", s(&sha256(&p.receipt_bytes))),
                            ]),
                        )
                    })
                    .collect(),
            ),
        ),
    ]))
}
fn verify_compensation(
    group: &GroupPrepared,
    guard: &Guard<'_>,
    plans: &BTreeMap<String, CancellationPlan>,
    completed: bool,
    runtime: Option<&Runtime>,
) -> Result<()> {
    for (entry, m) in group.mutations() {
        A::verify_cancellation(
            Path::new(entry),
            m,
            guard,
            &plans[entry],
            completed,
            runtime,
        )
        .map_err(|e| {
            error(&format!(
                "{}; member={entry}; preserve divergent bytes and retained journals",
                e.0
            ))
        })?;
    }
    Ok(())
}
fn compensate(
    group: &GroupPrepared,
    guard: &Guard<'_>,
    completed: bool,
    runtime: Option<&Runtime>,
) -> Result<()> {
    let coordinator = journal_for(group)?;
    let plans = plans(group)?;
    let phase = coordinator.with_extension("cancel");
    let expected = cancellation_bytes(group, &plans)?;
    require(
        FS::read(&phase)?.is_none_or(|raw| raw == expected),
        "group_cancellation_mismatch",
    )?;
    if !completed {
        require(
            FS::read(&coordinator.with_extension("ready"))?.as_ref() == Some(&ready_bytes(group)?),
            "invalid_group_ready_marker",
        )?;
        validate_guards(group, false)?;
        verify_compensation(group, guard, &plans, false, runtime)?;
        publish_path(first_root(group)?, &phase, &expected)?;
        for (entry, m) in group.mutations() {
            A::apply_cancellation(Path::new(entry), m, &plans[entry])?;
        }
    }
    verify_compensation(group, guard, &plans, true, runtime)
        .map_err(|e| error(&format!("transition_unfinalized: {e}")))?;
    if !completed {
        publish_path(
            first_root(group)?,
            &coordinator.with_extension("complete"),
            &complete_bytes(group, Direction::Before, "compensated")?,
        )?;
    }
    cleanup(group)
}
fn finish(
    group: &GroupPrepared,
    guard: &Guard<'_>,
    direction: Direction,
    runtime: Option<&Runtime>,
) -> Result<()> {
    if direction == Direction::Before && string_is(at(&group.to_data(), "direction")?, "activate") {
        return compensate(group, guard, false, runtime);
    }
    let all_targets = targets(group, false)?;
    for ((entry, m), paths) in group.mutations().iter().zip(all_targets) {
        FS::apply_transition(Path::new(entry).parent().unwrap(), m, &paths, direction)?;
    }
    verify(group, guard, runtime)
        .and_then(|()| final_images(group, direction))
        .map_err(|e| error(&format!("transition_unfinalized: {e}")))?;
    publish_path(
        first_root(group)?,
        &journal_for(group)?.with_extension("complete"),
        &complete_bytes(group, direction, "applied")?,
    )?;
    cleanup(group)
}
pub fn publish(
    group: &GroupPrepared,
    deployment: &dyn Deployment,
    runtime: Option<&Runtime>,
) -> Result<V> {
    let guard = guard(group, deployment)?;
    let coordinator = journal_for(group)?;
    let ready = coordinator.with_extension("ready");
    let completed = coordinator.with_extension("complete");
    if let Some(done) = FS::read(&completed)? {
        require(
            done == complete_bytes(group, Direction::After, "applied")?,
            "group_operation_collision",
        )?;
        verify(group, &guard, runtime)?;
        final_images(group, Direction::After)?;
        require(!coordinator.exists(), "group_recovery_required")?;
    } else {
        require(
            !coordinator.exists()
                && !ready.exists()
                && !coordinator.with_extension("cancel").exists(),
            "group_recovery_required",
        )?;
        for path in local_guards(group)?.keys() {
            require(!path.exists(), "recovery_required")?;
        }
        targets(group, true)?;
        verify(group, &guard, runtime)?;
        targets(group, true)?;
        publish_path(
            first_root(group)?,
            &coordinator.parent().unwrap().join(".gitignore"),
            b"*\n",
        )?;
        publish_path(first_root(group)?, &coordinator, &group.to_bytes()?)?;
        install_guards(group)?;
        validate_guards(group, false)?;
        targets(group, true)?;
        publish_path(first_root(group)?, &ready, &ready_bytes(group)?)?;
        finish(group, &guard, Direction::After, runtime)?;
    }
    for (entry, m) in group.mutations() {
        Verify::live_result(Path::new(entry), m, runtime)?;
    }
    let data = group.to_data();
    Ok(obj([
        (
            "state",
            s(if string_is(at(&data, "direction")?, "activate") {
                "activated"
            } else {
                "deactivated"
            }),
        ),
        ("operation", at(&data, "operation")?.clone()),
        ("entries", at(&data, "entries")?.clone()),
        ("journal", s(name(&coordinator)?)),
    ]))
}
fn cancel_unready(group: &GroupPrepared, guard: &Guard<'_>) -> Result<()> {
    for (path, (_, expected)) in local_guards(group)? {
        require(
            FS::read(&path)?.is_none_or(|raw| raw == expected),
            "group_guard_mismatch",
        )?;
    }
    for (entry, m) in group.mutations() {
        let entry = Path::new(entry);
        let root = entry.parent().unwrap();
        let guarded = FS::read(&FS::target(root, &layout(entry)?.journal)?)?.is_some();
        for item in m.files() {
            let current = FS::read(&FS::target(root, &item.path)?)?;
            if current != item.before {
                require(current != item.after, "incomplete_group_readiness")?;
                require(!guarded, "concurrent_edit")?;
            }
        }
        require(
            &guard.state(entry)? == at(&base(m)?, "project")?,
            "transition_project_changed",
        )?;
    }
    A::probe(&selection(group)?)?;
    // Recheck state and before/after images after the caller's launcher executes.
    for (entry, m) in group.mutations() {
        require(
            &guard.state(Path::new(entry))? == at(&base(m)?, "project")?,
            "transition_project_changed",
        )?;
        let root = Path::new(entry).parent().unwrap();
        let guarded = FS::read(&FS::target(root, &layout(Path::new(entry))?.journal)?)?.is_some();
        for item in m.files() {
            let current = FS::read(&FS::target(root, &item.path)?)?;
            if current != item.before {
                require(current != item.after, "incomplete_group_readiness")?;
                require(!guarded, "concurrent_edit")?;
            }
        }
    }
    publish_path(
        first_root(group)?,
        &journal_for(group)?.with_extension("complete"),
        &complete_bytes(group, Direction::Before, "cancelled-before-ready")?,
    )?;
    cleanup(group)
}
pub fn recover(
    journal: &Path,
    direction: Direction,
    deployment: &dyn Deployment,
    runtime: Option<&Runtime>,
) -> Result<V> {
    let journal = std::path::absolute(journal)?;
    let raw = FS::read(&journal)?.ok_or_else(|| error("no_recovery_pending"))?;
    let group = GroupPrepared::from_bytes(&raw)?;
    require(
        journal == journal_for(&group)?,
        "group_coordinator_mismatch",
    )?;
    let guard = guard(&group, deployment)?;
    require(
        FS::read(&journal)?.as_ref() == Some(&raw),
        "concurrent_edit",
    )?;
    let ready = journal.with_extension("ready");
    let done = FS::read(&journal.with_extension("complete"))?;
    let cancellation = FS::read(&journal.with_extension("cancel"))?;
    require(
        cancellation.is_none() || direction == Direction::Before,
        "group_cancellation_in_progress",
    )?;
    if let Some(done) = done {
        let cancelled = direction == Direction::Before
            && done == complete_bytes(&group, direction, "cancelled-before-ready")?;
        let compensated = direction == Direction::Before
            && done == complete_bytes(&group, direction, "compensated")?;
        require(
            cancelled || compensated || done == complete_bytes(&group, direction, "applied")?,
            "group_completion_direction_mismatch",
        )?;
        validate_guards(&group, true)?;
        if compensated {
            compensate(&group, &guard, true, runtime)?;
        } else if cancelled {
            cancel_unready(&group, &guard)?;
        } else {
            require(
                !(direction == Direction::Before
                    && string_is(at(&group.to_data(), "direction")?, "activate")),
                "group_compensation_required",
            )?;
            verify(&group, &guard, runtime)?;
            final_images(&group, direction)?;
            cleanup(&group)?;
        }
    } else {
        let proof = FS::read(&ready)?;
        require(
            proof
                .as_ref()
                .is_none_or(|raw| ready_bytes(&group).is_ok_and(|e| raw == &e)),
            "invalid_group_ready_marker",
        )?;
        if direction == Direction::Before
            && string_is(at(&group.to_data(), "direction")?, "activate")
            && (proof.is_some() || cancellation.is_some())
        {
            compensate(&group, &guard, false, runtime)?;
        } else if proof.is_none() && direction == Direction::Before {
            cancel_unready(&group, &guard)?;
        } else {
            if proof.is_none() {
                targets(&group, true)?;
                validate_guards(&group, true)?;
            } else {
                validate_guards(&group, false)?;
            }
            verify(&group, &guard, runtime)?;
            targets(&group, proof.is_none())?;
            install_guards(&group)?;
            publish_path(first_root(&group)?, &ready, &ready_bytes(&group)?)?;
            finish(&group, &guard, direction, runtime)?;
        }
    }
    let data = group.to_data();
    Ok(obj([
        ("state", s("recovered")),
        ("direction", s(direction_name(direction))),
        ("operation", at(&data, "operation")?.clone()),
        ("entries", at(&data, "entries")?.clone()),
    ]))
}

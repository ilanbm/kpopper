use super::{CommandOutput, Options};
use crate::{
    Result,
    history_contract::*,
    history_view::{list, map_mut},
    project_modes::WriteRoute,
    reasoning_runtime::Runtime,
    require,
    value::TypedValue as V,
};
use std::{collections::BTreeMap, path::PathBuf};

fn choices(values: &[String]) -> Result<V> {
    let mut out = Map::new();
    for value in values {
        let (subject, version) = value
            .rsplit_once('=')
            .ok_or_else(|| error("--choose takes SUBJECT=VERSION"))?;
        require(
            !subject.trim().is_empty() && crate::history_paths::object_id(version),
            "--choose takes SUBJECT=VERSION with an exact history version id",
        )?;
        require(
            out.insert(subject.into(), V::Text(version.into()))
                .is_none(),
            &format!("duplicate --choose subject: {subject}"),
        )?;
    }
    Ok(V::Map(out))
}
fn yaml(value: &V) -> Result<String> {
    Ok(
        String::from_utf8(crate::public_ordinary_readers::python_safe_dump(
            &crate::history_yaml::OrdinaryValue::from_typed(value),
        )?)
        .map_err(|_| error("invalid_branch_output"))?,
    )
}
fn evidence(
    store: &crate::history_store::Store,
    observations: &[crate::history_branch::Observation],
) -> Result<crate::history_authority::Files> {
    let mut files = crate::history_authority::Files::new();
    let mut total = 0usize;
    for observation in observations {
        let source = crate::history_branch::validate(&observation.envelope, &observation.files)?;
        for relative in map(&crate::history_branch_audit::evidence_requirements(
            observation,
            &source,
        )?)?
        .keys()
        {
            if files.contains_key(relative) {
                continue;
            }
            let path = crate::history_transaction_fs::target(&store.root, relative)?;
            let raw = crate::history_transaction_fs::read(&path)?
                .ok_or_else(|| error("branch_target_evidence_missing"))?;
            total = total.saturating_add(raw.len());
            require(
                raw.len() <= crate::reasoning_snapshot::MAX_REQUEST_BYTES
                    && total <= crate::history_branch::MAX_BYTES,
                "branch_target_evidence_limit",
            )?;
            files.insert(relative.clone(), raw);
        }
    }
    Ok(files)
}
pub(super) fn run(
    options: &Options,
    route: &WriteRoute,
    original: &[PathBuf],
    runtime: Option<&Runtime>,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<CommandOutput> {
    require(
        options.refute.is_none(),
        "--refute takes one hypothesis and its why, and nothing else",
    )?;
    let entry = &route.paths()[0];
    if !super::active_history(entry)? {
        return ordinary(options, route, runtime, probe);
    }
    require(
        options.names.is_empty() && options.take.is_empty() && options.drops.is_empty(),
        "history_branch_choices_required: --from cannot mix named hypotheses, --take or --drop; use --choose",
    )?;
    let store = crate::history_store::Store::new(entry)?;
    let _lock = crate::history_transaction_fs::DirectoryGuard::acquire(&store.root, true)?;
    let target = store.capture()?;
    let mut observations = Vec::new();
    let mut pinned = BTreeMap::new();
    for reference in &options.from_refs {
        let (_, root, relative, oid, _) = crate::public_readers::branch_read::context(
            route.paths(),
            &route.project().root,
            reference,
        )?;
        require(
            pinned.insert(oid.clone(), reference).is_none(),
            "duplicate_branch_source",
        )?;
        observations.push(crate::history_branch_git::capture(
            &root, &oid, &relative, None,
        )?);
    }
    let mut preview = if observations.len() == 1 {
        crate::history_branch_preview::preview(&target, &observations[0], None)?
    } else {
        crate::history_branch_preview::preview_set(&target, &observations, None)?
    };
    if options.dry_run {
        let fields = map_mut(&mut preview)?;
        fields.insert("state".into(), V::Text("prepared".into()));
        fields.insert(
            "admission".into(),
            V::Text("requires_explicit_choices".into()),
        );
        if observations.len() == 1 {
            fields.insert("source_ref".into(), V::Text(options.from_refs[0].clone()));
        } else {
            fields.insert(
                "source_refs".into(),
                V::List(options.from_refs.iter().cloned().map(V::Text).collect()),
            );
        }
        route.verify()?;
        return Ok(CommandOutput {
            stdout: format!(
                "BRANCH ADOPTION PREVIEW — no target acceptance\n{}",
                yaml(&preview)?
            ),
            stderr: String::new(),
            code: 0,
        });
    }
    let by = options
        .by
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| error("--by ACTOR is required for explicit history branch adoption"))?;
    if let Some(expected) = &options.source_revision {
        let key = if observations.len() == 1 {
            "source_revision"
        } else {
            "source_set_revision"
        };
        require(
            text(&map(&preview)?[key])? == expected,
            "branch_source_changed",
        )?;
    }
    let now = chrono::Utc::now();
    let settings = crate::history_branch_adoption::Options {
        operation: crate::public_history::fresh_id(if observations.len() == 1 {
            "branch-adopt"
        } else {
            "branch-adopt-set"
        })?,
        recorded_at: now.to_rfc3339(),
        by: by.into(),
    };
    let evidence = evidence(&store, &observations)?;
    let mutation = crate::history_branch_adoption::prepare(
        &target,
        &observations,
        &choices(&options.choices)?,
        &settings,
        &evidence,
    )?;
    route.verify()?;
    crate::direct_history::publish_prepared(&store, &mutation, route, original, runtime, probe)?;
    let operation = V::Text(settings.operation);
    let result = if observations.len() == 1 {
        let envelope = map(&observations[0].envelope)?;
        let source = map(&map(&envelope["manifest"])?["source"])?;
        V::Map(Map::from([
            ("state".into(), V::Text("adopted".into())),
            ("source_revision".into(), envelope["revision"].clone()),
            ("source_commit".into(), source["commit"].clone()),
            ("operation".into(), operation),
        ]))
    } else {
        let revisions = observations
            .iter()
            .map(|o| map(&o.envelope).map(|m| m["revision"].clone()))
            .collect::<Result<Vec<_>>>()?;
        let commits = observations
            .iter()
            .map(|o| Ok(map(&map(&o.envelope)?["manifest"])?["source"].clone()))
            .map(|r: Result<V>| r.and_then(|v| Ok(map(&v)?["commit"].clone())))
            .collect::<Result<Vec<_>>>()?;
        V::Map(Map::from([
            ("state".into(), V::Text("adopted".into())),
            (
                "source_set_revision".into(),
                map(&preview)?["source_set_revision"].clone(),
            ),
            ("source_revisions".into(), V::List(revisions)),
            ("source_commits".into(), V::List(commits)),
            ("operation".into(), operation),
        ]))
    };
    Ok(CommandOutput {
        stdout: format!("BRANCH ADOPTED\n{}", yaml(&result)?),
        stderr: String::new(),
        code: 0,
    })
}

fn ordinary(
    options: &Options,
    route: &WriteRoute,
    runtime: Option<&Runtime>,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<CommandOutput> {
    require(
        options.from_refs.len() == 1,
        "ordinary branch consolidation accepts one --from ref",
    )?;
    require(
        options.by.is_none() && options.choices.is_empty() && options.source_revision.is_none(),
        "--by, --choose and --source-revision require active-history --from",
    )?;
    let reference = &options.from_refs[0];
    let (_, root, relative, oid, day) = crate::public_readers::branch_read::context(
        route.paths(),
        &route.project().root,
        reference,
    )?;
    let captured = crate::source_target::records(&root, &relative, &oid, runtime)?;
    let data = map(&captured)?;
    let files = map(&data["files"])?;
    let raw = text(
        files
            .get(&relative)
            .ok_or_else(|| error("target_record_unavailable"))?,
    )?;
    let mut supplied = vec![crate::public_consolidation::ordinary::SuppliedHypothesis {
        name: reference.clone(),
        document: data["doc"].clone(),
        head: V::Map(Map::from([
            (
                "claim".into(),
                V::Text(format!(
                    "what {reference} committed ({}), read as a hypothesis",
                    &oid[..7]
                )),
            ),
            ("born".into(), V::Text(day)),
        ])),
        source: crate::history_yaml::decode_ordinary_source_value(raw.as_bytes())?,
        text: raw.into(),
    }];
    let layout = crate::history_transaction::Layout::for_entry(&relative)?;
    for item in list(&data["hypotheses"])? {
        let item = map(item)?;
        let name = text(&item["name"])?;
        let qualified = format!("{reference}:{name}");
        let suffixes = [
            format!("{}/{}.yaml", layout.hypotheses, name),
            format!("{}/{}.yml", layout.hypotheses, name),
        ];
        let raw = suffixes
            .iter()
            .find_map(|path| files.get(path))
            .and_then(|v| text(v).ok())
            .unwrap_or("");
        supplied.push(crate::public_consolidation::ordinary::SuppliedHypothesis {
            name: qualified,
            document: item["doc"].clone(),
            head: item["head"].clone(),
            source: crate::history_yaml::OrdinaryValue::from_typed(&item["doc"]),
            text: raw.into(),
        });
    }
    let mut selected = options.clone();
    selected
        .names
        .extend(supplied.iter().map(|h| h.name.clone()));
    crate::public_consolidation::ordinary::run_supplied(&selected, route, &supplied, runtime, probe)
}

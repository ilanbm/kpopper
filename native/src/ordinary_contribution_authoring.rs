//! Privacy-first routing for explicit ordinary `add`/`set` contributions.
use super::Options;
use crate::{
    Error, Result,
    history_authority::Files,
    history_contract::{Map, field, map, text, token},
    history_paths::Scheme,
    history_view::map_mut,
    history_yaml::SourceValue,
    legacy_authoring::{self, Preparation},
    pending_bundle,
    project_modes::WriteRoute,
    recording_privacy as Privacy, require,
    source_inventory::Inventory,
    value::TypedValue as V,
};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const MAX_EVIDENCE_BYTES: usize = 64 * 1024 * 1024;
const NODE_CONTRIBUTION: &str = "node_history_contribution_unsupported: a project contribution from compact history needs a versioned compact contribution format; nothing was captured or changed";

pub(crate) enum Outcome {
    Local(V),
    Handled(V),
    HandledWithNotice(V, String),
}

fn s(value: &str) -> V {
    V::Text(value.into())
}
fn explicit(options: &Options) -> bool {
    options.shareability.is_some()
        || options.scope.is_some()
        || options.environment.is_some()
        || options.commit.is_some()
        || options.event_id.is_some()
        || options.contribution_id.is_some()
        || options.evidence_root.is_some()
        || !options.disclose_locators.is_empty()
}

fn scope(options: &Options) -> V {
    let mut fields = Map::from([
        (
            "kind".into(),
            s(options.scope.as_deref().unwrap_or("unclear")),
        ),
        (
            "environment".into(),
            s(options.environment.as_deref().unwrap_or("")),
        ),
    ]);
    if let Some(commit) = &options.commit {
        fields.insert("commit".into(), s(commit));
    }
    V::Map(fields)
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

fn today() -> String {
    chrono::Utc::now()
        .date_naive()
        .max(chrono::Local::now().date_naive())
        .to_string()
}

fn human_notice(output: &str, diagnostics: &[String]) -> String {
    output
        .lines()
        .take_while(|line| {
            !line.starts_with("add ")
                && !line.starts_with("set ")
                && !line.starts_with("review ")
        })
        .filter(|line| !line.trim().is_empty() && !diagnostics.iter().any(|item| item == line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn preliminary(document: &V, action: &V) -> Result<V> {
    let action = map(action)?;
    let id = text(field(action, "id")?)?;
    let kind = text(field(action, "kind")?)?;
    let existing = crate::reasoning_snapshot::entries(document)?;
    let mut candidate = document.clone();
    if kind == "add" {
        for collection in crate::reasoning_fields::collections(document)?.keys() {
            if let Some(members) = map_mut(&mut candidate)?.get_mut(collection)
                && let Ok(members) = map_mut(members)
            {
                members.remove(id);
            }
        }
        let collection = action
            .get("into")
            .and_then(|value| text(value).ok())
            .or_else(|| existing.get(id).map(|(collection, _)| collection.as_str()))
            .unwrap_or("known");
        let members = map_mut(&mut candidate)?
            .entry(collection.into())
            .or_insert_with(|| V::Map(Map::new()));
        if *members == V::Null {
            *members = V::Map(Map::new());
        }
        map_mut(members)?.insert(id.into(), field(action, "body")?.clone());
    } else if kind == "set" {
        let (collection, old) = existing
            .get(id)
            .ok_or_else(|| Error("scoped set needs a complete entry body".into()))?;
        let mut body = map(old)
            .map_err(|_| Error("scoped set needs a complete entry body".into()))?
            .clone();
        let value_field = if body.contains_key("v") {
            "v"
        } else if body.contains_key("quoted") {
            "quoted"
        } else {
            return Err(Error("set needs an entry with v or quoted".into()));
        };
        body.insert(value_field.into(), field(action, "value")?.clone());
        body.insert(
            "of".into(),
            action
                .get("as_of")
                .filter(|value| **value != V::Null)
                .cloned()
                .unwrap_or_else(|| s(&today())),
        );
        if let Some(source) = action.get("source").filter(|value| **value != V::Null) {
            let at = action
                .get("at")
                .filter(|value| **value != V::Null)
                .ok_or_else(|| Error("a changed source needs its exact at location".into()))?;
            body.insert("from".into(), source.clone());
            body.insert("at".into(), at.clone());
        }
        if let Some(scope) = action.get("_record_scope") {
            body.insert("scope".into(), scope.clone());
        }
        map_mut(map_mut(&mut candidate)?.get_mut(collection).unwrap())?
            .insert(id.into(), V::Map(body));
    }
    Ok(candidate)
}

fn selected(document: &V, id: &str) -> Result<V> {
    if crate::reasoning_snapshot::entries(document)?.contains_key(id) {
        pending_bundle::closure(document, &[id.into()])
    } else {
        Ok(V::Map(Map::new()))
    }
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

fn evidence(document: &V, root: Option<&Path>) -> Result<(Files, BTreeMap<PathBuf, Vec<u8>>)> {
    let required = pending_bundle::required_files(document)?;
    if required.is_empty() {
        return Ok((Files::new(), BTreeMap::new()));
    }
    let root = root
        .ok_or_else(|| {
            Error("contribution needs an explicit --evidence-root for referenced files".into())
        })?
        .canonicalize()?;
    require(root.is_dir(), "invalid_evidence_root")?;
    let mut files = Files::new();
    let mut observed = BTreeMap::new();
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
        observed.insert(path, bytes.clone());
        files.insert(name, bytes);
    }
    Ok((files, observed))
}

type Loaded = (
    V,
    Inventory,
    Option<crate::history_capture::Capture>,
    Option<crate::source_capture::CapturedSource>,
);

/// A compact record is read as its own writer reads it: the verified semantic
/// view, rechecked by source bytes and history revision, never the legacy store.
fn load_document(route: &WriteRoute) -> Result<Loaded> {
    if crate::history_node_publication::selected(&route.paths()[0])? {
        let source = crate::source_capture::capture_source(
            route.paths(),
            &route.project().root,
            crate::source_capture::ReadMode::Frozen,
            None,
        )?;
        let document = source
            .node_history_capture()
            .ok_or_else(|| Error("node_semantic_missing_view".into()))?
            .document()
            .clone();
        return Ok((document, Inventory::default(), None, Some(source)));
    }
    if crate::legacy_authoring::authority_route(&route.paths()[0])?
        == crate::legacy_authoring::AuthorityRoute::History
    {
        let capture = crate::history_store::Store::new(&route.paths()[0])?.capture()?;
        let document = crate::history_authoring::document(&capture)?;
        return Ok((document, Inventory::default(), Some(capture), None));
    }
    let mut inventory = Inventory::default();
    let document = crate::source_document::load(route.paths(), &mut inventory, false)?;
    Ok((document.source.projected(), inventory, document.history, None))
}

pub(crate) fn route(
    kind: &str,
    options: &Options,
    mut action: V,
    source_body: Option<&SourceValue>,
    route: WriteRoute,
) -> Result<Outcome> {
    let explicit = explicit(options);
    let shareability = options.shareability.as_deref();
    require(
        shareability.is_none_or(|value| ["project", "private", "unclear"].contains(&value)),
        "shareability must be project, private or unclear",
    )?;
    if !explicit {
        return Ok(Outcome::Local(action));
    }
    let scope = scope(options);
    let scope_kind = text(field(map(&scope)?, "kind")?)?;
    require(
        ["project", "external", "code", "feature", "unclear"].contains(&scope_kind),
        "scope must be project, external, code, feature or unclear",
    )?;
    let declared_scope =
        options.scope.is_some() || options.environment.is_some() || options.commit.is_some();
    if shareability == Some("project") && declared_scope {
        let fields = map_mut(&mut action)?;
        fields.insert("_record_scope".into(), scope.clone());
        if kind == "add" {
            let body = map_mut(
                fields
                    .get_mut("body")
                    .ok_or_else(|| Error("scoped writes need a complete entry body".into()))?,
            )
            .map_err(|_| Error("scoped writes need a complete entry body".into()))?;
            if let Some(old) = body.get("scope") {
                require(
                    old.digest()? == scope.digest()?,
                    "entry body scope differs from the requested scope",
                )?;
            }
            body.insert("scope".into(), scope.clone());
        }
    }
    if let Some(value) = shareability {
        map_mut(&mut action)?.insert("shareability".into(), s(value));
    }
    if let Some(id) = &options.event_id {
        token(&s(id))?;
        map_mut(&mut action)?.insert("event_id".into(), s(id));
    }
    if let Some(id) = &options.contribution_id {
        token(&s(id))?;
        map_mut(&mut action)?.insert("contribution_id".into(), s(id));
    }

    if !route.paths()[0].exists() {
        if shareability != Some("project")
            || !route.pending_required()?
            || !["project", "external"].contains(&scope_kind)
        {
            return Ok(Outcome::Local(action));
        }
        require(
            kind == "add" && map(field(map(&action)?, "body")?).is_ok(),
            "a project contribution requires a complete add or set, not a review refresh",
        )?;
        let candidate = preliminary(&V::Map(Map::new()), &action)?;
        let selection = selected(&candidate, &options.subject)?;
        if Privacy::private_marker(&action) || Privacy::private_marker(&selection) {
            return Ok(Outcome::Handled(Privacy::draft(
                route.project(),
                &action,
                &selection,
                "private or unclear source permission",
            )?));
        }
        if private_locator(&selection) {
            return Ok(Outcome::Handled(Privacy::draft(
                route.project(),
                &action,
                &selection,
                "private source locator needs explicit portable evidence reconciliation",
            )?));
        }
        let (files, observations) = evidence(&selection, options.evidence_root.as_deref())?;
        let bundle = pending_bundle::prepare(
            &candidate,
            std::slice::from_ref(&options.subject),
            &scope,
            "project",
            &files,
        )?;
        let event_id = options
            .event_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
        let contribution_id = options
            .contribution_id
            .clone()
            .unwrap_or_else(|| options.subject.clone());
        let project = route.project().clone();
        let expected_policy = route.config().clone();
        route.verify()?;
        drop(route);
        let receipt = crate::public_knowledge::import_helper::capture_pending(
            &project,
            &bundle,
            &files,
            &event_id,
            &contribution_id,
            &mut || {
                require(
                    !project.record(Some(&expected_policy))?.exists(),
                    "snapshot_changed",
                )?;
                require(
                    project.config()? == expected_policy,
                    "project policy or destination changed before capture; retry",
                )?;
                require(
                    observations
                        .iter()
                        .all(|(path, bytes)| read_evidence(path).ok().as_ref() == Some(bytes)),
                    "snapshot_changed",
                )
            },
        )?;
        return Ok(Outcome::Handled(receipt));
    }

    let (document, initial_inventory, history, node) = load_document(&route)?;
    let unchanged = || -> Result<()> {
        initial_inventory.verify()?;
        node.as_ref().map(|source| source.verify()).transpose()?;
        Ok(())
    };
    let candidate = preliminary(&document, &action)?;
    let id = options.subject.as_str();
    let selection = selected(&candidate, id).unwrap_or_else(|_| candidate.clone());
    if let Some(draft) = Privacy::selected_draft(route.project(), &action, &candidate)? {
        unchanged()?;
        route.verify()?;
        return Ok(Outcome::Handled(draft));
    }
    if private_locator(&selection) {
        unchanged()?;
        route.verify()?;
        return Ok(Outcome::Handled(Privacy::draft(
            route.project(),
            &action,
            &selection,
            "private source locator needs explicit portable evidence reconciliation",
        )?));
    }
    if shareability != Some("project") {
        unchanged()?;
        route.verify()?;
        return Ok(Outcome::Handled(Privacy::draft(
            route.project(),
            &action,
            &selection,
            "sharing permission is private or unclear",
        )?));
    }
    if scope_kind == "code" {
        require(
            kind == "add" && map(field(map(&action)?, "body")?).is_ok(),
            "code scope needs an add with a complete body and exact commit",
        )?;
        let (files, _) = evidence(&selection, options.evidence_root.as_deref())?;
        pending_bundle::prepare(&candidate, &[id.into()], &scope, "project", &files)?;
        return Ok(Outcome::Local(action));
    }
    let advanced = route.pending_required()?;
    if !advanced || ["feature", "unclear"].contains(&scope_kind) {
        return Ok(Outcome::Local(action));
    }
    require(
        options.hypothesis.is_none(),
        "named hypotheses stay in the local record; use feature scope",
    )?;
    require(
        ["add", "set"].contains(&kind),
        "a project contribution requires a complete add or set, not a review refresh",
    )?;
    // The v3 transport binds a legacy source authority, and plain v2 would drop
    // this record's history. Until a versioned compact format is read by every
    // pending consumer, refuse before any ledger event or record change.
    if node.is_some() {
        unchanged()?;
        route.verify()?;
        return Err(Error(NODE_CONTRIBUTION.into()));
    }

    let event_id = options
        .event_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
    let (candidate, inventory, diagnostics, notice, history_prepared) = if let Some(captured) =
        history
    {
        let store = crate::history_store::Store::new(&route.paths()[0])?;
        let mut runtime_document = document.clone();
        if let Some(meta) = map_mut(&mut runtime_document)?.get_mut("meta") {
            map_mut(meta)?.remove("history");
        }
        let runtime = crate::public_workspace::runtime_for_document(&runtime_document)?;
        let day = options.as_of.clone().unwrap_or_else(today);
        let recorded_at = format!("{day}T00:00:00Z");
        let mutation = crate::history_authoring::prepare(
            &store,
            &captured,
            &action,
            &crate::history_authoring::Options {
                operation: format!("contribution-{event_id}"),
                recorded_at: recorded_at.clone(),
                recording_day: day,
                by: V::Null,
                strict: true,
                paths: Scheme::Hashed,
                receipt_version: None,
            },
            runtime.as_ref(),
        )
        .map_err(|e| Error(format!("history contribution authoring: {e}")))?;
        let candidate = crate::history_authoring::candidate(&captured, &mutation)?;
        let document = crate::history_adapter::from_store_capture(&candidate)?
            .document()
            .clone();
        (
            document,
            initial_inventory,
            Vec::new(),
            String::new(),
            Some((captured, mutation, recorded_at)),
        )
    } else {
        let prepared = legacy_authoring::prepare_pending_candidate(&action, &route, source_body)?;
        match prepared {
            Preparation::Draft { output, .. } => {
                return Err(Error(output.trim().into()));
            }
            Preparation::Mutation(prepared) => {
                let notice = human_notice(&prepared.output, &prepared.diagnostics);
                let diagnostics = prepared.diagnostics;
                let mut inventory = prepared.inventory;
                for image in prepared.mutation.files() {
                    inventory.stage(&prepared.root.join(&image.path), image.after.clone())?;
                }
                let document = crate::source_document::load(route.paths(), &mut inventory, false)?;
                (
                    document.source.projected(),
                    inventory,
                    diagnostics,
                    notice,
                    None,
                )
            }
        }
    };
    let selection = if history_prepared.is_some() {
        candidate.clone()
    } else {
        selected(&candidate, id)?
    };
    if let Some(draft) = Privacy::candidate_draft(route.project(), &action, &candidate)
        .map_err(|e| Error(format!("history candidate privacy: {e}")))?
    {
        inventory.verify()?;
        route.verify()?;
        return Ok(Outcome::Handled(draft));
    }
    if private_locator(&selection) {
        inventory.verify()?;
        route.verify()?;
        return Ok(Outcome::Handled(Privacy::draft(
            route.project(),
            &action,
            &selection,
            "private source locator needs explicit portable evidence reconciliation",
        )?));
    }
    let (mut files, mut evidence_observations) = if history_prepared.is_some() {
        (Files::new(), BTreeMap::new())
    } else {
        evidence(&selection, options.evidence_root.as_deref())?
    };
    let bundle = if let Some((captured, mutation, recorded_at)) = history_prepared {
        let disclosures = map(&action)?
            .get("disclosed_locators")
            .cloned()
            .unwrap_or_else(|| V::List(vec![]));
        let entry = route.paths()[0]
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or_else(|| Error("invalid_path".into()))?;
        let (artifact, history_files) = crate::history_contribution_prepare::prepare_subset(
            &captured,
            &[id.into()],
            &scope,
            &format!("contribution-subset-{event_id}"),
            &recorded_at,
            entry,
            &disclosures,
            &mutation,
        )
        .map_err(|e| Error(format!("history contribution subset: {e}")))?;
        let subset = crate::history_bundle::validate_artifact(
            field(map(&artifact)?, "manifest")?,
            field(map(&artifact)?, "revision")?,
            &history_files,
        )?;
        let subset_document = crate::history_adapter::from_store_capture(&subset)?
            .document()
            .clone();
        let captured_evidence = evidence(&subset_document, options.evidence_root.as_deref())?;
        files = captured_evidence.0;
        evidence_observations = captured_evidence.1;
        let wrapped = crate::history_contribution_prepare::wrap(&artifact, &history_files, &files)
            .map_err(|e| Error(format!("history contribution wrapper: {e}")))?;
        files = wrapped.1;
        wrapped.0
    } else {
        pending_bundle::prepare(&candidate, &[id.into()], &scope, "project", &files)?
    };
    let contribution_id = options.contribution_id.clone().unwrap_or_else(|| id.into());
    token(&s(&event_id))?;
    token(&s(&contribution_id))?;
    let project = route.project().clone();
    let expected_policy = route.config().clone();
    inventory.verify()?;
    route.verify()?;
    drop(route);
    let mut receipt = crate::public_knowledge::import_helper::capture_pending(
        &project,
        &bundle,
        &files,
        &event_id,
        &contribution_id,
        &mut || {
            inventory.verify()?;
            require(
                project.config()? == expected_policy,
                "project policy or destination changed before capture; retry",
            )?;
            require(
                evidence_observations
                    .iter()
                    .all(|(path, bytes)| read_evidence(path).ok().as_ref() == Some(bytes)),
                "snapshot_changed",
            )
        },
    )
    .map_err(|e| Error(format!("history contribution capture: {e}")))?;
    if !diagnostics.is_empty() {
        map_mut(&mut receipt)?.insert(
            "diagnostics".into(),
            V::List(diagnostics.iter().map(|value| s(value)).collect()),
        );
    }
    Ok(if notice.is_empty() {
        Outcome::Handled(receipt)
    } else {
        Outcome::HandledWithNotice(receipt, format!("{notice}\n"))
    })
}

#[cfg(test)]
mod tests {
    use super::human_notice;

    #[test]
    fn notice_excludes_canonical_diagnostics() {
        let diagnostic = "d.new.wrong_if: stored as a readable expression".to_owned();
        let output = format!(
            "nearest existing:\n  d.old: rests on p.base too - verdicts differ\n{diagnostic}\nadd d.new\n"
        );
        assert_eq!(
            human_notice(&output, std::slice::from_ref(&diagnostic)),
            "nearest existing:\n  d.old: rests on p.base too - verdicts differ"
        );
    }
}

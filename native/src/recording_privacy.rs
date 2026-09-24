//! Explicit historical acts retain private intent outside the shared project.
use crate::{
    Result,
    history_authoring::{empty, n, obj, s},
    history_contract::*,
    history_transaction_fs as F,
    history_view::map_mut,
    identity::sha256,
    project_modes::{self, Project},
    require,
    value::TypedValue as V,
};
use fs2::FileExt;
use std::{
    fs::{self, OpenOptions},
    path::PathBuf,
};

pub fn private_marker(value: &V) -> bool {
    match value {
        V::Map(m) => m.iter().any(|(key, value)| {
            key == "shareability" && !string_is(value, "project")
                || ["privacy", "visibility"].contains(&key.as_str())
                    && ["private", "unclear", "unknown", "personal"]
                        .iter()
                        .any(|s| string_is(value, s))
                || key == "private" && *value != V::Bool(false)
                || private_marker(value)
        }),
        V::List(a) => a.iter().any(private_marker),
        _ => false,
    }
}

pub fn draft(project: &Project, action: &V, document: &V, reason: &str) -> Result<V> {
    let home = std::env::var_os("KPOPPER_PRIVATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/kpopper/private"))
        })
        .ok_or_else(|| error("private_home_unavailable"))?;
    let home = project_modes::resolved(&home)?;
    require(
        !home.starts_with(&project.root),
        "private draft home must be outside the project",
    )?;
    let mut probe = home.clone();
    while !probe.exists() {
        require(probe.pop(), "private_home_unavailable")?;
    }
    require(
        !Project::open(&probe)?.is_git(),
        "private draft home must be outside Git",
    )?;
    let key = sha256(
        project
            .common
            .as_ref()
            .unwrap_or(&project.root)
            .to_str()
            .ok_or_else(|| error("nonportable_project_path"))?
            .as_bytes(),
    );
    let ident = map(action)?
        .get("event_id")
        .map(text)
        .transpose()?
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
    token(&s(&ident))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&home)?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(&home)?;
    let directory = F::target(&home, &key)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(&directory)?;
    // This private file lock is independent of the shared record directory order.
    let lock = F::target(&directory, "drafts.lock")?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(lock)?;
    FileExt::lock_exclusive(&lock)?;
    let body = obj([
        ("version", n("1")),
        ("state", s("private draft")),
        ("reason", s(reason)),
        ("action", action.clone()),
        ("document", document.clone()),
        ("evidence", empty()),
    ]);
    let bytes = body.canonical_bytes()?;
    let name = format!("{ident}.json");
    F::publish_immutable(&directory, &name, &bytes)?;
    Ok(obj([
        ("state", s("private draft")),
        (
            "path",
            s(directory
                .join(name)
                .to_str()
                .ok_or_else(|| error("nonportable_project_path"))?),
        ),
        ("reason", s(reason)),
    ]))
}

pub fn selected_draft(project: &Project, action: &V, document: &V) -> Result<Option<V>> {
    draft_for_selection(project, action, document, false)
}

/// Unannotated candidate writes retain the whole candidate on a closure failure.
/// This keeps privacy conservative and lets authoring supply the actual diagnostic.
pub fn candidate_draft(project: &Project, action: &V, document: &V) -> Result<Option<V>> {
    draft_for_selection(project, action, document, true)
}

fn selection(document: &V, id: &str, candidate: bool) -> Result<V> {
    let all = crate::reasoning_snapshot::entries(document)?;
    if all.contains_key(id) {
        match crate::pending_bundle::closure(document, &[id.into()]) {
            Ok(selected) => Ok(selected),
            Err(_) if candidate => Ok(document.clone()),
            Err(error) => Err(error),
        }
    } else {
        Ok(empty())
    }
}

fn draft_for_selection(
    project: &Project,
    action: &V,
    document: &V,
    candidate: bool,
) -> Result<Option<V>> {
    let id = text(field(map(action)?, "id")?)?;
    let mut selected = selection(document, id, candidate)?;
    let controls = V::Map(
        map(document)?
            .iter()
            .filter(|(k, _)| {
                ["meta", "privacy", "visibility", "private", "shareability"].contains(&k.as_str())
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );
    if private_marker(&controls) {
        map_mut(&mut selected)?.extend(map(&controls)?.clone());
        return draft(
            project,
            action,
            &selected,
            "private or unclear record permission",
        )
        .map(Some);
    }
    if private_marker(action) || private_marker(&selected) {
        return draft(
            project,
            action,
            &selected,
            "private or unclear source permission",
        )
        .map(Some);
    }
    Ok(None)
}

/// Recovery rechecks the same selection boundary without creating a new draft.
pub(crate) fn selection_is_private(action: &V, document: &V, candidate: bool) -> Result<bool> {
    let id = text(field(map(action)?, "id")?)?;
    let selected = selection(document, id, candidate)?;
    let controls = V::Map(
        map(document)?
            .iter()
            .filter(|(k, _)| {
                ["meta", "privacy", "visibility", "private", "shareability"].contains(&k.as_str())
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );
    Ok(private_marker(&controls) || private_marker(action) || private_marker(&selected))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_unresolved_candidate_does_not_hide_unrelated_private_content() {
        let doc = V::from_json(&serde_json::json!({
            "known": {"s.private": {"v":"private evidence", "private":true}},
            "judgments": {"d.new": {"verdict":"stop", "rests_on":["p.missing"]}}
        }))
        .unwrap();
        assert!(selection(&doc, "d.new", false).is_err());
        let selected = selection(&doc, "d.new", true).unwrap();
        assert_eq!(selected, doc);
        assert!(private_marker(&selected));
    }
}

//! Pure transformations for re-deciding an ordinary arrangement in place.
//!
//! Admission is handled by `reasoning_authoring_guards::may_supersede`; this
//! module only constructs the tool-authored fields after that decision has
//! succeeded.  It deliberately does not read a brief, page, or filesystem.

use crate::{
    Result,
    history_contract::{error, map, text, Map},
    history_transaction::PreparedMutation,
    ordinary_page_capture::PageCapture,
    require,
    source_inventory::{Inventory, Observation},
    value::TypedValue as V,
};
use std::{collections::BTreeSet, path::{Path, PathBuf}};

fn s(value: &str) -> V {
    V::Text(value.into())
}

fn trail(old: &V) -> Result<Vec<V>> {
    let Some(value) = map(old)?.get("replaced") else {
        return Ok(Vec::new());
    };
    match value {
        V::Text(value) => Ok(vec![s(value)]),
        V::List(values) => Ok(values.clone()),
        _ => Err(error("invalid_replaced_trail")),
    }
}

fn printed(value: Option<&V>) -> String {
    value
        .map(crate::source_text::ordinary_python_str)
        .unwrap_or_else(|| "undated".into())
}

/// Renew an arrangement's tool-owned `born` and `replaced` fields while
/// preserving the newly authored occasion/sign fields.  The caller appends
/// the freshly captured dependency snapshot after this transformation.
pub(crate) fn renewal(old: &V, new: &V, ended: &str, stamp: &str, stood: Option<&V>) -> Result<V> {
    let old_fields = map(old)?;
    let new_fields = map(new)?;
    let mut replaced = trail(old)?;
    let born = printed(old_fields.get("born"));
    let stood = stood
        .map(|value| {
            let count = text(value)
                .map(str::to_owned)
                .unwrap_or_else(|_| printed(Some(value)));
            format!(
                ", stood {count} session{}",
                if count == "1" { "" } else { "s" }
            )
        })
        .unwrap_or_default();
    replaced.push(s(&format!("born {born}{stood}; {ended} on {stamp}")));

    let mut body = new_fields
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "born" | "replaced" | "seen"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<crate::history_contract::Map>();
    require(
        body.insert("born".into(), s(stamp)).is_none(),
        "invalid_arrangement_body",
    )?;
    require(
        body.insert("replaced".into(), V::List(replaced)).is_none(),
        "invalid_arrangement_body",
    )?;
    Ok(V::Map(body))
}

fn observation(value: &Observation, root: &Path) -> Result<V> {
    Ok(match value {
        Observation::Bytes(value) => V::Map(Map::from([("bytes".into(), s(value))])),
        Observation::Unreadable(value) => V::Map(Map::from([("unreadable".into(), s(value))])),
        Observation::Exists(value) => V::Map(Map::from([("exists".into(), V::Bool(*value))])),
        Observation::Directory(value) => V::Map(Map::from([("directory".into(), V::Bool(*value))])),
        Observation::File(value) => V::Map(Map::from([("file".into(), V::Bool(*value))])),
        Observation::Glob(paths) => V::Map(Map::from([(
            "glob".into(),
            V::List(paths.iter().map(|path| super::relative(root, path).map(|v| s(&v)))
                .collect::<Result<Vec<_>>>()?),
        )])),
    })
}

/// Portable, bounded source/view observations needed to replay arrangement
/// admission after a crash.
pub(crate) fn recovery_guard(root: &Path, page: &PageCapture) -> Result<V> {
    let mut items = Map::new();
    for inventory in [page.source.inventory(), &page.view_inventory] {
        for ((kind, path), expected) in &inventory.events {
            let relative = super::relative(root, path)?;
            items.insert(format!("{kind}\0{relative}"), observation(expected, root)?);
        }
    }
    Ok(V::Map(Map::from([
        ("version".into(), V::Integer(crate::value::Integer::new("1")?)),
        ("observations".into(), V::Map(items)),
    ])))
}

fn current(kind: &str, path: &Path) -> Result<Observation> {
    let mut inventory = Inventory::default();
    match kind {
        "bytes" => match inventory.read(path) {
            Ok(raw) => Ok(Observation::Bytes(crate::identity::sha256(&raw))),
            Err(error) => Ok(Observation::Unreadable(error.0)),
        },
        "unreadable" => match inventory.read(path) {
            Ok(raw) => Ok(Observation::Bytes(crate::identity::sha256(&raw))),
            Err(error) => Ok(Observation::Unreadable(error.0)),
        },
        "exists" => Ok(Observation::Exists(inventory.exists(path)?)),
        "directory" => Ok(Observation::Directory(inventory.directory(path)?)),
        "file" => Ok(Observation::File(inventory.file(path)?)),
        "glob" => Ok(Observation::Glob(inventory.glob(path)?)),
        _ => Err(error("invalid_arrangement_guard")),
    }
}

fn expected(value: &V, root: &Path) -> Result<Observation> {
    let value = map(value)?;
    require(value.len() == 1, "invalid_arrangement_guard")?;
    let (kind, value) = value.first_key_value().unwrap();
    Ok(match kind.as_str() {
        "bytes" => Observation::Bytes(text(value)?.into()),
        "unreadable" => Observation::Unreadable(text(value)?.into()),
        "exists" => Observation::Exists(match value { V::Bool(value) => *value, _ => return Err(error("invalid_arrangement_guard")) }),
        "directory" => Observation::Directory(match value { V::Bool(value) => *value, _ => return Err(error("invalid_arrangement_guard")) }),
        "file" => Observation::File(match value { V::Bool(value) => *value, _ => return Err(error("invalid_arrangement_guard")) }),
        "glob" => Observation::Glob(crate::history_view::list(value)?.iter()
            .map(|value| Ok(root.join(text(value)?))).collect::<Result<Vec<_>>>()?),
        _ => return Err(error("invalid_arrangement_guard")),
    })
}

pub(crate) fn verify_recovery(
    root: &Path,
    mutation: &PreparedMutation,
    guard: &V,
) -> Result<()> {
    let guard = map(guard)?;
    require(guard.get("version") == Some(&V::Integer(crate::value::Integer::new("1")?)),
        "invalid_arrangement_guard")?;
    let changed = mutation.files().iter().map(|file| file.path.clone()).collect::<BTreeSet<_>>();
    for (key, encoded) in map(guard.get("observations").ok_or_else(|| error("invalid_arrangement_guard"))?)? {
        let (kind, relative) = key.split_once('\0').ok_or_else(|| error("invalid_arrangement_guard"))?;
        crate::history_authority::relative_path(relative)?;
        let path = root.join(relative);
        let expected = expected(encoded, root)?;
        let actual = current(kind, &path)?;
        if let (Observation::Glob(expected), Observation::Glob(actual)) = (&expected, &actual) {
            let keep = |path: &PathBuf| super::relative(root, path).is_ok_and(|path| !changed.contains(&path));
            require(expected.iter().filter(|p| keep(p)).eq(actual.iter().filter(|p| keep(p))),
                "project_route_changed")?;
        } else if let Some(image) = mutation.files().iter().find(|file| file.path == relative) {
            let bytes = crate::history_transaction_fs::read(&path)?;
            require(bytes == image.before || bytes == image.after, "concurrent_edit")?;
        } else {
            require(actual == expected, "project_route_changed")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_contract::Map;

    fn list(values: &[&str]) -> V {
        V::List(values.iter().map(|value| s(value)).collect())
    }

    #[test]
    fn renewal_restarts_born_and_appends_the_arrangement_trail() {
        let old = V::Map(
            [
                ("born".into(), s("2026-09-01")),
                ("replaced".into(), V::List(vec![s("earlier decision")])),
            ]
            .into_iter()
            .collect(),
        );
        let new = V::Map(
            [
                ("verdict".into(), s("keep")),
                ("rests_on".into(), list(&["s.ask"])),
                ("seen".into(), V::Map(Map::new())),
            ]
            .into_iter()
            .collect(),
        );
        let result = renewal(
            &old,
            &new,
            "its sign holds (count > 1)",
            "2026-09-20",
            Some(&V::Integer(crate::value::Integer::new("1").unwrap())),
        )
        .unwrap();
        let fields = map(&result).unwrap();
        assert_eq!(fields["born"], s("2026-09-20"));
        assert_eq!(
            fields["replaced"],
            V::List(vec![
                s("earlier decision"),
                s("born 2026-09-01, stood 1 session; its sign holds (count > 1) on 2026-09-20"),
            ])
        );
        assert!(!fields.contains_key("seen"));
    }
}

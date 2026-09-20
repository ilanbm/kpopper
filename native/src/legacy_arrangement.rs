//! Pure transformations for re-deciding an ordinary arrangement in place.
//!
//! Admission is handled by `reasoning_authoring_guards::may_supersede`; this
//! module only constructs the tool-authored fields after that decision has
//! succeeded.  It deliberately does not read a brief, page, or filesystem.

use crate::{
    Result,
    history_contract::{error, map, text},
    require,
    value::TypedValue as V,
};

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

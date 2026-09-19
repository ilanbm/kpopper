//! Retained versions and trails for ordinary legacy judgment replacement.
//!
//! This module is deliberately independent of filesystem publication.  The
//! caller supplies the complete sidecar preimage and receives the complete
//! sidecar after-image, so the record and its retained history can be guarded
//! and published as one mutation.

use crate::{
    Result,
    history_contract::{error, map},
    history_yaml::{self as Y, SourceValue},
    require,
    source_clock::python_equal,
    value::TypedValue as V,
};

fn source(value: &V) -> SourceValue {
    SourceValue::from_typed(value)
}

fn typed(value: &SourceValue) -> Result<V> {
    Ok(value.typed())
}

fn map_source(value: &SourceValue) -> Result<&[(String, SourceValue)]> {
    match value {
        SourceValue::Map(items) => Ok(items),
        _ => Err(error("invalid_replaced")),
    }
}

fn map_source_mut(value: &mut SourceValue) -> Result<&mut Vec<(String, SourceValue)>> {
    match value {
        SourceValue::Map(items) => Ok(items),
        _ => Err(error("invalid_replaced")),
    }
}

fn list_source(value: &SourceValue) -> Result<&[SourceValue]> {
    match value {
        SourceValue::List(items) => Ok(items),
        _ => Err(error("invalid_replaced")),
    }
}

fn field<'a>(value: &'a SourceValue, key: &str) -> Option<&'a SourceValue> {
    map_source(value)
        .ok()?
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
}

fn insert_field(value: &mut SourceValue, key: &str, item: SourceValue) -> Result<()> {
    let map = map_source_mut(value)?;
    if let Some((_, current)) = map.iter_mut().find(|(k, _)| k == key) {
        *current = item;
    } else {
        map.push((key.into(), item));
    }
    Ok(())
}

fn remove_field(value: &mut SourceValue, key: &str) -> Result<()> {
    let map = map_source_mut(value)?;
    map.retain(|(name, _)| name != key);
    Ok(())
}

fn without_tool_fields(value: &V) -> V {
    let V::Map(items) = value else {
        return value.clone();
    };
    V::Map(
        items
            .iter()
            .filter(|(key, _)| {
                !matches!(
                    key.as_str(),
                    "seen"
                        | "reviewed"
                        | "born"
                        | "replaced"
                        | "ended"
                        | "day"
                        | "same_as"
                        | "dropped"
                )
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

fn verdict(value: &V) -> Option<V> {
    map(value)
        .ok()
        .and_then(|m| m.get("verdict").or_else(|| m.get("title")).cloned())
}

fn same(left: &V, right: &V) -> bool {
    crate::ordinary_reader::same_legacy(left, right)
}

fn core(value: &V) -> V {
    let value = without_tool_fields(value);
    if let V::Map(items) = value {
        let mut out = items;
        if let Some((_, predicate)) = out.iter_mut().find(|(key, _)| *key == "wrong_if")
            && let V::Map(m) = predicate
            && let Some(V::Text(expr)) = m.get("expr")
        {
            *predicate = V::Text(expr.clone());
        }
        V::Map(out)
    } else {
        value
    }
}

fn source_core(value: &SourceValue) -> Result<V> {
    Ok(core(&typed(value)?))
}

fn version_at(versions: &[SourceValue], index: usize) -> SourceValue {
    let mut value = versions
        .get(index)
        .cloned()
        .unwrap_or(SourceValue::Map(vec![]));
    for _ in 0..versions.len() {
        let Some(pointer) = field(&value, "same_as") else {
            break;
        };
        let V::Integer(n) = typed(pointer).unwrap_or(V::Null) else {
            break;
        };
        let Ok(n) = n.as_str().parse::<usize>() else {
            break;
        };
        if n == 0 {
            break;
        }
        let Some(next) = versions.get(n - 1) else {
            break;
        };
        value = next.clone();
    }
    value
}

fn parse_sidecar(raw: Option<&[u8]>) -> Result<SourceValue> {
    match raw {
        None => Ok(SourceValue::Map(vec![])),
        Some([]) => Ok(SourceValue::Map(vec![])),
        Some(raw) => {
            let body = raw.strip_prefix(b"# Judgments this record's writes replaced, kept whole - read with `kpop pull <id> --history`.\n").unwrap_or(raw);
            Y::decode_source_document(body).map_err(|_| error("invalid_replaced"))
        }
    }
}

fn dropped_source(dropped: Option<&V>) -> Result<Option<SourceValue>> {
    let Some(dropped) = dropped else {
        return Ok(None);
    };
    if *dropped == V::Null {
        return Ok(None);
    }
    require(matches!(dropped, V::Map(_)), "drops must be a mapping")?;
    Ok(Some(source(dropped)))
}

/// Add the trail written onto the replacement body.  Existing tool trails are
/// retained in their original order and a new line is appended.
pub(crate) fn judgment_renewal(old: &V, new: &V, ended: &str) -> Result<V> {
    let mut body = map(new)?.clone();
    let mut trail = Vec::new();
    if let Some(existing) = map(old)?.get("replaced") {
        match existing {
            V::Text(value) => trail.push(V::Text(value.clone())),
            V::List(values) => trail.extend(values.iter().cloned()),
            _ => return Err(error("invalid_replaced_trail")),
        }
    }
    trail.push(V::Text(ended.to_string()));
    body.insert("replaced".into(), V::List(trail));
    Ok(V::Map(body))
}

#[derive(Clone, Debug)]
pub(crate) struct ReturnTo {
    pub index: usize,
    pub exact: bool,
    pub day: String,
    pub ended: String,
}

pub(crate) fn dropped_dependencies(
    old: &V,
    new: &V,
    deps_field: &str,
    drops: Option<&V>,
) -> Result<Vec<(String, Option<V>)>> {
    let as_ids = |body: &V| -> Result<Vec<String>> {
        let Some(value) = map(body)?.get(deps_field) else {
            return Ok(vec![]);
        };
        let V::List(items) = value else {
            return Err(error("invalid_dependencies"));
        };
        items
            .iter()
            .map(|item| crate::history_contract::text(item).map(str::to_owned))
            .collect()
    };
    let was = as_ids(old)?;
    let now = as_ids(new)?;
    let drop_map = drops.and_then(|v| map(v).ok());
    Ok(was
        .into_iter()
        .filter(|id| !now.iter().any(|current| current == id))
        .map(|id| (id.clone(), drop_map.and_then(|m| m.get(&id).cloned())))
        .collect())
}

/// Build the complete retained sidecar after-image and return its new version
/// number plus any return-to-version information for the CLI report.
pub(crate) fn keep_replaced(
    before: Option<&[u8]>,
    id: &str,
    old_source: &SourceValue,
    new_body: &V,
    ended: &str,
    stamp: &str,
    dropped: Option<&V>,
) -> Result<(Vec<u8>, usize, Option<ReturnTo>)> {
    let mut retained = parse_sidecar(before)?;
    let versions = {
        let root = map_source(&retained)?;
        root.iter()
            .find(|(key, _)| key == id)
            .map(|(_, value)| list_source(value))
            .transpose()?
            .map(ToOwned::to_owned)
            .unwrap_or_default()
    };
    let old_core = core(&typed(old_source)?);
    let mut pointer = None;
    for index in 0..versions.len() {
        let candidate = version_at(&versions, index);
        if python_equal(&source_core(&candidate)?, &old_core) {
            pointer = Some(index + 1);
            break;
        }
    }
    let mut kept = if let Some(pointer) = pointer {
        SourceValue::Map(vec![(
            "same_as".into(),
            SourceValue::Scalar(V::Integer(crate::value::Integer::new(
                &pointer.to_string(),
            )?)),
        )])
    } else {
        old_source.clone()
    };
    remove_field(&mut kept, "replaced")?;
    let dropped = dropped_source(dropped)?;
    insert_field(
        &mut kept,
        "ended",
        SourceValue::Scalar(V::Text(ended.into())),
    )?;
    insert_field(&mut kept, "day", SourceValue::Scalar(V::Text(stamp.into())))?;
    if let Some(dropped) = dropped {
        insert_field(&mut kept, "dropped", dropped)?;
    }
    let wanted = core(new_body);
    let mut return_to = None;
    if let Some(wanted_verdict) = verdict(new_body) {
        for (index, raw) in versions.iter().enumerate() {
            let resolved = typed(&version_at(&versions, index))?;
            if !verdict(&resolved).is_some_and(|old| same(&old, &wanted_verdict)) {
                continue;
            }
            let exact = python_equal(&core(&resolved), &wanted);
            let detail =
                |key| super::display(&field(raw, key).map(SourceValue::typed).unwrap_or(V::Null));
            let found = ReturnTo {
                index: index + 1,
                exact,
                day: detail("day")?,
                ended: detail("ended")?,
            };
            if exact {
                return_to = Some(found);
                break;
            }
            if return_to.is_none() {
                return_to = Some(found);
            }
        }
    }
    let mut versions_after = versions;
    versions_after.push(kept);
    let root = map_source_mut(&mut retained)?;
    if let Some((_, value)) = root.iter_mut().find(|(key, _)| key == id) {
        *value = SourceValue::List(versions_after.clone());
    } else {
        root.push((id.into(), SourceValue::List(versions_after.clone())));
    }
    let encoded = super::dump_ordered_yaml(&retained, 100)?.into_bytes();
    let header = b"# Judgments this record's writes replaced, kept whole - read with `kpop pull <id> --history`.\n";
    let mut after = header.to_vec();
    after.extend(encoded);
    Ok((after, versions_after.len(), return_to))
}

pub(crate) fn old_body(raw: &[u8], id: &str) -> Result<SourceValue> {
    let document = Y::decode_ordinary_source_value(raw)?;
    let Y::OrdinaryValue::Map(groups) = document else {
        return Err(error("invalid_history_yaml"));
    };
    for (_, group) in groups {
        if let Y::OrdinaryValue::Map(entries) = group
            && let Some((_, body)) = entries.iter().find(|(key, _)| key.text() == Some(id))
        {
            return super::ordinary_template(body).ok_or_else(|| error("invalid_yaml_key"));
        }
    }
    Err(error("invalid_history_yaml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn body(verdict: &str, because: &str) -> SourceValue {
        Y::decode_source_value(
            format!("verdict: {verdict}\nbecause: {because}\nrests_on: [p.a]\nwrong_if: p.a > 1\n")
                .as_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn dedup_keeps_the_old_body_and_retains_pointer_metadata() {
        let a = body("continue", "first");
        let b = body("stop", "second");
        let (first, _, _) = keep_replaced(
            None,
            "d.keep",
            &a,
            &b.typed(),
            "first ended",
            "2026-09-16",
            None,
        )
        .unwrap();
        let (second, _, back) = keep_replaced(
            Some(&first),
            "d.keep",
            &b,
            &a.typed(),
            "second ended",
            "2026-09-17",
            None,
        )
        .unwrap();
        assert_eq!(back.unwrap().index, 1);
        let retained = parse_sidecar(Some(&second)).unwrap();
        let versions = list_source(field(&retained, "d.keep").unwrap()).unwrap();
        assert_eq!(
            field(&versions[1], "verdict").unwrap().typed(),
            V::Text("stop".into())
        );
        let dropped = V::Map(crate::history_contract::Map::from([(
            "p.old".into(),
            V::Text("no longer relevant".into()),
        )]));
        let (third, _, back) = keep_replaced(
            Some(&second),
            "d.keep",
            &a,
            &b.typed(),
            "third ended",
            "2026-09-18",
            Some(&dropped),
        )
        .unwrap();
        assert_eq!(back.unwrap().index, 2);
        let retained = parse_sidecar(Some(&third)).unwrap();
        let versions = list_source(field(&retained, "d.keep").unwrap()).unwrap();
        let pointer = &versions[2];
        assert_eq!(
            field(pointer, "same_as").unwrap().typed(),
            V::Integer(crate::value::Integer::new("1").unwrap())
        );
        assert_eq!(
            field(pointer, "ended").unwrap().typed(),
            V::Text("third ended".into())
        );
        assert_eq!(
            field(pointer, "day").unwrap().typed(),
            V::Text("2026-09-18".into())
        );
        assert_eq!(field(pointer, "dropped").unwrap().typed(), dropped);
    }

    #[test]
    fn an_exact_return_takes_precedence_over_an_earlier_same_verdict() {
        let a = body("continue", "first");
        let revised = body("continue", "revised");
        let b = body("stop", "second");
        let (first, _, _) = keep_replaced(
            None,
            "d.keep",
            &a,
            &b.typed(),
            "first ended",
            "2026-09-16",
            None,
        )
        .unwrap();
        let (second, _, _) = keep_replaced(
            Some(&first),
            "d.keep",
            &revised,
            &b.typed(),
            "revised ended",
            "2026-09-17",
            None,
        )
        .unwrap();
        let (_, _, back) = keep_replaced(
            Some(&second),
            "d.keep",
            &b,
            &revised.typed(),
            "second ended",
            "2026-09-18",
            None,
        )
        .unwrap();
        let back = back.unwrap();
        assert!(back.exact);
        assert_eq!(back.index, 2);
        assert_eq!(back.day, "2026-09-17");
    }
}

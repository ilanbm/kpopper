//! Preserve alias identity while serializing the shared retained-version decision.
//! Source identities come from the captured graphs, never from equal values.
use super::*;
use crate::history_emit::OrdinaryIdentity as Identity;
fn empty(value: &O) -> Identity {
    Identity {
        id: None,
        children: match value {
            O::List(values) => values.iter().map(empty).collect(),
            O::Map(values) => values
                .iter()
                .flat_map(|(k, v)| [empty(&O::Scalar(k.scalar().clone())), empty(v)])
                .collect(),
            _ => vec![],
        },
    }
}
fn rebase(identity: &mut Identity, origin: usize) -> Result<()> {
    if let Some(id) = identity.id {
        identity.id = Some(
            id.checked_mul(2)
                .and_then(|id| id.checked_add(origin))
                .ok_or_else(|| error("history_limit"))?,
        );
    }
    for child in &mut identity.children {
        rebase(child, origin)?;
    }
    Ok(())
}
fn next_identity(identity: &Identity) -> usize {
    identity
        .children
        .iter()
        .map(next_identity)
        .chain(identity.id.map(|id| id.saturating_add(1)))
        .max()
        .unwrap_or(0)
}
fn copied_field(
    identity: &mut Identity,
    names: &mut BTreeMap<usize, usize>,
    next: &mut usize,
) -> Result<()> {
    if let Some(old) = identity.id {
        let assigned = if let Some(id) = names.get(&old) {
            *id
        } else {
            let id = *next;
            *next = next.checked_add(1).ok_or_else(|| error("history_limit"))?;
            names.insert(old, id);
            id
        };
        identity.id = Some(assigned);
    }
    for child in &mut identity.children {
        copied_field(child, names, next)?;
    }
    Ok(())
}
/// The shared version reducer consumes finite values; the captured alias graph
/// remains separate and is restored only in the final source after-image.
pub(super) fn decision_input(raw: Option<&[u8]>) -> Result<Option<Vec<u8>>> {
    raw.map(|raw| {
        if raw.is_empty() {
            return Ok(vec![]);
        }
        let value = crate::history_yaml::decode_ordinary_source_value(raw)?;
        crate::history_emit::encode_source(&ordered(&value)?, 100)
    })
    .transpose()
}
fn retained(before: &O, identity: &Identity, after: &O) -> Identity {
    match (before, after) {
        (O::Map(was), O::Map(now)) => Identity {
            id: identity.id,
            children: now
                .iter()
                .flat_map(|(key, value)| {
                    if let Some(index) = was.iter().position(|(old, _)| old.python_eq(key)) {
                        [
                            identity.children[index * 2].clone(),
                            retained(&was[index].1, &identity.children[index * 2 + 1], value),
                        ]
                    } else {
                        [empty(&O::Scalar(key.scalar().clone())), empty(value)]
                    }
                })
                .collect(),
        },
        (O::List(was), O::List(now)) => Identity {
            id: identity.id,
            children: now
                .iter()
                .enumerate()
                .map(|(i, value)| {
                    if let Some(old) = was.get(i) {
                        retained(old, &identity.children[i], value)
                    } else {
                        empty(value)
                    }
                })
                .collect(),
        },
        _ if before.projected() == after.projected() => identity.clone(),
        _ => empty(after),
    }
}
fn bounded_expansion(value: &O, identity: &Identity, target: usize, changed: &O) -> Result<()> {
    fn measure(
        value: &O,
        identity: Option<&Identity>,
        target: usize,
        changed: &O,
        depth: usize,
        total: &mut (usize, usize),
    ) -> Result<()> {
        let (value, identity) = if identity.is_some_and(|i| i.id == Some(target)) {
            (changed, None)
        } else {
            (value, identity)
        };
        total.0 = total.0.saturating_add(1);
        require(
            depth <= crate::value::MAX_DEPTH && total.0 <= crate::value::MAX_VALUES * 2,
            "history_limit",
        )?;
        match value {
            O::Scalar(v) => {
                total.1 = total
                    .1
                    .saturating_add(crate::history_yaml::compact_json_size(v))
            }
            O::List(values) => {
                total.1 = total.1.saturating_add(2 + values.len().saturating_sub(1));
                for (i, v) in values.iter().enumerate() {
                    measure(
                        v,
                        identity.and_then(|id| id.children.get(i)),
                        target,
                        changed,
                        depth + 1,
                        total,
                    )?;
                }
            }
            O::Map(values) => {
                total.1 = total.1.saturating_add(2 + values.len().saturating_sub(1));
                for (i, (key, v)) in values.iter().enumerate() {
                    total.0 = total.0.saturating_add(1);
                    total.1 = total
                        .1
                        .saturating_add(crate::history_yaml::compact_json_size(key.scalar()) + 1);
                    measure(
                        v,
                        identity.and_then(|id| id.children.get(i * 2 + 1)),
                        target,
                        changed,
                        depth + 1,
                        total,
                    )?;
                }
            }
        }
        require(
            total.1 <= crate::history_yaml::MAX_DOCUMENT_BYTES,
            "history_limit",
        )
    }
    measure(value, Some(identity), target, changed, 0, &mut (0, 0))
}
fn propagate(
    value: &mut O,
    identity: &mut Identity,
    target: usize,
    changed: &O,
    changed_identity: &Identity,
) {
    if identity.id == Some(target) {
        *value = changed.clone();
        *identity = changed_identity.clone();
        return;
    }
    match value {
        O::List(values) => {
            for (v, i) in values.iter_mut().zip(&mut identity.children) {
                propagate(v, i, target, changed, changed_identity)
            }
        }
        O::Map(values) => {
            for (n, (_, v)) in values.iter_mut().enumerate() {
                propagate(
                    v,
                    &mut identity.children[n * 2 + 1],
                    target,
                    changed,
                    changed_identity,
                )
            }
        }
        _ => {}
    }
}
fn body_identity<'a>(value: &O, identity: &'a Identity, id: &str) -> Option<(&'a Identity, usize)> {
    let O::Map(groups) = value else { return None };
    for (group_index, (_, group)) in groups.iter().enumerate() {
        if let O::Map(fields) = group
            && let Some(index) = fields.iter().position(|(key, _)| key.text() == Some(id))
        {
            return Some((&identity.children[group_index * 2 + 1], index));
        }
    }
    None
}
pub(super) fn preserve(
    before: Option<&[u8]>,
    record: &[u8],
    id: &str,
    encoded: &[u8],
    version: usize,
) -> Result<Vec<u8>> {
    let mut after = crate::history_yaml::decode_ordinary_source_value(encoded)?;
    let mut identity = if let Some(raw) = before {
        let source = crate::history_yaml::decode_ordinary_source_value(raw)?;
        let mut source_identity = I::source_identity(
            std::str::from_utf8(raw).map_err(|_| error("invalid_utf8"))?,
            &ordered(&source)?,
        )?;
        rebase(&mut source_identity, 0)?;
        retained(&source, &source_identity, &after)
    } else {
        empty(&after)
    };
    let old_record = crate::history_yaml::decode_ordinary_source_value(record)?;
    let old = source_body(&old_record, id).ok_or_else(|| error("identity_source_changed"))?;
    let original_identity = I::source_identity(
        std::str::from_utf8(record).map_err(|_| error("invalid_utf8"))?,
        &ordered(&old_record)?,
    )?;
    let (group_identity, body_index) = body_identity(&old_record, &original_identity, id)
        .ok_or_else(|| error("identity_source_changed"))?;
    let old_identity = &group_identity.children[body_index * 2 + 1];
    let O::Map(root) = &after else {
        return Err(error("invalid_replaced_archive"));
    };
    let index = root
        .iter()
        .position(|(k, _)| k.text() == Some(id))
        .ok_or_else(|| error("invalid_replaced_archive"))?;
    let O::List(versions) = &root[index].1 else {
        return Err(error("invalid_replaced_archive"));
    };
    let added = versions
        .get(
            version
                .checked_sub(1)
                .ok_or_else(|| error("invalid_replaced_archive"))?,
        )
        .ok_or_else(|| error("invalid_replaced_archive"))?;
    let mut added_identity = empty(added);
    let mut next = next_identity(&identity);
    if let O::Map(fields) = added {
        for (n, (key, value)) in fields.iter().enumerate() {
            let Some(field) = key.text() else {
                return Err(error("invalid_yaml_key"));
            };
            if ["ended", "day", "dropped", "same_as"].contains(&field) {
                continue;
            }
            if old
                .get(field)
                .is_some_and(|old| old.projected() == value.projected())
            {
                let O::Map(old_fields) = old else {
                    return Err(error("identity_source_changed"));
                };
                let old_index = old_fields
                    .iter()
                    .position(|(key, _)| key.text() == Some(field))
                    .ok_or_else(|| error("identity_source_changed"))?;
                let mut field_identity = old_identity.children[old_index * 2 + 1].clone();
                // Python deep-copies each retained field separately. Aliases
                // within a field survive; sharing across fields does not.
                copied_field(&mut field_identity, &mut BTreeMap::new(), &mut next)?;
                added_identity.children[n * 2 + 1] = field_identity;
            }
        }
    }
    identity.children[index * 2 + 1].children[version - 1] = added_identity;
    if let Some(target) = identity.children[index * 2 + 1].id {
        bounded_expansion(&after, &identity, target, &root[index].1)?;
        let changed = root[index].1.clone();
        let changed_identity = identity.children[index * 2 + 1].clone();
        propagate(
            &mut after,
            &mut identity,
            target,
            &changed,
            &changed_identity,
        );
    }
    let mut bytes=b"# Judgments this record's writes replaced, kept whole - read with `kpop pull <id> --history`.\n".to_vec();
    bytes.extend(crate::history_emit::encode_ordinary_source_identity(
        &after,
        100,
        Some(&identity),
    )?);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn alias_growth_is_bounded_before_cloning_retained_versions() {
        let source = O::List(vec![O::List(vec![]); 20]);
        let identity = Identity {
            id: None,
            children: vec![
                Identity {
                    id: Some(7),
                    children: vec![]
                };
                20
            ],
        };
        let changed = O::List(vec![O::Scalar(V::Text("x".repeat(1024 * 1024)))]);
        assert_eq!(
            bounded_expansion(&source, &identity, 7, &changed)
                .unwrap_err()
                .0,
            "history_limit"
        );
        let source = O::List(vec![O::List(vec![]); 1000]);
        let identity = Identity {
            id: None,
            children: vec![
                Identity {
                    id: Some(7),
                    children: vec![]
                };
                1000
            ],
        };
        let changed = O::List(vec![O::Scalar(V::Null); 1000]);
        assert_eq!(
            bounded_expansion(&source, &identity, 7, &changed)
                .unwrap_err()
                .0,
            "history_limit"
        );
    }
}

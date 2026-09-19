//! Detached immutable object validation. Valid identity is not accepted history.
use crate::{
    Error, Result, history_paths, identity::typed_object_identity, require, value::TypedValue as V,
};
use std::collections::{BTreeMap, BTreeSet};
type Map = BTreeMap<String, V>;
pub const MAX_OBJECTS: usize = 20_000;
const ID_SCHEME: &str = "typed-history/v2";
fn error(code: &str) -> Error {
    Error(code.into())
}
fn map(value: &V) -> Result<&Map> {
    if let V::Map(m) = value {
        Ok(m)
    } else {
        Err(error("invalid_schema"))
    }
}
fn field<'a>(m: &'a Map, key: &str) -> Result<&'a V> {
    m.get(key).ok_or_else(|| error("invalid_schema"))
}
fn text(v: &V) -> Result<&str> {
    if let V::Text(s) = v {
        Ok(s)
    } else {
        Err(error("invalid_identifier"))
    }
}
fn string_is(v: &V, s: &str) -> bool {
    matches!(v,V::Text(v) if v==s)
}
fn nonblank(s: &str) -> bool {
    !s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
        .is_empty()
}
fn is_int(v: &V, s: &str) -> bool {
    matches!(v,V::Integer(v) if v.as_str()==s)
}
fn schema<'a>(value: &'a V, required: &[&str], optional: &[&str]) -> Result<&'a Map> {
    let m = map(value)?;
    require(
        required.iter().all(|k| m.contains_key(*k))
            && m.keys()
                .all(|k| required.contains(&k.as_str()) || optional.contains(&k.as_str())),
        "invalid_schema",
    )?;
    Ok(m)
}
fn token(value: &V) -> Result<()> {
    let s = text(value)?;
    require(
        s.len() <= 160
            && s.bytes().next().is_some_and(|c| c.is_ascii_alphanumeric())
            && s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c)),
        "invalid_identifier",
    )
}
fn subject(v: &V) -> Result<()> {
    history_paths::subject(text(v)?)
}
fn id(v: &V) -> Result<()> {
    require(history_paths::object_id(text(v)?), "invalid_identifier")
}
fn ids(v: &V, unique: bool) -> Result<Vec<&str>> {
    let V::List(values) = v else {
        return Err(error("invalid_references"));
    };
    let values = values
        .iter()
        .map(|v| {
            id(v)?;
            text(v)
        })
        .collect::<Result<Vec<_>>>()?;
    require(
        values
            .windows(2)
            .all(|w| if unique { w[0] < w[1] } else { w[0] <= w[1] }),
        "invalid_references",
    )?;
    Ok(values)
}
fn pins(v: &V) -> Result<&Map> {
    let m = map(v).map_err(|_| error("invalid_pins"))?;
    for (name, value) in m {
        history_paths::subject(name)?;
        id(value)?;
    }
    Ok(m)
}
fn hypothesis_name(value: &V) -> Result<()> {
    let s = text(value)?;
    require(
        !s.is_empty()
            && s.len() <= 255
            && !s.starts_with('.')
            && !s.contains(['/', '\\'])
            && !s.chars().any(|c| c < '\u{20}' || c == '\u{7f}'),
        "invalid_history_hypothesis",
    )
}
fn authored(value: &V) -> Result<()> {
    let m = schema(
        value,
        &["collection", "fields", "profile"],
        &["locator", "hypothesis"],
    )?;
    subject(field(m, "collection")?)?;
    let fields = map(field(m, "fields")?).map_err(|_| error("invalid_field_roles"))?;
    require(
        fields
            .values()
            .all(|v| matches!(v,V::Text(s) if !s.is_empty())),
        "invalid_field_roles",
    )?;
    require(
        ["ordinary-reader/v1", "checked-reader/v1", "core/v1"]
            .iter()
            .any(|s| string_is(field(m, "profile").unwrap(), s)),
        "unsupported_profile",
    )?;
    if let Some(v) = m.get("locator") {
        require(matches!(v, V::Map(_)), "invalid_locator")?;
    }
    if let Some(v) = m.get("hypothesis") {
        let group = schema(v, &["version", "name", "head"], &[])?;
        require(
            is_int(field(group, "version")?, "1") && matches!(field(group, "head")?, V::Map(_)),
            "invalid_history_hypothesis",
        )?;
        hypothesis_name(field(group, "name")?)?;
    }
    Ok(())
}
fn pin_gaps(m: &Map) -> Result<()> {
    let empty = Map::new();
    let gaps = match m.get("pin_gaps") {
        None => &empty,
        Some(v) => map(v).map_err(|_| error("invalid_pin_gaps"))?,
    };
    for (name, reason) in gaps {
        history_paths::subject(name)?;
        require(
            string_is(reason, "not_recorded") || string_is(reason, "unavailable"),
            "invalid_pin_gaps",
        )?;
    }
    let pins = pins(field(m, "pins")?)?;
    require(
        gaps.keys().all(|k| !pins.contains_key(k)),
        "overlapping_pin_gap",
    )?;
    if gaps.is_empty() {
        return Ok(());
    }
    let body = map(field(m, "body")?).map_err(|_| error("pin_dependency_mismatch"))?;
    let fields = map(field(map(field(m, "authored")?)?, "fields")?)?;
    let deps_name = fields
        .get("deps")
        .and_then(|v| text(v).ok())
        .ok_or_else(|| error("pin_dependency_mismatch"))?;
    let deps = body
        .get(deps_name)
        .ok_or_else(|| error("pin_dependency_mismatch"))?;
    let names: BTreeSet<&str> = match deps {
        V::List(v) => v
            .iter()
            .map(text)
            .collect::<Result<_>>()
            .map_err(|_| error("pin_dependency_mismatch"))?,
        V::Map(v) => v.keys().map(String::as_str).collect(),
        _ => return Err(error("pin_dependency_mismatch")),
    };
    require(
        names == pins.keys().chain(gaps.keys()).map(String::as_str).collect(),
        "pin_dependency_mismatch",
    )?;
    if let V::Map(deps) = deps {
        for (name, original) in deps {
            if let Some(pinned) = pins.get(name) {
                require(original == pinned, "pin_dependency_mismatch")?;
            } else {
                require(
                    !matches!(original,V::Text(s) if history_paths::object_id(s)),
                    "positive_reference_pin_gap",
                )?;
            }
        }
    }
    Ok(())
}
pub fn validate_object(value: &V) -> Result<()> {
    crate::history_yaml::validate_value(value, 1_048_576)?;
    let m = schema(
        value,
        &["id", "subject", "kind", "by", "on", "op", "body", "saw"],
        &[
            "id_scheme",
            "schema_version",
            "authored",
            "pins",
            "pin_gaps",
            "at",
            "applies",
        ],
    )?;
    subject(field(m, "subject")?)?;
    id(field(m, "id")?)?;
    token(field(m, "op")?)?;
    let kind = field(m, "kind")?;
    require(
        ["reading", "judgment", "act"]
            .iter()
            .any(|s| string_is(kind, s)),
        "invalid_kind",
    )?;
    require(
        matches!(field(m, "by")?, V::Null | V::Text(_)),
        "invalid_actor",
    )?;
    require(
        matches!(field(m,"on")?,V::Text(s) if !s.is_empty()),
        "invalid_recorded_time",
    )?;
    let typed = m.get("id_scheme").is_some_and(|v| string_is(v, ID_SCHEME));
    let body = field(m, "body")?;
    require(
        matches!(body, V::Map(_))
            || typed && string_is(kind, "reading") && !matches!(body, V::List(_)),
        "invalid_body",
    )?;
    ids(field(m, "saw")?, m.contains_key("id_scheme"))?;
    if m.contains_key("id_scheme") {
        require(
            typed && m.get("schema_version").is_some_and(|v| is_int(v, "2")),
            "unsupported_identity",
        )?;
        if !string_is(kind, "act") {
            require(
                m.contains_key("authored") && m.contains_key("pins"),
                "missing_authored_mapping",
            )?;
            authored(field(m, "authored")?)?;
            pins(field(m, "pins")?)?;
            pin_gaps(m)?;
            if !matches!(body, V::Map(_)) {
                require(
                    map(field(m, "pins")?)?.is_empty()
                        && m.get("pin_gaps")
                            .is_none_or(|v| matches!(v,V::Map(m) if m.is_empty())),
                    "pin_dependency_mismatch",
                )?;
            }
        } else {
            require(
                !["authored", "pins", "pin_gaps"]
                    .iter()
                    .any(|k| m.contains_key(*k)),
                "invalid_act",
            )?;
        }
    } else {
        require(
            !["schema_version", "authored", "pins", "pin_gaps"]
                .iter()
                .any(|k| m.contains_key(*k)),
            "unsupported_identity",
        )?;
        if string_is(kind, "judgment")
            && let Some(v) = map(body)?.get("rests_on")
        {
            pins(v)?;
        }
    }
    if string_is(kind, "act") {
        let body = schema(body, &["act", "of", "over", "because"], &["read"])?;
        let act = text(field(body, "act")?)?;
        require(
            ["accept", "refute", "review", "correct"].contains(&act)
                || typed && ["propose", "retire"].contains(&act),
            "invalid_act",
        )?;
        if ["propose", "retire"].contains(&act) {
            require(
                matches!(field(body,"over")?,V::List(v) if v.is_empty())
                    && matches!(field(body,"because")?,V::Text(s) if nonblank(s)),
                "invalid_act",
            )?;
        }
        id(field(body, "of")?)?;
        ids(field(body, "over")?, typed)?;
        require(matches!(field(body, "because")?, V::Text(_)), "invalid_act")?;
        if let Some(v) = body.get("read") {
            pins(v)?;
        }
    }
    require(
        text(field(m, "id")?)? == typed_object_identity(value)?,
        "identity_mismatch",
    )
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub subject: String,
    pub id: String,
    pub claim: bool,
}
pub fn references(value: &V) -> Result<Vec<Reference>> {
    validate_object(value)?;
    let m = map(value)?;
    let name = text(field(m, "subject")?)?;
    let kind = text(field(m, "kind")?)?;
    let mut result = Vec::new();
    for version in ids(field(m, "saw")?, false)? {
        result.push(Reference {
            subject: name.into(),
            id: version.into(),
            claim: false,
        });
    }
    let pinned = if let Some(v) = m.get("pins") {
        Some(pins(v)?)
    } else if kind == "judgment" {
        map(field(m, "body")?)?
            .get("rests_on")
            .map(pins)
            .transpose()?
    } else {
        None
    };
    if let Some(pinned) = pinned {
        for (s, v) in pinned {
            result.push(Reference {
                subject: s.clone(),
                id: text(v)?.into(),
                claim: true,
            });
        }
    }
    if kind == "act" {
        let body = map(field(m, "body")?)?;
        for version in
            std::iter::once(text(field(body, "of")?)?).chain(ids(field(body, "over")?, false)?)
        {
            result.push(Reference {
                subject: name.into(),
                id: version.into(),
                claim: true,
            });
        }
        if let Some(read) = body.get("read") {
            for (s, v) in pins(read)? {
                result.push(Reference {
                    subject: s.clone(),
                    id: text(v)?.into(),
                    claim: true,
                });
            }
        }
    }
    Ok(result)
}
pub fn validate_closure(objects: &Map) -> Result<()> {
    require(objects.len() <= MAX_OBJECTS, "history_limit")?;
    for (id, value) in objects {
        validate_object(value)?;
        require(
            string_is(field(map(value)?, "id")?, id),
            "identity_mismatch",
        )?;
    }
    for value in objects.values() {
        for reference in references(value)? {
            let target = objects
                .get(&reference.id)
                .ok_or_else(|| error("incomplete_closure"))?;
            let target = map(target)?;
            require(
                string_is(field(target, "subject")?, &reference.subject)
                    && (!reference.claim || !string_is(field(target, "kind")?, "act")),
                "reference_mismatch",
            )?;
        }
    }
    Ok(())
}

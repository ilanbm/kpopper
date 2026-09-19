//! Generated views retain original claim bodies and immutable frontier templates.
use crate::{
    Result,
    history_authority::{self as A, Files, ObjectBytes},
    history_capture::{self as H, Capture},
    history_contract::*,
    history_reduce, history_yaml as Y, require,
    value::TypedValue as V,
};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn truth(v: &V) -> bool {
    match v {
        V::Null => false,
        V::Bool(v) => *v,
        V::Integer(v) => v.as_str() != "0",
        V::Float(v) => v.get() != 0.0,
        V::Text(v) => !v.is_empty(),
        V::List(v) => !v.is_empty(),
        V::Map(v) => !v.is_empty(),
        _ => true,
    }
}
pub(crate) fn map_mut(v: &mut V) -> Result<&mut Map> {
    match v {
        V::Map(m) => Ok(m),
        _ => Err(error("invalid_schema")),
    }
}
pub(crate) fn list(v: &V) -> Result<&[V]> {
    match v {
        V::List(a) => Ok(a),
        _ => Err(error("invalid_schema")),
    }
}
pub fn template(commits: &Files) -> Result<V> {
    let mut manifests = BTreeMap::new();
    let mut parents = BTreeSet::new();
    for (op, raw) in commits {
        let value = Y::decode_document(raw)?;
        A::validate_commit(&value)?;
        parents.extend(map(&map(&value)?["parents"])?.keys().cloned());
        manifests.insert(op, value);
    }
    let mut templates = BTreeMap::new();
    for (op, value) in manifests {
        if !parents.contains(op) {
            let template = map(&value)?
                .get("view_template")
                .ok_or_else(|| error("unresolved_template"))?;
            templates.insert(template.digest()?, template.clone());
        }
    }
    require(!templates.is_empty(), "unresolved_template")?;
    require(templates.len() == 1, "contested_template")?;
    Ok(templates.into_values().next().unwrap())
}

/// Reduce retained bytes, never a reconstruction whose source mapping order changed.
pub(crate) fn selected_state(capture: &Capture, objects: &Map, raw: &ObjectBytes) -> Result<V> {
    validate_closure(objects)?;
    let mut selected = ObjectBytes::new();
    for (id, obj) in objects {
        let subject = text(&map(obj)?["subject"])?;
        let key = (subject.to_owned(), id.clone());
        let bytes = raw.get(&key).ok_or_else(|| error("incomplete_commit"))?;
        require(
            Y::decode_document(bytes)?.digest()? == obj.digest()?,
            "object_bytes_mismatch",
        )?;
        selected.insert(key, bytes.clone());
    }
    history_reduce::reduce_bytes(&selected, Some(map(&map(&capture.state)?["rules"])?), None)
}

fn core_declaration(document: &V, declaration: &V) -> Result<()> {
    let d = map(declaration).map_err(|_| error("invalid_capability"))?;
    require(
        d.len() == 3
            && ["version", "profile", "requires"]
                .iter()
                .all(|k| d.contains_key(*k)),
        "invalid_capability",
    )?;
    require(
        matches!(d["version"], V::Integer(_)) && matches!(d["profile"], V::Text(_)),
        "invalid_capability",
    )?;
    let requirements = list(&d["requires"]).map_err(|_| error("invalid_capability"))?;
    let requirements = requirements
        .iter()
        .map(text)
        .collect::<Result<Vec<_>>>()
        .map_err(|_| error("invalid_capability"))?;
    require(
        requirements.windows(2).all(|w| w[0] < w[1]),
        "invalid_capability",
    )?;
    require(
        (is_int(&d["version"], "1") || is_int(&d["version"], "2"))
            && string_is(&d["profile"], "core/v1"),
        "unsupported_capability",
    )?;
    require(
        requirements
            .iter()
            .all(|v| ["arithmetic/v1", "composition/v1", "query/v1"].contains(v)),
        "unsupported_capability",
    )?;
    require(
        requirements.contains(&"arithmetic/v1"),
        "invalid_capability",
    )?;
    // Typed historical fields require the complete reasoning field-inference
    // boundary. Until it is available, never certify such a v1 declaration.
    if is_int(&d["version"], "1") {
        for (collection, members) in map(document)? {
            if ["meta", "schema", "record", "also"].contains(&collection.as_str()) {
                continue;
            }
            let V::Map(members) = members else {
                continue;
            };
            for body in members.values() {
                let V::Map(body) = body else {
                    continue;
                };
                for seen in body.values() {
                    let V::Map(seen) = seen else {
                        continue;
                    };
                    for old in seen.values() {
                        if map(old)
                            .ok()
                            .and_then(|m| m.get("computed"))
                            .and_then(|v| map(v).ok())
                            .and_then(|m| m.get("version"))
                            .is_some_and(|v| is_int(v, "2"))
                        {
                            return Err(error("unsupported_core_history_fields"));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn render_document(
    capture: &Capture,
    objects: &Map,
    raw: &ObjectBytes,
    commits: &Files,
) -> Result<V> {
    let state = selected_state(capture, objects, raw)?;
    let mut document = template(commits)?;
    let mut roles = map(&document)?
        .get("schema")
        .cloned()
        .unwrap_or(V::Map(Map::new()));
    let roles = map_mut(&mut roles).map_err(|_| error("unresolved_mapping"))?;
    for obj in objects.values() {
        let obj = map(obj)?;
        if string_is(&obj["kind"], "act") {
            continue;
        }
        let authored = obj
            .get("authored")
            .ok_or_else(|| error("unresolved_mapping"))?;
        let collection = text(&map(authored)?["collection"])?;
        require(
            !["meta", "schema", "record", "also"].contains(&collection)
                && map(&document)?
                    .get(collection)
                    .is_some_and(|v| matches!(v, V::Map(_))),
            "unresolved_mapping",
        )?;
    }
    let mut selected_profile: Option<String> = None;
    for (subject, entry) in map(&map(&state)?["subjects"])? {
        let entry = map(entry)?;
        if !string_is(&entry["acceptance"], "accepted") {
            continue;
        }
        let head = entry
            .get("head")
            .ok_or_else(|| error("unresolved_projection"))?;
        for version in list(&entry["heads"])? {
            let obj = map(&objects[text(version)?])?;
            let interpretation = obj
                .get("authored")
                .and_then(|a| map(a).ok())
                .and_then(|a| a.get("locator"))
                .and_then(|a| map(a).ok())
                .and_then(|a| a.get("interpretation"))
                .and_then(|a| map(a).ok());
            require(
                !interpretation.is_some_and(|i| {
                    i.get("scope")
                        .is_some_and(|v| string_is(v, "retained_archive_only"))
                        && i.get("original_condition_profile")
                            .is_some_and(|v| string_is(v, "unknown"))
                }),
                "profile_resolution_required",
            )?;
        }
        let obj = map(&objects[text(head)?])?;
        let authored = map(&obj["authored"])?;
        let profile = text(&authored["profile"])?;
        require(
            selected_profile.as_deref().is_none_or(|v| v == profile),
            "incompatible_authored_profiles",
        )?;
        selected_profile = Some(profile.into());
        for (role, field) in map(&authored["fields"])? {
            require(
                roles.get(role).is_none_or(|v| v == field),
                "incompatible_field_roles",
            )?;
            roles.insert(role.clone(), field.clone());
        }
        let collection = text(&authored["collection"])?;
        let members = map_mut(map_mut(&mut document)?.get_mut(collection).unwrap())?;
        require(
            !members.get(subject).is_some_and(truth),
            "duplicate_projection",
        )?;
        members.insert(subject.clone(), obj["body"].clone());
    }
    let declaration = map(&document)?
        .get("meta")
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("reasoning"))
        .filter(|v| !matches!(v, V::Null));
    if let Some(declaration) = declaration {
        require(
            map(declaration).ok().is_some_and(|d| {
                selected_profile
                    .as_ref()
                    .is_none_or(|p| d.get("profile").is_some_and(|v| string_is(v, p)))
            }),
            "incompatible_authored_profiles",
        )?;
    }
    if selected_profile.as_deref() == Some("core/v1") {
        core_declaration(
            &document,
            declaration.ok_or_else(|| error("missing_reasoning_declaration"))?,
        )?;
    }
    let bound = H::baseline(&capture.marker, commits, &state)?;
    let meta = map_mut(&mut document)?
        .entry("meta".into())
        .or_insert_with(|| V::Map(Map::new()));
    map_mut(meta)?.insert("history".into(), bound);
    Ok(document)
}
pub fn render(
    capture: &Capture,
    objects: &Map,
    raw: &ObjectBytes,
    commits: &Files,
) -> Result<Vec<u8>> {
    crate::history_emit::encode_document(&render_document(capture, objects, raw, commits)?)
}

/// Typed leaf changes preserve absence independently from an explicit null.
pub fn changes(before: &V, after: &V) -> Result<Vec<V>> {
    fn walk(before: &V, after: &V, path: &mut Vec<V>, result: &mut Vec<V>) -> Result<()> {
        if before.digest()? == after.digest()? {
            return Ok(());
        }
        if let (V::Map(left), V::Map(right)) = (before, after) {
            for key in left.keys().chain(right.keys()).collect::<BTreeSet<_>>() {
                path.push(V::Text(key.clone()));
                if let (Some(l), Some(r)) = (left.get(key), right.get(key)) {
                    walk(l, r, path, result)?;
                } else {
                    result.push(change(path, left.get(key), right.get(key)));
                }
                path.pop();
            }
        } else {
            result.push(change(path, Some(before), Some(after)));
        }
        Ok(())
    }
    fn change(path: &[V], before: Option<&V>, after: Option<&V>) -> V {
        let mut m = Map::from([
            ("path".into(), V::List(path.to_vec())),
            ("before_present".into(), V::Bool(before.is_some())),
            ("after_present".into(), V::Bool(after.is_some())),
        ]);
        if let Some(v) = before {
            m.insert("before".into(), v.clone());
        }
        if let Some(v) = after {
            m.insert("after".into(), v.clone());
        }
        V::Map(m)
    }
    let mut result = Vec::new();
    walk(before, after, &mut Vec::new(), &mut result)?;
    Ok(result)
}

#[derive(Clone, Debug)]
pub struct Alternative {
    pub name: String,
    pub bytes: Vec<u8>,
    pub document: V,
}
pub fn alternatives(raw: &[u8]) -> Result<Vec<Alternative>> {
    if let Ok(document) = Y::decode_document(raw) {
        return Ok(vec![Alternative {
            name: "view".into(),
            bytes: raw.to_vec(),
            document,
        }]);
    }
    let marker = regex::bytes::Regex::new(r"^(<{7,}|\|{7,}|={7,}|>{7,})(?:[ \t].*)?$").unwrap();
    let mut sides = [Vec::new(), Vec::new(), Vec::new()];
    let mut active: Option<usize> = None;
    let (mut width, mut count, mut bases) = (0, 0, 0);
    // Python bytes.splitlines recognizes CR, LF and CRLF, not Unicode separators.
    let mut start = 0;
    while start < raw.len() {
        let mut end = start;
        while end < raw.len() && !b"\r\n".contains(&raw[end]) {
            end += 1;
        }
        if end < raw.len() {
            end += if raw[end] == b'\r' && raw.get(end + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
        }
        let line = &raw[start..end];
        start = end;
        let stripped = line.trim_ascii_end_crlf();
        if let Some(found) = marker.captures(stripped) {
            let token = found.get(1).unwrap().as_bytes();
            if token[0] == b'<' {
                require(active.is_none(), "invalid_view_conflict")?;
                active = Some(0);
                width = token.len();
                count += 1;
            } else {
                require(
                    active.is_some() && width == token.len(),
                    "invalid_view_conflict",
                )?;
                match token[0] {
                    b'|' => {
                        require(active == Some(0), "invalid_view_conflict")?;
                        active = Some(2);
                        bases += 1;
                    }
                    b'=' => {
                        require(
                            matches!(active, Some(0 | 2)) && stripped == token,
                            "invalid_view_conflict",
                        )?;
                        active = Some(1);
                    }
                    _ => {
                        require(active == Some(1), "invalid_view_conflict")?;
                        active = None;
                    }
                }
            }
        } else if let Some(i) = active {
            sides[i].extend_from_slice(line);
        } else {
            for side in &mut sides {
                side.extend_from_slice(line);
            }
        }
    }
    require(
        count > 0 && active.is_none() && (bases == 0 || bases == count),
        "invalid_view_conflict",
    )?;
    sides
        .into_iter()
        .enumerate()
        .take(if bases == count { 3 } else { 2 })
        .map(|(i, bytes)| {
            let document = Y::decode_document(&bytes)?;
            Ok(Alternative {
                name: ["ours", "theirs", "base"][i].into(),
                bytes,
                document,
            })
        })
        .collect()
}
trait TrimCrLf {
    fn trim_ascii_end_crlf(&self) -> &[u8];
}
impl TrimCrLf for [u8] {
    fn trim_ascii_end_crlf(&self) -> &[u8] {
        let mut n = self.len();
        while n > 0 && b"\r\n".contains(&self[n - 1]) {
            n -= 1;
        }
        &self[..n]
    }
}

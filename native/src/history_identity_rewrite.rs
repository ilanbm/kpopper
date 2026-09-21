//! Identity references change; provenance strings and historical readings retain their meaning.
use crate::{
    Result,
    history_authoring::{obj, s},
    history_contract::*,
    history_view::{list, map_mut},
    history_yaml::SourceValue as Source,
    reasoning_language as L, require,
    value::TypedValue as V,
};

fn id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}
pub(crate) fn tokens(text: &str, retired: &str, survivor: &str) -> String {
    let mut out = String::new();
    let mut last = 0;
    for (start, _) in text.match_indices(retired) {
        let end = start + retired.len();
        let left = text[..start].chars().next_back();
        let mut right = text[end..].chars();
        let next = right.next();
        if left.is_some_and(|c| id_char(c) || c == '.')
            || next.is_some_and(id_char)
            || next == Some('.') && right.next().is_some_and(id_char)
        {
            continue;
        }
        out.push_str(&text[last..start]);
        out.push_str(survivor);
        last = end;
    }
    out.push_str(&text[last..]);
    out
}
fn render_expression(tree: &V, top: bool) -> Result<String> {
    let tree = map(tree)?;
    if let Some(v) = tree.get("ref") {
        let key = text(v)?;
        let bare = L::legacy_expression(&obj([("expr", s(key))]), false)
            .is_ok_and(|v| v == obj([("ref", s(key))]));
        return Ok(if bare {
            key.into()
        } else {
            format!("ref({})", serde_json::to_string(key)?)
        });
    }
    if let Some(v) = tree.get("num") {
        return Ok(text(v)?.into());
    }
    if let Some(v) = tree.get("text") {
        return Ok(serde_json::to_string(text(v)?)?);
    }
    if let Some(V::Bool(v)) = tree.get("bool") {
        return Ok(if *v { "true" } else { "false" }.into());
    }
    let symbol = match text(field(tree, "op")?)? {
        "add" => "+",
        "sub" => "-",
        "mul" => "*",
        "div" => "/",
        "eq" => "==",
        "ne" => "!=",
        "lt" => "<",
        "le" => "<=",
        "gt" => ">",
        "ge" => ">=",
        _ => return Err(error("invalid_identity_expression")),
    };
    let args = list(field(tree, "args")?)?;
    require(args.len() == 2, "invalid_identity_expression")?;
    let rendered = format!(
        "{} {symbol} {}",
        render_expression(&args[0], false)?,
        render_expression(&args[1], false)?
    );
    Ok(if top {
        rendered
    } else {
        format!("({rendered})")
    })
}
fn rename_tree(tree: &mut V, retired: &str, survivor: &str) -> Result<()> {
    let tree = map_mut(tree)?;
    if tree.get("ref").is_some_and(|v| string_is(v, retired)) {
        tree.insert("ref".into(), s(survivor));
    }
    if let Some(V::List(args)) = tree.get_mut("args") {
        for child in args {
            rename_tree(child, retired, survivor)?;
        }
    }
    Ok(())
}
pub(crate) fn expression(value: &V, retired: &str, survivor: &str) -> Result<V> {
    if !L::legacy_references(value).iter().any(|id| id == retired) {
        return Ok(value.clone());
    }
    let (mut tree, predicate) = match L::legacy_expression(value, false) {
        Ok(tree) => (tree, false),
        Err(_) => (L::legacy_expression(value, true)?, true),
    };
    rename_tree(&mut tree, retired, survivor)?;
    let rendered = obj([("expr", s(&render_expression(&tree, true)?))]);
    require(
        L::legacy_expression(&rendered, predicate)? == tree,
        "identity_expression_changed_meaning",
    )?;
    Ok(rendered)
}
fn rewrite_wrapped(
    value: &V,
    wrapper: &'static str,
    retired: &str,
    survivor: &str,
    predicate: &str,
    snapshot: &str,
) -> Result<V> {
    let raw = crate::history_emit::encode_document(&obj([(wrapper, value.clone())]))?;
    let raw = std::str::from_utf8(&raw).map_err(|_| error("invalid_identity_yaml"))?;
    let changed =
        crate::history_identity_text::rewrite(raw, retired, survivor, predicate, snapshot)?;
    let decoded = crate::history_yaml::decode_document(changed.as_bytes())?;
    Ok(field(map(&decoded)?, wrapper)?.clone())
}
fn validate_source(source: &Source) -> Result<()> {
    let mut todo = vec![(source, 0usize)];
    let mut visits = 0usize;
    while let Some((source, depth)) = todo.pop() {
        visits += 1;
        require(visits <= 100_000 && depth <= 128, "history_limit")?;
        match source {
            Source::Scalar(v) => {
                require(!matches!(v, V::Map(_) | V::List(_)), "invalid_source_value")?;
                v.validate()?;
            }
            Source::List(values) => todo.extend(values.iter().map(|v| (v, depth + 1))),
            Source::Map(values) => todo.extend(values.iter().map(|(_, v)| (v, depth + 1))),
        }
    }
    Ok(())
}
/// Source order matters when two dependency keys collapse to one identity.
pub fn body(source: &Source, retired: &str, survivor: &str, fields: &Map) -> Result<V> {
    require(!retired.is_empty(), "invalid_identity_subjects")?;
    validate_source(source)?;
    let predicate = fields
        .get("predicate")
        .map(text)
        .transpose()?
        .unwrap_or("wrong_if");
    let snapshot = fields
        .get("snapshot")
        .map(text)
        .transpose()?
        .unwrap_or("seen");
    let deps = fields
        .get("deps")
        .map(text)
        .transpose()?
        .unwrap_or("rests_on");
    let original = source.typed();
    let Source::Map(source_map) = source else {
        return rewrite_wrapped(&original, "value", retired, survivor, "wrong_if", "seen");
    };
    let mut value = original.clone();
    let value_map = map_mut(&mut value)?;
    let seen = value_map.remove(snapshot);
    let dep_mapping = source_map
        .iter()
        .find(|(key, v)| key == deps && matches!(v, Source::Map(_)))
        .map(|(_, v)| v);
    if dep_mapping.is_some() {
        value_map.remove(deps);
    }
    let mut protected = Map::new();
    for key in ["file", "path", "url", "uri"] {
        if let Some(value) = value_map.remove(key) {
            protected.insert(key.into(), value);
        }
    }
    let mut rewritten = rewrite_wrapped(&value, "body", retired, survivor, predicate, snapshot)?;
    let rewritten_map = map_mut(&mut rewritten)?;
    rewritten_map.extend(protected);
    if let Some(Source::Map(dependencies)) = dep_mapping {
        let mut renamed = Map::new();
        for (dep, version) in dependencies {
            renamed
                .entry(if dep == retired {
                    survivor.into()
                } else {
                    dep.clone()
                })
                .or_insert_with(|| version.typed());
        }
        rewritten_map.insert(deps.into(), V::Map(renamed));
    }
    if let Some(seen) = seen {
        if seen == V::Null {
            rewritten_map.insert(snapshot.into(), V::Null);
        } else {
            let mut renamed = Map::new();
            for (dep, evidence) in map(&seen).map_err(|_| error("identity_invalid_seen"))? {
                let key = if dep == retired { survivor } else { dep };
                if let Some(previous) = renamed.get(key) {
                    require(
                        V::digest(previous)? == evidence.digest()?,
                        "identity_seen_collision",
                    )?;
                }
                renamed.insert(key.into(), evidence.clone());
            }
            rewritten_map.insert(snapshot.into(), V::Map(renamed));
        }
    }
    if let Some(V::List(dependencies)) = rewritten_map.get_mut(deps) {
        let mut unique = vec![];
        for dependency in dependencies.iter() {
            require(
                !matches!(dependency, V::Map(_) | V::List(_)),
                "identity_invalid_dependencies",
            )?;
            if !unique
                .iter()
                .any(|old| crate::source_clock::python_equal(old, dependency))
            {
                unique.push(dependency.clone());
            }
        }
        *dependencies = unique;
    }
    Ok(rewritten)
}
pub fn mentions(source: &Source, retired: &str, survivor: &str, fields: &Map) -> Result<bool> {
    Ok(body(source, retired, survivor, fields)?.digest()? != source.typed().digest()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_aware_identity_rewrite_preserves_provenance_and_historical_payloads() {
        let data: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/history-identity-rewrite.json"
        ))
        .unwrap();
        let fields = Map::from([
            ("deps".into(), s("rests_on")),
            ("snapshot".into(), s("seen")),
            ("predicate".into(), s("wrong_if")),
        ]);
        for case in data["rewrite"].as_array().unwrap() {
            let source = crate::history_yaml::decode_source_document(
                case["body_source"].as_str().unwrap().as_bytes(),
            )
            .unwrap();
            let source = source.get("body").unwrap();
            let result = body(source, "p.other", "p.input", &fields);
            if let Some(expected) = case.get("output") {
                assert_eq!(
                    result.unwrap(),
                    V::from_tagged(expected).unwrap(),
                    "{}",
                    case["name"]
                );
                assert_eq!(
                    mentions(source, "p.other", "p.input", &fields).unwrap(),
                    case["mentions"].as_bool().unwrap()
                );
            } else {
                assert!(result.is_err(), "{}", case["name"]);
            }
        }
    }
}

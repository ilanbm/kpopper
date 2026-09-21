//! Imported originals remain evidence, never live record authority.
use crate::{
    Result, history_authority as A, history_contract::*, history_transaction as T,
    history_transaction_fs as F, history_view::list, identity::sha256, require,
    value::TypedValue as V,
};
use std::path::Path;

#[derive(Clone)]
pub struct RetainedSources {
    files: A::Files,
    members: Map,
}
impl RetainedSources {
    pub fn files(&self) -> &A::Files {
        &self.files
    }
    pub fn members(&self) -> &std::collections::BTreeMap<String, V> {
        &self.members
    }
}
#[derive(Clone)]
enum Pattern {
    Star,
    Any,
    Literal(char),
    Class(bool, Vec<(char, char)>),
}
fn pattern(text: &str) -> Vec<Pattern> {
    let p: Vec<_> = text.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < p.len() {
        let c = p[i];
        i += 1;
        if c == '*' {
            if !matches!(out.last(), Some(Pattern::Star)) {
                out.push(Pattern::Star);
            }
        } else if c == '?' {
            out.push(Pattern::Any);
        } else if c == '[' {
            let start = i;
            let negate = p.get(i) == Some(&'!');
            let mut end = i + usize::from(negate);
            if p.get(end) == Some(&']') {
                end += 1;
            }
            while end < p.len() && p[end] != ']' {
                end += 1;
            }
            if end == p.len() {
                out.push(Pattern::Literal('['));
                continue;
            }
            let mut ranges = vec![];
            let mut k = start + usize::from(negate);
            while k < end {
                if k + 2 < end && p[k + 1] == '-' {
                    if p[k] <= p[k + 2] {
                        ranges.push((p[k], p[k + 2]));
                    }
                    k += 3;
                } else {
                    ranges.push((p[k], p[k]));
                    k += 1;
                }
            }
            out.push(Pattern::Class(negate, ranges));
            i = end + 1;
        } else {
            out.push(Pattern::Literal(c));
        }
    }
    out
}
pub(crate) fn matches(name: &str, glob: &str) -> Result<bool> {
    let name: Vec<_> = name.chars().collect();
    let pattern = pattern(glob);
    require(
        pattern.len().saturating_mul(name.len() + 1) <= 10_000_000,
        "history_limit",
    )?;
    let mut row = vec![false; name.len() + 1];
    row[0] = true;
    for token in pattern {
        let mut next = vec![false; row.len()];
        if matches!(token, Pattern::Star) {
            next[0] = row[0];
            for j in 1..row.len() {
                next[j] = row[j] || next[j - 1];
            }
        } else {
            for (j, c) in name.iter().enumerate() {
                let accepts = match &token {
                    Pattern::Any => true,
                    Pattern::Literal(v) => v == c,
                    Pattern::Class(negate, ranges) => {
                        ranges.iter().any(|(a, b)| a <= c && c <= b) != *negate
                    }
                    Pattern::Star => unreachable!(),
                };
                next[j + 1] = row[j] && accepts;
            }
        }
        row = next;
    }
    Ok(row[name.len()])
}
/// Capture bounded, hash-checked imported members and validate every YAML pointer.
pub fn capture(root: &Path, entry: &str, document: &V) -> Result<RetainedSources> {
    let mut files = A::Files::new();
    let mut members = Map::new();
    let mut total = 0usize;
    let doc = map(document)?;
    let meta = doc.get("meta").map(map).transpose()?;
    if let Some(imported) = meta
        .and_then(|m| m.get("history_import"))
        .filter(|v| **v != V::Null)
    {
        let imported = schema(
            imported,
            &["version", "operation", "recorded_at", "members"],
            &[],
        )?;
        require(
            is_int(&imported["version"], "1"),
            "unsupported_history_import",
        )?;
        let op = text(&imported["operation"])?;
        require(
            !op.is_empty()
                && op.len() <= 160
                && op.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
                && op
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)),
            "invalid_identifier",
        )?;
        require(
            !text(&imported["recorded_at"])?.is_empty(),
            "invalid_history_import",
        )?;
        let items = list(&imported["members"])?;
        require(items.len() <= MAX_OBJECTS, "history_limit")?;
        for item in items {
            let m = schema(item, &["path", "sha256", "role"], &[])?;
            let name = text(&m["path"])?;
            A::relative_path(name)?;
            let digest = text(&m["sha256"])?;
            require(
                digest.len() == 64 && crate::history_paths::object_id(digest),
                "invalid_identifier",
            )?;
            require(
                ["retained_original", "replaced"]
                    .iter()
                    .any(|r| string_is(&m["role"], r))
                    && !members.contains_key(name)
                    && name != entry,
                "invalid_history_import",
            )?;
            let path = F::target(root, name)?;
            let raw = F::read(&path)?.ok_or_else(|| error("missing_retained_history_file"))?;
            require(
                raw.len() <= 16 * 1024 * 1024,
                "missing_retained_history_file",
            )?;
            total = total.saturating_add(raw.len());
            require(total <= T::MAX_TRANSACTION_BYTES, "history_limit")?;
            require(sha256(&raw) == digest, "retained_history_mismatch")?;
            files.insert(name.into(), raw);
            members.insert(name.into(), item.clone());
        }
    }
    for key in ["record", "also"] {
        let values: Vec<_> = match doc.get(key) {
            Some(V::Text(v)) => vec![v.as_str()],
            Some(V::List(a)) => a.iter().filter_map(|v| text(v).ok()).collect(),
            Some(V::Map(m)) => m.values().filter_map(|v| text(v).ok()).collect(),
            _ => vec![],
        };
        for pointer in values {
            if !pointer.ends_with(".yaml") && !pointer.ends_with(".yml") {
                continue;
            }
            A::relative_path(pointer)?;
            let mut present = false;
            for (name, member) in &members {
                if string_is(&map(member)?["role"], "retained_original") && matches(name, pointer)?
                {
                    present = true;
                    break;
                }
            }
            require(present, "history_composite_capture_unsupported")?;
        }
    }
    Ok(RetainedSources { files, members })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_members_are_hash_checked_and_pointers_require_evidence() {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/history-sources.json")).unwrap();
        for case in data.as_array().unwrap() {
            let root = tempfile::tempdir().unwrap();
            for (name, raw) in case["files"].as_object().unwrap() {
                let path = root.path().join(name);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, raw.as_str().unwrap()).unwrap();
            }
            let doc = V::from_tagged(&case["document"]).unwrap();
            let result = capture(root.path(), "GROUNDING.yaml", &doc);
            if let Some(expected) = case.get("output") {
                assert_eq!(
                    V::Map(result.unwrap().members),
                    V::from_tagged(expected).unwrap(),
                    "{}",
                    case["name"]
                );
            } else {
                assert!(result.is_err(), "accepted {}", case["name"]);
            }
        }
    }
    #[test]
    fn imported_source_pointer_patterns_match_python() {
        let data: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/history-source-patterns.json"
        ))
        .unwrap();
        for case in data.as_array().unwrap() {
            assert_eq!(
                matches(case[0].as_str().unwrap(), case[1].as_str().unwrap()).unwrap(),
                case[2].as_bool().unwrap(),
                "{case}"
            );
        }
    }
}

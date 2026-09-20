//! Post-write reach reports over captured records and their optional sidecars.
use crate::{
    Result,
    history_contract::*,
    history_view::truth,
    history_yaml::OrdinaryValue as O,
    public_ordinary_readers::{World, short},
    source_inventory::Inventory,
    source_text::ordinary_python_str as py,
    value::TypedValue as V,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::LazyLock,
};
static REFERENCE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*\}\}").unwrap()
});
fn get<'a>(v: &'a V, key: &str) -> &'a V {
    match v {
        V::Map(m) => m.get(key).unwrap_or(&V::Null),
        _ => &V::Null,
    }
}
fn list(v: &V) -> &[V] {
    match v {
        V::List(a) => a,
        _ => &[],
    }
}
pub(crate) struct Ancillary {
    pub brief: O,
    pub replaced: V,
    pub inputs: BTreeMap<PathBuf, Option<String>>,
}
impl Default for Ancillary {
    fn default() -> Self {
        Self {
            brief: O::Scalar(V::Null),
            replaced: V::Null,
            inputs: BTreeMap::new(),
        }
    }
}
impl Ancillary {
    pub(crate) fn capture(entry: &Path, inventory: &mut Inventory) -> Result<Self> {
        let layout = crate::history_transaction::Layout::for_entry(
            entry
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| error("invalid_path"))?,
        )?;
        let root = entry.parent().ok_or_else(|| error("invalid_path"))?;
        let mut inputs = BTreeMap::new();
        let brief_path = root.join(layout.view);
        let brief = if inventory.exists(&brief_path)? {
            let bytes = inventory.read(&brief_path)?;
            inputs.insert(brief_path, Some(crate::identity::sha256(&bytes)));
            crate::history_yaml::decode_ordinary_source_value(&bytes)?
        } else {
            inputs.insert(brief_path, None);
            O::Scalar(V::Null)
        };
        let replaced_path = root.join(layout.replaced);
        let replaced = if inventory.file(&replaced_path)? {
            let bytes = inventory.read(&replaced_path)?;
            inputs.insert(replaced_path, Some(crate::identity::sha256(&bytes)));
            crate::history_yaml::decode_ordinary_source_value(&bytes)
                .map(|v| v.projected())
                .unwrap_or(V::Null)
        } else {
            if !inventory.exists(&replaced_path)? {
                inputs.insert(replaced_path, None);
            }
            V::Null
        };
        Ok(Self {
            brief,
            replaced,
            inputs,
        })
    }
}
fn version(versions: &[V], index: usize) -> &V {
    let mut value = versions.get(index).unwrap_or(&V::Null);
    for _ in 0..versions.len() {
        let V::Integer(number) = get(value, "same_as") else {
            break;
        };
        let Some(next) = number
            .as_str()
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|n| versions.get(n))
        else {
            break;
        };
        value = next;
    }
    value
}
pub(crate) fn render(
    kind: &str,
    id: &str,
    world: &World<'_>,
    hypothesis: Option<&str>,
    side: &Ancillary,
) -> Result<Vec<String>> {
    if kind == "review" {
        let (state, why) = world.state(id)?;
        return Ok(vec![format!(
            "  {id} {}: {why}",
            state.to_ascii_lowercase()
        )]);
    }
    let (hit, moved, derived) = world.write_reach(&[id.into()])?;
    let mut reached = hit.into_iter().map(|(id, _)| id).collect::<Vec<_>>();
    let mut out = Vec::new();
    if !derived.is_empty() {
        out.push(format!("worked out from it: {}", derived.join(", ")));
    }
    if kind == "add" && world.judgments.contains_key(id) {
        let (state, why) = world.state(id)?;
        out.push(format!(
            "the new judgment {}: {why}",
            state.to_ascii_lowercase()
        ));
        reached.retain(|name| name != id);
    }
    if !reached.is_empty() {
        out.push(
            hypothesis
                .map(|name| format!("rests on it, under {name}:"))
                .unwrap_or_else(|| "rests on it:".into()),
        );
        reached.sort();
        for id in reached {
            let (state, why) = world.state_with_touched(&id, &moved)?;
            out.push(format!("  {state:<9} {id}: {why}"));
        }
    } else if kind == "set" {
        out.push("nothing rests on it".into());
        if hypothesis.is_none()
            && let V::Map(kept) = &side.replaced
        {
            for (judgment, versions) in kept {
                let versions = list(versions);
                let last = (0..versions.len()).rfind(|index| {
                    list(get(version(versions, *index), "rests_on"))
                        .iter()
                        .any(|v| string_is(v, id))
                });
                if let Some(index) = last {
                    out.push(format!("  listened to by nothing standing - {judgment} listened until {}: pull {judgment} --history", py(get(&versions[index], "day"))));
                }
            }
        }
    }
    if hypothesis.is_some() {
        return Ok(out);
    }
    fn children(value: Option<&O>) -> &[O] {
        match value {
            Some(O::List(a)) => a,
            _ => &[],
        }
    }
    fn repr(value: &O) -> String {
        match value {
            O::Scalar(v) => crate::source_text::ordinary_python_repr(v),
            O::List(a) => format!("[{}]", a.iter().map(repr).collect::<Vec<_>>().join(", ")),
            O::Map(m) => format!(
                "{{{}}}",
                m.iter()
                    .map(|(k, v)| format!(
                        "{}: {}",
                        crate::source_text::ordinary_python_repr(k.scalar()),
                        repr(v)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
    fn source_short(value: &O) -> String {
        let projected = value.projected();
        if matches!(value, O::Scalar(_))
            || map(&projected).is_ok_and(|m| m.contains_key("op") || m.contains_key("expr"))
        {
            short(&projected, 40)
        } else {
            short(&V::Text(repr(value)), 40)
        }
    }
    let mut sections = children(side.brief.get("sections"))
        .iter()
        .collect::<Vec<_>>();
    for tab in children(side.brief.get("tabs")) {
        sections.extend(children(tab.get("sections")));
    }
    let mut texts = Vec::new();
    let keys = std::iter::once(id)
        .chain(derived.iter().map(String::as_str))
        .collect::<Vec<_>>();
    for section in sections {
        let text = section.get("text").map(O::projected).unwrap_or(V::Null);
        if !truth(&text) {
            continue;
        }
        let text = py(&text);
        let refs = REFERENCE
            .captures_iter(&text)
            .map(|m| m[1].to_owned())
            .collect::<Vec<_>>();
        let seen = section.get("seen");
        let hit = keys
            .iter()
            .filter_map(|key| {
                let value = seen.and_then(|s| s.get(key));
                if value.is_none() && !refs.iter().any(|r| r == key) {
                    return None;
                }
                Some(if value.is_none_or(|v| v.projected() == V::Null) {
                    format!("{key} (never read against it)")
                } else {
                    format!("{key} = {}", source_short(value.unwrap()))
                })
            })
            .collect::<Vec<_>>();
        if !hit.is_empty() {
            let title = section.get("title").map(O::projected).unwrap_or(V::Null);
            let title = if truth(&title) {
                py(&title)
            } else {
                "?".into()
            };
            texts.push(format!(
                "  '{title}' saw {} - read it again, then: review \"{title}\"",
                hit.join(", ")
            ));
        }
    }
    if !texts.is_empty() {
        out.push("text that saw it:".into());
        out.extend(texts);
    }
    let mut moved_count = 0;
    let mut falsified = 0;
    let mut flagged = 0;
    for body in world.judgments.values() {
        if crate::reasoning_authoring_guards::arrangement(&world.reader, body) {
            continue;
        }
        let flags = crate::ordinary_counts::flags(&world.reader, body)?;
        moved_count += usize::from(flags.contains("moved"));
        falsified += usize::from(flags.contains("falsified"));
        flagged += usize::from(!flags.is_empty());
    }
    let suffix = if flagged == 0 {
        String::new()
    } else {
        format!(" ({moved_count} moved, {falsified} falsified)")
    };
    out.push(String::new());
    out.push(format!(
        "the record needs a person on {flagged} judgment{}{} - check says the rest",
        if flagged == 1 { "" } else { "s" },
        suffix
    ));
    Ok(out)
}

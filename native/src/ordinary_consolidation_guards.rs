//! Ordinary facts supplied to the shared replacement decision. This adapter
//! reads the real full-domain bodies, never a coerced finite source tree.
use super::*;
use crate::public_consolidation::supersession as S;
pub(super) fn arrangement(world: &Reader<'_>, body: &V) -> bool {
    R::arrangement(world, body)
}
pub(super) fn read_on(body: &V, world: &Reader<'_>) -> Option<String> {
    let m = map(body).ok()?;
    for key in ["of", "read"] {
        if let Some(day) = m.get(key).and_then(super::day) {
            return Some(day);
        }
    }
    let source = m
        .get("from")
        .and_then(|v| text(v).ok())
        .and_then(|id| world.raw.get(id))
        .and_then(|v| map(v).ok())?;
    ["read", "of"]
        .iter()
        .find_map(|key| source.get(key).and_then(super::day))
}
fn predicate_short(value: &V) -> String {
    let value = py(value).split_whitespace().collect::<Vec<_>>().join(" ");
    if value.chars().count() > 60 {
        value.chars().take(57).collect::<String>() + "..."
    } else {
        value
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) fn may_supersede(
    world: &Reader<'_>,
    id: &str,
    existing: &V,
    new: &V,
    captured_day: Option<&str>,
    page: Option<&V>,
    by_hand: bool,
) -> Result<S::Decision> {
    let stamp = || {
        captured_day
            .map(str::to_owned)
            .ok_or_else(|| error("write requires an explicitly captured day"))
    };
    if world.raw.get(id).is_some_and(|body| {
        map(body).is_ok_and(|m| m.contains_key(text(&world.fields["deps"]).unwrap_or("")))
    }) || shaped(existing, &world.fields)
    {
        let deps = text(&world.fields["deps"])?;
        let pred = || get(existing, text(&world.fields["predicate"]).unwrap_or(""));
        return S::judgment(
            shaped(new, &world.fields),
            deps,
            by_hand,
            || {
                let facts = page
                    .map(map)
                    .transpose()
                    .map_err(|_| error("invalid_page_facts"))?
                    .and_then(|page| page.get(id))
                    .map(map)
                    .transpose()
                    .map_err(|_| error("invalid_page_facts"))?;
                let stamp = stamp()?;
                let page = facts
                    .map(|facts| {
                        let boolean = |key| match facts.get(key) {
                            Some(V::Bool(v)) => Ok(*v),
                            _ => Err(error("invalid_page_facts")),
                        };
                        let linked = boolean("linked")?;
                        let fired = boolean("fired")?;
                        for key in ["cut", "reading"] {
                            if let Some(value) = facts.get(key) {
                                text(value).map_err(|_| error("invalid_page_facts"))?;
                            }
                        }
                        if facts
                            .get("stood")
                            .is_some_and(|v| !matches!(v, V::Integer(_)))
                        {
                            return Err(error("invalid_page_facts"));
                        }
                        Ok(S::PageFacts {
                            linked,
                            fired,
                            cut: facts
                                .get("cut")
                                .and_then(|v| text(v).ok())
                                .map(str::to_owned),
                            reading: facts
                                .get("reading")
                                .and_then(|v| text(v).ok())
                                .map(str::to_owned),
                        })
                    })
                    .transpose()?;
                Ok(S::JudgmentEvidence {
                    stamp,
                    born: day(get(existing, "born")),
                    page,
                    predicate: predicate_short(pred()),
                })
            },
            || world.predicate(pred()),
        );
    }
    Ok(S::reading(&stamp()?, read_on(existing, world).as_deref()))
}

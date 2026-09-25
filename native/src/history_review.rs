//! Review reservations derived from captured judgment transitions, never stored state.
use crate::{Result, history_contract::*, history_view::list, value::TypedValue as V};
use std::collections::BTreeSet;

fn known_actor(value: &V) -> bool {
    match value {
        V::Null => false,
        V::Text(text) => !text.trim().is_empty(),
        V::Map(fields) => !fields.is_empty(),
        _ => false,
    }
}
fn meaningful_body(object: &V) -> Result<V> {
    let o = map(object)?;
    let body = map(&o["body"])?;
    let snapshot = o
        .get("authored")
        .and_then(|a| map(a).ok())
        .and_then(|a| a.get("fields"))
        .and_then(|f| map(f).ok())
        .and_then(|f| f.get("snapshot"))
        .and_then(|s| text(s).ok())
        .unwrap_or("seen");
    Ok(V::Map(
        body.iter()
            .filter(|(key, _)| {
                key.as_str() != snapshot
                    && ![
                        "seen", "reviewed", "born", "replaced", "ended", "day", "same_as",
                        "dropped", "of", "as_of", "read", "name", "title",
                    ]
                    .contains(&key.as_str())
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    ))
}
fn arrangement(object: &V, objects: &Map) -> Result<bool> {
    let o = map(object)?;
    let fields = map(&map(&o["authored"])?["fields"])?;
    let body = map(&o["body"])?;
    let dep = text(&fields["deps"])?;
    let Some(deps) = body.get(dep).and_then(|v| list(v).ok()) else {
        return Ok(false);
    };
    let refs = body
        .get(text(&fields["predicate"])?)
        .map(crate::reasoning_language::legacy_references)
        .unwrap_or_default();
    let builtin = deps
        .iter()
        .filter_map(|v| text(v).ok())
        .chain(refs.iter().map(String::as_str))
        .any(|id| crate::reasoning_fields::BUILTINS.contains(&id));
    if !builtin {
        return Ok(false);
    }
    let pins = map(&o["pins"])?;
    Ok(deps.iter().filter_map(|v| text(v).ok()).any(|id| {
        pins.get(id)
            .and_then(|v| text(v).ok())
            .and_then(|id| objects.get(id))
            .and_then(|o| map(o).ok())
            .and_then(|o| map(&o["body"]).ok())
            .and_then(|b| b.get("asked"))
            .is_some_and(crate::history_view::truth)
    }))
}
/// Select accepting acts which change a judgment's meaning. Seeing an old
/// claim is insufficient: an explicit over edge must name that prior claim.
pub(crate) fn resolutions(objects: &Map, state: &V) -> Result<Vec<String>> {
    let state = map(state)?;
    let mut selected = BTreeSet::new();
    for head in list(&state["heads"])? {
        let id = text(head)?;
        let Some(current) = objects.get(id) else {
            return Err(error("missing_accepted_head"));
        };
        let c = map(current)?;
        if !string_is(&c["kind"], "judgment") || arrangement(current, objects)? {
            continue;
        }
        let Some(acts) = map(&state["open_acts"])?.get(id) else {
            continue;
        };
        for act_id in list(acts)? {
            let act_id = text(act_id)?;
            let act = map(objects.get(act_id).ok_or_else(|| error("missing_act"))?)?;
            let body = map(&act["body"])?;
            if !string_is(&body["act"], "accept") || body["of"] != *head {
                continue;
            }
            for old in list(&body["over"])? {
                let old_id = text(old)?;
                if old_id == id {
                    continue;
                }
                let prior = objects
                    .get(old_id)
                    .ok_or_else(|| error("missing_replacement"))?;
                if string_is(&map(prior)?["kind"], "judgment")
                    && meaningful_body(current)? != meaningful_body(prior)?
                {
                    selected.insert(act_id.to_owned());
                    break;
                }
            }
        }
    }
    Ok(selected.into_iter().collect())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReviewState {
    NotRequired,
    Unreviewed,
    Reviewed,
    ProvenanceMissing,
}

/// Evaluate only explicit evidence on this exact version and recorded basis.
/// Legacy snapshots without resolution evidence retain their original meaning.
pub(crate) fn state(projection: &Map, subject: &str, version: &str) -> Result<ReviewState> {
    let Some(d) = projection
        .get("dispositions")
        .and_then(|d| map(d).ok())
        .and_then(|d| d.get(subject))
    else {
        return Ok(ReviewState::NotRequired);
    };
    let d = map(d)?;
    let Some(resolutions) = d.get("resolutions") else {
        return Ok(ReviewState::NotRequired);
    };
    let resolutions = list(resolutions)?
        .iter()
        .filter(|a| {
            map(a)
                .ok()
                .and_then(|a| map(&a["body"]).ok())
                .is_some_and(|b| string_is(&b["of"], version))
        })
        .collect::<Vec<_>>();
    if resolutions.is_empty() {
        return Ok(ReviewState::NotRequired);
    }
    let pins = map(&projection["pins"])?;
    let witness = map(pins
        .get(version)
        .ok_or_else(|| error("missing_pin_witness"))?)?;
    let claim = map(&witness["object"])?;
    let basis = map(&claim["pins"])?;
    let writer = &claim["by"];
    let acceptors = resolutions
        .iter()
        .map(|r| Ok(&map(r)?["by"]))
        .collect::<Result<Vec<_>>>()?;
    let known = known_actor(writer) && acceptors.iter().all(|a| known_actor(a));
    let self_counts = map(&projection["rules"])?.get("self_review_counts") == Some(&V::Bool(true));
    let subjects = map(&projection["subjects"])?;
    for review in list(&d["reviews"])? {
        let r = map(review)?;
        let b = map(&r["body"])?;
        if !string_is(&b["of"], version) {
            continue;
        }
        let seen = list(&r["saw"])?;
        if resolutions
            .iter()
            .any(|a| map(a).is_ok_and(|a| !seen.contains(&a["id"])))
        {
            continue;
        }
        let Some(read) = b.get("read").and_then(|v| map(v).ok()) else {
            continue;
        };
        let mut dependencies = basis.keys().collect::<BTreeSet<_>>();
        if let Some(gaps) = claim.get("pin_gaps") {
            dependencies.extend(map(gaps)?.keys());
        }
        if read.keys().collect::<BTreeSet<_>>() != dependencies {
            continue;
        }
        let current_basis = read.iter().all(|(dep, pin)| {
            subjects
                .get(dep)
                .and_then(|s| map(s).ok())
                .is_some_and(|s| {
                    string_is(&s["acceptance"], "accepted")
                        && list(&s["heads"]).is_ok_and(|heads| heads.contains(pin))
                })
        });
        if !current_basis {
            continue;
        }
        // An exact legacy review acknowledges the pending reading. It does not
        // manufacture independent actor evidence that was never recorded.
        if !known {
            return Ok(ReviewState::Reviewed);
        }
        if known_actor(&r["by"])
            && (self_counts || (&r["by"] != writer && acceptors.iter().all(|a| **a != r["by"])))
        {
            return Ok(ReviewState::Reviewed);
        }
    }
    Ok(if known {
        ReviewState::Unreviewed
    } else {
        ReviewState::ProvenanceMissing
    })
}

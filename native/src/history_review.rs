//! Review reservations derived from captured judgment transitions, never stored state.
use crate::{Result, history_contract::*, history_view::list, require, value::TypedValue as V};
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
    let Some(authored) = o.get("authored").and_then(|a| map(a).ok()) else {
        return Ok(false);
    };
    let Some(fields) = authored.get("fields").and_then(|f| map(f).ok()) else {
        return Ok(false);
    };
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
    let Some(pins) = o.get("pins").and_then(|p| map(p).ok()) else {
        return Ok(false);
    };
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

// Captured review summaries contain IDs and dispositions only. Full source bodies
// and sparse observations remain in the verified capture, not in another history.
pub(crate) const PROFILE: &str = "lineage-review/v1";
const MAX_WORK: usize = 4_000_000;

struct Subject<'a> {
    ids: Vec<&'a str>,
    objects: Vec<&'a Map>,
    index: std::collections::BTreeMap<&'a str, usize>,
    enters: Vec<Vec<usize>>,
    accepts: Vec<Vec<usize>>,
    leaves: Vec<Vec<usize>>,
    claims: Vec<usize>,
    reviews: Vec<usize>,
    direct: &'a dyn Fn(&str, &str) -> Result<bool>,
    past: std::collections::BTreeMap<(usize, usize), bool>,
    meanings: std::collections::BTreeMap<usize, String>,
    work: usize,
}
impl<'a> Subject<'a> {
    fn new(objects: Vec<&'a Map>, direct: &'a dyn Fn(&str, &str) -> Result<bool>) -> Result<Self> {
        let ids = objects
            .iter()
            .map(|o| text(&o["id"]))
            .collect::<Result<Vec<_>>>()?;
        let index = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i))
            .collect::<std::collections::BTreeMap<_, _>>();
        let n = objects.len();
        let mut out = Self {
            ids,
            objects,
            index,
            enters: vec![vec![]; n],
            accepts: vec![vec![]; n],
            leaves: vec![vec![]; n],
            claims: vec![],
            reviews: vec![],
            direct,
            past: Default::default(),
            meanings: Default::default(),
            work: 0,
        };
        for i in 0..n {
            let o = out.objects[i];
            if string_is(&o["kind"], "judgment") {
                out.claims.push(i);
            }
            if !string_is(&o["kind"], "act") {
                continue;
            }
            let b = map(&o["body"])?;
            let Some(&target) = out.index.get(text(&b["of"])?) else {
                continue;
            };
            match text(&b["act"])? {
                "review" => out.reviews.push(i),
                "accept" | "correct" => {
                    out.accepts[target].push(i);
                    for old in list(&b["over"])? {
                        let Some(&old) = out.index.get(text(old)?) else {
                            continue;
                        };
                        if old != target && string_is(&out.objects[old]["kind"], "judgment") {
                            out.enters[target].push(i);
                            out.leaves[old].push(i);
                        }
                    }
                    out.enters[target].sort_unstable();
                    out.enters[target].dedup();
                }
                "retire" | "refute" => out.leaves[target].push(i),
                _ => {}
            }
        }
        Ok(out)
    }
    fn charge(&mut self) -> Result<()> {
        self.work += 1;
        require(self.work <= MAX_WORK, "history_review_limit")
    }
    // Query causal ancestry without materializing cumulative saw sets. Common
    // ordered pairs are answered directly by the compact membership index.
    fn before(&mut self, older: usize, newer: usize) -> Result<bool> {
        if older == newer {
            return Ok(false);
        }
        if let Some(v) = self.past.get(&(older, newer)) {
            return Ok(*v);
        }
        self.charge()?;
        if (self.direct)(self.ids[newer], self.ids[older])? {
            self.past.insert((older, newer), true);
            return Ok(true);
        }
        if (self.direct)(self.ids[older], self.ids[newer])? {
            self.past.insert((older, newer), false);
            return Ok(false);
        }
        let mut visited = BTreeSet::from([newer]);
        let mut queue = vec![newer];
        while let Some(current) = queue.pop() {
            for candidate in 0..self.ids.len() {
                self.charge()?;
                if candidate == current || visited.contains(&candidate) {
                    continue;
                }
                if (self.direct)(self.ids[current], self.ids[candidate])? {
                    if candidate == older {
                        self.past.insert((older, newer), true);
                        return Ok(true);
                    }
                    visited.insert(candidate);
                    queue.push(candidate);
                }
            }
        }
        self.past.insert((older, newer), false);
        Ok(false)
    }
    fn within(&mut self, event: usize, cut: Option<usize>) -> Result<bool> {
        match cut {
            Some(c) => self.before(event, c),
            None => Ok(true),
        }
    }
    fn maximal(&mut self, candidates: Vec<usize>) -> Result<Vec<usize>> {
        let mut frontier = Vec::new();
        for c in candidates.into_iter().collect::<BTreeSet<_>>() {
            let mut dominated = false;
            let mut next = Vec::new();
            for prior in frontier {
                if self.before(c, prior)? {
                    dominated = true;
                }
                if !self.before(prior, c)? {
                    next.push(prior);
                }
            }
            if !dominated {
                next.push(c);
            }
            frontier = next;
        }
        frontier.sort_unstable();
        frontier.dedup();
        Ok(frontier)
    }
    fn meaning(&mut self, id: usize) -> Result<String> {
        if let Some(v) = self.meanings.get(&id) {
            return Ok(v.clone());
        }
        let v = meaningful_body(&V::Map(self.objects[id].clone()))?.digest()?;
        self.meanings.insert(id, v.clone());
        Ok(v)
    }
    fn parents(&self, act: usize) -> Result<Vec<usize>> {
        let b = map(&self.objects[act]["body"])?;
        let target = text(&b["of"])?;
        Ok(list(&b["over"])?
            .iter()
            .filter_map(|v| text(v).ok())
            .filter(|id| *id != target)
            .filter_map(|id| self.index.get(id).copied())
            .filter(|id| string_is(&self.objects[*id]["kind"], "judgment"))
            .collect())
    }
    fn origins(&mut self, head: usize) -> Result<(BTreeSet<(usize, usize)>, BTreeSet<usize>)> {
        let mut origins = BTreeSet::new();
        let mut chain = BTreeSet::new();
        let mut visited = BTreeSet::new();
        let mut todo = vec![(head, None)];
        while let Some((version, cut)) = todo.pop() {
            if !visited.insert((version, cut)) {
                continue;
            }
            self.charge()?;
            chain.insert(version);
            let mut episode = Vec::new();
            for act in self.enters[version].clone() {
                if !self.within(act, cut)? {
                    continue;
                }
                let mut ended = false;
                for leave in self.leaves[version].clone() {
                    if self.within(leave, cut)? && self.before(act, leave)? {
                        ended = true;
                        break;
                    }
                }
                if !ended {
                    episode.push(act)
                }
            }
            let episode = self.maximal(episode)?;
            if !episode.is_empty() {
                for act in episode {
                    for parent in self.parents(act)? {
                        if self.meaning(version)? != self.meaning(parent)? {
                            origins.insert((version, act));
                        } else {
                            todo.push((parent, Some(act)));
                        }
                    }
                }
                continue;
            }
            // Empty-over acceptance cannot reset a retired/refuted subject or
            // turn a new conclusion about that same subject into a fresh root.
            let mut admissions = Vec::new();
            for act in self.accepts[version].clone() {
                if self.within(act, cut)? {
                    admissions.push(act);
                }
            }
            let admissions = self.maximal(admissions)?;
            for act in admissions {
                let mut retired = Vec::new();
                for prior in self.claims.clone() {
                    for leave in self.leaves[prior].clone() {
                        if self.before(leave, act)? {
                            retired.push((prior, leave));
                        }
                    }
                }
                let latest = self.maximal(retired.iter().map(|(_, a)| *a).collect())?;
                let mut parents = retired
                    .into_iter()
                    .filter(|(_, a)| latest.contains(a))
                    .map(|(p, _)| p)
                    .collect::<BTreeSet<_>>();
                if parents.is_empty() {
                    let mut observed = Vec::new();
                    for prior in self.claims.clone() {
                        if prior != version && self.before(prior, version)? {
                            observed.push(prior)
                        }
                    }
                    parents.extend(self.maximal(observed)?);
                }
                for parent in parents {
                    if parent == version || self.meaning(version)? != self.meaning(parent)? {
                        origins.insert((version, act));
                    } else {
                        todo.push((parent, Some(act)));
                    }
                }
            }
        }
        Ok((origins, chain))
    }
    fn summary(&mut self, head: usize, states: &Map, rules: &Map) -> Result<V> {
        let (origins, chain) = self.origins(head)?;
        let self_counts = rules.get("self_review_counts") == Some(&V::Bool(true));
        let mut provenance = true;
        let mut excluded = Vec::new();
        for (claim, act) in &origins {
            let writer = &self.objects[*claim]["by"];
            let acceptor = &self.objects[*act]["by"];
            // Missing provenance cannot erase the other participant's known
            // identity or turn their own review into independent evidence.
            for actor in [writer, acceptor] {
                if known_actor(actor) {
                    excluded.push(actor.clone());
                } else {
                    provenance = false;
                }
            }
        }
        let current = self.objects[head];
        let mut deps = BTreeSet::new();
        if let Some(pins) = current.get("pins").and_then(|v| map(v).ok()) {
            deps.extend(pins.keys().cloned());
        } else if let Some(pins) = map(&current["body"])?
            .get("rests_on")
            .and_then(|v| map(v).ok())
        {
            deps.extend(pins.keys().cloned());
        }
        if let Some(gaps) = current.get("pin_gaps").and_then(|v| map(v).ok()) {
            deps.extend(gaps.keys().cloned());
        }
        let mut satisfied = Vec::new();
        if !origins.is_empty() {
            for review in self.reviews.clone() {
                let r = self.objects[review];
                let b = map(&r["body"])?;
                let Some(&of) = self.index.get(text(&b["of"])?) else {
                    continue;
                };
                if !chain.contains(&of) {
                    continue;
                }
                let Some(read) = b.get("read").and_then(|v| map(v).ok()) else {
                    continue;
                };
                if read.keys().cloned().collect::<BTreeSet<_>>() != deps {
                    continue;
                }
                if !read.iter().all(|(d, pin)| {
                    states.get(d).and_then(|v| map(v).ok()).is_some_and(|v| {
                        string_is(&v["acceptance"], "accepted")
                            && list(&v["heads"]).is_ok_and(|h| h.contains(pin))
                    })
                }) {
                    continue;
                }
                let mut seen = true;
                for (_, act) in &origins {
                    if !self.before(*act, review)? {
                        seen = false;
                        break;
                    }
                }
                if !seen {
                    continue;
                }
                if !excluded.is_empty()
                    && (!known_actor(&r["by"]) || (!self_counts && excluded.contains(&r["by"])))
                {
                    continue;
                }
                satisfied.push(self.ids[review].to_owned());
            }
        }
        satisfied.sort();
        let status = if origins.is_empty() {
            "not_required"
        } else if !satisfied.is_empty() {
            "reviewed"
        } else if provenance {
            "unreviewed"
        } else {
            "provenance_missing"
        };
        let origin_acts = origins
            .iter()
            .map(|(_, a)| self.ids[*a])
            .collect::<BTreeSet<_>>();
        let origin_versions = origins
            .iter()
            .map(|(v, _)| self.ids[*v])
            .collect::<BTreeSet<_>>();
        V::from_json(
            &serde_json::json!({"state":status,"provenance":if provenance{"recorded"}else{"missing"},
            "origins":origin_acts,"origin_versions":origin_versions,"satisfied_by":satisfied}),
        )
    }
}

/// One canonical derivation over the complete, verified capture. The returned
/// summary has no bodies, cumulative observation sets or replicated world maps.
pub(crate) fn summaries(
    objects: &Map,
    state: &V,
    direct: &dyn Fn(&str, &str) -> Result<bool>,
) -> Result<Map> {
    let state = map(state)?;
    let states = map(&state["subjects"])?;
    let rules = map(&state["rules"])?;
    let mut grouped = std::collections::BTreeMap::<&str, Vec<&Map>>::new();
    for value in objects.values() {
        let o = map(value)?;
        grouped.entry(text(&o["subject"])?).or_default().push(o);
    }
    let mut result = Map::new();
    for (subject, value) in states {
        let st = map(value)?;
        let mut engine =
            Subject::new(grouped.remove(subject.as_str()).unwrap_or_default(), direct)?;
        let mut heads = Map::new();
        for head in list(&st["heads"])? {
            let id = text(head)?;
            let Some(&index) = engine.index.get(id) else {
                return Err(error("missing_accepted_head"));
            };
            let object = V::Map(engine.objects[index].clone());
            if !string_is(&map(&object)?["kind"], "judgment") {
                continue;
            }
            let summary = if arrangement(&object, objects)? {
                V::from_json(
                    &serde_json::json!({"state":"not_required","provenance":"recorded","origins":[],"origin_versions":[],"satisfied_by":[]}),
                )?
            } else {
                engine.summary(index, states, rules)?
            };
            heads.insert(id.to_owned(), summary);
        }
        if !heads.is_empty() {
            result.insert(subject.clone(), V::Map(heads));
        }
    }
    Ok(result)
}
pub(crate) fn attach(projection: &V, summaries: &Map) -> Result<V> {
    let mut p = map(projection)?.clone();
    let mut dispositions = map(&p["dispositions"])?.clone();
    for (subject, review) in summaries {
        let d = dispositions
            .get_mut(subject)
            .ok_or_else(|| error("invalid_review_scope"))?;
        crate::history_view::map_mut(d)?.insert("review".into(), review.clone());
    }
    p.insert("review_profile".into(), V::Text(PROFILE.into()));
    p.insert("dispositions".into(), V::Map(dispositions));
    let out = V::Map(p);
    crate::history_projection::validate_projection(&out)?;
    Ok(out)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReviewState {
    NotRequired,
    Unreviewed,
    Reviewed,
    ProvenanceMissing,
}
pub(crate) fn state(projection: &Map, subject: &str, version: &str) -> Result<ReviewState> {
    if projection.get("review_profile") != Some(&V::Text(PROFILE.into())) {
        return Ok(ReviewState::NotRequired);
    }
    let Some(summary) = projection
        .get("dispositions")
        .and_then(|v| map(v).ok())
        .and_then(|d| d.get(subject))
        .and_then(|d| map(d).ok())
        .and_then(|d| d.get("review"))
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get(version))
    else {
        return Ok(ReviewState::NotRequired);
    };
    match text(&map(summary)?["state"])? {
        "not_required" => Ok(ReviewState::NotRequired),
        "unreviewed" => Ok(ReviewState::Unreviewed),
        "reviewed" => Ok(ReviewState::Reviewed),
        "provenance_missing" => Ok(ReviewState::ProvenanceMissing),
        _ => Err(error("invalid_review_state")),
    }
}

pub(crate) fn validate_summary(
    projection: &Map,
    subject: &str,
    disposition: &Map,
    pins: &Map,
    current: &Map,
) -> Result<()> {
    let enabled = projection.get("review_profile") == Some(&V::Text(PROFILE.into()));
    let mut expected = BTreeSet::new();
    for head in list(&current["heads"])? {
        let head = text(head)?;
        if pins
            .get(head)
            .and_then(|v| map(v).ok())
            .and_then(|v| v.get("object"))
            .and_then(|v| map(v).ok())
            .is_some_and(|o| string_is(&o["kind"], "judgment"))
        {
            expected.insert(head.to_owned());
        }
    }
    let Some(review) = disposition.get("review") else {
        return require(
            !enabled || expected.is_empty(),
            "incomplete_review_evidence",
        );
    };
    require(enabled, "missing_review_profile")?;
    let review = map(review)?;
    require(
        review.keys().cloned().collect::<BTreeSet<_>>() == expected,
        "invalid_review_scope",
    )?;
    for (head, result) in review {
        let result = schema(
            result,
            &[
                "state",
                "provenance",
                "origins",
                "origin_versions",
                "satisfied_by",
            ],
            &[],
        )?;
        let state = text(&result["state"])?;
        let provenance = text(&result["provenance"])?;
        require(
            [
                "not_required",
                "unreviewed",
                "reviewed",
                "provenance_missing",
            ]
            .contains(&state)
                && ["recorded", "missing"].contains(&provenance),
            "invalid_review_state",
        )?;
        let origins = ids(&result["origins"], true)?;
        let versions = ids(&result["origin_versions"], true)?;
        let satisfied = ids(&result["satisfied_by"], true)?;
        require(
            match state {
                "not_required" => origins.is_empty() && versions.is_empty() && satisfied.is_empty(),
                "unreviewed" => {
                    !origins.is_empty()
                        && !versions.is_empty()
                        && satisfied.is_empty()
                        && provenance == "recorded"
                }
                "provenance_missing" => {
                    !origins.is_empty()
                        && !versions.is_empty()
                        && satisfied.is_empty()
                        && provenance == "missing"
                }
                "reviewed" => !origins.is_empty() && !versions.is_empty() && !satisfied.is_empty(),
                _ => false,
            },
            "invalid_review_state",
        )?;
        let object = map(&map(&pins[head])?["object"])?;
        require(
            string_is(&object["subject"], subject),
            "invalid_review_scope",
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prototype_judgments_without_authored_metadata_do_not_panic() {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/history-objects.json")).unwrap();
        let fixture = data["objects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["name"] == "legacy-1")
            .unwrap();
        let value = V::from_tagged(&fixture["value"]).unwrap();
        validate_object(&value).unwrap();
        let fields = map(&value).unwrap();
        let id = text(&fields["id"]).unwrap().to_owned();
        let subject = text(&fields["subject"]).unwrap().to_owned();
        let objects = Map::from([(id.clone(), value)]);
        let state=V::from_json(&serde_json::json!({"subjects":{subject.clone():{"heads":[id],"acceptance":"accepted"}},"rules":{}})).unwrap();
        let got = summaries(&objects, &state, &|_, _| Ok(false)).unwrap();
        assert_eq!(
            map(&map(&got[&subject]).unwrap()[&id]).unwrap()["state"],
            V::Text("not_required".into())
        );
    }
}

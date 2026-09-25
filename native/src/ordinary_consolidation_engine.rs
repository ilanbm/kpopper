// Shared consolidation policy, instantiated with finite or full ordinary domain adapters.
fn get<'a>(value: &'a V, key: &str) -> &'a V {
    map(value).ok().and_then(|m| m.get(key)).unwrap_or(&V::Null)
}
fn strings(value: &V) -> Vec<String> {
    match value {
        V::List(a) => a
            .iter()
            .filter_map(|v| text(v).ok().map(str::to_owned))
            .collect(),
        _ => vec![],
    }
}
fn entries(doc: &V) -> Result<Map> {
    Ok(F::collections(doc)?.into_values().flatten().collect())
}
fn claim(body: &V) -> V {
    if let Ok(m) = map(body) {
        for key in ["verdict", "v", "quoted", "rule", "title"] {
            if let Some(v) = m.get(key).filter(|v| **v != V::Null) {
                return if key == "rule" && matches!(v, V::Map(_)) {
                    L::legacy_rule(v).unwrap_or_else(|_| v.clone())
                } else {
                    v.clone()
                };
            }
        }
    }
    body.clone()
}
fn same_claim(a: &V, b: &V) -> bool {
    if matches!(a, V::Map(_) | V::List(_)) || matches!(b, V::Map(_) | V::List(_)) {
        python_equal(a, b)
    } else {
        R::same_legacy(a, b)
    }
}
fn verdict(body: &V) -> V {
    let value = get(body, "verdict");
    if value == &V::Null {
        get(body, "title").clone()
    } else {
        value.clone()
    }
}
fn shaped(body: &V, fields: &Map) -> bool {
    text(&fields["deps"]).ok().is_some_and(|field|matches!(get(body,field),V::List(a)if !a.is_empty()&&a.iter().all(|v|matches!(v,V::Text(_)))))
}
fn version_core(body: &V, snapshot_field: &str) -> V {
    let Ok(m) = map(body) else {
        return body.clone();
    };
    V::Map(
        m.iter()
            .filter(|(k, _)| {
                k.as_str() != snapshot_field
                    && ![
                        "reviewed", "born", "replaced", "ended", "day", "same_as", "dropped",
                    ]
                    .contains(&k.as_str())
            })
            .map(|(k, v)| {
                (
                    k.clone(),
                    if k == "wrong_if" {
                        s(&predicate_text(v))
                    } else {
                        v.clone()
                    },
                )
            })
            .collect(),
    )
}
fn day(value: &V) -> Option<String> {
    let value = py(value);
    let value = value.trim_start();
    let date = value.get(..10)?;
    crate::value::Date::new(date).ok().map(|_| date.into())
}
/// The day a body was read: its own `of` or `read`, else the `read` or `of` of the source
/// it names among `raw`. None when nothing dates it.
fn same_source_scope(left: &V, right: &V) -> bool {
    let (Ok(left), Ok(right)) = (map(left), map(right)) else {
        return false;
    };
    let temporal = [
        "read",
        "of",
        "day",
        "updated",
        "updated_at",
        "observed",
        "observed_at",
        "checked",
        "checked_at",
    ];
    let core = |body: &Map| {
        V::Map(
            body.iter()
                .filter(|(key, _)| !temporal.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    };
    python_equal(&core(left), &core(right))
}
fn read_day(body: &V, raw: &Map) -> Option<String> {
    let m = map(body).ok()?;
    if let Some(found) = ["of", "read"]
        .into_iter()
        .find_map(|key| m.get(key).and_then(day))
    {
        return Some(found);
    }
    let source = m
        .get("from")
        .and_then(|v| text(v).ok())
        .and_then(|id| raw.get(id))
        .and_then(|v| map(v).ok())?;
    ["read", "of"]
        .into_iter()
        .find_map(|key| source.get(key).and_then(day))
}
/// Another branch's committed record as the hypothesis it is laid over the base as: only
/// what it holds differently. An id the base does not hold stays, and so does another
/// claim under an id it does; a judgment stays when the branch changed its semantic core
/// or explicitly refreshed its snapshot, or an arrangement was re-decided with a new born - and an entry when the branch
/// read it on a later day. The rest is the base's own and is left out, so it neither votes
/// on the field roles of the record it is laid over nor contests the branch's other
/// readings. Only collections are kept; the branch's `schema:` and `meta:` stay the
/// branch's.
pub(crate) fn branch_differences(document: &V, base: &World<'_>) -> Result<V> {
    branch_differences_from(document, base, None)
}
pub(crate) fn branch_differences_from(
    document: &V,
    base: &World<'_>,
    comparison_base: Option<&V>,
) -> Result<V> {
    let collections = F::collections(document)?;
    let bodies = map(document)?
        .iter()
        .filter(|(key, _)| !["meta", "schema", "record", "also"].contains(&key.as_str()))
        .filter_map(|(_, members)| map(members).ok())
        .flat_map(|members| members.iter().map(|(id, body)| (id.clone(), body.clone())))
        .collect::<Map>();
    let reader = &base.reader;
    let snapshot_field = text(&reader.fields["snapshot"])?;
    let basis_doc = comparison_base
        .and_then(|value| map(value).ok().and_then(|m| m.get("doc")))
        .or(comparison_base);
    let basis = basis_doc
        .map(entries)
        .transpose()?
        .unwrap_or_else(|| reader.raw.clone());
    let dep = text(&reader.fields["deps"])?;
    let differs = |id: &str, body: &V| {
        if !basis.contains_key(id) {
            return true;
        }
        let old = basis.get(id).unwrap_or(&V::Null);
        if !same_claim(&claim(body), &claim(old)) {
            return true;
        }
        if map(old).is_ok_and(|m| m.contains_key(dep)) {
            // the same verdict on other grounds is a decision written again, and an
            // arrangement re-decided in place carries a new born; the same decision with
            // an explicit refresh of the declared snapshot is the branch's own review
            return matches!((body, old), (V::Map(_), V::Map(_)))
                && (!python_equal(
                    &version_core(body, snapshot_field),
                    &version_core(old, snapshot_field),
                ) || get(body, snapshot_field) != get(old, snapshot_field)
                    || get(body, "reviewed") != get(old, "reviewed")
                    || (G::arrangement(reader, old)
                        && !python_equal(get(body, "born"), get(old, "born"))));
        }
        read_day(body, &bodies)
            .is_some_and(|read| read_day(old, &basis).is_none_or(|held| read > held))
    };
    let mut out = map(document)?.clone();
    out.retain(|name, members| match members {
        V::Map(members) if collections.contains_key(name) => {
            members.retain(|id, body| differs(id, body));
            !members.is_empty()
        }
        _ => false,
    });
    Ok(V::Map(out))
}
fn layer(base: &V, hypothesis: &V) -> Result<V> {
    let mut out = base.clone();
    for (collection, members) in F::collections(hypothesis)? {
        for (name, values) in map_mut(&mut out)?.iter_mut() {
            if *name != collection
                && let V::Map(values) = values
            {
                for id in members.keys() {
                    values.remove(id);
                }
            }
        }
        let target = map_mut(&mut out)?
            .entry(collection)
            .or_insert_with(|| V::Map(Map::new()));
        if !matches!(target, V::Map(_)) {
            *target = V::Map(Map::new());
        }
        map_mut(target)?.extend(members);
    }
    Ok(out)
}
fn checks(projection: &Projection<'_>, brief: Option<&V>) -> Result<(Vec<String>, Vec<String>)> {
    let output = projection.check(brief.filter(|v| **v != V::Null))?.0;
    Ok((
        output
            .lines()
            .filter_map(|l| l.strip_prefix("FAIL ").map(str::to_owned))
            .collect(),
        output
            .lines()
            .filter_map(|l| l.strip_prefix("MOVED ").map(str::to_owned))
            .collect(),
    ))
}
fn projection<'a>(doc: &V, hyps: &Map, runtime: Option<&'a Runtime>) -> Result<Projection<'a>> {
    Projection::new(doc, hyps, &Map::new(), vec![], runtime)
}

struct Update {
    id: String,
    hyp: usize,
    old: V,
    new: V,
    allowed: bool,
    why: String,
}
struct Reversal {
    id: String,
    hyp: usize,
    old: V,
    new: V,
    allowed: bool,
    why: String,
}
struct Union<'a> {
    runtime: Option<&'a Runtime>,
    base_doc: V,
    doc: V,
    base: Projection<'a>,
    view: Option<Projection<'a>>,
    hyps: Vec<Hypothesis>,
    arrived: Vec<(String, usize)>,
    updates: Vec<Update>,
    reversed: Vec<Reversal>,
    refused: Vec<usize>,
    sourced: BTreeSet<String>,
    untaken: Vec<usize>,
    untakeable: BTreeSet<String>,
    drops_needed: BTreeMap<String, Vec<String>>,
    contested: BTreeMap<String, Vec<usize>>,
    moved: Vec<String>,
    falsified: Vec<String>,
    holes: Vec<String>,
    head_falsified: Vec<(usize, String)>,
    new_subjects: Vec<String>,
    candidates: Vec<String>,
}
impl Union<'_> {
    fn facts(&self) -> super::PreviewFacts {
        use super::{PreviewDecision, PreviewFacts, PreviewJudgment};
        let reversal = |r: &Reversal| PreviewDecision {
            hypothesis: self.hyps[r.hyp].name.clone(),
            allowed: r.allowed,
            reason: r.why.clone(),
        };
        PreviewFacts {
            base_ids: self.base.base.reader.ids.clone(),
            contested: self
                .contested
                .iter()
                .map(|(id, holders)| {
                    (
                        id.clone(),
                        holders.iter().map(|i| self.hyps[*i].name.clone()).collect(),
                    )
                })
                .collect(),
            refused: self
                .refused
                .iter()
                .map(|i| {
                    let update = &self.updates[*i];
                    (
                        update.id.clone(),
                        PreviewDecision {
                            hypothesis: self.hyps[update.hyp].name.clone(),
                            allowed: update.allowed,
                            reason: update.why.clone(),
                        },
                    )
                })
                .collect(),
            reversals: self
                .reversed
                .iter()
                .map(|r| (r.id.clone(), reversal(r)))
                .collect(),
            untaken: self
                .untaken
                .iter()
                .map(|i| {
                    let r = &self.reversed[*i];
                    (r.id.clone(), reversal(r))
                })
                .collect(),
            untakeable: self.untakeable.clone(),
            drops_needed: self.drops_needed.clone(),
            moved: self.moved.clone(),
            falsified: self.falsified.clone(),
            holes: self.holes.clone(),
            head_falsified: self
                .head_falsified
                .iter()
                .map(|(i, predicate)| (self.hyps[*i].name.clone(), predicate.clone()))
                .collect(),
            judgments: self
                .view
                .as_ref()
                .map(|view| {
                    view.base
                        .judgments
                        .iter()
                        .map(|(id, body)| {
                            let references =
                                R::predicate_refs(&R::predicate_of(body, &view.base.reader.fields))
                                    .into_iter()
                                    .collect::<BTreeSet<_>>();
                            let page_references = references
                                .iter()
                                .filter(|id| {
                                    id.starts_with("page.") && F::BUILTINS.contains(&id.as_str())
                                })
                                .cloned()
                                .collect();
                            (
                                id.clone(),
                                PreviewJudgment {
                                    predicate_references: references.into_iter().collect(),
                                    page_references,
                                },
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            red: self.red(),
        }
    }
    fn red(&self) -> bool {
        !self.contested.is_empty()
            || !self.refused.is_empty()
            || !self.untaken.is_empty()
            || !self.falsified.is_empty()
            || !self.holes.is_empty()
            || !self.head_falsified.is_empty()
    }
    fn blocked(&self) -> bool {
        self.red() || !self.moved.is_empty()
    }
}

/// Match the optional page application's relevance check over proposed shapes,
/// while retaining the base as the only source of page measurements.
fn needs_page(
    doc: &V,
    base: &Projection<'_>,
    hyps: &[Hypothesis],
    runtime: Option<&Runtime>,
) -> Result<bool> {
    let mut candidate = doc.clone();
    for h in hyps {
        candidate = layer(&candidate, &h.doc)?;
    }
    let candidate = Reader::new(&candidate, runtime)?;
    let reads_page = |body: &V, fields: &Map| {
        strings(get(body, text(&fields["deps"]).unwrap_or("")))
            .iter()
            .any(|id| id.starts_with("page.") && F::BUILTINS.contains(&id.as_str()))
            || R::predicate_refs(&R::predicate_of(body, fields))
                .iter()
                .any(|id| id.starts_with("page.") && F::BUILTINS.contains(&id.as_str()))
    };
    Ok(hyps.iter().flat_map(|h| h.raw.iter()).any(|(id, body)| {
        let old = base.base.reader.raw.get(id).unwrap_or(&V::Null);
        matches!(body, V::Map(_))
            && body != old
            && (G::arrangement(&candidate, body)
                || (base.base.judgments.contains_key(id) && G::arrangement(&base.base.reader, old)))
            && (reads_page(body, &candidate.fields) || reads_page(old, &base.base.reader.fields))
    }))
}

struct UnionInput<'i, 'r> {
    doc: &'i V,
    all: &'i Map,
    hyps: Vec<Hypothesis>,
    base: Projection<'r>,
    runtime: Option<&'r Runtime>,
    stamp: &'i str,
    take: &'i [String],
    drops: &'i Map,
    brief: Option<&'i V>,
    page: Option<&'i V>,
}
fn union<'a>(input: UnionInput<'_, 'a>) -> Result<Union<'a>> {
    let UnionInput {
        doc,
        all,
        hyps,
        base,
        runtime,
        stamp,
        take,
        drops,
        brief,
        page,
    } = input;
    let mut c = Union {
        runtime,
        base_doc: doc.clone(),
        doc: doc.clone(),
        base,
        view: None,
        hyps,
        arrived: vec![],
        updates: vec![],
        reversed: vec![],
        refused: vec![],
        sourced: BTreeSet::new(),
        untaken: vec![],
        untakeable: BTreeSet::new(),
        drops_needed: BTreeMap::new(),
        contested: BTreeMap::new(),
        moved: vec![],
        falsified: vec![],
        holes: vec![],
        head_falsified: vec![],
        new_subjects: vec![],
        candidates: vec![],
    };
    let mut holders = BTreeMap::<String, Vec<usize>>::new();
    let mut review_checks = Vec::<(String, Map, Vec<String>, Map)>::new();
    for (i, h) in c.hyps.iter().enumerate() {
        for id in &h.ids {
            holders.entry(id.clone()).or_default().push(i);
        }
    }
    for (id, hs) in &holders {
        let first = claim(&c.hyps[hs[0]].raw[id]);
        if hs
            .iter()
            .skip(1)
            .any(|i| !same_claim(&first, &claim(&c.hyps[*i].raw[id])))
        {
            c.contested.insert(id.clone(), hs.clone());
        }
    }
    if !c.contested.is_empty() {
        return Ok(c);
    }
    for h in &c.hyps {
        c.doc = layer(&c.doc, &h.doc)?;
    }
    c.view = Some(projection(&c.doc, &Map::new(), runtime)?);
    let world = &c.view.as_ref().unwrap().base;
    let base = &c.base.base;
    let snapshot_field = text(&base.reader.fields["snapshot"])?;
    let meta = map(get(doc, "meta"))
        .ok()
        .map(|m| m.keys().cloned().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    for (id, hs) in &holders {
        if meta.contains(id) {
            continue;
        }
        let hi = *hs.last().unwrap();
        let h = &c.hyps[hi];
        let new = &h.raw[id];
        if !base.reader.ids.contains(id) {
            c.arrived.push((id.clone(), hi));
            continue;
        }
        let old = base.reader.raw.get(id).unwrap_or(&V::Null);
        let (old_claim, new_claim) = (claim(old), claim(new));
        let same = same_claim(&old_claim, &new_claim);
        if base.judgments.contains_key(id) {
            let fork = get(&h.head, "_comparison_base");
            let fork_doc = map(fork).ok().and_then(|m| m.get("doc")).unwrap_or(fork);
            let fork_body = entries(fork_doc).ok().and_then(|all| all.get(id).cloned());
            let is_review_of_fork = fork_body.as_ref().is_some_and(|forked| {
                python_equal(
                    &version_core(forked, snapshot_field),
                    &version_core(new, snapshot_field),
                ) && (get(forked, snapshot_field) != get(new, snapshot_field)
                    || get(forked, "reviewed") != get(new, "reviewed"))
            });
            if is_review_of_fork && let Ok(snapshot) = map(get(new, snapshot_field)) {
                review_checks.push((
                    id.clone(),
                    snapshot.clone(),
                    strings(get(new, text(&base.reader.fields["deps"])?)),
                    entries(fork_doc).unwrap_or_default(),
                ));
            }
        }
        if same && old == new {
            continue;
        }
        let (allowed, mut why);
        if base.judgments.contains_key(id) {
            if same
                && matches!((old, new), (V::Map(_), V::Map(_)))
                && version_core(old, snapshot_field) == version_core(new, snapshot_field)
                && !(G::arrangement(&base.reader, old) && get(old, "born") != get(new, "born"))
            {
                continue;
            }
            if !matches!(new, V::Map(_)) || !shaped(new, &base.reader.fields) {
                allowed = false;
                why="the base holds a judgment under this id and the hypothesis an entry - a subject does not change kind at the fold, and no name takes this: give it what it rests on and a verdict, or a new id".into();
                c.untakeable.insert(id.clone());
            } else if G::arrangement(&base.reader, old) && !G::arrangement(&base.reader, new) {
                allowed = false;
                why="what replaces an arrangement is an arrangement - rest on the session sources of the occasion it decides and give it a sign over a count; no name takes a body that drops them".into();
                c.untakeable.insert(id.clone());
            } else if !G::arrangement(&base.reader, old) && G::arrangement(&base.reader, new) {
                allowed = false;
                why="the base holds a judgment under this id and the hypothesis an arrangement - an arrangement is born when it is written in place, and no name takes one over a judgment: write it as its own decision".into();
                c.untakeable.insert(id.clone());
            } else {
                let comparison_base = get(&h.head, "_comparison_base");
                let source_origin = map(comparison_base)
                    .ok()
                    .and_then(|m| m.get("doc"))
                    .unwrap_or(comparison_base);
                let source_origin = entries(source_origin)
                    .ok()
                    .and_then(|values| values.get(id).cloned());
                let only_reviewed_forked_judgment = source_origin.as_ref().is_some_and(|origin| {
                    python_equal(
                        &version_core(origin, snapshot_field),
                        &version_core(new, snapshot_field),
                    ) && !python_equal(
                        &version_core(old, snapshot_field),
                        &version_core(new, snapshot_field),
                    )
                });
                if only_reviewed_forked_judgment {
                    allowed = false;
                    why = "the branch only reviewed its fork's judgment, while the destination has since changed it - that review cannot take the older verdict".into();
                } else {
                    let decision = G::may_supersede(
                        &base.reader,
                        id,
                        old,
                        new,
                        Some(stamp),
                        page,
                        take.contains(id),
                    )?;
                    allowed = decision.allowed;
                    why = decision.reason;
                }
                let pred = R::predicate_of(&base.judgments[id], &base.reader.fields);
                if !allowed && truth(&pred) && world.reader.predicate(&pred)? == Some(true) {
                    why += &format!(
                        " - it would hold with {}'s readings ({})",
                        h.name,
                        short(&pred, 60)
                    );
                }
                if allowed && !G::arrangement(&base.reader, old) {
                    let dep = text(&base.reader.fields["deps"])?;
                    let now = strings(get(new, dep));
                    let gone = strings(get(old, dep))
                        .into_iter()
                        .filter(|d| !now.contains(d) && !drops.contains_key(d))
                        .collect::<Vec<_>>();
                    if !gone.is_empty() {
                        c.drops_needed.insert(id.clone(), gone);
                    }
                }
            }
            if !allowed {
                c.untaken.push(c.reversed.len());
            }
            c.reversed.push(Reversal {
                id: id.clone(),
                hyp: hi,
                old: old.clone(),
                new: new.clone(),
                allowed,
                why: why.clone(),
            });
        } else {
            let read = G::read_on(new, &world.reader);
            let old_day = G::read_on(old, &base.reader);
            if shaped(new, &base.reader.fields) {
                allowed = false;
                why="the base holds an entry under this id and the hypothesis a judgment - a subject does not change kind at the fold: set the entry, or give the judgment a new id".into();
            } else if read.as_ref().is_some_and(|d| d.as_str() > stamp) {
                allowed = false;
                why = format!(
                    "a reading dated {}, after today ({stamp}) - a day is the record's clock, and a day ahead is not read",
                    read.as_ref().unwrap()
                );
            } else if h.path.is_none()
                && matches!((old, new), (V::Map(_), V::Map(_)))
                && truth(get(new, "from"))
                && truth(get(old, "from"))
                && py(get(new, "from")) != py(get(old, "from"))
                && !same
            {
                if take.contains(id) {
                    allowed = true;
                    why = format!(
                        "a reading from {} where the base reads from {} - taken by name",
                        py(get(new, "from")),
                        py(get(old, "from"))
                    );
                } else {
                    allowed = false;
                    why = format!(
                        "a reading from {} where the base reads from {} - one id follows one source; take it by name: consolidate {} --take {id}",
                        py(get(new, "from")),
                        py(get(old, "from")),
                        h.name
                    );
                    c.sourced.insert(id.clone());
                }
            } else if read.is_none()
                && let Some(old_day) = old_day
            {
                allowed = false;
                why = format!("the reading is undated, and the base's is from {}", old_day);
            } else {
                let decision = G::may_supersede(
                    &base.reader,
                    id,
                    old,
                    &new_claim,
                    Some(read.as_deref().unwrap_or(stamp)),
                    None,
                    false,
                )?;
                allowed = decision.allowed;
                why = decision.reason;
            }
            if same && !allowed {
                continue;
            }
        }
        if !allowed && !base.judgments.contains_key(id) {
            c.refused.push(c.updates.len());
        }
        c.updates.push(Update {
            id: id.clone(),
            hyp: hi,
            old: old_claim,
            new: new_claim,
            allowed,
            why,
        });
    }
    let (before_fail, before_moved) = checks(&c.base, brief)?;
    let (after_fail, after_moved) = checks(c.view.as_ref().unwrap(), None)?;
    let missing_reference = regex::Regex::new(r"rests on (\S+), which is not an entry").unwrap();
    for mut line in after_fail {
        if before_fail.contains(&line) {
            continue;
        }
        let id = line.split(':').next().unwrap();
        let falsified = world
            .judgments
            .get(id)
            .is_some_and(|b| C::flags(&world.reader, b).is_ok_and(|f| f.contains("falsified")));
        if falsified {
            c.falsified.push(line);
        } else {
            if let Some(cap) = missing_reference.captures(&line) {
                let key = &cap[1];
                // A hypothesis holding the missing id folds together with this one; a
                // pending finding folds with nothing here, and a finding's own test names none.
                let finding = |name: &str| {
                    all.get(name)
                        .is_some_and(|h| get(h, "kind") == &s("contribution"))
                };
                let others = all
                    .iter()
                    .filter(|(name, h)| {
                        !c.hyps.iter().any(|x| finding(&x.name))
                            && !finding(name)
                            && !c.hyps.iter().any(|x| &x.name == *name)
                            && entries(get(h, "doc")).is_ok_and(|e| e.contains_key(key))
                    })
                    .map(|(n, _)| n.clone())
                    .collect::<Vec<_>>();
                if !others.is_empty() {
                    line += &format!(
                        " - held by hypothes{} {}: consolidate them together",
                        if others.len() == 1 { "is" } else { "es" },
                        others.join(", ")
                    );
                }
            }
            c.holes.push(line);
        }
    }
    c.moved = after_moved
        .into_iter()
        .filter(|l| !before_moved.contains(l))
        .collect();
    for (judgment, snapshot, dependencies, fork_raw) in review_checks {
        for dependency in dependencies {
            let current = base.reader.value(&dependency).ok();
            let reviewed = snapshot.get(&dependency);
            let value_matches =
                reviewed
                    .zip(current.as_ref())
                    .is_some_and(|(reviewed, current)| {
                        python_equal(reviewed, current)
                            || text(reviewed)
                                .ok()
                                .is_some_and(|value| value.starts_with("read "))
                                && fork_raw
                                    .get(&dependency)
                                    .zip(base.reader.raw.get(&dependency))
                                    .is_some_and(|(fork, current)| same_source_scope(fork, current))
                    });
            let basis_changed = !value_matches;
            let incoming_reading_accepted = c
                .updates
                .iter()
                .any(|update| update.id == dependency && update.allowed);
            if basis_changed && !incoming_reading_accepted {
                c.moved.push(format!(
                    "{judgment}: {dependency} differs from the source review's snapshot"
                ));
            }
        }
    }
    for (i, h) in c.hyps.iter().enumerate() {
        let v = get(&h.head, "wrong_if");
        let pred = if truth(v) { py(v) } else { String::new() };
        if pred.is_empty() {
            continue;
        }
        match world.reader.predicate(&s(&pred))?{Some(true)=>c.head_falsified.push((i,pred)),None=>c.holes.push(format!("{}: its wrong_if is not a comparison this reader decides ({}) - a falsifier nothing evaluates tests nothing",h.name,short(&s(&pred),60))),_=>{}}
    }
    let have = base
        .reader
        .ids
        .iter()
        .filter_map(|id| id.split_once('.').map(|(p, _)| p.to_owned()))
        .collect::<BTreeSet<_>>();
    c.new_subjects = c
        .arrived
        .iter()
        .filter_map(|(id, _)| id.split_once('.').map(|(p, _)| p.to_owned()))
        .filter(|p| !have.contains(p))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    c.candidates = report::candidates(&c, all)?;
    Ok(c)
}

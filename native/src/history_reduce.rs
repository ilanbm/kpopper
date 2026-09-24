//! Pure acceptance and act reduction. No predicate or review is evaluated here.
use crate::{
    Result,
    history_authority::ObjectBytes,
    history_contract::*,
    history_yaml::{self, SourceValue as S},
    require,
    source_clock::{self, Ancestry},
    source_text::python_str,
    value::{Integer, TypedValue as V},
};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

pub const MAX_REDUCTION_WORK: usize = 4_000_000;
pub(crate) trait SawProvider {
    fn saw_is_empty(&self, id: &str) -> Result<bool>;
    fn saw_contains(&self, observer: &str, target: &str) -> Result<bool>;
}
fn s(value: &str) -> V {
    V::Text(value.into())
}
fn n(value: usize) -> V {
    V::Integer(Integer::new(&value.to_string()).expect("count"))
}
fn strings<'a>(values: impl IntoIterator<Item = &'a String>) -> V {
    V::List(values.into_iter().map(|v| s(v)).collect())
}
fn list(v: &V) -> &[V] {
    let V::List(a) = v else {
        unreachable!("validated list")
    };
    a
}
pub(crate) fn claim_meaning(obj: &Map) -> Result<String> {
    let mut meaning: Map = ["kind", "body", "pins", "authored"]
        .into_iter()
        .map(|key| (key.into(), obj.get(key).cloned().unwrap_or(V::Null)))
        .collect();
    if let Some(V::Map(gaps)) = obj.get("pin_gaps")
        && !gaps.is_empty()
    {
        meaning.insert("pin_gaps".into(), V::Map(gaps.clone()));
    }
    if let Some(V::Map(authored)) = meaning.get_mut("authored") {
        authored.remove("locator");
        authored.remove("hypothesis");
    }
    V::Map(meaning).digest()
}
/// Values constructed in Rust have canonical mapping order. Filesystem consumers
/// use reduce_bytes to preserve each original source mapping's insertion order.
pub fn reduce(objects: &Map, rules: Option<&Map>, ancestry: Option<&Ancestry<'_>>) -> Result<V> {
    validate_closure(objects)?;
    let sources = objects
        .iter()
        .map(|(k, v)| (k.clone(), S::from_typed(v)))
        .collect();
    reduce_sources(objects, &sources, rules, ancestry)
}
pub(crate) fn reduce_compact(
    objects: &Map,
    saw: &dyn SawProvider,
    rules: Option<&Map>,
    ancestry: Option<&Ancestry<'_>>,
) -> Result<V> {
    let sources = objects
        .iter()
        .map(|(k, v)| (k.clone(), S::from_typed(v)))
        .collect();
    reduce_sources_with(objects, &sources, rules, ancestry, Some(saw))
}
pub fn reduce_bytes(
    raw: &ObjectBytes,
    rules: Option<&Map>,
    ancestry: Option<&Ancestry<'_>>,
) -> Result<V> {
    require(raw.len() <= MAX_OBJECTS, "history_limit")?;
    let mut objects = Map::new();
    let mut sources = BTreeMap::new();
    for ((subject, version), raw) in raw {
        let source = history_yaml::decode_source_document(raw)?;
        let value = source.typed();
        validate_object(&value)?;
        let m = map(&value)?;
        require(
            string_is(&m["id"], version) && string_is(&m["subject"], subject),
            "reference_mismatch",
        )?;
        objects.insert(version.clone(), value);
        sources.insert(version.clone(), source);
    }
    validate_closure(&objects)?;
    reduce_sources(&objects, &sources, rules, ancestry)
}
fn reduce_sources(
    objects: &Map,
    sources: &BTreeMap<String, S>,
    rules: Option<&Map>,
    ancestry: Option<&Ancestry<'_>>,
) -> Result<V> {
    reduce_sources_with(objects, sources, rules, ancestry, None)
}
fn reduce_sources_with(
    objects: &Map,
    sources: &BTreeMap<String, S>,
    rules: Option<&Map>,
    ancestry: Option<&Ancestry<'_>>,
    saw_provider: Option<&dyn SawProvider>,
) -> Result<V> {
    let mut active_rules = Map::from([
        ("version".into(), n(2)),
        ("self_review_counts".into(), V::Bool(false)),
    ]);
    if let Some(rules) = rules {
        active_rules.extend(rules.clone());
    }
    history_yaml::validate_value(&V::Map(active_rules.clone()), 1_048_576)?;
    let mut grouped: BTreeMap<String, Map> = BTreeMap::new();
    for (version, obj) in objects {
        grouped
            .entry(text(&map(obj)?["subject"])?.into())
            .or_default()
            .insert(version.clone(), obj.clone());
    }
    require(
        grouped.values().map(|v| v.len() * v.len()).sum::<usize>() <= MAX_REDUCTION_WORK,
        "history_limit",
    )?;
    let mut subjects = Map::new();
    let mut implied = Vec::new();
    for (subject, held) in grouped {
        let kinds = held
            .values()
            .map(|v| text(&map(v)?["kind"]))
            .collect::<Result<BTreeSet<_>>>()?;
        require(
            kinds.iter().filter(|k| **k != "act").count() <= 1,
            "mixed_subject_kind",
        )?;
        if let Some(entry) = subject_entry(&subject, &held, sources, ancestry, saw_provider)? {
            implied.extend(list(&map(&entry)?["implied"]).iter().cloned());
            subjects.insert(subject, entry);
        }
    }
    implied.sort_by_key(|v| {
        let m = map(v).expect("implied record");
        ["subject", "superseded", "by"].map(|k| text(&m[k]).expect("identifier").to_owned())
    });
    Ok(V::Map(Map::from([
        ("subjects".into(), V::Map(subjects)),
        ("rules".into(), V::Map(active_rules)),
        ("implied".into(), V::List(implied)),
    ])))
}
fn subject_entry(
    subject: &str,
    held: &Map,
    sources: &BTreeMap<String, S>,
    ancestry: Option<&Ancestry<'_>>,
    saw_provider: Option<&dyn SawProvider>,
) -> Result<Option<V>> {
    let mut claims = BTreeMap::new();
    let mut acts = BTreeMap::new();
    for (version, v) in held {
        let m = map(v)?;
        if string_is(&m["kind"], "act") {
            acts.insert(version, m);
        } else {
            claims.insert(version, m);
        }
    }
    if claims.is_empty() {
        return Ok(None);
    }
    let mut ordered: Vec<_> = acts.values().copied().collect();
    ordered.sort_by_key(|v| (text(&v["on"]).expect("time"), text(&v["id"]).expect("id")));
    let mut roots = BTreeSet::new();
    for (id, v) in &claims {
        if saw_provider.map_or_else(|| Ok(list(&v["saw"]).is_empty()), |p| p.saw_is_empty(id))? {
            roots.insert((*id).clone());
        }
    }
    let mut words: BTreeMap<String, Vec<(&Map, String)>> = BTreeMap::new();
    let mut reviews = Vec::new();
    for a in ordered {
        let b = map(&a["body"])?;
        let target = text(&b["of"])?;
        let action = text(&b["act"])?;
        if !claims.contains_key(&target.to_owned()) {
            continue;
        }
        match action {
            "accept" | "correct" => {
                words
                    .entry(target.into())
                    .or_default()
                    .push((a, "stands".into()));
                for old in list(&b["over"]) {
                    let old = text(old)?;
                    if old != target && claims.contains_key(&old.to_owned()) {
                        words.entry(old.into()).or_default().push((
                            a,
                            format!(
                                "{}:{target}",
                                if action == "correct" {
                                    "corrected"
                                } else {
                                    "replaced"
                                }
                            ),
                        ));
                    }
                }
            }
            "refute" | "propose" | "retire" => {
                words.entry(target.into()).or_default().push((
                    a,
                    match action {
                        "refute" => "out",
                        "propose" => "proposed",
                        _ => "retired",
                    }
                    .into(),
                ));
            }
            "review" => {
                let read = b.get("read").cloned().unwrap_or(V::Map(Map::new()));
                reviews.push(V::Map(Map::from([
                    ("of".into(), b["of"].clone()),
                    ("by".into(), a["by"].clone()),
                    ("read".into(), read),
                ])));
            }
            _ => unreachable!("validated act"),
        }
    }
    let mut standing = Vec::new();
    let mut proposals = Vec::new();
    let mut disputed = BTreeSet::new();
    let mut marks = Map::new();
    let mut accepted_by = Map::new();
    let mut open_acts = Map::new();
    for id in claims.keys() {
        let Some(ws) = words.get(*id) else {
            if roots.contains(*id) {
                standing.push((*id).clone());
                accepted_by.insert((*id).clone(), V::Null);
            } else {
                proposals.push((*id).clone());
            }
            continue;
        };
        let mut answered = BTreeSet::new();
        for (a, _) in ws {
            for (b, _) in ws {
                if a["id"] != b["id"]
                    && saw_provider.map_or_else(
                        || Ok(list(&b["saw"]).contains(&a["id"])),
                        |p| p.saw_contains(text(&b["id"])?, text(&a["id"])?),
                    )?
                {
                    answered.insert(text(&a["id"])?.to_owned());
                }
            }
        }
        let open: Vec<_> = ws
            .iter()
            .filter(|(a, _)| !answered.contains(text(&a["id"]).expect("id")))
            .collect();
        let mut ids: Vec<_> = open
            .iter()
            .map(|(a, _)| text(&a["id"]).expect("id").to_owned())
            .collect();
        ids.sort();
        open_acts.insert((*id).clone(), strings(&ids));
        let kinds: BTreeSet<_> = open
            .iter()
            .map(|(_, w)| w.split(':').next().expect("word"))
            .collect();
        if kinds.len() == 1 && kinds.contains("stands") {
            standing.push((*id).clone());
            accepted_by.insert((*id).clone(), open.last().expect("stands").0["by"].clone());
        } else if kinds.len() == 1 && kinds.contains("proposed") {
            proposals.push((*id).clone());
        } else if !kinds.contains("stands") {
            marks.insert(
                (*id).clone(),
                s(if kinds.contains("corrected") {
                    "corrected"
                } else if kinds.contains("out") {
                    "refuted"
                } else if kinds.contains("retired") {
                    "retired"
                } else {
                    "replaced"
                }),
            );
        } else {
            standing.push((*id).clone());
            disputed.insert((*id).clone());
            accepted_by.insert(
                (*id).clone(),
                open.iter().find(|(_, w)| w == "stands").expect("stands").0["by"].clone(),
            );
        }
    }
    let kind = &claims.values().next().expect("claims")["kind"];
    let mut implied = Vec::new();
    if string_is(kind, "reading") && standing.len() > 1 {
        let null = S::Scalar(V::Null);
        let mut groups: Vec<Vec<String>> = Vec::new();
        let mut index = BTreeMap::new();
        for id in &standing {
            let source = &sources[id];
            let from = source
                .get("body")
                .and_then(|b| b.get("from"))
                .unwrap_or(&null);
            let clock = source
                .get("at")
                .and_then(source_clock::clock_parts)
                .map(|(k, _)| k.to_owned());
            let key = (python_str(from), clock);
            let i = *index.entry(key).or_insert_with(|| {
                groups.push(Vec::new());
                groups.len() - 1
            });
            groups[i].push(id.clone());
        }
        let mut keep: BTreeSet<_> = standing.iter().cloned().collect();
        for group in groups {
            if group.len() < 2 {
                continue;
            }
            let Some((clock, _)) = sources[&group[0]]
                .get("at")
                .and_then(source_clock::clock_parts)
            else {
                continue;
            };
            if clock == "commit" {
                for id in &group {
                    for other in &group {
                        if id != other
                            && keep.contains(id)
                            && source_clock::order(
                                sources[id].get("at").expect("clock"),
                                sources[other].get("at").expect("clock"),
                                ancestry,
                            )? == Some(Ordering::Less)
                        {
                            keep.remove(id);
                            let value = |id: &String| {
                                python_str(
                                    source_clock::clock_parts(
                                        sources[id].get("at").expect("clock"),
                                    )
                                    .expect("commit")
                                    .1,
                                )
                            };
                            implied.push(implied_word(
                                subject,
                                id,
                                other,
                                &format!("commit {} descends from {}", value(other), value(id)),
                            ));
                            break;
                        }
                    }
                }
                continue;
            }
            let keyed = group
                .iter()
                .map(|id| source_clock::key(sources[id].get("at").expect("clock")))
                .collect::<Result<Vec<_>>>()?;
            if keyed.iter().any(Option::is_none) {
                continue;
            }
            let keyed: Vec<_> = keyed.into_iter().map(Option::unwrap).collect();
            let mut top = 0;
            for i in 1..keyed.len() {
                if keyed[i].compare(&keyed[top])? == Ordering::Greater {
                    top = i;
                }
            }
            // Group IDs retain the sorted standing order, so the first equal maximum wins.
            for (i, id) in group.iter().enumerate() {
                if keyed[i].compare(&keyed[top])? == Ordering::Less {
                    keep.remove(id);
                    let value = |id: &String| {
                        python_str(
                            source_clock::clock_parts(sources[id].get("at").expect("clock"))
                                .expect("clock")
                                .1,
                        )
                    };
                    implied.push(implied_word(
                        subject,
                        id,
                        &group[top],
                        &format!("{clock} {} before {}", value(id), value(&group[top])),
                    ));
                }
            }
        }
        standing.retain(|id| keep.contains(id));
        for item in &implied {
            marks
                .entry(text(&map(item)?["superseded"])?.to_owned())
                .or_insert(s("superseded"));
        }
    }
    let head_set: BTreeSet<_> = standing.iter().cloned().collect();
    let disputed: BTreeSet<_> = disputed.intersection(&head_set).cloned().collect();
    let mut entry = Map::from([
        ("kind".into(), kind.clone()),
        ("heads".into(), strings(&standing)),
        ("versions".into(), n(claims.len())),
        ("acts".into(), n(acts.len())),
        ("marks".into(), V::Map(marks.clone())),
        ("proposals".into(), strings(&proposals)),
        ("ids".into(), strings(claims.keys().copied())),
        ("implied".into(), V::List(implied)),
        (
            "writers".into(),
            V::Map(
                standing
                    .iter()
                    .map(|id| (id.clone(), claims[id]["by"].clone()))
                    .collect(),
            ),
        ),
        (
            "bodies".into(),
            V::Map(
                standing
                    .iter()
                    .map(|id| (id.clone(), claims[id]["body"].clone()))
                    .collect(),
            ),
        ),
        (
            "review_evidence".into(),
            V::List(
                reviews
                    .into_iter()
                    .filter(|r| {
                        head_set.contains(text(&map(r).expect("review")["of"]).expect("id"))
                    })
                    .collect(),
            ),
        ),
        ("disputed_acts".into(), strings(&disputed)),
        ("roots".into(), strings(&roots)),
        ("open_acts".into(), V::Map(open_acts)),
    ]);
    let meanings = standing
        .iter()
        .map(|id| claim_meaning(claims[id]))
        .collect::<Result<BTreeSet<_>>>()?;
    let accepted = meanings.len() == 1 && !standing.is_empty() && disputed.is_empty();
    if accepted {
        let h = &standing[0];
        entry.extend(Map::from([
            ("head".into(), s(h)),
            ("body".into(), claims[h]["body"].clone()),
            ("agreed".into(), n(standing.len())),
            (
                "accepted_by".into(),
                accepted_by.get(h).cloned().unwrap_or(V::Null),
            ),
            ("writer".into(), claims[h]["by"].clone()),
            ("root".into(), V::Bool(roots.contains(h))),
        ]));
    }
    let has_mark = |word| marks.values().any(|v| string_is(v, word));
    let acceptance = if accepted {
        "accepted"
    } else if standing.len() > 1 || !disputed.is_empty() {
        "contested"
    } else if !proposals.is_empty() {
        "proposed"
    } else if has_mark("corrected") {
        "corrected"
    } else if has_mark("refuted") {
        "refuted"
    } else if has_mark("retired") {
        "retired"
    } else {
        "unavailable"
    };
    entry.insert("acceptance".into(), s(acceptance));
    entry.insert("status".into(), s(acceptance));
    Ok(Some(V::Map(entry)))
}
fn implied_word(subject: &str, id: &str, by: &str, why: &str) -> V {
    V::Map(Map::from([
        ("rule".into(), s("source_clock")),
        ("subject".into(), s(subject)),
        ("superseded".into(), s(id)),
        ("by".into(), s(by)),
        ("why".into(), s(why)),
    ]))
}

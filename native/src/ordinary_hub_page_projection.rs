// Shared real page selection, counters and arrangement facts; contains no HTML.
struct PageProjection {
    selected_all: BTreeSet<String>,
    recorded_by: BTreeMap<String, BTreeSet<String>>,
    earned_by_tab: Vec<BTreeSet<String>>,
    page_values: Map,
    arrangement_owns: Vec<BTreeSet<usize>>,
    arrangement_facts: Map,
    hub: HubData,
}
#[allow(clippy::too_many_arguments)]
fn page_projection(
    brief: &Map,
    tabs: &[Tab],
    ids: &BTreeSet<String>,
    judgments: &BTreeSet<String>,
    states: &BTreeMap<String, BTreeSet<String>>,
    nodes: &Map,
    dep_field: &str,
    projection: &mut Projection<'_>,
) -> Result<PageProjection> {
    let mut hub = projection.hub_data()?;
    let mut selected_all = BTreeSet::new();
    let chosen_by_tab = tabs
        .iter()
        .map(|tab| {
            tab.sections
                .iter()
                .flat_map(|sec| sec.pick.iter())
                .flat_map(|selector| select(selector, ids, judgments, states))
                .collect::<BTreeSet<_>>()
        })
        .collect::<Vec<_>>();
    let references =
        regex::Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*\}\}").unwrap();
    let picked_by_tab = tabs
        .iter()
        .zip(&chosen_by_tab)
        .map(|(tab, chosen)| {
            let mut picked = chosen.clone();
            for section in &tab.sections {
                for reference in references.captures_iter(&section.text) {
                    if ids.contains(&reference[1]) {
                        picked.insert(reference[1].to_owned());
                    }
                }
            }
            picked
        })
        .collect::<Vec<_>>();
    selected_all.extend(
        picked_by_tab
            .iter()
            .flat_map(|picked| picked.iter().cloned()),
    );
    let recorded_by = ids
        .iter()
        .filter(|id| {
            !judgments.contains(*id)
                && body(nodes, id).is_ok_and(|b| b.get("asked").is_some_and(truth))
        })
        .map(|intent| {
            let recorded = ids
                .iter()
                .filter(|id| {
                    body(nodes, id).is_ok_and(|b| {
                        b.get("from").and_then(|v| text(v).ok()) == Some(intent.as_str())
                            || judgments.contains(*id) && deps(b, dep_field).contains(intent)
                    })
                })
                .cloned()
                .collect::<BTreeSet<_>>();
            (intent.clone(), recorded)
        })
        .collect::<BTreeMap<_, _>>();
    let earned_by_tab = tabs
        .iter()
        .enumerate()
        .map(|(index, tab)| {
            tab.serves
                .iter()
                .filter(|intent| {
                    recorded_by
                        .get(*intent)
                        .is_some_and(|recorded| !recorded.is_disjoint(&picked_by_tab[index]))
                })
                .cloned()
                .collect::<BTreeSet<_>>()
        })
        .collect::<Vec<_>>();
    let mut page_values = Map::new();
    if !brief.is_empty() {
        let integer =
            |count: usize| V::Integer(crate::value::Integer::new(&count.to_string()).unwrap());
        let unserved = recorded_by
            .iter()
            .filter(|(intent, recorded)| {
                !recorded.is_empty() && !earned_by_tab.iter().any(|earned| earned.contains(*intent))
            })
            .count();
        let mut recent = recorded_by
            .iter()
            .filter(|(_, recorded)| !recorded.is_empty())
            .map(|(intent, _)| {
                (
                    intent_date(intent, body(nodes, intent).ok()),
                    intent,
                    !earned_by_tab.iter().any(|earned| earned.contains(intent)),
                )
            })
            .collect::<Vec<_>>();
        recent.sort_by(|a, b| (&b.0, b.1).cmp(&(&a.0, a.1)));
        let mut streak = 0;
        let mut offset = 0;
        while offset < recent.len() {
            let count = recent[offset..]
                .iter()
                .take_while(|row| row.0 == recent[offset].0)
                .count();
            if recent[offset..offset + count].iter().any(|row| !row.2) {
                break;
            }
            streak += count;
            offset += count;
        }
        let born = judgments
            .iter()
            .filter_map(|id| {
                body(nodes, id)
                    .ok()?
                    .get("born")
                    .and_then(|v| parse_date(&textish(v)))
            })
            .max();
        let added = recorded_by
            .iter()
            .filter(|(intent, _)| {
                intent_date(intent, body(nodes, intent).ok())
                    .is_some_and(|date| born.is_some_and(|born| date >= born))
            })
            .flat_map(|(_, recorded)| recorded.iter().cloned())
            .collect::<BTreeSet<_>>();
        let drift = born.map(|_| {
            if added.is_empty() {
                0.0
            } else {
                format!(
                    "{:.2}",
                    added.difference(&selected_all).count() as f64 / added.len() as f64
                )
                .parse()
                .unwrap()
            }
        });
        let chosen = chosen_by_tab
            .iter()
            .flat_map(|ids| ids.iter().cloned())
            .collect::<BTreeSet<_>>();
        let spill = states
            .iter()
            .filter(|(id, flags)| !flags.is_empty() && !chosen.contains(*id))
            .count();
        for (key, value) in [
            ("page.unserved", Some(integer(unserved))),
            ("page.recent_unserved", Some(integer(streak))),
            ("page.covered", Some(integer(selected_all.len()))),
            ("page.spill", Some(integer(spill))),
            (
                "page.drift",
                drift.map(|v| V::Float(crate::value::FiniteFloat::new(v).unwrap())),
            ),
        ] {
            page_values.insert(key.into(), value.clone().unwrap_or(V::Null));
            if let Some(value) = value
                && let Some(V::Map(body)) = projection.base.reader.raw.get_mut(key)
            {
                body.insert("v".into(), value);
            }
        }
        hub = projection.hub_data()?;
    }
    let arrangement_owns = hub
        .arrangements
        .iter()
        .map(|arrangement| {
            tabs.iter()
                .enumerate()
                .filter(|(index, tab)| {
                    tabs.len() == 1 && tab.bare
                        || arrangement
                            .sources
                            .iter()
                            .any(|source| earned_by_tab[*index].contains(source))
                })
                .map(|(index, _)| index)
                .collect::<BTreeSet<_>>()
        })
        .collect::<Vec<_>>();
    let arrangement_facts = hub
        .arrangements
        .iter()
        .filter(|_| !brief.is_empty())
        .enumerate()
        .map(|(index, arrangement)| {
            let unearned = arrangement
                .sources
                .iter()
                .filter(|source| {
                    !arrangement_owns[index]
                        .iter()
                        .any(|tab| earned_by_tab[*tab].contains(*source))
                })
                .cloned()
                .collect::<Vec<_>>();
            let linked = (tabs.len() == 1 && tabs[0].bare) || unearned.is_empty();
            let born = arrangement.born.as_deref().and_then(parse_date);
            let stood = recorded_by
                .keys()
                .filter(|intent| {
                    intent_date(intent, body(nodes, intent).ok())
                        .is_some_and(|date| born.is_some_and(|born| date > born))
                        && arrangement_owns[index]
                            .iter()
                            .any(|tab| earned_by_tab[*tab].contains(*intent))
                })
                .count();
            let mut fact_fields = Map::from([
                ("linked".into(), V::Bool(linked)),
                (
                    "cut".into(),
                    V::Text(if unearned.is_empty() {
                        String::new()
                    } else {
                        format!("no tab's sections earn {}", unearned.join(", "))
                    }),
                ),
                ("fired".into(), V::Bool(arrangement.fired)),
                (
                    "stood".into(),
                    V::Integer(crate::value::Integer::new(&stood.to_string()).unwrap()),
                ),
            ]);
            if let Some(reading) = &arrangement.reading {
                fact_fields.insert("reading".into(), V::Text(reading.clone()));
            }
            (arrangement.id.clone(), V::Map(fact_fields))
        })
        .collect::<Map>();
    Ok(PageProjection {
        selected_all,
        recorded_by,
        earned_by_tab,
        page_values,
        arrangement_owns,
        arrangement_facts,
        hub,
    })
}

// Verification notes are observations about the rendered page, not failures.
fn push_note(notes: &mut Vec<String>, note: String) {
    if !notes.contains(&note) {
        notes.push(note);
    }
}
fn tab_title(tab: &Tab) -> &str {
    if tab.title.is_empty() {
        "Now"
    } else {
        &tab.title
    }
}
fn shape_moves(tab: &Tab, shape: &BTreeMap<&str, usize>) -> Vec<String> {
    ["entries", "judgments", "flagged", "blocked"]
        .into_iter()
        .filter_map(|key| {
            let old = tab.shape.get(key)?;
            let now = shape.get(key)?;
            (textish(old) != now.to_string()).then(|| format!("{key}: {} -> {now}", textish(old)))
        })
        .collect()
}
fn reasoning_notes(context: &RenderContext<'_>) -> Result<Vec<String>> {
    let mut cut = vec![];
    let mut resolved = vec![];
    for id in context.judgments {
        let written = reasoning(body(context.nodes, id)?);
        let drawn = resolve_references(context, &clipped_reasoning(&written))
            .chars()
            .count();
        if written.chars().count() > CARD_CHARS {
            cut.push((id, written.chars().count()));
        } else if drawn > CARD_CHARS {
            resolved.push((id, drawn));
        }
    }
    let mut notes = vec![];
    for (mut rows, said) in [
        (
            cut,
            "longer than the 400 characters a card carries; each is drawn to its last whole word and marked",
        ),
        (
            resolved,
            "within 400 characters as written and past them once their references resolve; what each names is long, so the card is drawn whole",
        ),
    ] {
        if rows.is_empty() {
            continue;
        }
        rows.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        notes.push(format!(
            "{} reasoning{} {said}. Longest first: {}{}",
            rows.len(),
            if rows.len() == 1 { "" } else { "s" },
            rows.iter()
                .take(3)
                .map(|(id, n)| format!("{id} ({n})"))
                .collect::<Vec<_>>()
                .join(", "),
            if rows.len() > 3 {
                format!(" and {} more", rows.len() - 3)
            } else {
                String::new()
            }
        ));
    }
    Ok(notes)
}
fn entry_notes(context: &RenderContext<'_>, ids: &BTreeSet<String>) -> Vec<String> {
    let unnamed = ids
        .difference(context.judgments)
        .filter(|id| {
            !context.labels.contains_key(*id)
                && body(context.nodes, id).is_ok_and(|b| named(&V::Map(b.clone())).is_empty())
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut notes = vec![];
    if !unnamed.is_empty() {
        notes.push(format!(
            "{} entries carry no human name, so the page has to fall back to generic labels: {}{}",
            unnamed.len(),
            unnamed
                .iter()
                .take(6)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            if unnamed.len() > 6 {
                format!(" and {} more", unnamed.len() - 6)
            } else {
                String::new()
            }
        ));
    }
    let (a, t) = context.anchored.get();
    if t > 0 {
        notes.push(format!("{a} of {t} live dependencies are named in the prose that cites them; the rest are reachable only by hovering the judgment"));
    }
    notes
}
#[allow(clippy::too_many_arguments)]
fn coverage_notes(
    tabs: &[Tab],
    ids: &BTreeSet<String>,
    nodes: &Map,
    recorded: &BTreeMap<String, BTreeSet<String>>,
    picks: &[BTreeSet<String>],
    earned: &[BTreeSet<String>],
    values: &Map,
    born: Option<chrono::NaiveDate>,
) -> Vec<String> {
    if values.is_empty() {
        return vec![];
    }
    let drift = if values["page.drift"] == V::Null {
        "-".into()
    } else {
        format!("{} since {}", textish(&values["page.drift"]), born.unwrap())
    };
    let mut notes = vec![format!(
        "coverage: {} covered · spill {} · {} intents no tab serves · {} recent in a row · drift {drift}",
        textish(&values["page.covered"]),
        textish(&values["page.spill"]),
        textish(&values["page.unserved"]),
        textish(&values["page.recent_unserved"])
    )];
    for (i, tab) in tabs.iter().enumerate() {
        if tab.serves.is_empty() {
            continue;
        }
        let rec = tab
            .serves
            .iter()
            .filter_map(|id| recorded.get(id))
            .flatten()
            .cloned()
            .collect::<BTreeSet<_>>();
        notes.push(format!(
            "tab '{}' serves {}: picks {} of {} they recorded",
            tab_title(tab),
            tab.serves.join(", "),
            rec.intersection(&picks[i]).count(),
            rec.len()
        ));
    }
    let mut intents = recorded.iter().collect::<Vec<_>>();
    intents.sort_by_key(|(id, _)| std::cmp::Reverse((intent_date(id, body(nodes, id).ok()), *id)));
    for (id, rec) in &intents {
        if rec.is_empty() || earned.iter().any(|set| set.contains(*id)) {
            continue;
        }
        let asked = body(nodes, id)
            .ok()
            .and_then(|b| b.get("asked"))
            .map(textish)
            .unwrap_or_default();
        let asked = crate::public_ordinary_readers::short(&V::Text(asked), 90);
        notes.push(format!("{id} is served by no tab - asked: {asked}"));
        let mut prefixes = BTreeMap::<&str, usize>::new();
        for id in *rec {
            *prefixes.entry(id.split('.').next().unwrap()).or_default() += 1;
        }
        let touched = prefixes
            .iter()
            .map(|(g, n)| format!("{g}. ({n})"))
            .collect::<Vec<_>>()
            .join(", ");
        let inside = tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                format!(
                    "{} of {} inside '{}'",
                    rec.intersection(&picks[i]).count(),
                    rec.len(),
                    tab_title(t)
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        notes.push(format!(
            "  hint: it wrote {touched} - {}",
            if inside.is_empty() {
                "no tab yet"
            } else {
                &inside
            }
        ));
    }
    for (id, rec) in intents {
        if rec.is_empty() {
            notes.push(format!(
                "{id} recorded nothing, so no tab can serve it and none needs to"
            ));
        }
    }
    let all = picks.iter().flatten().collect::<BTreeSet<_>>();
    let mut prefixes = BTreeMap::<&str, bool>::new();
    for id in ids
        .iter()
        .filter(|id| !crate::reasoning_fields::BUILTINS.contains(&id.as_str()))
    {
        *prefixes.entry(id.split('.').next().unwrap()).or_default() |= all.contains(id);
    }
    let unpicked = prefixes
        .iter()
        .filter(|(_, picked)| !**picked)
        .map(|(p, _)| *p)
        .collect::<Vec<_>>();
    if !unpicked.is_empty() {
        notes.push(format!("no section picks: {}", unpicked.join(", ")));
    }
    notes
}

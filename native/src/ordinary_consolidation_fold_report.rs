// Publication refusals only; read-only full-domain previews never include this adapter.
pub(super) fn fold_refusal(c: &Union<'_>, take: &[String]) -> Option<String> {
    if !c.contested.is_empty() {
        return Some(format!(
            "refused - a contested id stops the fold: {}",
            c.contested.keys().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    if let Some(why) = stray(c, take) {
        return Some(why);
    }
    if !c.drops_needed.is_empty() && !c.blocked() {
        return Some(format!(
            "refused - a replacement no longer rests on what the judgment it replaces rested on, and a dependency dropped is a decision with a reason: {}",
            c.reversed
                .iter()
                .filter_map(|r| c.drops_needed.get(&r.id).map(|ds| format!(
                    "consolidate {} {}",
                    c.hyps[r.hyp].name,
                    ds.iter()
                        .map(|d| format!("--drop {}", quote(&format!("{d}: <why>"))))
                        .collect::<Vec<_>>()
                        .join(" ")
                )))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    if !c.untaken.is_empty()
        && c.falsified.is_empty()
        && c.holes.is_empty()
        && c.head_falsified.is_empty()
        && c.refused.is_empty()
    {
        let cannot = c
            .untaken
            .iter()
            .map(|i| &c.reversed[*i])
            .filter(|r| c.untakeable.contains(&r.id))
            .collect::<Vec<_>>();
        return Some(if !cannot.is_empty() {
            format!(
                "refused - no name takes {}: {}",
                cannot
                    .iter()
                    .map(|r| r.id.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
                cannot
                    .iter()
                    .map(|r| r.why.clone())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        } else {
            format!(
                "refused - a verdict the base's own condition has not broken folds only when a person names it: {}",
                c.untaken
                    .iter()
                    .map(|i| &c.reversed[*i])
                    .map(|r| format!("consolidate {} --take {}", c.hyps[r.hyp].name, r.id))
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
    }
    if c.blocked() {
        return Some("refused - the dry run is not clean; nothing folds until it is".into());
    }
    let never = c
        .hyps
        .iter()
        .filter(|h| get(&h.head, "folds") == &s("never"))
        .map(|h| h.name.clone())
        .collect::<Vec<_>>();
    if !never.is_empty() {
        return Some(format!(
            "refused - {} never fold{}: a what-if is evaluated and never written; consolidate the others by name",
            never.join(", "),
            if never.len() == 1 { "s" } else { "" }
        ));
    }
    None
}

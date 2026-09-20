use super::*;
fn cut(value: &str, width: usize) -> String {
    if value.chars().count() < width {
        value.into()
    } else {
        value.chars().take(width).collect::<String>() + " ..."
    }
}
fn describe(id: &str, body: &V, world: &World<'_>, suffix: &str) -> Result<String> {
    let width = 110usize.saturating_sub(suffix.chars().count());
    let dep = text(&world.reader.fields["deps"])?;
    if map(body).is_ok_and(|m| m.contains_key(dep)) {
        let v = verdict(body);
        return Ok(cut(
            &format!("+ {id}: {}", if truth(&v) { py(&v) } else { id.into() }),
            width,
        ) + suffix);
    }
    if !matches!(body, V::Map(_)) {
        return Ok(cut(&format!("{id}: {}", py(body)), width) + suffix);
    }
    let v = world.reader.value(id)?;
    let rule = get(body, "rule");
    let authored = get(body, "v");
    let shown = if v != V::Null {
        py(&v)
    } else if truth(rule) || matches!(authored,V::Text(v)if R::EXPR.is_match(v)) {
        format!(
            "= {}",
            predicate_text(if truth(rule) { rule } else { authored })
        )
    } else if truth(get(body, "quoted")) {
        format!("\"{}\"", py(get(body, "quoted")))
    } else {
        String::new()
    };
    let name = crate::reasoning_authoring::named(body);
    let mut out = format!("{id}: {shown}");
    if !name.is_empty() {
        out += &format!(" ({name})");
    }
    if truth(get(body, "from")) {
        out += &format!(" <- {}", py(get(body, "from")));
        if truth(get(body, "at")) {
            out += &format!(", at {}", py(get(body, "at")));
        }
    }
    let of = if truth(get(body, "of")) {
        get(body, "of")
    } else {
        get(body, "read")
    };
    if truth(of) {
        out += &format!(" as of {}", py(of));
    }
    Ok(cut(&out, width) + suffix)
}
fn state(world: &World<'_>, id: &str, touched: &BTreeSet<String>) -> Result<String> {
    let (tag, why) = world.state_with_touched(id, touched)?;
    Ok(format!("{tag:<9} {id}: {why}"))
}
fn side(who: &str, body: &V, world: &World<'_>, fields: &Map) -> Result<Vec<String>> {
    let mut out = vec![];
    if !matches!(body, V::Map(_)) {
        return Ok(out);
    }
    let because = get(body, "because");
    if truth(because) {
        let said = world.said(because, 400)?;
        out.push(cut(
            &format!(
                "    {who}: because: {}",
                said.first().map(String::as_str).unwrap_or("")
            ),
            110,
        ));
    }
    let deps = text(&fields["deps"])?;
    if let V::List(values) = get(body, deps) {
        out.push(cut(
            &format!(
                "    {who}: {deps}: [{}]",
                values.iter().map(py).collect::<Vec<_>>().join(", ")
            ),
            110,
        ));
    }
    if truth(get(body, "wrong_if")) {
        out.push(cut(
            &format!(
                "    {who}: wrong_if: {}",
                predicate_text(get(body, "wrong_if"))
            ),
            110,
        ));
    }
    Ok(out)
}
fn head(h: &Hypothesis, today: chrono::NaiveDate) -> String {
    let mut bits = vec![];
    let born = get(&h.head, "born");
    if truth(born) {
        let date = day(born);
        let age = date
            .as_ref()
            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
            .map(|d| {
                let n = (today - d).num_days();
                if n <= 0 {
                    "today".into()
                } else if n == 1 {
                    "1 day".into()
                } else {
                    format!("{n} days")
                }
            })
            .unwrap_or("undated".into());
        bits.push(format!("born {}, {age}", date.unwrap_or_else(|| py(born))));
    }
    if get(&h.head, "folds") == &s("never") {
        bits.push("never folds".into());
    }
    let out = format!(
        "  {}{}",
        h.name,
        if bits.is_empty() {
            String::new()
        } else {
            format!(" ({})", bits.join(", "))
        }
    );
    if truth(get(&h.head, "claim")) {
        format!(
            "{out}: {}",
            short(
                get(&h.head, "claim"),
                110usize.saturating_sub(out.chars().count() + 2)
            )
        )
    } else {
        out
    }
}
fn sources(c: &Union<'_>, id: &str, hi: usize) -> Result<String> {
    let h = &c.hyps[hi];
    let doc = layer(&c.base_doc, &h.doc)?;
    let p = projection(&doc, &Map::new(), c.runtime)?;
    describe(id, &h.raw[id], &p.base, "")
}
fn quote(v: &str) -> String {
    if !v.is_empty()
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c))
    {
        v.into()
    } else {
        format!("'{}'", v.replace('\'', "'\"'\"'"))
    }
}
pub(super) fn lines(c: &Union<'_>, today: chrono::NaiveDate) -> Result<Vec<String>> {
    let names = c.hyps.iter().map(|h| h.name.clone()).collect::<Vec<_>>();
    let base = &c.base.base;
    let mut out = vec![format!(
        "the base with {} laid over it{}",
        names.join(", "),
        if names.len() > 1 {
            ", in name order"
        } else {
            ""
        }
    )];
    out.extend(c.hyps.iter().map(|h| head(h, today)));
    out.push(String::new());
    if !c.contested.is_empty() {
        out.push(format!("contested ({}): an id two hypotheses hold with different claims - the union has no value to lay, so the run stops here",c.contested.len()));
        for (id, hs) in &c.contested {
            out.push(format!("  {id}:"));
            if base.reader.ids.contains(id) {
                out.push(
                    "    the base holds ".to_owned()
                        + &describe(id, &base.reader.raw[id], base, "")?,
                );
            }
            for hi in hs {
                out.push(format!(
                    "    {} says {}",
                    c.hyps[*hi].name,
                    sources(c, id, *hi)?
                ));
            }
        }
        out.extend([String::new(),"re-read against the merged tree: pull each id to see every reading beside the base's, then set what holds today - in the base with a later day, or in the hypothesis that read it - or refute one, and consolidate again".into()]);
        return Ok(out);
    }
    let world = &c.view.as_ref().unwrap().base;
    out.push(format!(
        "arrived ({}): what the fold would add",
        c.arrived.len()
    ));
    for (id, hi) in &c.arrived {
        let h = &c.hyps[*hi];
        out.push(
            "  ".to_owned() + &describe(id, &h.raw[id], world, &format!(" - from {}", h.name))?,
        );
        if world.judgments.contains_key(id) {
            out.push("    ".to_owned() + &state(world, id, &BTreeSet::new())?);
        }
    }
    let readings = c
        .updates
        .iter()
        .filter(|u| !base.judgments.contains_key(&u.id))
        .collect::<Vec<_>>();
    out.push(format!(
        "updates ({}): what the base holds that a hypothesis replaces, and what rests on each",
        readings.len()
    ));
    for u in readings {
        out.push(format!(
            "  {}: {} -> {}, from {}",
            u.id,
            short(&u.old, 36),
            short(&u.new, 36),
            c.hyps[u.hyp].name
        ));
        out.push(format!(
            "    {}{}",
            u.why,
            if u.allowed {
                ""
            } else {
                " - the base keeps what it holds"
            }
        ));
        let (hit, touched, derived) = world.write_reach(std::slice::from_ref(&u.id))?;
        if !derived.is_empty() {
            out.push(cut(
                &format!("    worked out from it: {}", derived.join(", ")),
                110,
            ));
        }
        for (id, _) in hit {
            if id != u.id {
                out.push("    ".to_owned() + &state(world, &id, &touched)?);
            }
        }
    }
    out.push(format!("reversed ({}): a verdict, or other grounds, laid over a standing judgment - by its own condition, by a person's name, or waiting for one",c.reversed.len()));
    for r in &c.reversed {
        let (ov, nv) = (verdict(&r.old), verdict(&r.new));
        out.push(format!(
            "  {}: {}, from {}",
            r.id,
            if !crate::reasoning_authoring::same(&ov, &nv)? {
                format!("{} -> {}", short(&ov, 36), short(&nv, 36))
            } else {
                "the same verdict on other grounds".into()
            },
            c.hyps[r.hyp].name
        ));
        out.push(if r.allowed && r.why.contains("by name") {
            format!("    taken by name - {}", r.why)
        } else if r.allowed {
            format!("    by its own condition - {}", r.why)
        } else if c.untakeable.contains(&r.id) {
            format!("    {}", r.why)
        } else {
            format!(
                "    {} - take it by name: consolidate {} --take {}",
                r.why, c.hyps[r.hyp].name, r.id
            )
        });
        for d in c.drops_needed.get(&r.id).into_iter().flatten() {
            out.push(format!(
                "    no longer rests on {d} - name the reason at the fold: --drop {}",
                quote(&format!("{d}: <why>"))
            ));
        }
        out.extend(side("base", &r.old, base, &base.reader.fields)?);
        out.extend(side(
            &c.hyps[r.hyp].name,
            &r.new,
            world,
            &base.reader.fields,
        )?);
        let (hit, touched, _) = world.write_reach(std::slice::from_ref(&r.id))?;
        for (id, _) in hit {
            if id != r.id {
                out.push("    ".to_owned() + &state(world, &id, &touched)?);
            }
        }
    }
    out.push(format!(
        "moved / falsified ({}): what the union moves or breaks",
        c.moved.len() + c.falsified.len() + c.holes.len() + c.head_falsified.len()
    ));
    out.extend(c.falsified.iter().map(|l| format!("  FALSIFIED {l}")));
    for (hi, pred) in &c.head_falsified {
        out.push(format!(
            "  FALSIFIED {}: its own wrong_if holds ({pred})",
            c.hyps[*hi].name
        ));
    }
    out.extend(c.holes.iter().map(|l| format!("  FAIL {l}")));
    out.extend(c.moved.iter().map(|l| format!("  MOVED {l}")));
    out.push(format!(
        "contested ({}){}",
        c.refused.len(),
        if c.refused.is_empty() {
            ""
        } else {
            ": the door refuses the reading, so the base keeps what it holds"
        }
    ));
    for idx in &c.refused {
        let u = &c.updates[*idx];
        out.push(format!("  {}: {}", u.id, u.why));
        out.push(
            "    the base holds ".to_owned() + &describe(&u.id, &base.reader.raw[&u.id], base, "")?,
        );
        out.push(format!(
            "    {} says {}",
            c.hyps[u.hyp].name,
            sources(c, &u.id, u.hyp)?
        ));
    }
    if !c.refused.is_empty() {
        out.push("  read again on a later day - set it in the base or in the hypothesis with --as-of - or refute the hypothesis".into());
    }
    out.push(format!(
        "candidates ({}): pairs for a person to judge as the same subject or distinct",
        c.candidates.len()
    ));
    out.extend(c.candidates.iter().map(|l| format!("  {l}")));
    out.push(format!(
        "new subjects ({}): prefixes the base does not hold",
        c.new_subjects.len()
    ));
    for prefix in &c.new_subjects {
        out.push(cut(
            &format!(
                "  {prefix}: {}",
                c.arrived
                    .iter()
                    .filter(|(id, _)| id.split('.').next() == Some(prefix))
                    .map(|(id, _)| id.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            110,
        ));
    }
    out.push(String::new());
    let foldable = c
        .hyps
        .iter()
        .filter(|h| get(&h.head, "folds") != &s("never"))
        .map(|h| h.name.clone())
        .collect::<Vec<_>>();
    if c.red() || !c.drops_needed.is_empty() {
        let mut what = vec![];
        if !c.falsified.is_empty() || !c.head_falsified.is_empty() {
            what.push("a falsifier holds".into());
        }
        if !c.holes.is_empty() {
            what.push("a hole".into());
        }
        if !c.refused.is_empty() {
            what.push("a contested reading".into());
        }
        if !c.untaken.is_empty() {
            what.push(format!(
                "{} reversal{} to take by name",
                c.untaken.len(),
                if c.untaken.len() == 1 { "" } else { "s" }
            ));
        }
        if !c.drops_needed.is_empty() {
            what.push("a dropped dependency to name".into());
        }
        let other = !c.falsified.is_empty()
            || !c.head_falsified.is_empty()
            || !c.holes.is_empty()
            || !c.refused.is_empty()
            || !c.contested.is_empty();
        out.push(format!("not clean: {}{}",what.join(", "),if other{" - nothing folds until it is read again"}else if !c.untaken.is_empty(){" - a verdict the base's own condition has not broken folds only when a person names it"}else{" - a dependency dropped is a decision with a reason, named at the fold"}));
        let mut choices = Vec::<(String, Vec<String>)>::new();
        let mut add = |name: &str, arg: String| {
            if let Some((_, args)) = choices.iter_mut().find(|(n, _)| n == name) {
                args.push(arg);
            } else {
                choices.push((name.into(), vec![arg]));
            }
        };
        for idx in &c.untaken {
            let r = &c.reversed[*idx];
            if !c.untakeable.contains(&r.id) {
                add(&c.hyps[r.hyp].name, format!("--take {}", r.id));
            }
        }
        for r in &c.reversed {
            for d in c.drops_needed.get(&r.id).into_iter().flatten() {
                add(
                    &c.hyps[r.hyp].name,
                    format!("--drop {}", quote(&format!("{d}: <why>"))),
                );
            }
        }
        out.extend(
            choices
                .into_iter()
                .map(|(h, args)| format!("  consolidate {h} {}", args.join(" "))),
        );
    } else if !c.moved.is_empty() {
        out.push(format!(
            "moved: {} judgment{} to re-review before the fold - a premise moved under {}",
            c.moved.len(),
            if c.moved.len() == 1 { "" } else { "s" },
            if c.moved.len() == 1 { "it" } else { "them" }
        ));
    } else if foldable.is_empty() {
        out.push(format!(
            "clean - and nothing here folds: {}{}, evaluated and never written",
            names.join(", "),
            if names.len() > 1 {
                " are what-ifs"
            } else {
                " is a what-if"
            }
        ));
    } else if c.arrived.is_empty() && c.updates.is_empty() {
        out.push(format!("clean - and nothing to write: the base already holds everything {} propose{}; consolidate {} removes the file{}",foldable.join(", "),if foldable.len()==1{"s"}else{""},foldable.join(" "),if foldable.len()>1{"s"}else{""}));
    } else {
        out.push(format!(
            "clean: {} may fold - consolidate {}",
            foldable.join(", "),
            foldable.join(" ")
        ));
    }
    Ok(out)
}
pub(super) fn stray(c: &Union<'_>, take: &[String]) -> Option<String> {
    if !c.contested.is_empty() {
        return None;
    }
    let named = c
        .reversed
        .iter()
        .map(|r| r.id.as_str())
        .collect::<BTreeSet<_>>();
    let stray = take
        .iter()
        .filter(|id| !named.contains(id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !stray.is_empty() {
        return Some(format!(
            "refused - --take names {}, which no hypothesis here lays over a standing judgment, or reads from another source than the base",
            stray.join(", ")
        ));
    }
    let cannot = take
        .iter()
        .filter(|id| c.untakeable.contains(*id))
        .cloned()
        .collect::<Vec<_>>();
    if cannot.is_empty() {
        None
    } else {
        Some(format!(
            "refused - no name takes {}: {}",
            cannot.join(", "),
            c.untaken
                .iter()
                .map(|i| &c.reversed[*i])
                .filter(|r| cannot.contains(&r.id))
                .map(|r| r.why.clone())
                .collect::<Vec<_>>()
                .join("; ")
        ))
    }
}
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
pub(super) fn candidates(c: &Union<'_>, all: &Map) -> Result<Vec<String>> {
    let base = entries(&c.base_doc)?;
    let mut beside = BTreeMap::<String, Map>::new();
    for (name, h) in all {
        if !truth(get(h, "error")) {
            beside.insert(name.clone(), entries(get(h, "doc"))?);
        }
    }
    for h in &c.hyps {
        beside.insert(h.name.clone(), h.raw.clone());
    }
    let raws = std::iter::once(&base)
        .chain(beside.values())
        .collect::<Vec<_>>();
    let live = raws
        .iter()
        .flat_map(|r| r.keys().cloned())
        .collect::<BTreeSet<_>>();
    let mut raw_all = Map::new();
    for raw in raws.iter().rev() {
        raw_all.extend((*raw).clone());
    }
    let dep = text(&c.base.base.reader.fields["deps"])?;
    let mut retired = BTreeMap::new();
    let mut distinct = BTreeSet::new();
    let idish = regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)*$").unwrap();
    for raw in &raws {
        for (id, body) in *raw {
            if dep != "also" {
                let aliases = match get(body, "also") {
                    V::Text(v) => vec![v.clone()],
                    v => strings(v),
                };
                for alias in aliases {
                    if idish.is_match(&alias) && !live.contains(&alias) && alias != *id {
                        retired.entry(alias).or_insert(id.clone());
                    }
                }
            }
            if truth(get(body, "distinct_from")) {
                for other in py(get(body, "distinct_from"))
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|s| !s.is_empty())
                {
                    if id != other {
                        let mut pair = [id.clone(), other.into()];
                        pair.sort();
                        distinct.insert((pair[0].clone(), pair[1].clone()));
                    }
                }
            }
        }
    }
    let arrivals = c
        .hyps
        .iter()
        .flat_map(|h| {
            h.ids
                .iter()
                .filter(|id| !c.base.base.reader.ids.contains(*id))
                .map(move |id| (h.name.clone(), id.clone()))
        })
        .collect::<Vec<_>>();
    let mut out = vec![];
    for (name, id) in &arrivals {
        let body = &beside[name][id];
        let mut pool = base
            .iter()
            .filter(|(id, b)| c.base.base.reader.ids.contains(*id) && matches!(b, V::Map(_)))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Map>();
        let mut where_ = pool
            .keys()
            .map(|id| (id.clone(), None))
            .collect::<BTreeMap<String, Option<String>>>();
        for (other_name, other_id) in &arrivals {
            if other_id != id && (other_name, other_id) > (name, id) {
                pool.insert(other_id.clone(), beside[other_name][other_id].clone());
                where_.insert(other_id.clone(), Some(other_name.clone()));
            }
        }
        for near in crate::public_identity::ordinary_sameness::near(
            id,
            body,
            &pool,
            dep,
            &raw_all,
            &retired,
            &distinct,
            Some(5),
        ) {
            out.push(format!(
                "{id} ({name}) and {}{}: {}",
                near.id,
                where_[&near.id]
                    .as_ref()
                    .map(|n| format!(" ({n})"))
                    .unwrap_or_default(),
                near.reasons.join("; ")
            ));
        }
    }
    Ok(out)
}

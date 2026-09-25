//! Pending findings laid over the base the way hypotheses are, and never written: a finding
//! reaches the record through the knowledge PR, and nothing here adopts one.
use super::*;
use report::{cut, describe};

/// A finding as the report names it: its revision cut to the twelve characters the PENDING
/// line shows.
fn name(h: &Hypothesis) -> String {
    h.name.chars().take("pending-".len() + 12).collect()
}

/// The pending ledger's contributions the live overlay keeps active, in name order: it has
/// already left out every one withdrawn, rejected or superseded, and a frozen read has none.
pub(super) fn findings(capture: &CapturedSource) -> Result<Vec<Hypothesis>> {
    let mut pool = BTreeMap::new();
    for (name, h) in map(capture.hypotheses())? {
        if get(h, "kind") != &s("contribution") || truth(get(h, "error")) {
            continue;
        }
        let doc = get(h, "doc");
        let raw = entries(doc)?;
        pool.insert(
            name.clone(),
            Hypothesis {
                name: name.clone(),
                path: text(get(h, "path"))
                    .ok()
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from),
                doc: doc.clone(),
                head: get(h, "head").clone(),
                ids: raw.keys().cloned().collect(),
                comparison_base: None,
                raw,
            },
        );
    }
    Ok(pool.into_values().collect())
}

/// The reader's own words for why a layer cannot be read differ between runtimes; check says
/// them.
const UNREAD: &str = "check says why";
const TOGETHER_UNREAD: &str = "their fields cannot be told apart";

fn unreadable(doc: &V, runtime: Option<&Runtime>) -> bool {
    Reader::new(doc, runtime).is_err()
}

/// What a document reads as judgments, and by which fields; None when its fields cannot be
/// told apart.
fn roles(document: &V) -> Option<(BTreeSet<String>, Map)> {
    F::semantic_roles(document).ok().flatten()
}

/// Why the base with `finds` laid over it would read an id otherwise than what holds it - the
/// finding that brings it, or the base - reads it: an entry taken for a judgment or the reverse,
/// or a judgment's dependencies, snapshot or condition read from another field. A finding tested
/// under a reading its author did not give it would pass for what it never said, and one that
/// changes how the base is read would hide a judgment from the test.
fn misread(
    finds: &[Hypothesis],
    base: &V,
    base_roles: Option<&(BTreeSet<String>, Map)>,
    layered: &V,
) -> Result<Option<String>> {
    let Some((over_judgments, over_fields)) = roles(layered) else {
        return Ok(None);
    };
    let (who, holds, reads) = if finds.len() == 1 {
        ("it", "holds", "reads")
    } else {
        ("they", "hold", "read")
    };
    let mut readers = BTreeMap::new();
    for (id, body) in entries(base)? {
        readers.insert(id, (body, base_roles.cloned(), false));
    }
    for h in finds {
        let own = roles(&h.doc);
        for id in &h.ids {
            let body = h.raw.get(id).cloned().unwrap_or(V::Null);
            readers.insert(id.clone(), (body, own.clone(), true));
        }
    }
    let field = |fields: &Map, role: &str| {
        fields
            .get(role)
            .and_then(|v| text(v).ok())
            .filter(|f| !f.is_empty())
            .map(str::to_owned)
    };
    for (id, (body, roles, brought)) in &readers {
        let Some((judgments, fields)) = roles else {
            continue;
        };
        let (mine, theirs) = (judgments.contains(id), over_judgments.contains(id));
        let (was, becomes) = if mine {
            ("a judgment", "an entry")
        } else {
            ("an entry", "a judgment")
        };
        if mine != theirs {
            return Ok(Some(if *brought {
                format!("{who} {holds} {id} as {was}, and the base would read {becomes}")
            } else {
                format!(
                    "laid over the base, {who} would turn the base's {id} from {was} into {becomes}"
                )
            }));
        }
        let Some(body) = map(body).ok().filter(|_| mine) else {
            continue;
        };
        for role in ["deps", "snapshot", "predicate"] {
            let over = field(&over_fields, role);
            if let Some(own) = field(fields, role)
                && body.contains_key(&own)
                && over.as_deref() != Some(own.as_str())
            {
                let other = over.unwrap_or_else(|| "nothing".into());
                return Ok(Some(if *brought {
                    format!("{who} {reads} {id}'s {role} from {own}, and the base from {other}")
                } else {
                    format!(
                        "laid over the base, {who} would read the base's {id} {role} from {other} instead of {own}"
                    )
                }));
            }
        }
    }
    Ok(None)
}

/// Ids two findings hold with different bodies, with their holders in name order: two
/// findings of one id that differ in anything are two versions, and at most one of them is
/// accepted - even when they agree on the value.
fn contested(finds: &[Hypothesis]) -> Result<BTreeMap<String, Vec<String>>> {
    let mut held = BTreeMap::<String, Vec<(String, String)>>::new();
    for h in finds {
        for (collection, members) in F::collections(&h.doc)? {
            for (id, body) in members.iter() {
                let identity = V::List(vec![s(&collection), body.clone()])
                    .try_typed()?
                    .digest()?;
                held.entry(id.clone())
                    .or_default()
                    .push((h.name.clone(), identity));
            }
        }
    }
    Ok(held
        .into_iter()
        .filter(|(_, holders)| {
            holders
                .iter()
                .map(|(_, identity)| identity)
                .collect::<BTreeSet<_>>()
                .len()
                > 1
        })
        .map(|(id, holders)| (id, holders.into_iter().map(|(name, _)| name).collect()))
        .collect())
}

pub(super) struct Pending<'a> {
    /// Every active finding, in name order.
    finds: Vec<Hypothesis>,
    base_doc: V,
    base: Projection<'a>,
    runtime: Option<&'a Runtime>,
    /// A finding the reader cannot lay over the base, or would read otherwise than it says, is
    /// reported, never tested, and never stops the run.
    unreadable: BTreeMap<String, String>,
    /// Each readable finding over the base by itself.
    alone: BTreeMap<String, Union<'a>>,
    contested: BTreeMap<String, Vec<String>>,
    /// Why every readable finding together cannot be read.
    together: Option<String>,
    /// What only all of them together break or move, when none contests another.
    falsified: Vec<String>,
    holes: Vec<String>,
    moved: Vec<String>,
}
impl Pending<'_> {
    /// A finding that cannot be read, a contested id, and anything that makes a hypothesis's
    /// own test red, or would name a dropped dependency, for one finding alone or for all of
    /// them together.
    pub(super) fn red(&self) -> bool {
        !self.unreadable.is_empty()
            || self.together.is_some()
            || !self.contested.is_empty()
            || !self.falsified.is_empty()
            || !self.holes.is_empty()
            || self
                .alone
                .values()
                .any(|c| c.red() || !c.drops_needed.is_empty())
    }
}

pub(super) struct Input<'i> {
    pub(super) capture: &'i CapturedSource,
    pub(super) stamp: &'i str,
    pub(super) brief: Option<&'i V>,
    /// What the brief holds an arrangement to, given findings; asked only of findings the base
    /// can read.
    pub(super) page_for: &'i dyn Fn(&[Hypothesis]) -> Result<Option<V>>,
}

/// Each readable finding over the base alone, for what accepting it would change and break;
/// the ids two of them contest; and, when none contests another, what only all of them
/// together break beyond any one. Nothing is written.
pub(super) fn test<'a>(
    input: &Input<'_>,
    finds: Vec<Hypothesis>,
    runtime: Option<&'a Runtime>,
) -> Result<Pending<'a>> {
    let doc = input.capture.ordinary_document();
    let base_roles = roles(doc);
    let mut pending = Pending {
        finds: vec![],
        base_doc: doc.clone(),
        base: captured_projection(input.capture, runtime)?,
        runtime,
        unreadable: BTreeMap::new(),
        alone: BTreeMap::new(),
        contested: BTreeMap::new(),
        together: None,
        falsified: vec![],
        holes: vec![],
        moved: vec![],
    };
    let mut readable = vec![];
    for h in &finds {
        let layered = layer(doc, &h.doc)?;
        let why = if unreadable(&layered, runtime) {
            Some(UNREAD.to_owned())
        } else {
            misread(std::slice::from_ref(h), doc, base_roles.as_ref(), &layered)?
        };
        match why {
            Some(why) => {
                pending.unreadable.insert(h.name.clone(), why);
            }
            None => readable.push(h.clone()),
        }
    }
    pending.finds = finds;
    let mut page = None;
    for h in &readable {
        page = (input.page_for)(std::slice::from_ref(h))?;
        if page.is_some() {
            break;
        }
    }
    let drops = Map::new();
    let run = |hyps: Vec<Hypothesis>| -> Result<Union<'a>> {
        union(UnionInput {
            doc,
            all: map(input.capture.hypotheses())?,
            hyps,
            base: captured_projection(input.capture, runtime)?,
            runtime,
            stamp: input.stamp,
            take: &[],
            drops: &drops,
            brief: input.brief,
            page: page.as_ref(),
        })
    };
    for h in &readable {
        pending.alone.insert(h.name.clone(), run(vec![h.clone()])?);
    }
    if readable.len() < 2 {
        return Ok(pending);
    }
    pending.contested = contested(&readable)?;
    if !pending.contested.is_empty() {
        return Ok(pending);
    }
    let merged = readable
        .iter()
        .try_fold(doc.clone(), |merged, h| layer(&merged, &h.doc))?;
    pending.together = if unreadable(&merged, runtime) {
        Some(TOGETHER_UNREAD.to_owned())
    } else {
        misread(&readable, doc, base_roles.as_ref(), &merged)?
    };
    if pending.together.is_some() {
        return Ok(pending);
    }
    let whole = run(readable)?;
    // A line one finding already brings is its own, whatever the union files it under.
    let seen = pending
        .alone
        .values()
        .flat_map(|c| c.falsified.iter().chain(&c.holes).chain(&c.moved))
        .cloned()
        .collect::<BTreeSet<_>>();
    let beyond = |lines: &[String]| {
        lines
            .iter()
            .filter(|line| !seen.contains(*line))
            .cloned()
            .collect::<Vec<_>>()
    };
    pending.falsified = beyond(&whole.falsified);
    pending.holes = beyond(&whole.holes);
    pending.moved = beyond(&whole.moved);
    Ok(pending)
}

/// What one finding laid over the base changes and breaks, in the words of the hypotheses'
/// report; empty when it changes nothing the base holds.
fn if_accepted(c: &Union<'_>) -> Result<Vec<String>> {
    let judgments = &c.base.base.judgments;
    let mut out = vec![];
    for u in c.updates.iter().filter(|u| !judgments.contains_key(&u.id)) {
        out.push(format!(
            "  {}: {} -> {}",
            u.id,
            short(&u.old, 36),
            short(&u.new, 36)
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
    }
    for r in &c.reversed {
        let (old, new) = (verdict(&r.old), verdict(&r.new));
        out.push(format!(
            "  {}: {}",
            r.id,
            if !same(&old, &new)? {
                format!("{} -> {}", short(&old, 36), short(&new, 36))
            } else {
                "the same verdict on other grounds".into()
            }
        ));
        out.push(if r.allowed {
            format!("    by its own condition - {}", r.why)
        } else if c.untakeable.contains(&r.id) {
            format!("    {}", r.why)
        } else {
            format!("    {} - a person decides whether it stands", r.why)
        });
        for d in c.drops_needed.get(&r.id).into_iter().flatten() {
            out.push(format!(
                "    no longer rests on {d} - accepting it names the reason"
            ));
        }
    }
    out.extend(c.falsified.iter().map(|l| format!("  FALSIFIED {l}")));
    out.extend(c.holes.iter().map(|l| format!("  FAIL {l}")));
    out.extend(c.moved.iter().map(|l| format!("  MOVED {l}")));
    if !out.is_empty() && !c.arrived.is_empty() {
        let added = c
            .arrived
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>();
        out.insert(0, cut(&format!("  adds {}", added.join(", ")), 110));
    }
    Ok(out)
}

/// The dry run's part for the pending findings, after the hypotheses': the findings, the ids
/// two of them contest, what each would change and break if accepted, what only all of them
/// together break; then whether accepting them breaks anything, and that nothing but this run
/// waits on it.
pub(super) fn lines(p: &Pending<'_>) -> Result<Vec<String>> {
    let base = &p.base.base;
    let mut out = vec![format!(
        "pending ({}): shared findings, each laid over the base alone - tested here, never written",
        p.finds.len()
    )];
    for h in &p.finds {
        let status = get(&h.head, "publication");
        let scope = get(status, "scope");
        out.push(format!(
            "  {} · {} · {}: {} · {}",
            name(h),
            py(get(status, "state")),
            py(get(scope, "kind")),
            py(get(scope, "environment")),
            strings(get(status, "roots")).join(", ")
        ));
    }
    out.push(String::new());
    if !p.contested.is_empty() {
        out.push(format!(
            "contested ({}): an id two pending findings hold differently - at most one of them is accepted",
            p.contested.len()
        ));
        for (id, holders) in &p.contested {
            out.push(format!("  {id}:"));
            if base.reader.ids.contains(id) {
                out.push(
                    "    the base holds ".to_owned()
                        + &describe(id, &base.reader.raw[id], base, "")?,
                );
            }
            for holder in holders {
                let h = p.finds.iter().find(|h| &h.name == holder).unwrap();
                let world = projection(&layer(&p.base_doc, &h.doc)?, &Map::new(), p.runtime)?;
                out.push(format!(
                    "    {} says {}",
                    name(h),
                    describe(id, &h.raw[id], &world.base, "")?
                ));
            }
        }
    }
    for h in &p.finds {
        if let Some(why) = p.unreadable.get(&h.name) {
            out.push(format!(
                "{}, if accepted: it cannot be read over the base - {why}",
                name(h)
            ));
            continue;
        }
        let c = &p.alone[&h.name];
        let lines = if_accepted(c)?;
        if !lines.is_empty() {
            out.push(format!("{}, if accepted:", name(h)));
            out.extend(lines);
        } else if !c.arrived.is_empty() {
            let added = c
                .arrived
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>();
            out.push(cut(
                &format!(
                    "{}, if accepted: nothing breaks - it adds {}",
                    name(h),
                    added.join(", ")
                ),
                110,
            ));
        } else {
            out.push(format!(
                "{}, if accepted: nothing changes - the base already holds what it says",
                name(h)
            ));
        }
    }
    if let Some(why) = &p.together {
        out.push(format!(
            "together, if every one is accepted: they cannot be read over the base as one - {why}"
        ));
    } else if !(p.falsified.is_empty() && p.holes.is_empty() && p.moved.is_empty()) {
        out.push("together, if every one is accepted:".into());
        out.extend(p.falsified.iter().map(|l| format!("  FALSIFIED {l}")));
        out.extend(p.holes.iter().map(|l| format!("  FAIL {l}")));
        out.extend(p.moved.iter().map(|l| format!("  MOVED {l}")));
    }
    out.push(String::new());
    let alone = p.alone.values().collect::<Vec<_>>();
    let mut what = vec![];
    if !p.unreadable.is_empty() {
        what.push("a finding that cannot be read over the base".to_owned());
    }
    if p.together.is_some() {
        what.push("findings that cannot be read together".into());
    }
    if !p.contested.is_empty() {
        what.push("an id two findings contest".into());
    }
    if !p.falsified.is_empty()
        || alone
            .iter()
            .any(|c| !c.falsified.is_empty() || !c.head_falsified.is_empty())
    {
        what.push("a falsifier holds on a pending value".into());
    }
    if !p.holes.is_empty() || alone.iter().any(|c| !c.holes.is_empty()) {
        what.push("a hole".into());
    }
    if alone.iter().any(|c| !c.refused.is_empty()) {
        what.push("a contested reading".into());
    }
    let untaken = alone.iter().map(|c| c.untaken.len()).sum::<usize>();
    if untaken > 0 {
        what.push(format!(
            "{untaken} reversal{} a person decides",
            if untaken == 1 { "" } else { "s" }
        ));
    }
    if alone.iter().any(|c| !c.drops_needed.is_empty()) {
        what.push("a dropped dependency to name".into());
    }
    let moved = p.moved.len() + alone.iter().map(|c| c.moved.len()).sum::<usize>();
    if !what.is_empty() {
        out.push(format!(
            "pending not clean: {} - the pending findings alone make this run red; no fold, commit or merge waits on it",
            what.join(", ")
        ));
        out.push("  a person decides each: one that is wrong is rejected with its reason (pending reject <revision> --reason \"<why>\"); one that stands is read again and set in the base, or accepted in the knowledge PR".into());
    } else if moved > 0 {
        out.push(format!(
            "pending: {moved} judgment{} to re-review when accepted - a premise moved under {}",
            if moved == 1 { "" } else { "s" },
            if moved == 1 { "it" } else { "them" }
        ));
    } else {
        out.push(format!(
            "pending clean: accepting {} breaks nothing the base holds",
            if p.finds.len() == 1 { "it" } else { "them" }
        ));
    }
    Ok(out)
}

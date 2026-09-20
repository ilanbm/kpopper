//! Ordinary reader projections retain the legacy interpretation of authored text.
use crate::ordinary_value::py;
use crate::{
    Result,
    history_contract::error,
    ordinary_fields as F, ordinary_findings as A, ordinary_language as L,
    ordinary_semantics::{self as R, Reader},
    ordinary_value::Value as V,
    ordinary_value::{Map, list, map, string_is, text, truth},
    ordinary_value::{blocked_text, named, reopened_text},
    reasoning_runtime::Runtime,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};
type Reach = (Vec<(String, String)>, BTreeSet<String>);
static MISFILED_REOPENER: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*(<=|>=|==|!=|<|>)")
        .unwrap()
});
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn get<'a>(m: &'a Map, k: &str) -> &'a V {
    m.get(k).unwrap_or(&V::Null)
}
fn iterable(v: &V) -> Result<Vec<String>> {
    if !truth(v) {
        return Ok(vec![]);
    }
    match v {
        V::List(a) => a.iter().map(|v| Ok(py(v))).collect(),
        V::Map(m) => Ok(m.keys().cloned().collect()),
        V::Text(s) => Ok(s.chars().map(|c| c.to_string()).collect()),
        _ => Err(error("dependency declaration is not iterable")),
    }
}

pub(crate) fn cut(s: &str, n: usize) -> String {
    if s.chars().count() < n {
        s.into()
    } else {
        s.chars().take(n).collect::<String>() + " ..."
    }
}
pub(crate) fn predicate_text(v: &V) -> String {
    if let V::Map(m) = v {
        let predicate = matches!(get(m, "op"), V::Text(op) if ["eq", "ne", "lt", "le", "gt", "ge"].contains(&op.as_str()))
            || L::legacy_expression_detailed(v, true).is_ok();
        let tree = match L::legacy_expression_detailed(v, predicate) {
            Ok(t) => t,
            Err(e) => return format!("<invalid expression: {}>", e.0),
        };
        if let Some(V::Text(t)) = m.get("expr") {
            return t.clone();
        }
        fn render(v: &V) -> String {
            let Ok(m) = map(v) else {
                return String::new();
            };
            if let Some(v) = m.get("ref").or_else(|| m.get("num")) {
                return py(v);
            }
            if let Some(V::Text(t)) = m.get("text") {
                return serde_json::to_string(t).unwrap();
            }
            if let Some(V::Bool(b)) = m.get("bool") {
                return b.to_string();
            }
            let op = match text(get(m, "op")).unwrap_or("") {
                "add" => "+",
                "sub" => "-",
                "mul" => "*",
                "div" => "/",
                "eq" => "==",
                "ne" => "!=",
                "lt" => "<",
                "le" => "<=",
                "gt" => ">",
                "ge" => ">=",
                x => x,
            };
            let args = list(get(m, "args")).unwrap_or(&[]);
            if args.len() != 2 {
                return String::new();
            }
            format!("({} {op} {})", render(&args[0]), render(&args[1]))
        }
        let t = render(&tree);
        t.strip_prefix('(')
            .and_then(|v| v.strip_suffix(')'))
            .unwrap_or(&t)
            .into()
    } else if truth(v) {
        py(v)
    } else {
        String::new()
    }
}
pub(crate) fn short(v: &V, n: usize) -> String {
    let rendered = if map(v).is_ok_and(|m| m.contains_key("op") || m.contains_key("expr")) {
        predicate_text(v)
    } else {
        py(v)
    };
    let normalized = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= n {
        normalized
    } else {
        normalized
            .chars()
            .take(n.saturating_sub(1))
            .collect::<String>()
            + "…"
    }
}
pub(crate) fn apart(a: &V, b: &V, n: usize) -> (String, String) {
    let a = py(a).split_whitespace().collect::<Vec<_>>().join(" ");
    let b = py(b).split_whitespace().collect::<Vec<_>>().join(" ");
    if a.chars().count() <= n && b.chars().count() <= n {
        return (a, b);
    }
    let ac = a.chars().collect::<Vec<_>>();
    let bc = b.chars().collect::<Vec<_>>();
    let same = ac.iter().zip(&bc).take_while(|(a, b)| a == b).count();
    if same < n / 2 {
        return (short(&s(&a), n), short(&s(&b), n));
    }
    let mut at = ac[..same]
        .iter()
        .rposition(|c| *c == ' ')
        .map_or(0, |i| i + 1);
    if at == 0 || same - at > n {
        at = same.saturating_sub(n / 4);
    }
    (
        "…".to_owned()
            + &short(
                &s(&ac[at..].iter().collect::<String>()),
                n.saturating_sub(1),
            ),
        "…".to_owned()
            + &short(
                &s(&bc[at..].iter().collect::<String>()),
                n.saturating_sub(1),
            ),
    )
}
fn claim(body: &V) -> V {
    let Ok(b) = map(body) else {
        return body.clone();
    };
    for f in ["verdict", "v", "quoted", "rule", "title"] {
        if let Some(v) = b.get(f).filter(|v| **v != V::Null) {
            return if f == "rule" && matches!(v, V::Map(_)) {
                L::legacy_rule(v).unwrap_or_else(|_| v.clone())
            } else {
                v.clone()
            };
        }
    }
    body.clone()
}
fn same_claim(a: &V, b: &V) -> bool {
    if matches!(a, V::Map(_) | V::List(_)) || matches!(b, V::Map(_) | V::List(_)) {
        crate::ordinary_value::python_equal(a, b)
    } else {
        R::same_legacy(a, b)
    }
}
pub struct World<'a> {
    pub(crate) reader: Reader<'a>,
    pub(crate) judgments: Map,
    pub(crate) states: Map,
}
impl<'a> World<'a> {
    pub(crate) fn new(
        doc: &V,
        layers: &Map,
        conflicts: &BTreeSet<String>,
        runtime: Option<&'a Runtime>,
    ) -> Result<Self> {
        let mut reader =
            Reader::new(doc, runtime)?.with_layers(layers.clone(), conflicts.clone())?;
        // Public text views walk entry IDs lexically. Only this lookup table is
        // ordered here; each source body retains its original mapping order.
        reader.raw = std::mem::take(&mut reader.raw)
            .into_iter()
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            .collect();
        let dep = text(&reader.fields["deps"])?;
        let judgments = reader
            .raw
            .iter()
            .filter(|(_, v)| map(v).is_ok_and(|m| m.contains_key(dep)))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Map>();
        for body in judgments.values() {
            iterable(get(map(body)?, dep))?;
        }
        let states = judgments
            .iter()
            .map(|(id, b)| Ok((id.clone(), A::judgment_state(&reader, b)?)))
            .collect::<Result<Map>>()?;
        Ok(Self {
            reader,
            judgments,
            states,
        })
    }
    pub(crate) fn deps(&self, id: &str) -> Result<Vec<String>> {
        iterable(get(
            map(&self.judgments[id])?,
            text(&self.reader.fields["deps"])?,
        ))
    }
    pub(crate) fn pred(&self, id: &str) -> V {
        R::predicate_of(&self.judgments[id], &self.reader.fields)
    }
    pub(crate) fn reach(&self, changed: &[String]) -> Result<Reach> {
        let mut feeds = BTreeMap::<String, Vec<String>>::new();
        let mut judgments = BTreeMap::<String, Vec<String>>::new();
        for id in &self.reader.ids {
            if self.judgments.contains_key(id) {
                continue;
            }
            let Some(b) = self.reader.raw.get(id).and_then(|b| map(b).ok()) else {
                continue;
            };
            let refs = if matches!(b.get("rule"), Some(V::Map(_))) {
                L::legacy_references(&b["rule"])
            } else {
                ["rule", "v"]
                    .iter()
                    .filter_map(|f| b.get(f))
                    .filter(|v| matches!(v,V::Text(t) if R::EXPR.is_match(t)))
                    .flat_map(R::predicate_refs)
                    .filter(|id| self.reader.ids.contains(id))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect()
            };
            for dep in refs
                .into_iter()
                .filter(|d| self.reader.ids.contains(d) && d != id)
            {
                feeds.entry(dep).or_default().push(id.clone());
            }
        }
        for id in self.judgments.keys() {
            for dep in self.deps(id)?.into_iter().collect::<BTreeSet<_>>() {
                judgments.entry(dep).or_default().push(id.clone());
            }
        }
        let mut hit = vec![];
        let mut hit_ids = BTreeSet::new();
        let mut moved = changed.iter().cloned().collect::<BTreeSet<_>>();
        let mut frontier = changed.to_vec();
        while let Some(source) = frontier.pop() {
            for id in feeds.get(&source).into_iter().flatten() {
                if moved.insert(id.clone()) {
                    frontier.push(id.clone());
                }
            }
            for id in judgments.get(&source).into_iter().flatten() {
                if hit_ids.insert(id.clone()) {
                    hit.push((id.clone(), source.clone()));
                    frontier.push(id.clone());
                }
            }
        }
        moved.extend(hit_ids);
        Ok((hit, moved))
    }
}
pub struct Projection<'a> {
    pub(crate) base: World<'a>,
    pub(crate) layers: BTreeMap<String, World<'a>>,
    pub(crate) hypotheses: Map,
    pub(crate) unread: Vec<String>,
    unread_failures: Vec<String>,
    pub(crate) disputed: BTreeMap<String, Vec<(String, V)>>,
    pub(crate) knowledge: Vec<String>,
}

/// Stable data needed by the session stop gate.  This deliberately excludes
/// page coverage: the Hub is the sole owner of optional HTML assessment.
impl<'a> Projection<'a> {
    pub fn new(
        document: &V,
        hypotheses: &Map,
        conflicts: &Map,
        knowledge: Vec<String>,
        runtime: Option<&'a Runtime>,
    ) -> Result<Self> {
        let hypotheses = hypotheses
            .iter()
            .map(|(name, h)| {
                let mut h = map(h)?.clone();
                let doc = h
                    .get("document")
                    .or_else(|| h.get("doc"))
                    .cloned()
                    .unwrap_or(V::Null);
                h.insert("document".into(), doc.clone());
                if !h.contains_key("ids") {
                    h.insert(
                        "ids".into(),
                        V::List(
                            F::collections(&doc)?
                                .values()
                                .flat_map(|m| m.keys().cloned().map(V::Text))
                                .collect(),
                        ),
                    );
                }
                h.insert("doc".into(), doc);
                Ok((name.clone(), V::Map(h)))
            })
            .collect::<Result<Map>>()?;
        let conflict_ids = conflicts.keys().cloned().collect();
        let base = World::new(document, &hypotheses, &conflict_ids, runtime)?;
        let mut layers = BTreeMap::new();
        let mut unread = vec![];
        let mut unread_failures = vec![];
        let mut holders = BTreeMap::<String, Vec<(String, V)>>::new();
        for (name, h) in &hypotheses {
            let h = map(h)?;
            let label = if string_is(get(h, "kind"), "contribution") {
                "contribution"
            } else {
                "hypothesis"
            };
            if truth(get(h, "error")) {
                unread_failures.push(format!(
                    "{label} {name} could not be read: {}",
                    py(&h["error"])
                ));
                unread.push(format!(
                    "! hypothesis {name} could not be read: {}",
                    py(&h["error"])
                ));
                continue;
            }
            let doc = crate::ordinary_semantics::layer(
                document,
                &V::Map(hypotheses.clone()),
                std::slice::from_ref(name),
            )?;
            match World::new(&doc, &hypotheses, &BTreeSet::new(), runtime) {
                Ok(world) => {
                    layers.insert(name.clone(), world);
                }
                Err(e) => {
                    unread_failures.push(format!(
                        "{label} {name} cannot be read over the base: {}",
                        e.0
                    ));
                    unread.push(format!(
                        "! hypothesis {name} cannot be read over the base: {}",
                        e.0
                    ));
                    continue;
                }
            }
            for (id, b) in F::collections(&h["document"])?
                .values()
                .flat_map(|m| m.iter())
            {
                holders
                    .entry(id.clone())
                    .or_default()
                    .push((name.clone(), claim(b)));
            }
        }
        let mut disputed = holders
            .into_iter()
            .filter(|(_, v)| v.len() > 1 && v[1..].iter().any(|(_, c)| !same_claim(c, &v[0].1)))
            .collect::<BTreeMap<_, _>>();
        for (id, variants) in conflicts {
            let values = list(variants)?
                .iter()
                .map(|v| {
                    let a = list(v)?;
                    crate::require(a.len() == 2, "invalid ordinary conflict")?;
                    Ok((text(&a[0])?.to_owned(), claim(&a[1])))
                })
                .collect::<Result<_>>()?;
            disputed.insert(id.clone(), values);
        }
        Ok(Self {
            base,
            layers,
            hypotheses,
            unread,
            unread_failures,
            disputed,
            knowledge,
        })
    }
    fn every(&self) -> BTreeSet<String> {
        self.base
            .reader
            .ids
            .iter()
            .cloned()
            .chain(
                self.layers
                    .values()
                    .flat_map(|w| w.reader.ids.iter().cloned()),
            )
            .collect()
    }

    fn expanded(&self, seeds: &[String]) -> Result<(Vec<String>, Vec<String>)> {
        let every = self.every();
        let mut ids = vec![];
        let mut lines = vec![];
        for seed in seeds {
            if every.contains(seed) {
                ids.push(seed.clone());
                continue;
            }
            let matches = every
                .iter()
                .filter(|id| {
                    id.starts_with(&format!("{seed}.")) || id.split('.').next() == Some(seed)
                })
                .cloned()
                .collect::<Vec<_>>();
            crate::require(
                !matches.is_empty(),
                &format!(
                    "{seed} is not an entry or a prefix in this record. Run `open` to see what it holds."
                ),
            )?;
            lines.push(format!(
                "# {seed} -> {} entries: {}{}",
                matches.len(),
                matches
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
                if matches.len() > 8 { " ..." } else { "" }
            ));
            ids.extend(matches);
        }
        Ok((ids, lines))
    }
    fn held(&self, name: &str, id: &str) -> bool {
        map(&self.hypotheses[name])
            .ok()
            .and_then(|h| h.get("ids"))
            .and_then(|v| list(v).ok())
            .is_some_and(|ids| ids.iter().any(|i| string_is(i, id)))
    }
    fn label(&self, name: &str) -> String {
        format!(
            "{} {name}",
            if map(&self.hypotheses[name]).is_ok_and(|m| string_is(get(m, "kind"), "contribution"))
            {
                "contribution"
            } else {
                "hypothesis"
            }
        )
    }
    pub fn affects(&self, seeds: &[String]) -> Result<String> {
        let (expanded, mut lines) = self.expanded(seeds)?;
        let mut total = 0;
        for (name, world) in
            std::iter::once((None, &self.base)).chain(self.layers.iter().map(|(n, w)| (Some(n), w)))
        {
            let changed = expanded
                .iter()
                .filter(|id| world.reader.ids.contains(*id))
                .cloned()
                .collect::<Vec<_>>();
            let (hit, moved) = world.reach(&changed)?;
            for (id, via) in hit {
                if name.is_some_and(|name| !self.held(name, &id)) {
                    continue;
                }
                let pred = world.pred(&id);
                let refs = R::predicate_refs(&pred);
                let named = world
                    .deps(&id)?
                    .into_iter()
                    .filter(|d| moved.contains(d) && refs.contains(d))
                    .collect::<Vec<_>>();
                let why = if named.is_empty() {
                    "flagged only".into()
                } else {
                    format!("evaluate the predicate against {}", named.join(", "))
                };
                lines.push(format!(
                    "{id}{}\n    via {via} -> {why}{}",
                    name.map(|n| format!(" (in {})", self.label(n)))
                        .unwrap_or_default(),
                    if truth(&pred) {
                        format!("\n    predicate: {}", predicate_text(&pred))
                    } else {
                        String::new()
                    }
                ));
                total += 1;
            }
        }
        lines.push(if total == 0 {
            "nothing rests on that".into()
        } else {
            format!("\n{total} judgments reached")
        });
        Ok(lines.join("\n") + "\n")
    }
}

fn display(value: &V) -> String {
    let rational = match value {
        V::Integer(_) | V::Float(_) => {
            crate::reasoning_scope::query_value(&value.try_typed().unwrap(), 0)
                .map(|v| V::from_typed(&v))
                .ok()
                .and_then(|v| {
                    map(&v)
                        .ok()
                        .map(|m| (py(get(m, "numerator")), py(get(m, "denominator"))))
                })
        }
        V::Map(m) => m
            .get("rational")
            .and_then(|v| list(v).ok())
            .filter(|a| a.len() == 2)
            .map(|a| (py(&a[0]), py(&a[1]))),
        _ => None,
    };
    if let Some((a, b)) = rational
        && let (Ok(mut a), Ok(mut b)) = (
            a.parse::<num_bigint::BigInt>(),
            b.parse::<num_bigint::BigInt>(),
        )
        && b != 0.into()
    {
        let (mut x, mut y) = (a.clone(), b.clone());
        while y != 0.into() {
            let z = x % &y;
            x = y;
            y = z;
        }
        a /= &x;
        b /= &x;
        if b < 0.into() {
            a = -a;
            b = -b;
        }
        return if b == 1.into() {
            a.to_string()
        } else {
            format!("{a}/{b}")
        };
    }
    py(value)
}
fn fmt(value: &V) -> String {
    if let V::Integer(n) = value {
        let t = n.as_str();
        let (sign, digits) = t.strip_prefix('-').map_or(("", t), |d| ("-", d));
        let mut s = String::from(sign);
        for (i, c) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                s.push(',');
            }
            s.push(c);
        }
        s
    } else if let V::Float(f) = value {
        let scientific = format!("{:.9e}", f.get());
        let (mantissa, exponent) = scientific.split_once('e').unwrap();
        let exponent = exponent.parse::<i32>().unwrap();
        let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
        if !(-4..10).contains(&exponent) {
            return format!("{mantissa}e{exponent:+03}");
        }
        let (sign, mantissa) = mantissa
            .strip_prefix('-')
            .map_or(("", mantissa), |m| ("-", m));
        let mut digits = mantissa.replace('.', "");
        let fixed = if exponent >= 0 {
            let point = exponent as usize + 1;
            if digits.len() < point {
                digits.push_str(&"0".repeat(point - digits.len()));
            }
            if digits.len() > point {
                digits.insert(point, '.');
            }
            digits
        } else {
            format!("0.{}{}", "0".repeat((-exponent - 1) as usize), digits)
        };
        let (whole, fraction) = fixed
            .split_once('.')
            .map_or((fixed.as_str(), None), |(w, f)| (w, Some(f)));
        let mut out = String::from(sign);
        for (i, c) in whole.chars().enumerate() {
            if i > 0 && (whole.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
        if let Some(f) = fraction {
            out.push('.');
            out.push_str(f);
        }
        out
    } else if matches!(value, V::Map(_)) {
        display(value)
    } else {
        py(value)
    }
}
impl World<'_> {
    /// The ordinary reader's exact post-observation state and explanation. This
    /// uses captured inputs and the existing computation runtime, never source discovery.
    pub(crate) fn state(&self, id: &str) -> Result<(String, String)> {
        let world = self;
        fn field<'a>(value: &'a V, key: &str) -> &'a V {
            match value {
                V::Map(m) => m.get(key).unwrap_or(&V::Null),
                _ => &V::Null,
            }
        }
        fn names(value: &V) -> Vec<String> {
            match value {
                V::List(a) => a.iter().map(crate::ordinary_value::py).collect(),
                V::Map(m) => m.keys().cloned().collect(),
                V::Text(s) => s.chars().map(|c| c.to_string()).collect(),
                _ => vec![],
            }
        }

        let body = &world.judgments[id];
        let fields = world.reader.fields();
        let deps = names(field(body, text(&fields["deps"])?));
        let pred = R::predicate_of(body, fields);
        let blocked = blocked_text(body);
        let missing = deps
            .iter()
            .filter(|d| !world.reader.ids.contains(*d))
            .cloned()
            .collect::<Vec<_>>();
        let result = |tag: &str, reason: String| Ok((tag.into(), reason));
        if !missing.is_empty() {
            return if blocked.is_empty() {
                result(
                    "BROKEN",
                    format!("rests on {}, which is not an entry", missing.join(", ")),
                )
            } else {
                result(
                    "BLOCKED",
                    format!(
                        "waiting on {} - {}",
                        missing.join(", "),
                        blocked.chars().take(70).collect::<String>()
                    ),
                )
            };
        }
        let named = R::predicate_refs(&pred)
            .into_iter()
            .filter(|id| world.reader.ids.contains(id))
            .collect::<Vec<_>>();
        let evaluated = world.reader.predicate(&pred)?;
        if !named.is_empty() && evaluated == Some(true) {
            return result(
                "FIRED",
                format!(
                    "wrong_if holds ({}) - broken by its own condition",
                    predicate_text(&pred)
                ),
            );
        }
        let snapshot = text(&fields["snapshot"])?;
        let seen = field(body, snapshot);
        let unchecked = deps
            .iter()
            .filter(|d| !snapshot.is_empty() && !map(seen).is_ok_and(|m| m.contains_key(d)))
            .cloned()
            .collect::<Vec<_>>();
        let empty = Map::new();
        let seen = map(seen).unwrap_or(&empty);
        let formula_only = seen
            .iter()
            .filter(|(id, old)| {
                world.reader.raw().get(id).is_some_and(|b| {
                    let rule = field(b, "rule");
                    crate::ordinary_domain_counts::legacy_rule(old, rule)
                        .is_some_and(|old| crate::ordinary_domain_counts::same_rule(&old, rule))
                })
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let moves = world.moved(id)?;
        if let Some((dep, old, new, _)) = moves.iter().find(|(_, _, _, s)| *s == "moved") {
            let snapshot = seen.get(dep).unwrap_or(&V::Null);
            let current = world.reader.raw().get(dep).unwrap_or(&V::Null);
            if matches!(field(snapshot, "computed"), V::Map(_))
                && !crate::ordinary_domain_counts::same_rule(
                    field(field(snapshot, "computed"), "rule"),
                    field(current, "rule"),
                )
            {
                return result("MOVED", format!("{dep}: formula changed since review"));
            }
            if crate::ordinary_domain_counts::legacy_rule(snapshot, field(current, "rule"))
                .is_some()
            {
                return result(
                    "MOVED",
                    format!(
                        "{dep}: formula changed since legacy review; no historical result was recorded"
                    ),
                );
            }
            let (was, now) = apart(old, new, 40);
            return result(
                "MOVED",
                format!(
                    "{dep} moved {was} -> {now} since it was reviewed - if it still holds: review {id}"
                ),
            );
        }
        if !unchecked.is_empty() {
            return result(
                "UNCHECKED",
                format!(
                    "never checked against {} - if it holds: review {id}",
                    unchecked.join(", ")
                ),
            );
        }
        if !formula_only.is_empty() {
            return result(
                "UNCHECKED",
                format!(
                    "legacy snapshot records only the formula for {}, not a historical result; review {id} to record the current calculation",
                    formula_only.join(", ")
                ),
            );
        }
        if let Some((dep, old, new, _)) = moves.iter().find(|(_, _, _, s)| *s == "muted") {
            let (was, now) = apart(old, new, 40);
            return result(
                "MUTED",
                format!(
                    "{dep} moved {was} -> {now}, inside wrong_if ({}) - nothing is asked",
                    predicate_text(&pred)
                ),
            );
        }
        if named.is_empty() && blocked.is_empty() {
            let reopened = if !truth(&pred) {
                reopened_text(body)
            } else {
                String::new()
            };
            return if reopened.is_empty() {
                result(
                    "NO_PREDICATE",
                    "nothing evaluable would say otherwise".into(),
                )
            } else {
                result(
                    "HOLDS",
                    format!("decided; reopened by {}", short(&V::Text(reopened), 80)),
                )
            };
        }
        if named.is_empty() {
            return result(
                "DECLARED",
                format!(
                    "no predicate to evaluate; declared - {}",
                    short(&V::Text(blocked), 80)
                ),
            );
        }
        let short = short(&pred, 80);
        if evaluated.is_none() {
            if !R::why_undecided(&pred).is_empty() {
                return result(
                    "UNKNOWN",
                    format!("wrong_if is not a comparison this reader decides ({short})"),
                );
            }
            if R::predicate_refs(&pred)
                .iter()
                .any(|id| id.starts_with("page."))
            {
                return result(
                    "UNKNOWN",
                    format!(
                        "wrong_if is counted when the page is built ({short}) - kpop experimental hub --verify decides it"
                    ),
                );
            }
            return result(
                "UNKNOWN",
                format!(
                    "wrong_if cannot currently be evaluated ({short}); a value or the Lean core is unavailable, or types differ"
                ),
            );
        }
        result("HOLDS", format!("wrong_if does not hold ({short})"))
    }
}

impl World<'_> {
    pub(crate) fn moved(&self, id: &str) -> Result<Vec<(String, V, V, &'static str)>> {
        let body = map(&self.judgments[id])?;
        let empty = Map::new();
        let seen = map(get(
            body,
            text(&self.reader.fields["snapshot"]).unwrap_or(""),
        ))
        .unwrap_or(&empty);
        let pred = self.pred(id);
        let refs = R::predicate_refs(&pred)
            .into_iter()
            .filter(|r| self.reader.ids.contains(r))
            .collect::<BTreeSet<_>>();
        let verdict = if refs.is_empty() {
            None
        } else {
            self.reader.predicate(&pred)?
        };
        let mut out = vec![];
        for (dep, old) in seen {
            let rule = self
                .reader
                .raw
                .get(dep)
                .and_then(|b| map(b).ok())
                .map(|m| get(m, "rule"))
                .unwrap_or(&V::Null);
            if let Some(legacy) = crate::ordinary_domain_counts::legacy_rule(old, rule) {
                if !crate::ordinary_domain_counts::same_rule(&legacy, rule) {
                    out.push((dep.clone(), old.clone(), s(&predicate_text(rule)), "moved"));
                }
                continue;
            }
            let snapshot = map(old)
                .ok()
                .and_then(|m| m.get("computed"))
                .and_then(|v| map(v).ok());
            if self.reader.ids.contains(dep) && (matches!(rule, V::Map(_)) || snapshot.is_some()) {
                let now = self.reader.value(dep)?;
                let previous = snapshot.map(|m| get(m, "value")).unwrap_or(old);
                let rule_moved = snapshot.is_some_and(|m| {
                    !crate::ordinary_domain_counts::same_rule(get(m, "rule"), rule)
                });
                if rule_moved
                    || (now != V::Null && previous != &V::Null && !A::equal_value(previous, &now))
                {
                    let state = if refs.contains(dep) && verdict == Some(true) {
                        "crossed"
                    } else if refs.contains(dep) && verdict == Some(false) && !rule_moved {
                        "muted"
                    } else {
                        "moved"
                    };
                    out.push((dep.clone(), previous.clone(), now, state));
                }
                continue;
            }
            if !self.reader.ids.contains(dep) || matches!(old, V::List(_) | V::Map(_)) {
                continue;
            }
            let now = self.reader.value(dep)?;
            if now == V::Null || matches!(now, V::List(_) | V::Map(_)) || R::same_legacy(old, &now)
            {
                continue;
            }
            let state = if refs.contains(dep) {
                match verdict {
                    Some(true) => "crossed",
                    Some(false) => "muted",
                    None => "moved",
                }
            } else {
                "moved"
            };
            out.push((dep.clone(), old.clone(), now, state));
        }
        Ok(out)
    }
    fn said(&self, value: &V, width: usize) -> Result<Vec<String>> {
        let expression =
            regex::Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+)\s*\}\}")
                .unwrap();
        let mut failure = None;
        let replaced = expression
            .replace_all(&py(value), |capture: &regex::Captures| {
                let id = &capture[1];
                if let Some(body) = self.judgments.get(id) {
                    return self.verdict(id, body);
                }
                if !self.reader.ids.contains(id) {
                    return capture[0].into();
                }
                match self.reader.value(id) {
                    Ok(v)
                        if v != V::Null
                            && (!matches!(v, V::List(_) | V::Map(_))
                                || A::number(&v).is_some()) =>
                    {
                        fmt(&v)
                    }
                    Err(e) => {
                        failure = Some(e);
                        String::new()
                    }
                    _ => {
                        let name = self.reader.raw.get(id).map(named).unwrap_or_default();
                        if name.is_empty() {
                            id.rsplit('.').next().unwrap().replace('_', " ")
                        } else {
                            name
                        }
                    }
                }
            })
            .into_owned();
        if let Some(e) = failure {
            return Err(e);
        }
        let mut lines = vec![];
        let mut line = String::new();
        for word in replaced.split_whitespace() {
            let mut chars = word.chars().collect::<Vec<_>>();
            if !line.is_empty() && line.chars().count() + 1 + chars.len() > width {
                lines.push(std::mem::take(&mut line));
            }
            while chars.len() > width {
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                lines.push(chars.drain(..width).collect());
            }
            if !chars.is_empty() {
                if !line.is_empty() {
                    line.push(' ');
                }
                line.extend(chars);
            }
        }
        if !line.is_empty() || lines.is_empty() {
            lines.push(line);
        }
        Ok(lines)
    }
    fn verdict(&self, id: &str, body: &V) -> String {
        map(body)
            .ok()
            .and_then(|b| {
                b.get("verdict")
                    .filter(|v| truth(v))
                    .or_else(|| b.get("title").filter(|v| truth(v)))
            })
            .map(py)
            .unwrap_or_else(|| id.into())
    }
    fn describe(&self, id: &str) -> Result<(String, String)> {
        let value = self.reader.raw.get(id).cloned().unwrap_or(V::Null);
        let body = if matches!(value, V::Map(_)) {
            value
        } else {
            V::Map(Map::from([("v".into(), value)]))
        };
        let b = map(&body)?;
        let (mut v, mut rule) = (get(b, "v").clone(), get(b, "rule").clone());
        if matches!(&v,V::Text(t) if R::EXPR.is_match(t)&&R::predicate_refs(&v).iter().any(|id|self.reader.ids.contains(id)))
        {
            if !truth(&rule) {
                rule = v.clone();
            }
            v = V::Null;
        }
        if matches!(rule, V::Map(_)) {
            v = self.reader.value(id)?;
        }
        let shown = if v != V::Null {
            display(&v)
                + &if matches!(rule, V::Map(_)) {
                    format!(" = {}", predicate_text(&rule))
                } else {
                    String::new()
                }
        } else if truth(&rule) {
            format!("= {}", predicate_text(&rule))
        } else if truth(get(b, "quoted")) {
            format!("\"{}\"", py(&b["quoted"]))
        } else {
            String::new()
        };
        let name = named(&body);
        let mut line = format!(
            "{id}: {shown}{}",
            if name.is_empty() {
                String::new()
            } else {
                format!(" ({name})")
            }
        );
        if truth(get(b, "measure")) {
            line.push_str(&format!(" measured by {}", py(&b["measure"])));
        }
        if truth(get(b, "from")) {
            line.push_str(&format!(" <- {}", py(&b["from"])));
            if truth(get(b, "at")) {
                line.push_str(&format!(", at {}", py(&b["at"])));
            }
        }
        if let Some(of) = b
            .get("of")
            .filter(|v| truth(v))
            .or_else(|| b.get("read").filter(|v| truth(v)))
        {
            line.push_str(&format!(" as of {}", py(of)));
        }
        Ok((line, shown))
    }
    fn judgment_lines(&self, id: &str, tag: &str) -> Result<Vec<String>> {
        let body = &self.judgments[id];
        let b = map(body)?;
        let pred = self.pred(id);
        let deps = self.deps(id)?;
        let mut out = vec![cut(
            &format!("+ {id}{tag}: {}", self.verdict(id, body)),
            110,
        )];
        let blocked = blocked_text(body);
        let missing = deps
            .iter()
            .filter(|d| !self.reader.ids.contains(*d))
            .cloned()
            .collect::<Vec<_>>();
        let empty = Map::new();
        let seen =
            map(get(b, text(&self.reader.fields["snapshot"]).unwrap_or(""))).unwrap_or(&empty);
        let stale = deps
            .iter()
            .filter(|d| !seen.contains_key(d))
            .cloned()
            .collect::<Vec<_>>();
        let state = if !missing.is_empty() {
            if blocked.is_empty() {
                format!(
                    "broken: rests on {}, which is not an entry",
                    missing.join(", ")
                )
            } else {
                format!("blocked: {blocked}")
            }
        } else if string_is(
            &map(&map(&self.states[id])?["falsifier"])?["status"],
            "holds",
        ) {
            format!("broken: wrong_if holds ({})", predicate_text(&pred))
        } else if truth(&self.reader.fields["snapshot"]) && !stale.is_empty() {
            format!("unchecked: never checked against {}", stale.join(", "))
        } else {
            "holds".into()
        };
        out.push(cut(&format!("    {state}"), 110));
        if truth(get(b, "because")) {
            for (i, line) in self.said(&b["because"], 96)?.iter().enumerate() {
                out.push(format!(
                    "{}{line}",
                    if i == 0 {
                        "    because: "
                    } else {
                        "             "
                    }
                ));
            }
        }
        if truth(&pred) {
            out.push(cut(
                &format!("    wrong_if: {}", predicate_text(&pred)),
                110,
            ));
        }
        let reopened = reopened_text(body);
        if !reopened.is_empty() {
            out.push(cut(&format!("    reopened by: {reopened}"), 110));
        }
        if truth(get(b, "request")) {
            out.push(cut(
                &format!("    on the word of: {}", py(&b["request"])),
                110,
            ));
        }
        for (dep, old, now, state) in self.moved(id)? {
            let mark = match state {
                "muted" => " - within wrong_if",
                "crossed" => " - across wrong_if",
                _ => "",
            };
            let head = format!("    moved since review: {dep} ");
            let room = 110usize.saturating_sub(head.chars().count() + mark.len() + 4);
            let (a, b) = apart(&old, &now, 24.max(room / 2));
            out.push(format!("{head}{a} -> {b}{mark}"));
        }
        Ok(out)
    }
}
impl Projection<'_> {
    fn hypothesis_line(&self) -> Result<Option<String>> {
        let today = chrono::Local::now().date_naive();
        let mut items = Vec::new();
        for (name, hypothesis) in &self.hypotheses {
            let h = map(hypothesis)?;
            if string_is(get(h, "kind"), "contribution") {
                continue;
            }
            if truth(get(h, "error")) {
                items.push((
                    (chrono::NaiveDate::MIN, name.clone()),
                    format!("{name} (unreadable: {})", py(get(h, "error"))),
                ));
                continue;
            }
            let Some(world) = self.layers.get(name) else {
                let prefix = format!("! hypothesis {name} cannot be read over the base: ");
                let why = self
                    .unread
                    .iter()
                    .find_map(|s| s.strip_prefix(&prefix))
                    .unwrap_or("unavailable");
                items.push((
                    (chrono::NaiveDate::MIN, name.clone()),
                    format!("{name} (unreadable over the base: {why})"),
                ));
                continue;
            };
            let head = map(get(h, "head")).ok();
            let born = head.and_then(|h| h.get("born")).map(py).and_then(|s| {
                s.get(..10)
                    .and_then(|day| chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok())
            });
            let age = born
                .map(|day| match (today - day).num_days() {
                    n if n <= 0 => "today".into(),
                    1 => "1 day".into(),
                    n => format!("{n} days"),
                })
                .unwrap_or_else(|| "undated".into());
            let ids = list(get(h, "ids"))?
                .iter()
                .filter_map(|v| text(v).ok())
                .collect::<BTreeSet<_>>();
            let mut count = 0;
            for id in world.judgments.keys() {
                if world.deps(id)?.iter().any(|d| ids.contains(d.as_str())) {
                    count += 1;
                }
            }
            let mut bits = vec![age];
            if head
                .and_then(|h| h.get("folds"))
                .is_some_and(|v| string_is(v, "never"))
            {
                bits.push("never folds".into());
            }
            bits.push(format!(
                "{count} rest{} on it",
                if count == 1 { "s" } else { "" }
            ));
            items.push((
                (born.unwrap_or(chrono::NaiveDate::MAX), name.clone()),
                format!("{name} ({})", bits.join(", ")),
            ));
        }
        if items.is_empty() {
            return Ok(None);
        }
        items.sort_by(|a, b| a.0.cmp(&b.0));
        let count = items.len();
        let line = format!(
            "{count} hypothes{} - {}",
            if count == 1 { "is waits" } else { "es wait" },
            items
                .into_iter()
                .map(|(_, s)| s)
                .collect::<Vec<_>>()
                .join(" · ")
        );
        Ok(Some(if line.chars().count() > 110 {
            line.chars().take(110).collect::<String>() + " ..."
        } else {
            line
        }))
    }
    fn page_unserved(&self, brief: Option<&V>) -> Result<Vec<(String, String, String)>> {
        let Some(brief) = brief else {
            return Ok(vec![]);
        };
        let brief = map(brief)?;
        let mut served = BTreeSet::new();
        if let Some(tabs) = brief.get("tabs").and_then(|value| list(value).ok()) {
            for tab in tabs {
                if let Ok(tab) = map(tab)
                    && let Some(ids) = tab.get("serves").and_then(|value| list(value).ok())
                {
                    served.extend(
                        ids.iter()
                            .filter_map(|value| text(value).ok().map(str::to_owned)),
                    );
                }
            }
        }
        let mut picked = BTreeSet::new();
        let mut collect_sections = |sections: Option<&V>| {
            if let Some(sections) = sections.and_then(|value| list(value).ok()) {
                for section in sections {
                    if let Ok(section) = map(section)
                        && let Some(ids) = section.get("pick").and_then(|value| list(value).ok())
                    {
                        picked.extend(
                            ids.iter()
                                .filter_map(|value| text(value).ok().map(str::to_owned)),
                        );
                    }
                }
            }
        };
        collect_sections(brief.get("sections"));
        if let Some(tabs) = brief.get("tabs").and_then(|value| list(value).ok()) {
            for tab in tabs {
                if let Ok(tab) = map(tab) {
                    collect_sections(tab.get("sections"));
                }
            }
        }
        let mut out = vec![];
        for (id, body) in &self.base.reader.raw {
            let Ok(body) = map(body) else { continue };
            let Some(asked) = body.get("asked").filter(|value| truth(value)) else {
                continue;
            };
            if served.contains(id) {
                continue;
            }
            let authored = self
                .base
                .reader
                .raw
                .iter()
                .filter_map(|(entry, body)| {
                    let body = map(body).ok()?;
                    let from = body.get("from")?;
                    let names = match from {
                        V::Text(name) => vec![name.as_str()],
                        V::List(values) => {
                            values.iter().filter_map(|value| text(value).ok()).collect()
                        }
                        _ => vec![],
                    };
                    names.contains(&id.as_str()).then_some(entry.clone())
                })
                .collect::<Vec<_>>();
            let mut groups = BTreeMap::<String, usize>::new();
            for entry in &authored {
                let prefix = entry
                    .split_once('.')
                    .map_or(entry.as_str(), |(head, _)| head);
                *groups.entry(prefix.to_owned()).or_insert(0) += 1;
            }
            let written = groups
                .into_iter()
                .map(|(prefix, count)| format!("{prefix}. ({count})"))
                .collect::<Vec<_>>()
                .join(", ");
            let inside = authored
                .iter()
                .filter(|entry| picked.contains(*entry))
                .count();
            let hint = format!(
                "  hint: it wrote {written} - {inside} of {} inside 'Now'",
                authored.len()
            );
            out.push((id.clone(), py(asked), hint));
        }
        out.sort_by(|a, b| b.0.cmp(&a.0));
        Ok(out)
    }

    fn holders(&self, id: &str) -> Vec<(&String, &World<'_>)> {
        self.layers
            .iter()
            .filter(|(name, _)| self.held(name, id))
            .collect()
    }
    fn is_judgment(&self, id: &str) -> bool {
        self.base.judgments.contains_key(id)
            || self
                .holders(id)
                .iter()
                .any(|(_, w)| w.judgments.contains_key(id))
    }
    fn dispute(&self, id: &str) -> String {
        cut(
            &format!(
                "    CONTESTED: {}",
                self.disputed[id]
                    .iter()
                    .map(|(name, c)| format!("{name} says {}", short(c, 40)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            110,
        )
    }

    fn held_counts(&self) -> (usize, usize, BTreeMap<String, usize>, usize) {
        let judgments = self.base.judgments.len();
        let held = self
            .base
            .reader
            .ids
            .iter()
            .filter(|id| !F::BUILTINS.contains(&id.as_str()))
            .count();
        let mut prefixes = BTreeMap::new();
        for id in self.base.reader.ids.iter().filter(|id| {
            !F::BUILTINS.contains(&id.as_str()) && !self.base.judgments.contains_key(id)
        }) {
            let prefix = id.split_once('.').map_or(id.as_str(), |(head, _)| head);
            *prefixes.entry(prefix.to_owned()).or_insert(0) += 1;
        }
        let loose = prefixes.values().filter(|count| **count == 1).sum();
        (held, judgments, prefixes, loose)
    }

    pub fn opening_with_orientation(
        &self,
        budget: i64,
        brief: Option<&V>,
        prefix_order: &[String],
        orientation: &[String],
    ) -> Result<String> {
        crate::require(budget > 0, "--budget must be positive")?;
        let (held, judgments, prefixes, loose) = self.held_counts();
        let doc = map(&self.base.reader.document)?;
        let meta = doc.get("meta").and_then(|value| map(value).ok());
        let mut head = vec![];
        if let Some(scope) = meta
            .and_then(|meta| meta.get("scope").or_else(|| meta.get("about")))
            .filter(|value| truth(value))
        {
            head.push(cut(&py(scope), 300));
        }
        head.extend_from_slice(orientation);
        let mut heavy = prefixes
            .iter()
            .filter(|(_, count)| **count > 1)
            .map(|(prefix, count)| (prefix.clone(), *count))
            .collect::<Vec<_>>();
        heavy.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        if !heavy.is_empty() {
            let mut holds = heavy
                .into_iter()
                .map(|(prefix, count)| format!("{prefix} ({count})"))
                .collect::<Vec<_>>()
                .join(" · ");
            if loose > 0 {
                holds.push_str(&format!(" · and {loose} standalone"));
            }
            head.push(format!("holds: {holds}"));
        }
        if let Some(legend) = meta
            .and_then(|meta| meta.get("prefixes"))
            .and_then(|value| map(value).ok())
        {
            let ordered = prefix_order
                .iter()
                .chain(legend.keys().filter(|key| !prefix_order.contains(key)))
                .collect::<Vec<_>>();
            let entries = ordered
                .into_iter()
                .filter_map(|prefix| legend.get(prefix).map(|value| (prefix, value)))
                .filter(|(prefix, value)| {
                    truth(value)
                        && self.base.reader.ids.iter().any(|id| {
                            !F::BUILTINS.contains(&id.as_str())
                                && id.split_once('.').map_or(id.as_str(), |(head, _)| head)
                                    == prefix.as_str()
                        })
                })
                .map(|(prefix, value)| format!("{prefix}={}", py(value)))
                .collect::<Vec<_>>();
            if !entries.is_empty() {
                head.push(format!("prefixes: {}", entries.join(" · ")));
            }
        }
        let open = ["open", "questions"]
            .into_iter()
            .filter_map(|key| doc.get(key))
            .filter_map(|value| map(value).ok())
            .flat_map(|values| values.keys())
            .collect::<BTreeSet<_>>()
            .len();
        let mut summary = format!("{held} entries, {judgments} judgments");
        if open > 0 {
            summary.push_str(&format!(", {open} open questions"));
        }
        if let Some(updated) = meta
            .and_then(|meta| meta.get("updated"))
            .filter(|v| truth(v))
        {
            summary.push_str(&format!(", updated {}", py(updated)));
        }
        head.push(summary);
        head.extend(self.knowledge.clone());
        if let Some(waiting) = self.hypothesis_line()? {
            head.push(waiting);
        }

        let mut items = vec![];
        for (id, body) in &self.base.judgments {
            let flags = crate::ordinary_domain_counts::flags(&self.base.reader, body)?;
            if flags.contains("falsified") {
                items.push((
                    95,
                    id.clone(),
                    format!(
                        "wrong_if holds ({}) - broken by its own condition",
                        predicate_text(&self.base.pred(id))
                    ),
                ));
            }
            if flags.contains("broken") {
                let missing = self
                    .base
                    .deps(id)?
                    .into_iter()
                    .filter(|dep| !self.base.reader.ids.contains(dep))
                    .collect::<Vec<_>>();
                if !missing.is_empty() {
                    items.push((
                        100,
                        id.clone(),
                        format!("rests on {}, which is not an entry", missing.join(", ")),
                    ));
                }
            }
            if flags.contains("unchecked") {
                let b = map(body)?;
                let empty = Map::new();
                let seen = map(get(
                    b,
                    text(&self.base.reader.fields["snapshot"]).unwrap_or(""),
                ))
                .unwrap_or(&empty);
                for dep in self
                    .base
                    .deps(id)?
                    .into_iter()
                    .filter(|dep| self.base.reader.ids.contains(dep) && !seen.contains_key(dep))
                {
                    items.push((80, id.clone(), format!("never checked against {dep}")));
                }
            }
            for (dep, old, now, state) in self.base.moved(id)? {
                if state == "moved" {
                    let (old, now) = apart(&old, &now, 28);
                    items.push((
                        70,
                        id.clone(),
                        format!("{dep} differs from what it last saw: {old} -> {now}"),
                    ));
                }
            }
            if flags.contains("no_predicate") {
                items.push((40, id.clone(), "nothing evaluable would falsify it".into()));
            }
        }
        for (id, variants) in &self.disputed {
            items.push((
                110,
                id.clone(),
                format!(
                    "CONTESTED - {}",
                    variants
                        .iter()
                        .map(|(name, claim)| format!("{name} says {}", short(claim, 30)))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
        items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let flagged = items
            .iter()
            .map(|(_, id, _)| id.clone())
            .collect::<BTreeSet<_>>();
        let total = items.len();
        let kept = items.into_iter().take(budget as usize).collect::<Vec<_>>();
        let mut needs = if total == 0 {
            vec!["nothing needs a person right now.".into()]
        } else {
            vec![format!("needs a person ({total}):")]
        };
        let mut reasoned = BTreeSet::new();
        for (_, id, why) in kept {
            needs.push(format!("  {id}: {why}"));
            if reasoned.insert(id.clone())
                && let Some(because) = self
                    .base
                    .judgments
                    .get(&id)
                    .and_then(|v| map(v).ok())
                    .and_then(|b| b.get("because"))
                    .filter(|v| truth(v))
            {
                let line = format!("      because: {}", self.base.said(because, 400)?[0]);
                needs.push(if line.chars().count() < 100 {
                    line
                } else {
                    line.chars().take(100).collect::<String>() + " ..."
                });
            }
        }
        if total > budget as usize {
            needs.push(format!(
                "  ... {} more - raise the budget to see them",
                total - budget as usize
            ));
        }
        let mut questions = vec![];
        for key in ["open", "questions"] {
            if let Some(values) = doc.get(key).and_then(|value| map(value).ok()) {
                for (id, value) in values {
                    let line = format!("  ? {id}: {}", py(value));
                    questions.push(if line.chars().count() < 100 {
                        line
                    } else {
                        line.chars().take(100).collect::<String>() + " ..."
                    });
                }
            }
        }
        questions.sort();
        let mut standing = vec![];
        let live = self
            .base
            .judgments
            .keys()
            .filter(|id| !flagged.contains(*id))
            .collect::<Vec<_>>();
        if !live.is_empty() {
            standing.push(if flagged.is_empty() {
                "standing:".into()
            } else {
                format!(
                    "standing:  ({} above {} a person)",
                    flagged.len(),
                    if flagged.len() == 1 { "needs" } else { "need" }
                )
            });
            for id in live {
                let body = &self.base.judgments[id];
                let b = map(body)?;
                let verdict = b
                    .get("verdict")
                    .filter(|value| truth(value))
                    .or_else(|| b.get("title").filter(|value| truth(value)))
                    .map(py)
                    .unwrap_or_else(|| id.clone());
                let mut line = format!("  = {id}");
                if let Some(request) = b.get("request").filter(|value| truth(value)) {
                    line.push_str(&format!(" (on the word of {})", py(request)));
                }
                line.push_str(&format!(": {verdict}"));
                standing.push(if line.chars().count() < 80 {
                    line
                } else {
                    line.chars().take(80).collect::<String>() + " ..."
                });
            }
        }
        let unserved = self.page_unserved(brief)?;
        let footer = if let Some((id, _, _)) = unserved.first() {
            format!(
                "next: check - {id} is served by no tab{} · pull <entry|prefix> (values with sources) · affects <entry> (what a change reaches)",
                if unserved.len() > 1 {
                    format!(" (and {} more)", unserved.len() - 1)
                } else {
                    String::new()
                }
            )
        } else {
            "next: pull <entry|prefix> (values with sources) · affects <entry> (what a change reaches) · check".into()
        };
        let mut sections = vec![head.join("\n"), needs.join("\n")];
        if !questions.is_empty() {
            sections.push(questions.join("\n"));
        }
        if !standing.is_empty() {
            sections.push(standing.join("\n"));
        }
        sections.push(footer);
        Ok(sections.join("\n\n") + "\n")
    }

    pub fn check(&self, brief: Option<&V>) -> Result<(String, i32)> {
        let (held, judgments, _, _) = self.held_counts();
        let mut fail = vec![];
        let mut note = self.knowledge.clone();
        let mut moved = vec![];

        // Keep manual edits under the same structured-expression admission rules as
        // tool-authored writes. Computed entries use the same ordinary evaluator
        // as value reads; optional page evaluation remains separate.
        let predicate_field = text(&self.base.reader.fields["predicate"]).ok();
        let dependency_field = text(&self.base.reader.fields["deps"])?;
        for (id, body) in self
            .base
            .reader
            .raw
            .iter()
            .filter(|(id, _)| !F::BUILTINS.contains(&id.as_str()))
        {
            let Ok(fields) = map(body) else {
                continue;
            };
            let problems_before = fail.len();
            let mut expressions = vec![("rule", false)];
            if let Some(field) = predicate_field {
                expressions.push((field, true));
            }
            for (field, predicate) in expressions {
                let Some(value @ V::Map(_)) = fields.get(field) else {
                    continue;
                };
                if let Err(error) = L::legacy_expression_detailed(value, predicate) {
                    fail.push(format!("{id}: {field}: {}", error.0));
                    continue;
                }
                let references = L::legacy_references(value);
                let missing = references
                    .iter()
                    .filter(|reference| {
                        !self.base.reader.ids.contains(*reference)
                            && !F::BUILTINS.contains(&reference.as_str())
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if !missing.is_empty() && blocked_text(body).is_empty() {
                    fail.push(format!(
                        "{id}: {field}: unknown references: {}",
                        missing.join(", ")
                    ));
                }
                if predicate {
                    let dependencies = iterable(get(fields, dependency_field))?
                        .into_iter()
                        .collect::<BTreeSet<_>>();
                    let undeclared = references
                        .into_iter()
                        .filter(|reference| !dependencies.contains(reference))
                        .collect::<Vec<_>>();
                    if !undeclared.is_empty() {
                        fail.push(format!(
                            "{id}: predicate reads undeclared references: {}",
                            undeclared.join(", ")
                        ));
                    }
                } else if fields.contains_key("v") || fields.contains_key("quoted") {
                    fail.push(format!(
                        "{id}: a structured rule cannot also store v or quoted"
                    ));
                }
            }
            if fail.len() == problems_before && matches!(fields.get("rule"), Some(V::Map(_))) {
                let result = R::compute(
                    &self.base.reader.raw,
                    &self.base.reader.ids,
                    None,
                    self.base.reader.program(),
                );
                if result["values"][id]["value"].is_null() {
                    let reason = result["values"][id]["reason"]
                        .as_str()
                        .or_else(|| result["error"].as_str())
                        .unwrap_or("unavailable");
                    let finding = format!("{id}: rule cannot be computed: {reason}");
                    if blocked_text(body).is_empty() {
                        fail.push(finding);
                    } else {
                        note.push(finding);
                    }
                }
            }
        }

        for (id, body) in &self.base.judgments {
            let flags = crate::ordinary_domain_counts::flags(&self.base.reader, body)?;
            let pred = self.base.pred(id);
            let blocked = blocked_text(body);
            let reopened = reopened_text(body);
            let references = R::predicate_refs(&pred)
                .into_iter()
                .collect::<BTreeSet<_>>();
            let page_refs = references
                .iter()
                .filter(|reference| {
                    reference.starts_with("page.") && F::BUILTINS.contains(&reference.as_str())
                })
                .cloned()
                .collect::<Vec<_>>();
            let dependencies = self.base.deps(id)?;
            if flags.contains("broken") {
                for dep in dependencies
                    .iter()
                    .filter(|dep| !self.base.reader.ids.contains(*dep))
                {
                    fail.push(format!("{id}: rests on {dep}, which is not an entry"));
                }
            }
            if flags.contains("unchecked") {
                let b = map(body)?;
                let empty = Map::new();
                let seen = map(get(
                    b,
                    text(&self.base.reader.fields["snapshot"]).unwrap_or(""),
                ))
                .unwrap_or(&empty);
                for dep in dependencies
                    .iter()
                    .filter(|dep| self.base.reader.ids.contains(*dep) && !seen.contains_key(dep))
                {
                    fail.push(format!(
                        "{id}: no snapshot for {dep} - never checked against it"
                    ));
                }
            }
            let declared = dependencies.into_iter().collect::<BTreeSet<_>>();
            for reference in R::predicate_refs(&pred)
                .into_iter()
                .collect::<BTreeSet<_>>()
            {
                if self.base.reader.ids.contains(&reference) && !declared.contains(&reference) {
                    fail.push(format!(
                        "{id}: predicate reads {reference}, which it does not declare as a dependency - a change to it would never reach this"
                    ));
                }
            }
            if MISFILED_REOPENER
                .captures(&reopened)
                .is_some_and(|captures| self.base.reader.ids.contains(&captures[1]))
            {
                fail.push(format!(
                    "{id}: reopened_by reads as a comparison ({}) - a predicate belongs in wrong_if, where it is evaluated; a re-opener is the sign a person reads",
                    short(&s(&reopened), 60)
                ));
            }
            // Declared unevaluable predicates are check notes, independent of
            // the flags used by selectors and graph counters.
            let undecided = if truth(&pred) {
                R::why_undecided(&pred)
            } else {
                String::new()
            };
            let named = references
                .iter()
                .any(|reference| self.base.reader.ids.contains(reference));
            let evaluable = named && undecided.is_empty();
            let arrangement = crate::ordinary_semantics::arrangement(&self.base.reader, body);
            if !evaluable && !(arrangement && !undecided.is_empty()) {
                let what = if !truth(&pred) {
                    "no predicate at all".to_owned()
                } else if !named {
                    "prose, not an evaluable predicate".to_owned()
                } else {
                    format!("wrong_if {undecided}")
                };
                if !blocked.is_empty() {
                    note.push(format!(
                        "{id}: {what} (declared: {})",
                        blocked.chars().take(90).collect::<String>()
                    ));
                } else if !reopened.is_empty() && !truth(&pred) {
                    note.push(format!(
                        "{id}: {what} - decided; reopened by: {}",
                        reopened.chars().take(90).collect::<String>()
                    ));
                } else if !reopened.is_empty() {
                    fail.push(format!("{id}: {what} - a re-opener does not stand in for it: a predicate is evaluated, or declared un-evaluable with blocked_on"));
                } else {
                    fail.push(format!(
                        "{id}: {what} - and nothing says why not, so it can never be re-checked"
                    ));
                }
            } else if flags.contains("falsified") {
                fail.push(format!(
                    "{id}: wrong_if holds ({}) - broken by its own condition",
                    predicate_text(&pred)
                ));
            } else if !page_refs.is_empty() {
                note.push(format!(
                    "{id}: wrong_if reads {}, which is counted when the page is built - `kpop experimental hub --verify` decides it",
                    page_refs.join(", ")
                ));
            }
            for (dep, old, now, state) in self.base.moved(id)? {
                if state == "moved" {
                    let (old, now) = apart(&old, &now, 60);
                    moved.push(format!("{id}: {dep} differs from its snapshot ({old} -> {now}) - re-review, or refresh seen"));
                }
            }
            if let Some(day) = R::reversal_pending(body) {
                note.push(format!(
                    "{id}: reversed on {day} - the verdict under this id changed; review it once read, or pull {id} --history"
                ));
            }
        }
        let mut prior_judgments = 0usize;
        let mut high_priors = 0usize;
        for id in self.base.judgments.keys() {
            let priors = self
                .base
                .deps(id)?
                .into_iter()
                .filter(|dep| dep.starts_with("prior."))
                .collect::<Vec<_>>();
            if !priors.is_empty() {
                prior_judgments += 1;
                high_priors += priors
                    .iter()
                    .filter(|dep| {
                        py(&self.base.reader.value(dep).unwrap_or(V::Null))
                            .parse::<f64>()
                            .is_ok_and(|value| value >= 0.8)
                    })
                    .count();
            }
        }
        if prior_judgments > 0 {
            note.push(format!(
                "{prior_judgments} judgments rest on prior.* claims, {high_priors} of them on a prior at 0.8 or above"
            ));
        }
        for (id, asked, hint) in self.page_unserved(brief)? {
            note.push(format!("{id} is served by no tab - asked: {asked}"));
            note.push(hint);
        }
        fail.extend(self.unread_failures.clone());
        let mut lines = vec![];
        lines.extend(note.iter().map(|line| format!("NOTE {line}")));
        lines.extend(moved.iter().map(|line| format!("MOVED {line}")));
        for (id, variants) in &self.disputed {
            let claims = variants
                .iter()
                .map(|(name, value)| format!("{name} says {}", short(value, 40)))
                .collect::<Vec<_>>()
                .join(", ");
            let reason = if self.base.reader.knowledge_conflicts.contains(id) {
                "contribution versions need explicit reconciliation"
            } else {
                "one of them folds, or neither; a person decides"
            };
            lines.push(format!("CONTESTED {id}: {claims} - {reason}"));
        }
        lines.extend(fail.iter().map(|line| format!("FAIL {line}")));
        let mut summary = format!(
            "{judgments} judgments, {held} entries, {} problems",
            fail.len()
        );
        if !moved.is_empty() {
            summary.push_str(&format!(", {} moved", moved.len()));
        }
        if !self.disputed.is_empty() {
            summary.push_str(&format!(", {} contested", self.disputed.len()));
        }
        if !note.is_empty() {
            summary.push_str(&format!(", {} declared", note.len()));
        }
        lines.push(String::new());
        lines.push(summary);
        Ok((lines.join("\n") + "\n", i32::from(!fail.is_empty())))
    }

    pub fn history(&self, replaced: Option<&V>, path: &str, seeds: &[String]) -> Result<String> {
        let (names, _) = self.expanded(seeds)?;
        let empty = Map::new();
        let kept = replaced.and_then(|value| map(value).ok()).unwrap_or(&empty);
        let mut out = vec![];
        for id in names {
            let Some(versions) = kept.get(&id).and_then(|value| list(value).ok()) else {
                continue;
            };
            if versions.is_empty() {
                continue;
            }
            out.push(format!(
                "history of {id}: {} version{} kept in {path}",
                versions.len(),
                if versions.len() == 1 { "" } else { "s" }
            ));
            for (index, version) in versions.iter().enumerate() {
                let body = map(version)?;
                let mut head = format!(
                    "  {}. until {} - {}",
                    index + 1,
                    py(get(body, "day")),
                    py(get(body, "ended"))
                );
                if let Some(same) = body.get("same_as").filter(|value| truth(value)) {
                    head.push_str(&format!(" (the same decision as version {})", py(same)));
                    out.push(head);
                    continue;
                }
                out.push(head);
                for field in ["verdict", "because"] {
                    if let Some(value) = body.get(field).filter(|value| truth(value)) {
                        out.push(format!("     {field}: {}", short(value, 100)));
                    }
                }
                if let Some(deps) = body.get("rests_on").and_then(|value| list(value).ok()) {
                    out.push(format!(
                        "     rests_on: [{}]",
                        deps.iter().map(py).collect::<Vec<_>>().join(", ")
                    ));
                }
                if let Some(value) = body.get("wrong_if").filter(|value| truth(value)) {
                    out.push(format!("     wrong_if: {}", predicate_text(value)));
                }
                if let Some(value) = body.get("request").filter(|value| truth(value)) {
                    out.push(format!("     request: {}", py(value)));
                }
                if let Some(dropped) = body.get("dropped").and_then(|value| map(value).ok()) {
                    for (dep, why) in dropped {
                        out.push(format!("     no longer rested on {dep}: {}", py(why)));
                    }
                }
            }
        }
        Ok(if out.is_empty() {
            String::new()
        } else {
            out.join("\n") + "\n"
        })
    }

    pub fn pull(&self, seeds: &[String], budget: i64) -> Result<String> {
        let (expanded, _) = self.expanded(seeds)?;
        let every = self.every();
        let mut entries = BTreeSet::new();
        let mut judgments = BTreeSet::new();
        for id in expanded {
            if self.is_judgment(&id) {
                judgments.insert(id.clone());
                for world in
                    std::iter::once(&self.base).chain(self.holders(&id).iter().map(|(_, w)| *w))
                {
                    if world.judgments.contains_key(&id) {
                        entries.extend(
                            world
                                .deps(&id)?
                                .into_iter()
                                .filter(|d| every.contains(d) && !self.is_judgment(d)),
                        );
                    }
                }
            } else {
                entries.insert(id);
            }
        }
        for (name, world) in
            std::iter::once((None, &self.base)).chain(self.layers.iter().map(|(n, w)| (Some(n), w)))
        {
            for id in world.judgments.keys() {
                if name.is_none_or(|n| self.held(n, id))
                    && world.deps(id)?.iter().any(|d| entries.contains(d))
                {
                    judgments.insert(id.clone());
                }
            }
        }
        let mut lines = self.knowledge.clone();
        lines.extend(self.unread.clone());
        for id in entries {
            let holders = self.holders(&id);
            if self.base.reader.raw.contains_key(&id) {
                let (line, shown) = self.base.describe(&id)?;
                lines.push(cut(&line, 110));
                for (name, world) in &holders {
                    let (other, now) = world.describe(&id)?;
                    if now != shown {
                        lines.push(cut(
                            &format!("    proposes {shown} -> {now}, from {name}"),
                            110,
                        ));
                    } else if other != line {
                        let rest = other.strip_prefix(&format!("{id}: ")).unwrap_or(&other);
                        lines.push(cut(
                            &format!("    proposes instead, from {name}: {}", rest.trim()),
                            110,
                        ));
                    }
                }
            } else if let Some((_, world)) = holders.first() {
                let (line, _) = world.describe(&id)?;
                lines.push(cut(
                    &format!(
                        "{line} - held by {}",
                        holders
                            .iter()
                            .map(|(n, _)| n.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    110,
                ));
            }
            if self.disputed.contains_key(&id) {
                lines.push(self.dispute(&id));
            }
        }
        for id in judgments {
            let holders = self
                .holders(&id)
                .into_iter()
                .filter(|(_, w)| w.judgments.contains_key(&id))
                .collect::<Vec<_>>();
            if self.base.judgments.contains_key(&id) {
                lines.extend(self.base.judgment_lines(&id, "")?);
                let was = self.base.verdict(&id, &self.base.judgments[&id]);
                for (name, world) in &holders {
                    let now = world.verdict(&id, &world.judgments[&id]);
                    if !R::same_legacy(&s(&was), &s(&now)) {
                        lines.push(cut(
                            &format!("    proposes instead, from {name}: {now}"),
                            110,
                        ));
                    }
                }
            } else if let Some((name, world)) = holders.first() {
                lines.extend(world.judgment_lines(&id, &format!(" (in {})", self.label(name)))?);
            }
            if self.disputed.contains_key(&id) {
                lines.push(self.dispute(&id));
            }
        }
        let keep = if budget < 0 {
            (lines.len() as i64).saturating_add(budget).max(0) as usize
        } else {
            budget as usize
        };
        let remaining = (lines.len() as i128 - budget as i128).max(0);
        lines.truncate(keep);
        if remaining > 0 {
            lines.push(format!("... {remaining} more lines - raise the budget"));
        }
        lines.push("\naffects <entry> shows what a change reaches".into());
        Ok(lines.join("\n") + "\n")
    }
}

#[cfg(test)]
mod nonfinite_check_tests {
    use super::*;
    #[test]
    fn unavailable_rule_is_reported_without_rewriting_its_source() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = crate::history_authoring::tests::runtime(cache.path())
            .with_ordinary_program(crate::ordinary_reader::tests::program());
        for scalar in [".nan", ".inf", "-.inf"] {
            let source = format!(
                "known:\n  p.value: {{v: {scalar}}}\n  p.result: {{rule: {{expr: 'p.value + 1'}}}}\n"
            );
            let document =
                crate::history_yaml::decode_full_ordinary_source_value(source.as_bytes())
                    .unwrap()
                    .projected();
            let projection =
                Projection::new(&document, &Map::new(), &Map::new(), vec![], Some(&runtime))
                    .unwrap();
            let result = projection.check(None).unwrap();
            assert_eq!(result, ("FAIL p.result: rule cannot be computed: unavailable\n\n0 judgments, 2 entries, 1 problems\n".into(), 1));
            assert_eq!(
                document,
                crate::history_yaml::decode_full_ordinary_source_value(source.as_bytes())
                    .unwrap()
                    .projected()
            );
        }
    }
}

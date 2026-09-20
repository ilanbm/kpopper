// Shared candidate ordering and field semantics for finite and full ordinary readers.
static WORDS: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[\p{L}\p{N}]+").unwrap());
const STOP: &[&str] = &[
    "the", "and", "for", "its", "this", "that", "with", "from", "not", "are", "was", "were", "one",
    "what", "which", "when", "where", "how", "here", "into", "than", "then", "over", "under",
    "about", "after", "before", "does", "did", "has", "had",
];
fn field(body: &V, key: &str, retired: Option<&BTreeMap<String, String>>) -> Option<String> {
    let V::Text(value) = get(body, key) else {
        return None;
    };
    if value.trim().is_empty() {
        return None;
    }
    let value = norm(&s(value));
    Some(
        retired
            .and_then(|m| m.get(&value))
            .cloned()
            .unwrap_or(value),
    )
}
fn rule(body: &V) -> Option<V> {
    match get(body, "rule") {
        v @ V::Map(_) => Some(v.clone()),
        V::Text(s) if !s.trim().is_empty() => Some(V::Text(s.clone())),
        _ => match get(body, "v") {
            V::Text(s) if R::EXPR.is_match(s) && R::ID.is_match(s) => Some(V::Text(s.clone())),
            _ => None,
        },
    }
}
fn canon_rule(value: &V, retired: &BTreeMap<String, String>) -> V {
    let input = if matches!(value, V::Map(_)) {
        value.clone()
    } else {
        V::Map(Map::from([("expr".into(), value.clone())]))
    };
    if let Ok(mut tree) = L::legacy_rule(&input) {
        fn rewrite(value: &mut V, retired: &BTreeMap<String, String>) {
            if let V::Map(m) = value {
                if let Some(V::Text(id)) = m.get_mut("ref")
                    && let Some(new) = retired.get(id)
                {
                    *id = new.clone();
                }
                if let Some(V::List(a)) = m.get_mut("args") {
                    for child in a {
                        rewrite(child, retired);
                    }
                }
            }
        }
        rewrite(&mut tree, retired);
        tree
    } else {
        let rewritten = R::ID
            .replace_all(&py(value), |capture: &regex::Captures| {
                retired
                    .get(&capture[0])
                    .cloned()
                    .unwrap_or_else(|| capture[0].into())
            })
            .to_string();
        s(&norm(&s(&rewritten)))
    }
}
fn premises(body: &V, deps: &str, raw: &Map) -> BTreeSet<String> {
    match get(body, deps) {
        V::List(a) => a
            .iter()
            .filter_map(|v| text(v).ok())
            .filter(|id| !candidate_lookup(raw, id).is_some_and(|b| truth(get(b, "asked"))))
            .map(str::to_owned)
            .collect(),
        _ => BTreeSet::new(),
    }
}
fn tokens(value: &str) -> BTreeSet<String> {
    WORDS
        .find_iter(&value.to_lowercase())
        .map(|m| m.as_str().to_owned())
        .filter(|v| v.chars().count() > 2 && !STOP.contains(&v.as_str()))
        .collect()
}
fn overlap(left: &str, right: &str) -> f64 {
    let a = tokens(left);
    let b = tokens(right);
    if a.is_empty() || b.is_empty() {
        0.0
    } else {
        a.intersection(&b).count() as f64 / a.union(&b).count() as f64
    }
}
fn scalar_label(value: &str) -> String {
    static BARE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_.\-]*$").unwrap());
    if BARE.is_match(value)
        && crate::history_yaml::decode_ordinary_source_value(value.as_bytes()).is_ok_and(
            |v| matches!(v.projected(), crate::value::TypedValue::Text(ref v) if v == value),
        )
    {
        value.into()
    } else {
        format!(
            "\"{}\"",
            value
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
        )
    }
}
fn verdict(body: &V) -> V {
    let value = match get(body, "verdict") {
        V::Null => get(body, "title"),
        v => v,
    };
    if truth(value) { value.clone() } else { s("") }
}
#[derive(Clone, Debug)]
pub(crate) struct Candidate {
    pub id: String,
    pub rank: u8,
    pub reasons: Vec<String>,
    pub score: f64,
}
// Keep the captured-world inputs aligned with the ordinary candidate contract.
#[allow(clippy::too_many_arguments)]
pub(crate) fn near(
    subject: &str,
    body: &V,
    pool: &Map,
    deps: &str,
    raw: &Map,
    retired: &BTreeMap<String, String>,
    distinct: &BTreeSet<(String, String)>,
    limit: Option<usize>,
) -> Vec<Candidate> {
    let from = field(body, "from", Some(retired));
    let at = field(body, "at", None);
    let locations = ["url", "file"]
        .into_iter()
        .filter_map(|key| field(body, key, None).map(|v| (key, v)))
        .collect::<Vec<_>>();
    let rule = rule(body).map(|v| canon_rule(&v, retired));
    let prem = premises(body, deps, raw);
    let judgment = matches!(get(body, deps), V::List(_));
    let name = named(body);
    let mut out = vec![];
    for (id, other) in pool {
        if id == subject
            || !matches!(other, V::Map(_))
            || F::BUILTINS.contains(&id.as_str())
            || distinct.contains(&pair(subject, id))
        {
            continue;
        }
        let mut reasons = vec![];
        let mut rank = None;
        let other_from = field(other, "from", Some(retired));
        let other_at = field(other, "at", None);
        if let Some(from) = from.as_ref().filter(|_| from == other_from) {
            if let Some(at) = at.as_ref().filter(|_| at == other_at) {
                reasons.push(format!(
                    "same from and at ({}, {}) - certain",
                    from,
                    scalar_label(at)
                ));
                rank = Some(0);
            } else {
                reasons.push(format!("same from ({from})"));
                rank = Some(1);
            }
        }
        for (key, value) in &locations {
            if field(other, key, None).as_ref() == Some(value) {
                reasons.push(format!("same {key} ({}) - certain", short(&s(value), 60)));
                rank = Some(0);
            }
        }
        if let (Some(rule), Some(other_rule)) = (&rule, self::rule(other))
            && canon_rule(&other_rule, retired) == *rule
        {
            reasons.push(format!("same rule ({})", short(&other_rule, 60)));
            rank = Some(rank.unwrap_or(2).min(2));
        }
        let other_prem = premises(other, deps, raw);
        if judgment
            && !prem.is_empty()
            && !other_prem.is_empty()
            && (prem.is_subset(&other_prem) || other_prem.is_subset(&prem))
        {
            let shared = prem
                .intersection(&other_prem)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            let same = R::same_legacy(&verdict(body), &verdict(other));
            reasons.push(format!(
                "rests on {shared} too{}",
                if same {
                    ", with the same verdict - a duplicate"
                } else {
                    " - verdicts differ, a pair to judge"
                }
            ));
            rank = Some(rank.unwrap_or(3).min(3));
        }
        if !reasons.is_empty() {
            out.push(Candidate {
                id: id.clone(),
                rank: rank.unwrap(),
                reasons,
                score: overlap(&name, &named(other)),
            });
        }
    }
    out.sort_by(|a, b| {
        a.rank
            .cmp(&b.rank)
            .then_with(|| b.score.total_cmp(&a.score))
            .then_with(|| a.id.cmp(&b.id))
    });
    if let Some(limit) = limit {
        out.truncate(limit);
    }
    out
}
fn pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.into(), b.into())
    } else {
        (b.into(), a.into())
    }
}

fn candidate_lookup<'a>(raw: &'a Map, id: &str) -> Option<&'a V> {
    raw.get(id)
}

/// Immutable ordinary semantics needed by the optional Hub presentation.
///
/// This deliberately contains no layout choices: the Hub owns the brief and maps
/// these already-assessed facts onto tabs without reimplementing Reader semantics.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HubArrangement {
    pub id: String,
    pub sources: Vec<String>,
    pub born: Option<String>,
    pub request: Option<String>,
    pub predicate: String,
    pub fired: bool,
    pub reading: Option<String>,
    pub moved: Vec<(String, V, V, &'static str)>,
    pub contested: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HubData {
    pub arrangements: Vec<HubArrangement>,
    pub flags: BTreeMap<String, BTreeSet<String>>,
    pub reader_lines: Vec<String>,
}

impl Projection<'_> {
    pub(crate) fn hub_data(&self) -> Result<HubData> {
        let flags: BTreeMap<String, BTreeSet<String>> = self
            .base
            .judgments
            .iter()
            .map(|(id, body)| {
                Ok((
                    id.clone(),
                    hub_flags(&self.base.reader, body)?
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let mut arrangements = Vec::new();
        for (id, body) in &self.base.judgments {
            if !hub_arrangement(&self.base.reader, body) {
                continue;
            }
            let sources: Vec<String> = self
                .base
                .deps(id)?
                .into_iter()
                .filter(|source| {
                    self.base
                        .reader
                        .raw
                        .get(source)
                        .and_then(|body| map(body).ok())
                        .and_then(|body| body.get("asked"))
                        .is_some_and(truth)
                })
                .collect();
            let b = map(body)?;
            let request = b
                .get("request")
                .and_then(|value| text(value).ok())
                .filter(|request| {
                    sources.iter().any(|source| source == request)
                        && hub_lookup(&self.base.reader.raw, request).is_some_and(|body| {
                            map(body)
                                .ok()
                                .and_then(|body| body.get("asked"))
                                .is_some_and(truth)
                        })
                })
                .map(str::to_owned);
            let contested = self
                .disputed
                .get(id)
                .into_iter()
                .flatten()
                .map(|(name, claim)| (name.clone(), short(claim, 120)))
                .collect();
            let predicate = self.base.pred(id);
            let comparison_ref = if matches!(predicate, V::Map(_)) {
                L::legacy_expression(&predicate, true)
                    .ok()
                    .and_then(|tree| {
                        let args = map(&tree).ok()?.get("args")?;
                        let args = list(args).ok()?;
                        let left = map(args.first()?).ok()?;
                        if left.len() != 1 || map(args.get(1)?).ok()?.contains_key("op") {
                            return None;
                        }
                        text(left.get("ref")?).ok().map(str::to_owned)
                    })
            } else {
                R::CMP.captures(&py(&predicate)).map(|m| m[1].to_owned())
            };
            arrangements.push(HubArrangement {
                id: id.clone(),
                sources,
                born: b.get("born").filter(|v| truth(v)).map(py),
                request,
                predicate: predicate_text(&self.base.pred(id)),
                fired: flags[id].contains("falsified"),
                reading: comparison_ref.and_then(|dependency| {
                    let value = self.base.reader.value(&dependency).ok()?;
                    (value != V::Null).then(|| format!("{dependency} is {}", py(&value)))
                }),
                moved: self.base.moved(id)?,
                contested,
            });
        }
        Ok(HubData {
            arrangements,
            flags,
            reader_lines: self.knowledge.clone(),
        })
    }
}

fn hub_lookup<'a>(raw: &'a Map, id: &str) -> Option<&'a V> {
    raw.get(id)
}

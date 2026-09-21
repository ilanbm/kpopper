//! Exact scope and node grants over one detached Snapshot.
use crate::{
    Result,
    history_contract::*,
    history_view::{list, truth},
    reasoning_basis::InputBasis,
    reasoning_fields::snapshot_fields,
    reasoning_snapshot::{Snapshot, digest},
    require,
    value::{Integer, TypedValue as V},
};
use std::collections::{BTreeMap, BTreeSet};
fn s(value: &str) -> V {
    V::Text(value.into())
}
fn obj(fields: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn strings(values: impl IntoIterator<Item = String>) -> V {
    V::List(values.into_iter().map(V::Text).collect())
}
fn names(value: &V) -> Result<Vec<String>> {
    list(value)?
        .iter()
        .map(|v| text(v).map(str::to_owned))
        .collect()
}

pub struct ScopeCapture<'a> {
    snapshot: &'a Snapshot,
    data: V,
}
impl Snapshot {
    pub fn capture_scope(&self, id: &str, limits: Option<&V>) -> Result<ScopeCapture<'_>> {
        ScopeCapture::new(self, id, limits, false)
    }
    pub fn capture_query_scope(&self, id: &str, limits: Option<&V>) -> Result<ScopeCapture<'_>> {
        let data = map(self.data())?;
        let context = map(&data["context"])?;
        if let Some(history) = context.get("history").filter(|v| **v != V::Null) {
            let h = map(history)?;
            let c = map(&h["coverage"])?;
            let i = map(&h["integrity"])?;
            require(
                string_is(&c["scope"], "all")
                    && truth(&c["complete"])
                    && truth(&i["complete"])
                    && !truth(&i["findings"]),
                "incomplete_history_scope",
            )?;
        }
        let capture = ScopeCapture::new(self, id, limits, true)?;
        if let Some(history) = context.get("history").filter(|v| **v != V::Null) {
            let subjects = names(&map(&map(history)?["coverage"])?["subjects"])?;
            require(
                capture.members()?.iter().all(|id| subjects.contains(id)),
                "incomplete_history_scope",
            )?;
        }
        Ok(capture)
    }
}
impl<'a> ScopeCapture<'a> {
    fn new(snapshot: &'a Snapshot, id: &str, limits: Option<&V>, query: bool) -> Result<Self> {
        let data = map(snapshot.data())?;
        let nodes = map(&data["nodes"])?;
        let document = &data["document"];
        let context = map(&data["context"])?;
        let empty = V::Map(Map::new());
        let conflicts = map(context.get("conflicts").unwrap_or(&empty))?;
        let definition = nodes
            .get(id)
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get("body"))
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get("collection_scope"))
            .ok_or_else(|| error("invalid_scope"))?;
        let d = schema(definition, &["collection", "fields"], &[])
            .map_err(|_| error("invalid_scope"))?;
        let collection = text(&d["collection"]).map_err(|_| error("invalid_scope"))?;
        let fields = names(&d["fields"]).map_err(|_| error("invalid_scope"))?;
        require(fields.iter().all(|f| !f.is_empty()), "invalid_scope")?;
        require(!conflicts.get(id).is_some_and(truth), "scope_unavailable")?;
        require(
            fields.windows(2).all(|w| w[0] < w[1])
                && (!query
                    || fields
                        .iter()
                        .all(|f| !["rule", "computed"].contains(&f.as_str()))),
            "invalid_scope",
        )?;
        let source = map(document)?
            .get(collection)
            .and_then(|v| map(v).ok())
            .ok_or_else(|| error("scope_unavailable"))?;
        require(
            !["meta", "schema", "record", "also"].contains(&collection),
            "scope_unavailable",
        )?;
        let roles = snapshot_fields(document)?;
        let seen = text(&roles["snapshot"])?;
        require(
            fields
                .iter()
                .all(|f| ![seen, "assessment", "current_assessment"].contains(&f.as_str())),
            "invalid_scope",
        )?;
        let (mut members_bound, mut cells_bound) = (10_000usize, 100_000usize);
        if let Some(limits) = limits.filter(|v| **v != V::Null) {
            let m = schema(limits, &[], &["members", "field_cells"])
                .map_err(|_| error("invalid_limits"))?;
            for (key, bound) in [
                ("members", &mut members_bound),
                ("field_cells", &mut cells_bound),
            ] {
                if let Some(v) = m.get(key) {
                    let V::Integer(n) = v else {
                        return Err(error("invalid_limits"));
                    };
                    let n = n
                        .as_str()
                        .parse::<usize>()
                        .map_err(|_| error("invalid_limits"))?;
                    require(n > 0 && n <= *bound, "invalid_limits")?;
                    *bound = n;
                }
            }
        }
        require(
            source.len().saturating_mul(fields.len()) <= cells_bound,
            "limit: scope candidate field limit exceeded",
        )?;
        require(
            source.len() <= members_bound,
            "limit: collection member limit exceeded",
        )?;
        let mut candidates = Map::new();
        let mut dependencies = Vec::new();
        let mut input_basis = None;
        for member in source.keys() {
            let node = nodes
                .get(member)
                .and_then(|v| map(v).ok())
                .ok_or_else(|| error("scope_unavailable"))?;
            require(
                string_is(&node["collection"], collection),
                "scope_unavailable",
            )?;
            let body = map(&node["body"]).ok();
            let mut cells = Map::new();
            for field in &fields {
                let variants = conflicts.get(member).filter(|v| truth(v));
                let present = body.and_then(|m| m.get(field));
                let mut observation = Map::from([(
                    "status".into(),
                    s(if variants.is_some() {
                        "contested"
                    } else if present.is_some() {
                        "known"
                    } else {
                        "missing"
                    }),
                )]);
                if query
                    && variants.is_none()
                    && present.is_none()
                    && field == "v"
                    && body.is_some_and(|m| m.contains_key("rule"))
                {
                    observation.insert("status".into(), s("unavailable"));
                    observation.insert("reason".into(), s("formula_value"));
                }
                if let Some(value) = present {
                    observation.insert("value".into(), value.clone());
                }
                let mut projected = Vec::new();
                if let Some(variants) = variants {
                    let mut alternatives = Vec::new();
                    for variant in list(variants)? {
                        let pair = list(variant)?;
                        require(pair.len() == 2, "invalid_conflict_variant")?;
                        let value = map(&pair[1]).ok().and_then(|m| m.get(field));
                        alternatives.push(V::List(vec![
                            pair[0].clone(),
                            V::Map(
                                value
                                    .map(|v| Map::from([(field.clone(), v.clone())]))
                                    .unwrap_or_default(),
                            ),
                        ]));
                        let mut item = Map::from([
                            ("name".into(), pair[0].clone()),
                            ("present".into(), V::Bool(value.is_some())),
                        ]);
                        if let Some(v) = value {
                            item.insert("value".into(), v.clone());
                        }
                        projected.push(V::Map(item));
                    }
                    observation.insert("alternatives".into(), V::List(alternatives));
                }
                if !query
                    && ["v", "rule"].contains(&field.as_str())
                    && body.is_some_and(|m| m.contains_key("rule"))
                {
                    if input_basis.is_none() {
                        input_basis = Some(InputBasis::new(snapshot.data())?);
                    }
                    observation.insert(
                        "computed_basis".into(),
                        input_basis.as_mut().unwrap().summary(member)?,
                    );
                }
                let mut preimage = observation.clone();
                preimage.remove("alternatives");
                if variants.is_some() {
                    preimage.insert("alternatives".into(), V::List(projected));
                }
                let fingerprint = s(&digest(&V::Map(preimage))?);
                observation.insert("fingerprint".into(), fingerprint.clone());
                cells.insert(field.clone(), V::Map(observation));
                dependencies.push(obj([
                    ("id", s(member)),
                    ("field", s(field)),
                    ("fingerprint", fingerprint),
                ]));
            }
            candidates.insert(member.clone(), V::Map(cells));
        }
        let members = strings(source.keys().cloned());
        let dependencies = V::List(dependencies);
        let witness = obj([
            ("kind", s("scope")),
            ("scope_id", s(id)),
            ("definition_digest", s(&digest(definition)?)),
            ("membership_digest", s(&digest(&members)?)),
            ("projected_inputs_digest", s(&digest(&dependencies)?)),
        ]);
        let mut basis = Map::from([
            ("version".into(), V::Integer(Integer::new("1")?)),
            ("recipe".into(), s("scope-inputs/v2")),
            ("profile".into(), s("core/v1")),
            ("modules".into(), strings(["arithmetic/v1".into()])),
            ("witness".into(), witness.clone()),
            ("members".into(), members),
            ("fields".into(), d["fields"].clone()),
            ("dependencies".into(), dependencies),
            ("historical_detail".into(), s("fingerprints_only")),
            ("as_of".into(), data["as_of"].clone()),
        ]);
        basis.insert("digest".into(), s(&digest(&V::Map(basis.clone()))?));
        let value = obj([
            ("type", s("record")),
            (
                "fields",
                obj([(
                    "member_count",
                    obj([
                        ("type", s("number")),
                        ("numerator", s(&source.len().to_string())),
                        ("denominator", s("1")),
                    ]),
                )]),
            ),
        ]);
        Ok(Self {
            snapshot,
            data: obj([
                ("scope_id", s(id)),
                ("definition", definition.clone()),
                ("value", value),
                ("witness", witness),
                ("basis", V::Map(basis)),
                ("candidates", V::Map(candidates)),
            ]),
        })
    }
    pub fn to_data(&self) -> V {
        self.data.clone()
    }
    pub fn definition(&self) -> &V {
        &map(&self.data).unwrap()["definition"]
    }
    pub fn value(&self) -> &V {
        &map(&self.data).unwrap()["value"]
    }
    pub fn witness(&self) -> &V {
        &map(&self.data).unwrap()["witness"]
    }
    pub fn basis(&self) -> &V {
        &map(&self.data).unwrap()["basis"]
    }
    pub fn candidates(&self) -> &V {
        &map(&self.data).unwrap()["candidates"]
    }
    fn members(&self) -> Result<Vec<String>> {
        names(&map(self.basis())?["members"])
    }
    pub fn query_rows(&self) -> Result<V> {
        let fields = names(&map(self.definition())?["fields"])?;
        require(
            fields
                .iter()
                .all(|f| !["rule", "computed"].contains(&f.as_str())),
            "invalid_scope",
        )?;
        let mut rows = Vec::new();
        for (id, cells) in map(self.candidates())? {
            let mut projected = Map::new();
            for field in &fields {
                let cell = map(&map(cells)?[field])?;
                let status = text(&cell["status"])?;
                let value = match status {
                    "contested" => obj([("status", s("contested"))]),
                    "missing" => {
                        if field == "v" && cell.contains_key("computed_basis") {
                            obj([("status", s("unavailable")), ("reason", s("formula_value"))])
                        } else {
                            obj([("status", s("missing"))])
                        }
                    }
                    "unavailable" => obj([
                        ("status", s("unavailable")),
                        ("reason", cell["reason"].clone()),
                    ]),
                    _ => match query_value(cell.get("value").unwrap_or(&V::Null), 0) {
                        Ok(v) => obj([("status", s("known")), ("value", v)]),
                        Err(e) => obj([
                            ("status", s("unavailable")),
                            (
                                "reason",
                                s(if e.0 == "unsupported_type" {
                                    "unsupported_type"
                                } else {
                                    "invalid_value"
                                }),
                            ),
                        ]),
                    },
                };
                projected.insert(field.clone(), value);
            }
            rows.push(obj([("id", s(id)), ("fields", V::Map(projected))]));
        }
        Ok(V::List(rows))
    }
    fn read_field(&self, field: &str) -> Result<V> {
        require(
            names(&map(self.definition())?["fields"])?
                .iter()
                .any(|f| f == field),
            "undeclared_dependency",
        )?;
        Ok(V::List(
            map(self.candidates())?
                .iter()
                .map(|(id, cells)| {
                    let mut cell = map(&map(cells)?[field])?.clone();
                    cell.insert("id".into(), s(id));
                    Ok(V::Map(cell))
                })
                .collect::<Result<Vec<_>>>()?,
        ))
    }
    pub fn view(&self) -> Result<SnapshotView<'a>> {
        Ok(SnapshotView::new(
            self.snapshot,
            &[],
            &[text(&map(&self.data)?["scope_id"])?.into()],
            None,
        ))
    }
}
pub(crate) fn query_value(value: &V, depth: usize) -> Result<V> {
    use num_bigint::BigInt;
    require(depth <= 128, "invalid_value")?;
    if let V::Map(m) = value
        && m.contains_key("type")
    {
        crate::reasoning_values::validate(value)?;
        crate::reasoning_query::typed_value(&value.to_json()?)?;
        return Ok(value.clone());
    }
    let mut number = None;
    match value {
        V::Integer(n) => number = Some((n.as_str().parse::<BigInt>().unwrap(), BigInt::from(1))),
        V::Float(f) => {
            let text = crate::identity::python_float(f.get());
            let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
            let exponent = exponent.parse::<i32>().unwrap();
            let fraction = mantissa.split_once('.').map_or(0, |(_, s)| s.len() as i32);
            let digits = mantissa.replace('.', "");
            let mut n = digits
                .parse::<BigInt>()
                .map_err(|_| error("invalid_value"))?;
            let d = if fraction >= exponent {
                BigInt::from(10).pow((fraction - exponent) as u32)
            } else {
                n *= BigInt::from(10).pow((exponent - fraction) as u32);
                BigInt::from(1)
            };
            number = Some((n, d));
        }
        V::Map(m) if m.len() == 1 && m.contains_key("rational") => {
            if let Ok(pair) = list(&m["rational"])
                && pair.len() == 2
                && let (Ok(n), Ok(d)) = (text(&pair[0]), text(&pair[1]))
            {
                let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
                if n.len() <= 1024
                    && d.len() <= 1024
                    && digits(n.strip_prefix('-').unwrap_or(n))
                    && digits(d)
                {
                    let n = n.parse::<BigInt>().unwrap();
                    let d = d.parse::<BigInt>().unwrap();
                    if d != BigInt::from(0) {
                        number = Some((n, d));
                    }
                }
            }
        }
        _ => {}
    }
    if let Some((mut n, mut d)) = number {
        let (mut a, mut b) = (n.clone(), d.clone());
        while b != BigInt::from(0) {
            let r = &a % &b;
            a = b;
            b = r;
        }
        if a < BigInt::from(0) {
            a = -a;
        }
        n /= &a;
        d /= &a;
        return Ok(obj([
            ("type", s("number")),
            ("numerator", s(&n.to_string())),
            ("denominator", s(&d.to_string())),
        ]));
    }
    Ok(match value {
        V::Null => obj([("type", s("null"))]),
        V::Bool(v) => obj([("type", s("boolean")), ("value", V::Bool(*v))]),
        V::Text(v) => obj([("type", s("text")), ("value", s(v))]),
        V::List(items) => obj([
            ("type", s("list")),
            (
                "items",
                V::List(
                    items
                        .iter()
                        .map(|v| query_value(v, depth + 1))
                        .collect::<Result<Vec<_>>>()?,
                ),
            ),
        ]),
        V::Map(fields) => obj([
            ("type", s("record")),
            (
                "fields",
                V::Map(
                    fields
                        .iter()
                        .map(|(k, v)| Ok((k.clone(), query_value(v, depth + 1)?)))
                        .collect::<Result<Map>>()?,
                ),
            ),
        ]),
        _ => return Err(error("unsupported_type")),
    })
}
pub struct SnapshotView<'a> {
    snapshot: &'a Snapshot,
    nodes: BTreeSet<String>,
    scopes: BTreeSet<String>,
    reads: Map,
    limits: Option<V>,
    basis: Option<InputBasis>,
    captures: BTreeMap<String, ScopeCapture<'a>>,
}
impl<'a> SnapshotView<'a> {
    pub fn new(
        snapshot: &'a Snapshot,
        nodes: &[String],
        scopes: &[String],
        limits: Option<V>,
    ) -> Self {
        Self {
            snapshot,
            nodes: nodes.iter().cloned().collect(),
            scopes: scopes.iter().cloned().collect(),
            reads: Map::new(),
            limits,
            basis: None,
            captures: BTreeMap::new(),
        }
    }
    pub fn read_node(&mut self, id: &str) -> Result<V> {
        require(self.nodes.contains(id), "undeclared_dependency")?;
        if self.basis.is_none() {
            self.basis = Some(InputBasis::new(self.snapshot.data())?);
        }
        let basis = self.basis.as_mut().unwrap().summary(id)?;
        let witness = obj([
            ("kind", s("node")),
            ("id", s(id)),
            ("fingerprint", map(&basis)?["fingerprint"].clone()),
        ]);
        self.reads.insert(digest(&witness)?, witness);
        Ok(map(&map(self.snapshot.data())?["nodes"])?
            .get(id)
            .cloned()
            .unwrap_or(V::Null))
    }
    pub fn read_scope(&mut self, id: &str, field: &str) -> Result<V> {
        require(self.scopes.contains(id), "undeclared_dependency")?;
        if !self.captures.contains_key(id) {
            self.captures.insert(
                id.into(),
                self.snapshot.capture_scope(id, self.limits.as_ref())?,
            );
        }
        let capture = &self.captures[id];
        let values = capture.read_field(field)?;
        let witness = capture.witness().clone();
        self.reads.insert(digest(&witness)?, witness);
        Ok(values)
    }
    pub fn executed_reads(&self) -> V {
        V::List(self.reads.values().cloned().collect())
    }
}

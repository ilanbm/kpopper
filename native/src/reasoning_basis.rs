//! Canonical Merkle input identities. Cycles retain a joint identity and remain
//! unavailable; no edge is removed to invent an evaluation order.
use crate::{
    Result,
    history_contract::*,
    history_view::list,
    reasoning_fields, reasoning_language as L, reasoning_query as Q,
    reasoning_snapshot::digest,
    require,
    value::{Integer, TypedValue as V},
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
pub const RECIPE: &str = "merkle-inputs/v1";
fn s(value: &str) -> V {
    V::Text(value.into())
}
fn obj(values: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(values.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn strings(values: impl IntoIterator<Item = String>) -> V {
    V::List(values.into_iter().map(V::Text).collect())
}
fn arithmetic() -> BTreeSet<String> {
    BTreeSet::from(["arithmetic/v1".into()])
}

struct Normalized {
    atom: V,
    refs: BTreeSet<String>,
    problems: BTreeSet<String>,
    expression: serde_json::Value,
}
fn normalize(node: &V, nodes: &Map) -> Normalized {
    let normalized = (|| -> Result<_> {
        let n = map(node)?;
        let body = n.get("body").unwrap_or(&V::Null);
        let rule = map(body).ok().and_then(|m| m.get("rule"));
        if let Some(V::Map(rule)) = rule
            && rule.len() == 1
            && rule.contains_key("query")
        {
            let scope = map(&rule["query"])?
                .get("scope")
                .and_then(|v| text(v).ok())
                .ok_or_else(|| error("invalid_scope"))?;
            let fields = nodes
                .get(scope)
                .and_then(|n| map(n).ok())
                .and_then(|n| n.get("body"))
                .and_then(|b| map(b).ok())
                .and_then(|b| b.get("collection_scope"))
                .and_then(|d| map(d).ok())
                .and_then(|d| d.get("fields"))
                .ok_or_else(|| error("invalid_scope"))?;
            let fields = list(fields)?
                .iter()
                .map(|v| text(v).map(str::to_owned))
                .collect::<Result<Vec<_>>>()?;
            require(fields.windows(2).all(|w| w[0] < w[1]), "invalid_scope")?;
            let expression = Q::lower(&V::Map(rule.clone()).to_json()?, &fields)?;
            return Ok(Normalized {
                atom: obj([
                    ("status", s("present")),
                    ("expression", V::from_json(&expression)?),
                ]),
                refs: BTreeSet::new(),
                problems: BTreeSet::new(),
                expression,
            });
        }
        let expression = L::node_expression(node)?;
        let unavailable = expression
            .get("unavailable")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        Ok(Normalized {
            atom: obj([
                (
                    "status",
                    s(if unavailable.is_some() {
                        "unavailable"
                    } else {
                        "present"
                    }),
                ),
                ("expression", V::from_json(&expression)?),
            ]),
            refs: L::references(&expression).into_iter().collect(),
            problems: unavailable.into_iter().map(str::to_owned).collect(),
            expression,
        })
    })();
    normalized.unwrap_or_else(|_| {
        let body = map(node)
            .ok()
            .and_then(|m| m.get("body"))
            .unwrap_or(&V::Null);
        let raw = if let V::Map(body) = body {
            V::Map(
                body.iter()
                    .filter(|(k, _)| ["rule", "v", "quoted"].contains(&k.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            )
        } else {
            body.clone()
        };
        Normalized {
            atom: obj([("status", s("invalid")), ("input", raw)]),
            refs: BTreeSet::new(),
            problems: BTreeSet::from(["invalid_expression".into()]),
            expression: serde_json::json!({}),
        }
    })
}

pub struct InputBasis {
    identity: Map,
    modules: BTreeMap<String, BTreeSet<String>>,
    edges: BTreeMap<String, Vec<String>>,
    fingerprints: BTreeMap<String, String>,
    problems: BTreeMap<String, BTreeSet<String>>,
}
impl InputBasis {
    pub fn new(snapshot: &V) -> Result<Self> {
        let snapshot = map(snapshot)?;
        reasoning_fields::capabilities(field(snapshot, "document")?, Some("core/v1"))?;
        let nodes = map(field(snapshot, "nodes")?)?;
        require(nodes.len() <= 20_000, "node_limit")?;
        let empty = V::Map(Map::new());
        let context = map(snapshot.get("context").unwrap_or(&empty))?;
        let conflicts = map(context.get("conflicts").unwrap_or(&empty))
            .map_err(|_| error("invalid_conflicts"))?;
        let identity = Map::from([
            ("version".into(), V::Integer(Integer::new("1")?)),
            ("recipe".into(), s(RECIPE)),
            ("profile".into(), s("core/v1")),
            ("modules".into(), strings(arithmetic())),
            (
                "as_of".into(),
                snapshot.get("as_of").cloned().unwrap_or(V::Null),
            ),
        ]);
        let mut result = Self {
            identity,
            modules: BTreeMap::new(),
            edges: BTreeMap::new(),
            fingerprints: BTreeMap::new(),
            problems: BTreeMap::new(),
        };
        let mut atoms = Map::new();
        let mut edge_count = 0;
        for id in nodes
            .keys()
            .chain(conflicts.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
        {
            let mut n = nodes
                .get(&id)
                .map(|v| normalize(v, nodes))
                .unwrap_or_else(|| Normalized {
                    atom: obj([("status", s("missing"))]),
                    refs: BTreeSet::new(),
                    problems: BTreeSet::from(["missing_reference".into()]),
                    expression: serde_json::json!({}),
                });
            let mut modules = if n.expression.get("query").is_some() {
                Q::required_modules(&n.expression).into_iter().collect()
            } else {
                L::required_modules(&n.expression)
                    .into_iter()
                    .collect::<BTreeSet<_>>()
            };
            if let Some(variants) = conflicts.get(&id) {
                let mut normalized = Vec::new();
                for variant in list(variants).map_err(|_| error("invalid_conflict_variants"))? {
                    let pair = list(variant).map_err(|_| error("invalid_conflict_variant"))?;
                    require(
                        pair.len() == 2 && matches!(pair[0], V::Text(_)),
                        "invalid_conflict_variant",
                    )?;
                    let other = normalize(&obj([("body", pair[1].clone())]), nodes);
                    modules.extend(L::required_modules(&other.expression));
                    n.refs.extend(other.refs);
                    n.problems.extend(other.problems);
                    let variant = obj([("holder", pair[0].clone()), ("input", other.atom)]);
                    normalized.push((digest(&variant)?, variant));
                }
                normalized.sort_by(|a, b| a.0.cmp(&b.0));
                n.atom = obj([
                    ("status", s("contested")),
                    ("base", n.atom),
                    (
                        "variants",
                        V::List(normalized.into_iter().map(|(_, v)| v).collect()),
                    ),
                ]);
                n.problems.insert("contested".into());
            }
            edge_count += n.refs.len();
            require(edge_count <= 100_000, "edge_limit")?;
            result.modules.insert(id.clone(), modules);
            result
                .edges
                .insert(id.clone(), n.refs.into_iter().collect());
            result.problems.insert(id.clone(), n.problems);
            atoms.insert(id, n.atom);
        }
        let missing = result
            .edges
            .values()
            .flatten()
            .filter(|id| !result.edges.contains_key(*id))
            .cloned()
            .collect::<BTreeSet<_>>();
        for id in missing {
            result.modules.insert(id.clone(), arithmetic());
            result.edges.insert(id.clone(), Vec::new());
            result
                .problems
                .insert(id.clone(), BTreeSet::from(["missing_reference".into()]));
            atoms.insert(id, obj([("status", s("missing"))]));
        }
        result.index_components(&atoms)?;
        Ok(result)
    }
    fn index_components(&mut self, atoms: &Map) -> Result<()> {
        let mut order = Vec::new();
        let mut visited = BTreeSet::new();
        for root in self.edges.keys() {
            if !visited.insert(root.clone()) {
                continue;
            }
            let mut pending = vec![(root.clone(), 0)];
            while let Some((id, index)) = pending.last_mut() {
                if let Some(child) = self.edges[id].get(*index) {
                    *index += 1;
                    if visited.insert(child.clone()) {
                        pending.push((child.clone(), 0));
                    }
                } else {
                    let (id, _) = pending.pop().unwrap();
                    order.push(id);
                }
            }
        }
        let mut reverse = self
            .edges
            .keys()
            .map(|id| (id.clone(), Vec::new()))
            .collect::<BTreeMap<_, _>>();
        for (id, targets) in &self.edges {
            for target in targets {
                reverse.get_mut(target).unwrap().push(id.clone());
            }
        }
        let mut components: Vec<Vec<String>> = Vec::new();
        let mut owner = BTreeMap::new();
        for root in order.into_iter().rev() {
            if owner.contains_key(&root) {
                continue;
            }
            let number = components.len();
            owner.insert(root.clone(), number);
            let mut pending = vec![root];
            let mut members = Vec::new();
            while let Some(id) = pending.pop() {
                for parent in &reverse[&id] {
                    if !owner.contains_key(parent) {
                        owner.insert(parent.clone(), number);
                        pending.push(parent.clone());
                    }
                }
                members.push(id);
            }
            members.sort();
            components.push(members);
        }
        let mut dependencies = vec![BTreeSet::new(); components.len()];
        let mut dependants = dependencies.clone();
        for (id, targets) in &self.edges {
            for target in targets {
                let source = owner[id];
                let dest = owner[target];
                if source != dest {
                    dependencies[source].insert(dest);
                    dependants[dest].insert(source);
                }
            }
        }
        let mut remaining = dependencies.iter().map(BTreeSet::len).collect::<Vec<_>>();
        let mut ready = remaining
            .iter()
            .enumerate()
            .filter_map(|(n, c)| (*c == 0).then_some(n))
            .collect::<VecDeque<_>>();
        while let Some(number) = ready.pop_front() {
            let members = &components[number];
            let cyclic = members.len() > 1 || self.edges[&members[0]].contains(&members[0]);
            let mut problems = BTreeSet::new();
            let mut modules = BTreeSet::new();
            for id in members {
                modules.extend(self.modules[id].iter().cloned());
                problems.extend(self.problems[id].iter().cloned());
                for target in &self.edges[id] {
                    if owner[target] != number {
                        modules.extend(self.modules[target].iter().cloned());
                        problems.extend(self.problems[target].iter().cloned());
                    }
                }
            }
            let mut identity = self.identity.clone();
            identity.insert("modules".into(), strings(modules.iter().cloned()));
            if cyclic {
                problems.insert("cyclic_reference".into());
                let mut component = identity.clone();
                component.insert("kind".into(), s("cycle"));
                component.insert(
                    "members".into(),
                    V::List(
                        members
                            .iter()
                            .map(|id| {
                                obj([
                                    ("id", s(id)),
                                    ("input", atoms[id].clone()),
                                    ("references", strings(self.edges[id].iter().cloned())),
                                ])
                            })
                            .collect(),
                    ),
                );
                component.insert(
                    "external_dependencies".into(),
                    V::List(
                        members
                            .iter()
                            .flat_map(|id| {
                                self.edges[id]
                                    .iter()
                                    .filter(|target| owner[*target] != number)
                                    .map(|target| {
                                        obj([
                                            ("from", s(id)),
                                            ("id", s(target)),
                                            ("fingerprint", s(&self.fingerprints[target])),
                                        ])
                                    })
                            })
                            .collect(),
                    ),
                );
                let hash = digest(&V::Map(component))?;
                for id in members {
                    let mut preimage = identity.clone();
                    preimage.extend(Map::from([
                        ("kind".into(), s("cycle-member")),
                        ("id".into(), s(id)),
                        ("component".into(), s(&hash)),
                    ]));
                    self.fingerprints
                        .insert(id.clone(), digest(&V::Map(preimage))?);
                }
            } else {
                let id = &members[0];
                let mut preimage = identity;
                preimage.extend(Map::from([
                    ("kind".into(), s("node")),
                    ("id".into(), s(id)),
                    ("input".into(), atoms[id].clone()),
                    (
                        "dependencies".into(),
                        V::List(
                            self.edges[id]
                                .iter()
                                .map(|target| self.witness(target))
                                .collect(),
                        ),
                    ),
                ]));
                self.fingerprints
                    .insert(id.clone(), digest(&V::Map(preimage))?);
            }
            for id in members {
                self.modules.insert(id.clone(), modules.clone());
                self.problems.insert(id.clone(), problems.clone());
            }
            for dependant in &dependants[number] {
                remaining[*dependant] -= 1;
                if remaining[*dependant] == 0 {
                    ready.push_back(*dependant);
                }
            }
        }
        require(
            self.fingerprints.len() == self.edges.len(),
            "unresolved_computational_component",
        )
    }
    fn ensure(&mut self, id: &str) -> Result<()> {
        if !self.fingerprints.contains_key(id) {
            self.modules.insert(id.into(), arithmetic());
            self.edges.insert(id.into(), Vec::new());
            self.problems
                .insert(id.into(), BTreeSet::from(["missing_reference".into()]));
            let mut preimage = self.identity.clone();
            preimage.extend(Map::from([
                ("kind".into(), s("node")),
                ("id".into(), s(id)),
                ("input".into(), obj([("status", s("missing"))])),
                ("dependencies".into(), V::List(Vec::new())),
            ]));
            self.fingerprints
                .insert(id.into(), digest(&V::Map(preimage))?);
        }
        Ok(())
    }
    fn witness(&self, id: &str) -> V {
        obj([
            ("kind", s("node")),
            ("id", s(id)),
            ("fingerprint", s(&self.fingerprints[id])),
        ])
    }
    pub fn fingerprint(&mut self, id: &str) -> Result<String> {
        self.ensure(id)?;
        Ok(self.fingerprints[id].clone())
    }
    pub fn modules(&mut self, roots: &[String]) -> Result<Vec<String>> {
        let mut required = arithmetic();
        for id in roots {
            self.ensure(id)?;
            required.extend(self.modules[id].iter().cloned());
        }
        Ok(required.into_iter().collect())
    }
    pub fn dependencies(&mut self, roots: &[String]) -> Result<V> {
        for id in roots {
            self.ensure(id)?;
        }
        let mut pending = roots.to_vec();
        let mut seen = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if seen.insert(id.clone()) {
                pending.extend(self.edges[&id].iter().cloned());
            }
        }
        Ok(V::List(seen.iter().map(|id| self.witness(id)).collect()))
    }
    pub fn summary(&mut self, id: &str) -> Result<V> {
        self.ensure(id)?;
        Ok(obj([
            (
                "status",
                s(if self.problems[id].is_empty() {
                    "available"
                } else {
                    "unavailable"
                }),
            ),
            ("diagnostics", strings(self.problems[id].iter().cloned())),
            ("fingerprint", s(&self.fingerprints[id])),
        ]))
    }
    pub fn basis(&mut self, id: &str) -> Result<V> {
        self.ensure(id)?;
        let dependencies = self.dependencies(&[id.into()])?;
        let mut envelope = self.identity.clone();
        envelope.insert("modules".into(), strings(self.modules[id].iter().cloned()));
        envelope.insert("dependencies".into(), dependencies.clone());
        envelope.insert("digest".into(), s(&digest(&V::Map(envelope.clone()))?));
        let mut result = map(&self.summary(id)?)?.clone();
        result.insert("dependencies".into(), dependencies);
        result.insert("basis".into(), V::Map(envelope));
        Ok(V::Map(result))
    }
}

//! Lossless separation of node-local computation payloads from snapshot context.
//! Only recipes verified against the original identifier may replace an identifier.
//! This is a representation codec; it neither evaluates evidence nor authorizes reuse.
use crate::{Error, Result, require, value::TypedValue as V};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
type Map = BTreeMap<String, V>;
const FORMAT: &str = "node-evidence/v1";
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_RECIPES: usize = 4096;
const MAX_DECLARATIONS: usize = 1024;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(untagged)]
enum Step {
    Key(String),
    Index(usize),
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "declared", deny_unknown_fields)]
enum Kind {
    Snapshot,
    ScopeBasis,
    Basis,
    Expression(Vec<String>),
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Recipe {
    path: Vec<Step>,
    recipe: Kind,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    format: String,
    value: serde_json::Value,
    recipes: Vec<Recipe>,
}
fn bad() -> Error {
    Error("node_evidence_invalid".into())
}
fn field(m: &Map, key: &str) -> Result<V> {
    m.get(key).cloned().ok_or_else(bad)
}
fn preimage(m: &Map, kind: &Kind) -> Result<V> {
    let common = [
        ("snapshot_id".into(), field(m, "snapshot_id")?),
        ("resources".into(), field(m, "resource_profile")?),
    ];
    let mut out = Map::from(common);
    match kind {
        Kind::ScopeBasis => {
            out.insert("scope_basis".into(), field(m, "basis")?);
        }
        Kind::Basis => {
            out.insert("basis".into(), field(m, "basis")?);
        }
        Kind::Expression(declared) => {
            let V::Map(basis) = field(m, "basis")? else {
                return Err(bad());
            };
            out.extend([
                ("expression".into(), field(&basis, "expression")?),
                ("profile".into(), V::Text("core/v1".into())),
                ("modules".into(), field(m, "modules")?),
                (
                    "declared".into(),
                    V::List(declared.iter().cloned().map(V::Text).collect()),
                ),
                ("as_of".into(), field(&basis, "as_of")?),
            ]);
        }
        Kind::Snapshot => return Err(bad()),
    }
    Ok(V::Map(out))
}
fn strings(v: &V) -> Option<Vec<String>> {
    let V::List(v) = v else { return None };
    v.iter()
        .map(|v| {
            if let V::Text(v) = v {
                Some(v.clone())
            } else {
                None
            }
        })
        .collect()
}
fn normalize(
    value: &mut V,
    snapshot: &str,
    declarations: &[Vec<String>],
    path: &mut Vec<Step>,
    recipes: &mut Vec<Recipe>,
) -> Result<()> {
    require(
        path.len() <= crate::value::MAX_DEPTH && recipes.len() <= MAX_RECIPES,
        "node_evidence_limit",
    )?;
    match value {
        V::Map(m) => {
            if let Some(V::Text(original)) = m.get("computation_id") {
                let mut candidates = vec![Kind::ScopeBasis, Kind::Basis];
                let mut seen = BTreeSet::new();
                for mut declared in m
                    .get("potential_ids")
                    .and_then(strings)
                    .into_iter()
                    .chain(declarations.iter().cloned())
                {
                    declared.sort();
                    declared.dedup();
                    if seen.insert(declared.clone()) {
                        candidates.push(Kind::Expression(declared));
                    }
                }
                if let Some(recipe) = candidates.into_iter().find(|kind| {
                    preimage(m, kind).and_then(|v| v.digest()).ok().as_deref() == Some(original)
                }) {
                    recipes.push(Recipe {
                        path: path.clone(),
                        recipe,
                    });
                    m.insert("computation_id".into(), V::Null);
                }
            }
            if m.get("snapshot_id") == Some(&V::Text(snapshot.into())) {
                recipes.push(Recipe {
                    path: path.clone(),
                    recipe: Kind::Snapshot,
                });
                m.insert("snapshot_id".into(), V::Null);
            }
            for (key, value) in m.iter_mut() {
                path.push(Step::Key(key.clone()));
                normalize(value, snapshot, declarations, path, recipes)?;
                path.pop();
            }
        }
        V::List(values) => {
            for (i, value) in values.iter_mut().enumerate() {
                path.push(Step::Index(i));
                normalize(value, snapshot, declarations, path, recipes)?;
                path.pop();
            }
        }
        _ => (),
    }
    Ok(())
}
fn locate<'a>(mut value: &'a mut V, path: &[Step]) -> Result<&'a mut Map> {
    require(path.len() <= crate::value::MAX_DEPTH, "node_evidence_limit")?;
    for step in path {
        value = match (step, value) {
            (Step::Key(key), V::Map(m)) => m.get_mut(key).ok_or_else(bad)?,
            (Step::Index(i), V::List(v)) => v.get_mut(*i).ok_or_else(bad)?,
            _ => return Err(bad()),
        };
    }
    if let V::Map(m) = value {
        Ok(m)
    } else {
        Err(bad())
    }
}
impl Evidence {
    /// A typed embedding for node patches. Keeping the normalized payload typed lets
    /// the node codec patch maps instead of replacing an entire tagged transport list.
    pub fn to_value(&self) -> Result<V> {
        self.encode()?;
        Ok(V::Map(Map::from([
            ("format".into(), V::Text(self.format.clone())),
            ("value".into(), V::from_tagged(&self.value)?),
            (
                "recipes".into(),
                V::from_json(&serde_json::to_value(&self.recipes)?)?,
            ),
        ])))
    }
    pub fn from_value(value: &V) -> Result<Self> {
        let m = crate::history_contract::schema(value, &["format", "value", "recipes"], &[])?;
        require(
            m["format"] == V::Text(FORMAT.into()),
            "node_evidence_invalid",
        )?;
        let evidence = Self {
            format: FORMAT.into(),
            value: m["value"].to_tagged()?,
            recipes: serde_json::from_value(m["recipes"].to_json()?)?,
        };
        Self::decode(&evidence.encode()?)
    }
    pub fn pack(value: &V, snapshot: &str, declarations: &[Vec<String>]) -> Result<Self> {
        require(
            snapshot.len() == 64 && crate::history_paths::object_id(snapshot),
            "node_evidence_snapshot",
        )?;
        require(
            declarations.len() <= MAX_DECLARATIONS
                && declarations.iter().map(Vec::len).sum::<usize>() <= MAX_DECLARATIONS,
            "node_evidence_limit",
        )?;
        require(
            value.canonical_bytes()?.len() <= MAX_BYTES,
            "node_evidence_limit",
        )?;
        let mut normalized = value.clone();
        let mut recipes = Vec::new();
        normalize(
            &mut normalized,
            snapshot,
            declarations,
            &mut vec![],
            &mut recipes,
        )?;
        require(recipes.len() <= MAX_RECIPES, "node_evidence_limit")?;
        let evidence = Self {
            format: FORMAT.into(),
            value: normalized.to_tagged()?,
            recipes,
        };
        evidence.encode()?;
        require(
            evidence.restore(snapshot)? == *value,
            "node_evidence_roundtrip",
        )?;
        Ok(evidence)
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        let raw = serde_json::to_vec(self)?;
        require(raw.len() <= MAX_BYTES, "node_evidence_limit")?;
        Ok(raw)
    }
    pub fn decode(raw: &[u8]) -> Result<Self> {
        require(raw.len() <= MAX_BYTES, "node_evidence_limit")?;
        let evidence: Self = serde_json::from_slice(raw)?;
        require(
            evidence.format == FORMAT
                && evidence.recipes.len() <= MAX_RECIPES
                && evidence.encode()? == raw,
            "node_evidence_invalid",
        )?;
        V::from_tagged(&evidence.value)?;
        Ok(evidence)
    }
    pub fn restore(&self, snapshot: &str) -> Result<V> {
        require(
            snapshot.len() == 64 && crate::history_paths::object_id(snapshot),
            "node_evidence_snapshot",
        )?;
        require(
            self.format == FORMAT && self.recipes.len() <= MAX_RECIPES,
            "node_evidence_invalid",
        )?;
        let mut value = V::from_tagged(&self.value)?;
        let mut paths = BTreeSet::new();
        for item in self.recipes.iter().filter(|r| r.recipe == Kind::Snapshot) {
            require(
                paths.insert((item.path.clone(), "snapshot_id")),
                "node_evidence_duplicate",
            )?;
            let node = locate(&mut value, &item.path)?;
            require(
                node.get("snapshot_id") == Some(&V::Null),
                "node_evidence_target",
            )?;
            node.insert("snapshot_id".into(), V::Text(snapshot.into()));
        }
        for item in self.recipes.iter().filter(|r| r.recipe != Kind::Snapshot) {
            require(
                paths.insert((item.path.clone(), "computation_id")),
                "node_evidence_duplicate",
            )?;
            let node = locate(&mut value, &item.path)?;
            require(
                node.get("computation_id") == Some(&V::Null),
                "node_evidence_target",
            )?;
            if let Kind::Expression(ids) = &item.recipe {
                require(
                    ids.len() <= MAX_DECLARATIONS && ids.windows(2).all(|v| v[0] < v[1]),
                    "node_evidence_declarations",
                )?;
            }
            node.insert(
                "computation_id".into(),
                V::Text(preimage(node, &item.recipe)?.digest()?),
            );
        }
        require(
            value.canonical_bytes()?.len() <= MAX_BYTES,
            "node_evidence_limit",
        )?;
        Ok(value)
    }
}

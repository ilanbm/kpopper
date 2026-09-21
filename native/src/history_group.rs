//! Exact, bounded group envelopes. Decoding never selects projects or grants access.
use crate::{
    Result,
    history_contract::*,
    history_transaction::{self as T, PreparedMutation},
    require,
    value::{Integer, TypedValue as V},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};
const KIND: &str = "history-authority-group/v1";
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn n() -> V {
    V::Integer(Integer::new("1").unwrap())
}
fn obj(fields: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn resolved(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut path = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                path.pop();
            }
            part => path.push(part.as_os_str()),
        }
        if path.exists() {
            path = path.canonicalize()?;
        }
    }
    Ok(path)
}
#[derive(Clone, Debug)]
pub struct GroupPrepared {
    data: V,
    mutations: BTreeMap<String, PreparedMutation>,
}
impl GroupPrepared {
    pub fn prepare(
        operation: &str,
        entries: &[String],
        mutations: &BTreeMap<String, PreparedMutation>,
        inventory: &V,
        expected_digests: &V,
    ) -> Result<Self> {
        token(&s(operation))?;
        require((2..=32).contains(&entries.len()), "invalid_group_entries")?;
        let mut entries = entries
            .iter()
            .map(|p| {
                resolved(Path::new(p)).and_then(|p| {
                    p.to_str()
                        .map(str::to_owned)
                        .ok_or_else(|| error("invalid_group_entries"))
                })
            })
            .collect::<Result<Vec<_>>>()?;
        entries.sort();
        require(
            entries.windows(2).all(|w| w[0] != w[1]),
            "duplicate_group_entry",
        )?;
        require(
            entries
                .iter()
                .map(|p| Path::new(p).parent())
                .collect::<BTreeSet<_>>()
                .len()
                == entries.len(),
            "overlapping_group_entries",
        )?;
        require(
            entries.iter().cloned().collect::<BTreeSet<_>>() == mutations.keys().cloned().collect(),
            "group_membership_mismatch",
        )?;
        let encoded_entries = V::List(entries.iter().map(|p| s(p)).collect());
        let group = obj([
            ("version", n()),
            ("operation", s(operation)),
            ("entries", encoded_entries.clone()),
        ]);
        let mut members = Vec::new();
        let mut directions = BTreeSet::new();
        for entry in &entries {
            let mutation = &mutations[entry];
            let data = mutation.to_data();
            let d = map(&data)?;
            let baseline = map(&d["baseline"])?;
            require(
                is_int(&d["version"], "2")
                    && baseline
                        .get("kind")
                        .is_some_and(|v| string_is(v, "history-authority-transition/v1"))
                    && baseline
                        .get("group")
                        .is_some_and(|v| crate::source_clock::python_equal(v, &group))
                    && string_is(
                        &d["entry"],
                        Path::new(entry).file_name().unwrap().to_str().unwrap(),
                    )
                    && baseline.get("transaction_root").is_some_and(|v| {
                        string_is(v, Path::new(entry).parent().unwrap().to_str().unwrap())
                    }),
                "group_member_mismatch",
            )?;
            let deployment = map(field(baseline, "deployment")?)?;
            require(
                field(deployment, "inventory")?.digest()? == inventory.digest()?
                    && field(deployment, "expected_digests")?.digest()?
                        == expected_digests.digest()?,
                "group_deployment_mismatch",
            )?;
            directions.insert(text(field(baseline, "direction")?)?.to_owned());
            members.push(obj([
                ("entry", s(entry)),
                ("mutation", T::blob(Some(&mutation.to_bytes()?))),
            ]));
        }
        require(
            directions.len() == 1
                && directions
                    .iter()
                    .all(|d| ["activate", "deactivate"].contains(&d.as_str())),
            "group_direction_mismatch",
        )?;
        let mut data = obj([
            ("version", n()),
            ("kind", s(KIND)),
            ("operation", s(operation)),
            ("entries", encoded_entries),
            ("direction", s(directions.first().unwrap())),
            ("members", V::List(members)),
            ("inventory", inventory.clone()),
            ("expected_digests", expected_digests.clone()),
        ]);
        let digest = data.digest()?;
        crate::history_view::map_mut(&mut data)?.insert("digest".into(), s(&digest));
        crate::history_yaml::validate_value(&data, T::MAX_TRANSACTION_BYTES)?;
        let result = Self {
            data,
            mutations: mutations.clone(),
        };
        result.to_bytes()?;
        Ok(result)
    }
    pub fn to_data(&self) -> V {
        self.data.clone()
    }
    pub fn mutations(&self) -> &BTreeMap<String, PreparedMutation> {
        &self.mutations
    }
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let raw = self.data.canonical_bytes()?;
        require(raw.len() <= T::MAX_TRANSACTION_BYTES, "history_limit")?;
        Ok(raw)
    }
    pub fn from_bytes(raw: &[u8]) -> Result<Self> {
        let data = T::parse_journal(raw, "invalid_group_journal")?;
        let d = schema(
            &data,
            &[
                "version",
                "kind",
                "operation",
                "entries",
                "direction",
                "members",
                "inventory",
                "expected_digests",
                "digest",
            ],
            &[],
        )?;
        require(
            is_int(&d["version"], "1") && string_is(&d["kind"], KIND),
            "invalid_group_journal",
        )?;
        let members = crate::history_view::list(&d["members"])
            .map_err(|_| error("group_membership_mismatch"))?;
        let observed = members
            .iter()
            .map(|v| {
                map(v)
                    .and_then(|m| field(m, "entry"))
                    .cloned()
                    .map_err(|_| error("invalid_group_journal"))
            })
            .collect::<Result<Vec<_>>>()?;
        require(
            crate::source_clock::python_equal(&V::List(observed), &d["entries"]),
            "group_membership_mismatch",
        )?;
        let entries = crate::history_view::list(&d["entries"])?
            .iter()
            .map(|v| {
                text(v)
                    .map(str::to_owned)
                    .map_err(|_| error("invalid_group_journal"))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut mutations = BTreeMap::new();
        for item in members {
            let item = schema(item, &["entry", "mutation"], &[])?;
            let bytes = T::unblob(&item["mutation"])?.ok_or_else(|| error("history_limit"))?;
            mutations.insert(
                text(&item["entry"])?.to_owned(),
                PreparedMutation::from_bytes(&bytes)?,
            );
        }
        let result = Self::prepare(
            text(&d["operation"])?,
            &entries,
            &mutations,
            &d["inventory"],
            &d["expected_digests"],
        )?;
        require(result.to_bytes()? == raw, "invalid_group_journal")?;
        Ok(result)
    }
}

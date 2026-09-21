//! Deterministic generation reservation cancellation, without publication authority.
use crate::{
    Result, history_authority as A,
    history_contract::*,
    history_emit as E,
    history_transaction::{Layout, PreparedMutation},
    history_yaml as Y,
    identity::sha256,
    require,
    value::{Integer, TypedValue as V},
};
use num_bigint::BigInt;
use std::path::Path;
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn n(v: &str) -> V {
    V::Integer(Integer::new(v).expect("validated generation"))
}
fn obj(fields: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancellationPlan {
    pub operation: String,
    pub mutation_digest: String,
    pub receipt_path: String,
    pub receipt_bytes: Vec<u8>,
    pub marker_bytes: Vec<u8>,
    pub original_entry: Vec<u8>,
    pub immutable_images: Vec<(String, Vec<u8>)>,
    pub digest: String,
}
impl CancellationPlan {
    /// Portable representation for inspecting exact retained images.
    pub fn evidence(&self) -> V {
        let hex = |raw: &[u8]| s(&raw.iter().map(|b| format!("{b:02x}")).collect::<String>());
        obj([
            ("version", n("1")),
            ("operation", s(&self.operation)),
            ("mutation_digest", s(&self.mutation_digest)),
            ("receipt_path", s(&self.receipt_path)),
            ("receipt_bytes", hex(&self.receipt_bytes)),
            ("marker_bytes", hex(&self.marker_bytes)),
            ("original_entry", hex(&self.original_entry)),
            (
                "immutable_images",
                V::List(
                    self.immutable_images
                        .iter()
                        .map(|(path, raw)| obj([("path", s(path)), ("after", hex(raw))]))
                        .collect(),
                ),
            ),
            ("digest", s(&self.digest)),
        ])
    }
}
/// Cancelling an activation reserves both its unused history generation and the
/// following legacy generation. Retained immutable evidence is never rolled back.
/// The returned plan alone grants no right to publish or clear any journal.
pub fn cancellation_plan(entry: &Path, mutation: &PreparedMutation) -> Result<CancellationPlan> {
    let entry = entry
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| error("invalid_generation_cancellation"))?;
    let data = mutation.to_data();
    let d = map(&data)?;
    let baseline = map(&d["baseline"])?;
    require(
        string_is(&d["entry"], entry)
            && baseline
                .get("kind")
                .is_some_and(|v| string_is(v, "history-authority-transition/v1"))
            && baseline
                .get("direction")
                .is_some_and(|v| string_is(v, "activate")),
        "invalid_generation_cancellation",
    )?;
    let reserved = map(d
        .get("transition")
        .ok_or_else(|| error("invalid_generation_cancellation"))?)?
    .get("after")
    .ok_or_else(|| error("invalid_generation_cancellation"))?;
    let before = &d["authority"];
    let r = map(reserved)?;
    let b = map(before)?;
    let generation = |v: &V| -> Result<BigInt> {
        if let V::Integer(v) = v {
            v.as_str()
                .parse()
                .map_err(|_| error("invalid_generation_cancellation"))
        } else {
            Err(error("invalid_generation_cancellation"))
        }
    };
    let reserved_generation = generation(&r["generation"])?;
    require(
        string_is(&r["authority"], "history")
            && string_is(&b["authority"], "legacy")
            && reserved_generation == generation(&b["generation"])? + 1,
        "invalid_generation_cancellation",
    )?;
    let record = mutation
        .files()
        .iter()
        .find(|i| i.role == "record")
        .ok_or_else(|| error("invalid_generation_cancellation"))?;
    let commit = mutation
        .files()
        .iter()
        .find(|i| i.role == "history_commit")
        .ok_or_else(|| error("invalid_generation_cancellation"))?;
    let original = record
        .before
        .as_ref()
        .ok_or_else(|| error("invalid_bytes"))?;
    let commit_bytes = commit
        .after
        .as_ref()
        .ok_or_else(|| error("invalid_bytes"))?;
    let manifest = Y::decode_document(commit_bytes)?;
    A::validate_commit(&manifest)?;
    let immutable = mutation
        .files()
        .iter()
        .filter(|i| {
            ["history_object", "history_commit", "history_retained"].contains(&i.role.as_str())
        })
        .collect::<Vec<_>>();
    let mut objects = Map::new();
    for item in crate::history_view::list(&map(&manifest)?["objects"])? {
        let item = map(item)?;
        objects.insert(
            text(&item["id"])?.into(),
            obj([
                ("subject", item["subject"].clone()),
                ("sha256", item["sha256"].clone()),
            ]),
        );
    }
    let artifacts = immutable
        .iter()
        .filter(|i| i.role == "history_retained")
        .map(|i| {
            Ok((
                i.path.clone(),
                s(&sha256(
                    i.after.as_ref().ok_or_else(|| error("invalid_bytes"))?,
                )),
            ))
        })
        .collect::<Result<Map>>()?;
    let legacy = n(&(reserved_generation.clone() + BigInt::from(1)).to_string());
    let receipt = obj([
        ("version", n("1")),
        ("kind", s(A::CANCELLATION)),
        ("record_id", r["record_id"].clone()),
        ("operation", d["operation"].clone()),
        ("reserved_generation", r["generation"].clone()),
        ("legacy_generation", legacy.clone()),
        ("before_authority_digest", s(&before.digest()?)),
        ("reserved_authority_digest", s(&reserved.digest()?)),
        ("mutation_digest", d["digest"].clone()),
        ("entry", s(entry)),
        ("original_entry_sha256", s(&sha256(original))),
        ("manifest_sha256", s(&sha256(commit_bytes))),
        ("objects", V::Map(objects)),
        ("artifacts", V::Map(artifacts)),
        ("visibility", s("never_released_to_readers")),
    ]);
    A::validate_cancellation(&receipt)?;
    let receipt_bytes = E::encode_document(&receipt)?;
    let operation = text(&d["operation"])?.to_owned();
    let filename = format!("{operation}.yaml");
    let mut cancellations = b
        .get("cancellations")
        .map(map)
        .transpose()?
        .cloned()
        .unwrap_or_default();
    let key = reserved_generation.to_string();
    require(
        !cancellations.contains_key(&key),
        "cancellation_generation_collision",
    )?;
    cancellations.insert(
        key,
        obj([
            ("operation", s(&operation)),
            ("path", s(&filename)),
            ("sha256", s(&sha256(&receipt_bytes))),
            ("reserved_generation", r["generation"].clone()),
            ("legacy_generation", legacy.clone()),
        ]),
    );
    let marker = A::authority(text(&r["record_id"])?, "legacy", &legacy, cancellations)?;
    let marker_bytes = E::encode_document(&marker)?;
    let digest = obj([
        ("mutation", d["digest"].clone()),
        ("receipt", s(&sha256(&receipt_bytes))),
        ("marker", s(&sha256(&marker_bytes))),
        ("entry", s(&sha256(original))),
    ])
    .digest()?;
    Ok(CancellationPlan {
        operation,
        mutation_digest: text(&d["digest"])?.into(),
        receipt_path: format!("{}/{}", Layout::for_entry(entry)?.cancellations, filename),
        receipt_bytes,
        marker_bytes,
        original_entry: original.clone(),
        immutable_images: immutable
            .into_iter()
            .map(|i| {
                Ok((
                    i.path.clone(),
                    i.after.clone().ok_or_else(|| error("invalid_bytes"))?,
                ))
            })
            .collect::<Result<_>>()?,
        digest,
    })
}

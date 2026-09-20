//! One recoverable ordinary-record transaction assembled from canonical writes.
//!
//! Each step reads a bounded in-memory view, while the retained inventory keeps
//! the original filesystem observations used by the publication verifier.
use crate::{
    Result,
    history_contract::{error, field, map, text},
    history_transaction::{self as T, FileImage, PreparedMutation},
    legacy_authoring::{self as A, Preparation, Prepared},
    project_modes::WriteRoute,
    source_inventory::Inventory,
    value::TypedValue as V,
};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug)]
pub(crate) struct Options {
    pub operation: String,
    pub context: V,
}

fn with_batch_context(value: &V, context: &V, actions: &[V], steps: &[V]) -> Result<V> {
    let mut value = map(value)?.clone();
    value.insert(
        "batch".into(),
        V::Map(BTreeMap::from([
            ("version".into(), V::Integer(crate::value::Integer::new("1")?)),
            ("context".into(), context.clone()),
            ("actions".into(), V::List(actions.to_vec())),
            ("steps".into(), V::List(steps.to_vec())),
        ])),
    );
    Ok(V::Map(value))
}

/// Prepare all actions against one captured preimage.
pub(crate) fn prepare(
    actions: &[V],
    route: &WriteRoute,
    options: &Options,
) -> Result<Prepared> {
    crate::require(!actions.is_empty() && actions.len() <= 33, "invalid_batch")?;
    let mut inventory = Inventory::default();
    let mut images = BTreeMap::<String, FileImage>::new();
    let mut outputs = Vec::new();
    let mut receipts = Vec::new();
    let mut first_data = None::<V>;
    let mut root = None;
    let mut journal = None;
    let mut subjects = Vec::new();

    for (index, action) in actions.iter().enumerate() {
        let prepared = match A::prepare_with_inventory(action, route, None, inventory)
            .map_err(|e| error(&format!("batch action {index}: {e}")))? {
            Preparation::Draft(_) => {
                return Err(error("report operation was not applied"));
            }
            Preparation::Mutation(prepared) => prepared,
        };
        let data = prepared.mutation.to_data();
        if first_data.is_none() {
            first_data = Some(data.clone());
            root = Some(prepared.root.clone());
            journal = Some(prepared.journal.clone());
        }
        let batch_root = root.as_ref().ok_or_else(|| error("invalid_batch"))?;
        crate::require(prepared.root == *batch_root, "batch_transaction_root_changed")?;
        receipts.push(field(map(&data)?, "receipt")?.clone());
        outputs.push(prepared.output.trim_end().to_owned());
        subjects.push(prepared.subject);
        inventory = prepared.inventory;
        for image in prepared.mutation.files() {
            let path = image.path.clone();
            let target = batch_root.join(Path::new(&path));
            inventory.stage(&target, image.after.clone())?;
            images
                .entry(path.clone())
                .and_modify(|combined| {
                    combined.after = image.after.clone();
                    combined.role = image.role.clone();
                })
                .or_insert_with(|| image.clone());
        }
    }

    let first = first_data.ok_or_else(|| error("invalid_batch"))?;
    let first = map(&first)?;
    let first_receipt = map(receipts.first().ok_or_else(|| error("invalid_batch"))?)?;
    let last_receipt = map(receipts.last().ok_or_else(|| error("invalid_batch"))?)?;
    let step_digests = receipts
        .iter()
        .map(|receipt| Ok(V::Text(receipt.digest()?)))
        .collect::<Result<Vec<_>>>()?;
    let before = with_batch_context(
        field(first_receipt, "before")?,
        &options.context,
        actions,
        &step_digests,
    )?;
    let after = with_batch_context(
        field(last_receipt, "after")?,
        &options.context,
        actions,
        &step_digests,
    )?;
    let capabilities = V::Map(BTreeMap::from([
        (
            "before".into(),
            field(first_receipt, "capabilities")?.clone(),
        ),
        (
            "after".into(),
            field(last_receipt, "capabilities")?.clone(),
        ),
    ]));
    let receipt = T::semantic_receipt(
        text(field(last_receipt, "profile")?)?,
        &capabilities,
        &before,
        &after,
    )?;
    let mutation = PreparedMutation::prepare(
        &options.operation,
        field(first, "authority")?,
        field(first, "baseline")?,
        images.into_values().collect(),
        &receipt,
        text(field(first, "entry")?)?,
        None,
    )?;
    Ok(Prepared {
        inventory,
        mutation,
        output: format!("{}\n", outputs.join("\n")),
        root: root.unwrap(),
        journal: journal.unwrap(),
        subject: subjects.join(","),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_transaction_fs as F;
    use std::fs;

    fn prepared() -> (tempfile::TempDir, std::path::PathBuf, WriteRoute, Prepared) {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("GROUNDING.yaml");
        fs::write(&entry, include_str!("../tests/fixtures/legacy-review/supersede-before.yaml")).unwrap();
        let route = WriteRoute::capture(std::slice::from_ref(&entry), temp.path()).unwrap();
        let actions = [
            V::from_json(&serde_json::json!({"kind":"set","id":"p.alpha","value":3,"as_of":"2026-09-20"})).unwrap(),
            V::from_json(&serde_json::json!({"kind":"add","id":"d.keep","body":{"verdict":"stop","rests_on":["p.alpha","p.beta"],"wrong_if":"p.alpha > 9","because":"new evidence"},"as_of":"2026-09-20"})).unwrap(),
        ];
        let prepared = prepare(&actions, &route, &Options {
            operation: "report-recovery".into(), context: V::Map(BTreeMap::new()),
        }).unwrap();
        assert_eq!(prepared.mutation.files().len(), 2);
        (temp, entry, route, prepared)
    }

    #[test]
    fn partial_batch_recovers_forward_and_back_from_the_same_preimage() {
        for before in [false, true] {
            let (temp, entry, route, prepared) = prepared();
            let images = prepared.mutation.files().to_vec();
            let mut verify = |_: &V| { route.verify()?; prepared.inventory.verify() };
            let mut stop = |_: &V| Err(error("retained_after_write"));
            assert_eq!(F::publish_legacy(&prepared.root, &prepared.journal, &prepared.mutation,
                &mut verify, Some(&mut stop)).unwrap_err().0, "retained_after_write");
            let side = images.iter().find(|image| image.role == "replaced").unwrap();
            let side_path = prepared.root.join(&side.path);
            match &side.before {
                Some(raw) => fs::write(&side_path, raw).unwrap(),
                None => fs::remove_file(&side_path).unwrap(),
            }
            drop(route);
            A::recover(std::slice::from_ref(&entry), temp.path(), before).unwrap();
            for image in images {
                assert_eq!(F::read(&prepared.root.join(&image.path)).unwrap(),
                    if before { image.before } else { image.after });
            }
        }
    }
}

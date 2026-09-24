//! Sparse source mapping-order witness. Typed identity sorts maps, whereas
//! legacy source-clock grouping uses their original printed order.
use crate::{
    Result, history_contract::*, history_view::list, history_yaml::SourceValue as S, require,
    value::TypedValue as V,
};

/// Null is canonical order. Only changed map order and paths leading to it remain.
pub(crate) fn encode(source: &S) -> V {
    match source {
        S::Scalar(_) => V::Null,
        S::List(items) => {
            let children = items
                .iter()
                .enumerate()
                .filter_map(|(i, item)| {
                    let value = encode(item);
                    (value != V::Null).then(|| (i.to_string(), value))
                })
                .collect::<Map>();
            if children.is_empty() {
                V::Null
            } else {
                V::List(vec![V::Text("list".into()), V::Map(children)])
            }
        }
        S::Map(items) => {
            let keys = items.iter().map(|(k, _)| k).collect::<Vec<_>>();
            let mut sorted = keys.clone();
            sorted.sort();
            let order = if keys == sorted {
                V::Null
            } else {
                V::List(keys.into_iter().map(|k| V::Text(k.clone())).collect())
            };
            let children = items
                .iter()
                .filter_map(|(k, item)| {
                    let value = encode(item);
                    (value != V::Null).then(|| (k.clone(), value))
                })
                .collect::<Map>();
            if order == V::Null && children.is_empty() {
                V::Null
            } else {
                V::List(vec![V::Text("map".into()), order, V::Map(children)])
            }
        }
    }
}
pub(crate) fn decode(value: &V, witness: &V) -> Result<S> {
    fn apply(value: &V, witness: &V) -> Result<S> {
        if *witness == V::Null {
            return Ok(S::from_typed(value));
        }
        let row = list(witness)?;
        match value {
            V::Map(values) => {
                require(
                    row.len() == 3 && string_is(&row[0], "map"),
                    "node_source_order",
                )?;
                let children = map(&row[2])?;
                require(
                    children.keys().all(|k| values.contains_key(k)),
                    "node_source_order",
                )?;
                let keys = if row[1] == V::Null {
                    values.keys().map(String::as_str).collect::<Vec<_>>()
                } else {
                    list(&row[1])?
                        .iter()
                        .map(text)
                        .collect::<Result<Vec<_>>>()?
                };
                require(
                    keys.len() == values.len()
                        && keys
                            .iter()
                            .copied()
                            .collect::<std::collections::BTreeSet<_>>()
                            == values.keys().map(String::as_str).collect(),
                    "node_source_order",
                )?;
                Ok(S::Map(
                    keys.into_iter()
                        .map(|k| {
                            Ok((
                                k.into(),
                                apply(&values[k], children.get(k).unwrap_or(&V::Null))?,
                            ))
                        })
                        .collect::<Result<_>>()?,
                ))
            }
            V::List(values) => {
                require(
                    row.len() == 2 && string_is(&row[0], "list"),
                    "node_source_order",
                )?;
                let children = map(&row[1])?;
                for key in children.keys() {
                    require(
                        key.parse::<usize>()
                            .ok()
                            .is_some_and(|i| i < values.len() && i.to_string() == *key),
                        "node_source_order",
                    )?;
                }
                Ok(S::List(
                    values
                        .iter()
                        .enumerate()
                        .map(|(i, v)| apply(v, children.get(&i.to_string()).unwrap_or(&V::Null)))
                        .collect::<Result<_>>()?,
                ))
            }
            _ => Err(error("node_source_order")),
        }
    }
    crate::history_yaml::validate_value(witness, crate::reasoning_snapshot::MAX_REQUEST_BYTES)?;
    let source = apply(value, witness)?;
    require(
        encode(&source) == *witness,
        "node_source_order_noncanonical",
    )?;
    Ok(source)
}
pub(crate) fn without_saw(mut source: S) -> Result<S> {
    let S::Map(fields) = &mut source else {
        return Err(error("invalid_schema"));
    };
    fields.retain(|(k, _)| k != "saw");
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{history_node_observation::ObservationNode, history_node_semantics::History};
    use std::collections::BTreeMap;
    #[test]
    fn compact_retains_source_order_for_every_legacy_reduction_fixture() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/history-reduction.json")).unwrap();
        let mut divergent = 0;
        for case in fixture["cases"].as_array().unwrap() {
            if case.get("error").is_some() {
                continue;
            }
            let input = V::from_tagged(&case["input"]).unwrap();
            let input = map(&input).unwrap();
            let rules = map(&input["rules"]).unwrap();
            let pairs = list(&input["ancestry"]).unwrap();
            let ancestry = |a: &V, b: &V| pairs.contains(&V::List(vec![a.clone(), b.clone()]));
            let mut objects = Vec::new();
            let mut orders = BTreeMap::new();
            for row in case["raw_objects"].as_array().unwrap() {
                let source = crate::history_yaml::decode_source_document(
                    row[2].as_str().unwrap().as_bytes(),
                )
                .unwrap();
                let object = source.typed();
                let object = map(&object).unwrap();
                let id = text(&object["id"]).unwrap();
                let saw = list(&object["saw"])
                    .unwrap()
                    .iter()
                    .map(|v| text(v).unwrap().to_owned())
                    .collect();
                let compact = without_saw(source).unwrap();
                let witness = encode(&compact);
                if witness != V::Null {
                    orders.insert(id.to_owned(), witness);
                }
                objects.push((compact.typed(), ObservationNode::root(id, &saw).unwrap()));
            }
            let history = History::from_ordered(objects, orders).unwrap();
            let actual = history.reduce(Some(rules), Some(&ancestry)).unwrap();
            assert_eq!(
                actual.to_tagged().unwrap(),
                case["output"],
                "{}",
                case["name"]
            );
            divergent += usize::from(case["output"] != case["canonical_output"]);
        }
        assert!(
            divergent > 0,
            "fixture must exercise semantic source-order differences"
        );
    }
    #[test]
    fn witness_cannot_add_drop_repeat_or_retype_a_source_key() {
        let value = V::from_json(&serde_json::json!({"a":1,"b":2})).unwrap();
        for keys in [
            serde_json::json!(["a"]),
            serde_json::json!(["a", "a"]),
            serde_json::json!(["c", "b"]),
        ] {
            let witness = V::from_json(&serde_json::json!(["map", keys, {}])).unwrap();
            assert!(decode(&value, &witness).is_err());
        }
        let witness = V::from_json(&serde_json::json!(["map", ["a", "b"], {}])).unwrap();
        assert!(decode(&value, &witness).is_err()); // redundant, noncanonical witness
    }
}

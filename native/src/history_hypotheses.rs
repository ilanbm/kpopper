//! Named proposal layers derived exclusively from immutable recorded witnesses.
use crate::{
    Result,
    history_adapter::adapt_body,
    history_contract::*,
    history_projection,
    history_reduce::claim_meaning,
    history_view::{list, map_mut},
    reasoning_snapshot::entries,
    require,
    value::{Integer, TypedValue as V},
};
use std::collections::{BTreeMap, BTreeSet};

pub const KIND: &str = "named-history-hypothesis/v1";
fn s(value: &str) -> V {
    V::Text(value.into())
}
#[derive(Default)]
struct Group {
    versions: BTreeMap<String, Vec<String>>,
    heads: Vec<(String, V)>,
    claims: Vec<(String, V)>,
}

/// Returns layers and their exact immutable-version index. A contested layer
/// remains explicit evidence and must not be silently accepted by a consumer.
pub fn layers(projection: &V, document: &V) -> Result<(V, V)> {
    history_projection::validate_projection(projection)?;
    let p = map(projection)?;
    let empty = V::Map(Map::new());
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for (subject, disposition) in map(p.get("dispositions").unwrap_or(&empty))? {
        for version in list(&map(disposition)?["proposals"])? {
            let version = text(version)?;
            let witness = map(&p["pins"])?
                .get(version)
                .ok_or_else(|| error("missing_hypothesis_witness"))?;
            let witness = map(witness)?;
            require(
                string_is(&witness["status"], "recorded"),
                "missing_hypothesis_witness",
            )?;
            let claim = &witness["object"];
            let authored = map(claim)?.get("authored").unwrap_or(&empty);
            let Some(hypothesis) = map(authored)?.get("hypothesis") else {
                continue;
            };
            let hypothesis = map(hypothesis)?;
            let group = groups.entry(text(&hypothesis["name"])?.into()).or_default();
            group
                .versions
                .entry(subject.clone())
                .or_default()
                .push(version.into());
            let head_id = hypothesis["head"].digest()?;
            if !group.heads.iter().any(|(id, _)| id == &head_id) {
                group.heads.push((head_id, hypothesis["head"].clone()));
            }
            if !group.claims.iter().any(|(id, _)| id == version) {
                group.claims.push((version.into(), claim.clone()));
            }
        }
    }
    let mut result = Map::new();
    let mut index = Map::new();
    for (name, group) in groups {
        let mut errors = Vec::new();
        let mut headers: Vec<(String, V)> = Vec::new();
        let mut profiles = BTreeSet::new();
        let mut fields = Map::new();
        let mut conflicting_fields = false;
        for (_, claim) in &group.claims {
            let authored = map(&map(claim)?["authored"])?;
            let locator = map(authored.get("locator").unwrap_or(&empty))?;
            if let Some(header) = locator.get("document_headers") {
                let digest = header.digest()?;
                if !headers.iter().any(|(id, _)| id == &digest) {
                    headers.push((digest, header.clone()));
                }
            }
            profiles.insert(text(&authored["profile"])?.to_owned());
            for (role, field) in map(&authored["fields"])? {
                conflicting_fields |= fields.get(role).is_some_and(|old| old != field);
                fields.insert(role.clone(), field.clone());
            }
        }
        if headers.len() > 1 || profiles.len() != 1 || conflicting_fields {
            errors.push("contested hypothesis interpretation".to_owned());
        }
        let mut body = if let Some((_, header)) = headers.first() {
            map(header).map_err(|_| error("invalid_hypothesis_headers"))?;
            header.clone()
        } else {
            let doc = map(document)?;
            let mut body =
                Map::from([("schema".into(), doc.get("schema").unwrap_or(&empty).clone())]);
            if let Some(declaration) = doc
                .get("meta")
                .and_then(|v| map(v).ok())
                .and_then(|m| m.get("reasoning"))
                && *declaration != V::Null
            {
                body.insert(
                    "meta".into(),
                    V::Map(Map::from([("reasoning".into(), declaration.clone())])),
                );
            }
            V::Map(body)
        };
        if group.heads.len() != 1 {
            errors.push("contested hypothesis head".into());
        }
        let mut versions_index = Map::new();
        for (subject, mut versions) in group.versions {
            versions.sort();
            let claims = versions
                .iter()
                .map(|id| &group.claims.iter().find(|(v, _)| v == id).unwrap().1)
                .collect::<Vec<_>>();
            let meanings = claims
                .iter()
                .map(|obj| claim_meaning(map(obj)?))
                .collect::<Result<BTreeSet<_>>>()?;
            if meanings.len() != 1 {
                errors.push(format!("contested hypothesis subject: {subject}"));
            } else {
                let claim = claims[0];
                let collection = text(&map(&map(claim)?["authored"])?["collection"])?;
                let members = map_mut(&mut body)?
                    .entry(collection.into())
                    .or_insert_with(|| empty.clone());
                map_mut(members)?.insert(subject.clone(), adapt_body(claim)?);
            }
            versions_index.insert(
                subject,
                V::List(versions.into_iter().map(V::Text).collect()),
            );
        }
        let ids = V::List(entries(&body)?.keys().cloned().map(V::Text).collect());
        let mut raw = Map::new();
        for (collection, members) in map(&body)? {
            if !["meta", "schema", "record", "also"].contains(&collection.as_str())
                && let V::Map(members) = members
            {
                raw.extend(members.clone());
            }
        }
        result.insert(
            name.clone(),
            V::Map(Map::from([
                ("kind".into(), s(KIND)),
                ("doc".into(), body),
                (
                    "head".into(),
                    if group.heads.len() == 1 {
                        group.heads[0].1.clone()
                    } else {
                        empty.clone()
                    },
                ),
                (
                    "error".into(),
                    if errors.is_empty() {
                        V::Null
                    } else {
                        s(&errors.join("; "))
                    },
                ),
                ("ids".into(), ids),
                ("raw".into(), V::Map(raw)),
                ("name".into(), s(&name)),
                (
                    "profile".into(),
                    if profiles.len() == 1 {
                        s(profiles.first().unwrap())
                    } else {
                        V::Null
                    },
                ),
                (
                    "fields".into(),
                    if conflicting_fields {
                        V::Null
                    } else {
                        V::Map(fields)
                    },
                ),
            ])),
        );
        index.insert(name, V::Map(versions_index));
    }
    Ok((
        V::Map(result),
        V::Map(Map::from([
            ("version".into(), V::Integer(Integer::new("1")?)),
            ("groups".into(), V::Map(index)),
        ])),
    ))
}

//! Shared proposal claims, disposition and hypothetical evidence.
use crate::{
    Result,
    history_authoring::{Options, Proposal, destination, empty, make_object, n, obj, s, strings},
    history_authoring_audit::ReplayAudit,
    history_authoring_core::Input,
    history_contract::*,
    history_paths as P, history_transaction as T,
    history_view::{list, map_mut, truth},
    reasoning_fields as F,
    reasoning_runtime::Runtime,
    require,
    value::TypedValue as V,
};
use std::collections::BTreeSet;
pub(crate) struct Plan {
    pub objects: Vec<V>,
    pub document: V,
    pub hypothetical: V,
    capabilities: V,
    version: u8,
}
pub(crate) fn prepare(
    input: &dyn Input,
    proposal: &Proposal,
    options: &Options,
    runtime: Option<&Runtime>,
) -> Result<Plan> {
    P::subject(&proposal.subject)?;
    P::subject(&proposal.collection)?;
    require(
        !["meta", "schema", "record", "also"].contains(&proposal.collection.as_str()),
        "invalid_proposal_collection",
    )?;
    require(!proposal.because.trim().is_empty(), "act_reason_required")?;
    require(options.strict, "strict_history_capability_required")?;
    let mut version = options.receipt_version.unwrap_or(9);
    require([5, 9].contains(&version), "invalid_authoring_receipt")?;
    let doc = input.document()?;
    let fields = F::snapshot_fields(&doc)?;
    let subject = proposal.subject.as_str();
    let collection = proposal.collection.as_str();
    let profile = map(&map(input.state())?["subjects"])?
        .get(subject)
        .map(map)
        .transpose()?
        .and_then(|m| m.get("head"))
        .map(|h| {
            let o = input.object(text(h)?)?;
            text(field(map(field(map(o)?, "authored")?)?, "profile")?)
        })
        .transpose()?;
    let cap = F::capabilities(&doc, profile)?;

    if !string_is(&map(&cap)?["profile"], "core/v1") {
        version = 5;
    }
    let deps_field = text(&fields["deps"])?;
    let snapshot_field = text(&fields["snapshot"])?;
    let mut body = proposal.body.clone();
    let deps = map(&body)
        .ok()
        .and_then(|b| b.get(deps_field))
        .map(list)
        .transpose()
        .map_err(|_| error("invalid_proposal_dependencies"))?
        .unwrap_or(&[])
        .to_vec();
    require(
        deps.iter().all(|v| matches!(v, V::Text(_))),
        "invalid_proposal_dependencies",
    )?;
    let judgment = map(&body).is_ok_and(|m| m.contains_key(deps_field));
    require(
        version != 9 || !judgment || !map(&body)?.contains_key(snapshot_field),
        "authored_snapshot_forbidden",
    )?;
    let mut authored = obj([
        ("collection", s(collection)),
        (
            "fields",
            V::Map(
                fields
                    .iter()
                    .filter(|(_, v)| truth(v))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
        ),
        ("profile", map(&cap)?["profile"].clone()),
    ]);
    if let Some(hypothesis) = &proposal.hypothesis {
        map_mut(&mut authored)?.insert("hypothesis".into(), hypothesis.clone());
    }
    let mut hypothetical = doc.clone();
    for name in F::collections(&doc)?.keys() {
        map_mut(map_mut(&mut hypothetical)?.get_mut(name).unwrap())?.remove(subject);
    }
    map_mut(
        map_mut(&mut hypothetical)?
            .entry(collection.into())
            .or_insert_with(empty),
    )?
    .insert(subject.into(), body.clone());
    hypothetical = destination(&hypothetical)?;
    if version == 9 && judgment {
        let mut hypothetical_world =
            crate::history_authoring_reader::AuthoringReader::new(&hypothetical, runtime)?;
        let mut seen = Map::new();
        for dep in &deps {
            let dep = text(dep)?;
            if hypothetical_world.raw().contains_key(dep) {
                seen.insert(dep.into(), hypothetical_world.history(dep)?);
            }
        }
        map_mut(&mut body)?.insert(snapshot_field.into(), V::Map(seen));
        map_mut(map_mut(&mut hypothetical)?.get_mut(collection).unwrap())?
            .insert(subject.into(), body.clone());
    }
    let saw = input.saw(subject)?;
    let (pins, _) = input.pins(&deps, false)?;
    let claim = make_object(
        subject,
        if judgment { "judgment" } else { "reading" },
        body,
        strings(saw.clone()),
        Some(authored),
        pins,
        empty(),
        options,
    )?;
    let claim_id = map(&claim)?["id"].clone();
    let mut saw = saw;
    saw.push(text(&claim_id)?.into());
    saw.sort();
    let act = make_object(
        subject,
        "act",
        obj([
            ("act", s("propose")),
            ("of", claim_id.clone()),
            ("over", V::List(vec![])),
            ("because", s(&proposal.because)),
        ]),
        strings(saw),
        None,
        empty(),
        empty(),
        options,
    )?;
    let new = vec![claim, act];
    Ok(Plan {
        objects: new,
        document: doc,
        hypothetical,
        capabilities: cap,
        version,
    })
}

pub(crate) fn receipt(
    input: &dyn Input,
    proposal: &Proposal,
    plan: &Plan,
    options: &Options,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
    frozen_archive: &V,
) -> Result<V> {
    let subject = proposal.subject.as_str();
    let collection = proposal.collection.as_str();
    let new = &plan.objects;
    let doc = &plan.document;
    let hypothetical = &plan.hypothetical;
    let cap = &plan.capabilities;
    let version = plan.version;
    let claim_id = map(&new[0])?["id"].clone();
    let versions = input.accepted_versions()?;
    let mut before = crate::history_authoring_reader::AuthoringReader::document_evidence(
        &doc, runtime, audit, &versions,
    )?;
    map_mut(&mut before)?.insert(
        "authoring".into(),
        obj([
            ("version", n(&version.to_string())),
            ("kind", s("proposal")),
            ("subject", s(subject)),
            ("body", proposal.body.clone()),
            ("collection", s(collection)),
            ("because", s(&proposal.because)),
            ("hypothesis", proposal.hypothesis.clone().unwrap_or(V::Null)),
            ("by", options.by.clone()),
            ("recorded_at", s(&options.recorded_at)),
            ("archive", frozen_archive.clone()),
            ("baseline", input.baseline().clone()),
        ]),
    );
    let mut after = crate::history_authoring_reader::AuthoringReader::document_evidence(
        &doc, runtime, audit, &versions,
    )?;
    map_mut(&mut after)?.insert(
        "proposal".into(),
        crate::history_authoring_reader::AuthoringReader::document_evidence(
            &hypothetical,
            runtime,
            audit,
            &versions,
        )?,
    );
    let ids = new
        .iter()
        .map(|o| text(&map(o)?["id"]).map(str::to_owned))
        .collect::<Result<BTreeSet<_>>>()?;
    map_mut(&mut after)?.insert(
        "authoring".into(),
        obj([
            ("objects", strings(ids)),
            ("subject", s(subject)),
            ("proposal", claim_id),
        ]),
    );
    let receipt = T::semantic_receipt(text(&map(&cap)?["profile"])?, &cap, &before, &after)?;
    Ok(receipt)
}

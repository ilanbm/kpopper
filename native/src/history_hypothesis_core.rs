//! Provider-neutral named hypothesis intent and semantic evidence preparation.
use crate::{
    Result,
    history_authoring::{self as A, Options, empty, n, obj, s, strings},
    history_authoring_audit::ReplayAudit,
    history_authoring_core::Input,
    history_contract::*,
    history_hypotheses as HH,
    history_hypothesis_authoring::{Context, Fold, act, guard_names, layer, pins, world},
    history_transaction as T,
    history_view::{list, map_mut, truth},
    reasoning_authoring as W, reasoning_fields as F,
    reasoning_runtime::Runtime,
    reasoning_snapshot::entries,
    require,
    value::TypedValue as V,
};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct Plan {
    pub objects: Vec<V>,
    pub intent: V,
    pub before: V,
    pub after: V,
    pub groups_before: Option<V>,
    pub groups_after: Option<V>,
    pub stamp: Option<String>,
}

pub(crate) fn prepare(
    captured: &dyn Input,
    context: Context,
    name: &str,
    action: &V,
    head: Option<&V>,
    options: &Options,
    runtime: Option<&Runtime>,
) -> Result<Plan> {
    let Context {
        base,
        mut groups,
        index,
        physical,
    } = context;
    require(options.strict, "explicit_root_disposition_required")?;
    guard_names(&[name.into()], &groups, &physical, false)?;
    let mut action = action.clone();
    let a = map_mut(&mut action)?;
    let kind = text(field(a, "kind")?)?.to_owned();
    require(
        ["add", "set", "review"].contains(&kind.as_str()) && !a.get("section").is_some_and(truth),
        "invalid_hypothesis_action",
    )?;
    require(
        a.get("hypothesis")
            .is_none_or(|v| *v == V::Null || string_is(v, name)),
        "hypothesis_name_mismatch",
    )?;
    a.insert("hypothesis".into(), s(name));
    if !a.get("as_of").is_some_and(truth) {
        require(!options.recording_day.is_empty(), "missing_recording_time")?;
        a.insert("as_of".into(), s(&options.recording_day));
    }
    let head = if let Some(group) = map(&groups)?.get(name) {
        let group = map(group)?;
        require(
            !group.get("error").is_some_and(truth),
            "unresolved_history_hypothesis",
        )?;
        require(
            head.filter(|v| **v != V::Null)
                .is_none_or(|v| v.digest().ok() == group["head"].digest().ok()),
            "hypothesis_head_change_requires_group_edit",
        )?;
        group["head"].clone()
    } else {
        let head = head
            .filter(|v| **v != V::Null)
            .cloned()
            .unwrap_or_else(|| obj([("born", a["as_of"].clone())]));
        map(&head).map_err(|_| error("invalid_history_hypothesis"))?;
        map_mut(&mut groups)?.insert(
            name.into(),
            obj([
                ("name", s(name)),
                ("kind", s(HH::KIND)),
                ("doc", empty()),
                ("head", head.clone()),
                ("ids", V::List(vec![])),
                ("raw", empty()),
                ("error", V::Null),
            ]),
        );
        head
    };
    let before = layer(&base, &groups, &[name.into()])?;
    let profile = map(&F::capabilities(&before, None)?)?["profile"].clone();
    require(
        map(&map(&groups)?[name])?
            .get("profile")
            .is_none_or(|v| *v == V::Null || *v == profile),
        "history_hypothesis_profile_migration_required",
    )?;
    require(
        map(&action)?
            .get("profile")
            .is_none_or(|v| *v == V::Null || *v == profile),
        "history_profile_migration_required",
    )?;
    let mut prior_world = world(&before, Some(&groups), runtime)?;
    let (normalized, _) = prior_world.normalize(&action, None)?;
    let normalized_map = map(&normalized)?;
    let subject = text(field(normalized_map, "id")?)?;
    let mut validation_groups = groups.clone();
    if kind == "add" {
        // Python's validation-only ids set removes this member, while the actual
        // document and before/after evidence retain the complete named world.
        let group = map_mut(map_mut(&mut validation_groups)?.get_mut(name).unwrap())?;
        if list(&group["ids"])?.contains(&s(subject)) {
            for values in map_mut(group.get_mut("doc").unwrap())?.values_mut() {
                if let V::Map(values) = values {
                    values.remove(subject);
                }
            }
        }
    }
    let refusals = world(&before, Some(&validation_groups), runtime)?.validate(&normalized)?;
    require(
        refusals.is_empty(),
        &format!("refused - {}", refusals.join("; ")),
    )?;
    let version = options.receipt_version.unwrap_or(2);
    require([1, 2].contains(&version), "invalid_hypothesis_receipt")?;
    let previous = map(&map(&index)?["groups"])?
        .get(name)
        .map(map)
        .transpose()?
        .and_then(|g| g.get(subject))
        .map(list)
        .transpose()?
        .unwrap_or(&[]);
    let fields = prior_world.fields().clone();
    let deps_field = text(&fields["deps"])?;
    let mut after = before.clone();
    let mut objects = vec![];
    let why = normalized_map
        .get("why")
        .filter(|v| truth(v))
        .map(text)
        .transpose()?;
    if kind == "review" {
        require(!previous.is_empty(), "review_requires_group_proposal")?;
        let target = map(captured.object(text(&previous[0])?)?)?;
        require(string_is(&target["kind"], "judgment"), "invalid_review")?;
        require(
            normalized_map.get("_record_scope").is_none_or(|v| {
                v.digest().ok()
                    == map(&target["body"])
                        .ok()
                        .and_then(|b| b.get("scope"))
                        .unwrap_or(&V::Null)
                        .digest()
                        .ok()
            }),
            "history_review_scope_change_requires_claim",
        )?;
        let deps = list(field(map(&entries(&before)?[subject].1)?, deps_field)?)?.to_vec();
        let (pins, gaps) = pins(
            captured,
            &index,
            name,
            &deps,
            !W::blocked_text(&target["body"]).is_empty(),
        )?;
        require(!truth(&gaps), "unavailable_review_pin")?;
        for version in previous {
            objects.push(act(
                captured,
                captured.object(text(version)?)?,
                "review",
                why.unwrap_or("explicit hypothesis review"),
                V::List(vec![]),
                &[],
                Some(pins.clone()),
                options,
            )?);
        }
    } else {
        let candidate = prior_world.candidate(&normalized)?;
        let known = entries(candidate.document())?;
        let (collection, mut body) = known
            .get(subject)
            .ok_or_else(|| error("unresolved_history_subject"))?
            .clone();
        let mut authored = if kind == "set" {
            let target = if previous.is_empty() {
                captured.head(subject)?
            } else {
                captured.object(text(&previous[0])?)?
            };
            map(target)?["authored"].clone()
        } else {
            let mut authored = obj([
                ("collection", s(&collection)),
                ("profile", profile),
                ("fields", V::Map(fields.clone())),
            ]);
            let group_versions = map(&map(&index)?["groups"])?
                .get(name)
                .map(map)
                .transpose()?;
            let has_headers = group_versions
                .into_iter()
                .flat_map(|g| g.values())
                .map(list)
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .any(|v| {
                    text(v)
                        .ok()
                        .and_then(|id| captured.object(id).ok())
                        .and_then(|o| map(o).ok())
                        .and_then(|o| o.get("authored"))
                        .and_then(|a| map(a).ok())
                        .and_then(|a| a.get("locator"))
                        .and_then(|l| map(l).ok())
                        .is_some_and(|l| l.contains_key("document_headers"))
                });
            if has_headers {
                let headers = map(&map(&map(&groups)?[name])?["doc"])?
                    .iter()
                    .filter(|(k, _)| ["meta", "schema", "record", "also"].contains(&k.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                map_mut(&mut authored)?.insert(
                    "locator".into(),
                    obj([("document_headers", V::Map(headers))]),
                );
            }
            authored
        };
        let judgment = map(&body).is_ok_and(|b| b.contains_key(deps_field));
        let deps = map(&body)
            .ok()
            .and_then(|b| b.get(deps_field))
            .map(list)
            .transpose()?
            .unwrap_or(&[])
            .to_vec();
        if version == 2 && judgment {
            let mut seen = Map::new();
            for dep in &deps {
                let dep = text(dep)?;
                if prior_world.raw().contains_key(dep) {
                    seen.insert(dep.into(), prior_world.history(dep)?);
                }
            }
            map_mut(&mut body)?.insert(text(&fields["snapshot"])?.into(), V::Map(seen));
        }
        map_mut(&mut authored)?.insert(
            "hypothesis".into(),
            obj([
                ("version", n("1")),
                ("name", s(name)),
                ("head", head.clone()),
            ]),
        );
        let (pins, gaps) = pins(
            captured,
            &index,
            name,
            &deps,
            !W::blocked_text(&body).is_empty(),
        )?;
        let claim = A::make_object(
            subject,
            if judgment { "judgment" } else { "reading" },
            body.clone(),
            strings(captured.saw(subject)?),
            Some(authored),
            pins,
            gaps,
            options,
        )?;
        let id = text(&map(&claim)?["id"])?.to_owned();
        objects.push(claim.clone());
        objects.push(act(
            captured,
            &claim,
            "propose",
            why.unwrap_or("explicit named hypothesis"),
            V::List(vec![]),
            std::slice::from_ref(&id),
            None,
            options,
        )?);
        for version in previous {
            objects.push(act(
                captured,
                captured.object(text(version)?)?,
                "retire",
                &format!("superseded within hypothesis {name}"),
                V::List(vec![]),
                std::slice::from_ref(&id),
                None,
                options,
            )?);
        }
        map_mut(map_mut(&mut after)?.entry(collection).or_insert_with(empty))?
            .insert(subject.into(), body);
    }
    let intent = obj([
        ("version", n(&version.to_string())),
        ("kind", s("edit")),
        ("name", s(name)),
        ("head", head),
        ("action", action),
        ("operation", s(&options.operation)),
        ("recorded_at", s(&options.recorded_at)),
        ("by", options.by.clone()),
    ]);
    Ok(Plan {
        objects,
        intent,
        before,
        after,
        groups_before: Some(groups.clone()),
        groups_after: Some(groups),
        stamp: None,
    })
}

pub(crate) fn fold(
    captured: &dyn Input,
    context: &Context,
    fold: &Fold,
    options: &Options,
    runtime: Option<&Runtime>,
) -> Result<Plan> {
    require(
        fold.assessment_version <= 1,
        "unsupported_hypothesis_assessment",
    )?;
    require(options.strict, "explicit_root_disposition_required")?;
    guard_names(&fold.names, &context.groups, &context.physical, true)?;
    require(!fold.because.trim().is_empty(), "act_reason_required")?;
    let mut names = fold.names.clone();
    names.sort();
    let mut chosen = BTreeMap::new();
    for name in &names {
        for (subject, versions) in map(&map(&map(&context.index)?["groups"])?[name])? {
            require(
                chosen.insert(subject.clone(), versions.clone()).is_none(),
                "overlapping_hypothesis_selection",
            )?;
        }
    }
    require(
        fold.take.iter().all(|v| chosen.contains_key(v)),
        "invalid_hypothesis_take",
    )?;
    let after = layer(&context.base, &context.groups, &names)?;
    let prospective = world(&after, Some(&context.groups), runtime)?;
    for name in &names {
        let head = map(&map(&map(&context.groups)?[name])?["head"])?;
        require(
            !head.get("folds").is_some_and(|v| string_is(v, "never")),
            "hypothesis_never_folds",
        )?;
        if let Some(condition) = head.get("wrong_if").filter(|v| truth(v)) {
            require(
                prospective.predicate(condition, None)? != Some(true),
                "hypothesis_head_falsified",
            )?;
        }
    }
    let base_world = world(&context.base, None, runtime)?;
    let fields = base_world.fields();
    let deps_field = text(&fields["deps"])?;
    let predicate_field = text(&fields["predicate"])?;
    let existing = entries(&context.base)?;
    let states = map(&map(captured.state())?["subjects"])?;
    let stamp = options
        .recorded_at
        .get(..10)
        .filter(|v| crate::value::Date::new(v).is_ok())
        .or_else(|| (!options.recording_day.is_empty()).then_some(options.recording_day.as_str()))
        .ok_or_else(|| error("missing_recording_time"))?;
    let mut objects = vec![];
    for (subject, versions) in &chosen {
        let versions = list(versions)?;
        let target = map(captured.object(text(
            versions
                .first()
                .ok_or_else(|| error("missing_hypothesis_witness"))?,
        )?)?)?;
        let body = &target["body"];
        if string_is(&target["kind"], "judgment")
            && let Some(condition) = map(body)?
                .get(text(&prospective.fields()["predicate"])?)
                .filter(|v| truth(v))
        {
            require(
                prospective.predicate(condition, None)? != Some(true),
                "hypothesis_falsified",
            )?;
        }
        let heads = states
            .get(subject)
            .map(map)
            .transpose()?
            .and_then(|v| v.get("heads"))
            .cloned()
            .unwrap_or(V::List(vec![]));
        if truth(&heads) {
            let old = &existing
                .get(subject)
                .ok_or_else(|| error("unresolved_history_subject"))?
                .1;
            if old.digest()? != body.digest()? {
                let is_judgment = map(old).is_ok_and(|m| m.contains_key(deps_field));
                let permitted = if is_judgment {
                    let shaped = map(body)
                        .ok()
                        .and_then(|m| m.get(deps_field))
                        .is_some_and(|v| {
                            list(v)
                                .is_ok_and(|v| !v.is_empty() && v.iter().all(|v| text(v).is_ok()))
                        });
                    let fired = base_world
                        .predicate(map(old)?.get(predicate_field).unwrap_or(&V::Null), None)?;
                    shaped && (fired == Some(true) || fold.take.contains(subject))
                } else {
                    crate::reasoning_authoring_guards::read_on(old, &base_world)
                        .is_none_or(|when| stamp > when.as_str())
                };
                require(permitted, "hypothesis_fold_requires_resolution")?;
                if is_judgment {
                    let old_deps = map(old)?
                        .get(deps_field)
                        .map(list)
                        .transpose()?
                        .unwrap_or(&[]);
                    let new_deps = map(body)?
                        .get(deps_field)
                        .map(list)
                        .transpose()?
                        .unwrap_or(&[]);
                    for dep in old_deps.iter().filter(|d| !new_deps.contains(d)) {
                        let reason = map(&fold.drops)
                            .ok()
                            .and_then(|d| text(dep).ok().and_then(|dep| d.get(dep)))
                            .and_then(|v| text(v).ok());
                        require(
                            reason.is_some_and(|s| !s.trim().is_empty()),
                            "hypothesis_drop_reason_required",
                        )?;
                    }
                }
            }
        }
        for version in versions {
            objects.push(act(
                captured,
                captured.object(text(version)?)?,
                "accept",
                &fold.because,
                heads.clone(),
                &[],
                None,
                options,
            )?);
        }
    }
    let mut take = fold.take.clone();
    take.sort();
    let mut intent = obj([
        ("version", n("1")),
        ("kind", s("fold")),
        ("names", strings(names)),
        ("because", s(&fold.because)),
        ("take", strings(take)),
        (
            "drops",
            if truth(&fold.drops) {
                fold.drops.clone()
            } else {
                empty()
            },
        ),
        ("operation", s(&options.operation)),
        ("recorded_at", s(&options.recorded_at)),
        ("by", options.by.clone()),
    ]);
    if fold.assessment_version == 1 {
        map_mut(&mut intent)?.insert("assessment_version".into(), n("1"));
    }
    Ok(Plan {
        objects,
        intent,
        before: context.base.clone(),
        after,
        groups_before: None,
        groups_after: Some(context.groups.clone()),
        stamp: Some(stamp.into()),
    })
}

pub(crate) fn refute(
    captured: &dyn Input,
    context: &Context,
    names: &[String],
    because: &str,
    options: &Options,
) -> Result<Plan> {
    require(options.strict, "explicit_root_disposition_required")?;
    guard_names(names, &context.groups, &context.physical, true)?;
    require(!because.trim().is_empty(), "act_reason_required")?;
    let mut names = names.to_vec();
    names.sort();
    let mut chosen = BTreeMap::new();
    for name in &names {
        for (subject, versions) in map(&map(&map(&context.index)?["groups"])?[name])? {
            require(
                chosen.insert(subject.clone(), versions.clone()).is_none(),
                "overlapping_hypothesis_selection",
            )?;
        }
    }
    let mut objects = vec![];
    for versions in chosen.values() {
        for version in list(versions)? {
            objects.push(act(
                captured,
                captured.object(text(version)?)?,
                "refute",
                because,
                V::List(vec![]),
                &[],
                None,
                options,
            )?);
        }
    }
    let intent = obj([
        ("version", n("1")),
        ("kind", s("refute")),
        ("names", strings(names)),
        ("because", s(because)),
        ("take", V::List(vec![])),
        ("drops", empty()),
        ("operation", s(&options.operation)),
        ("recorded_at", s(&options.recorded_at)),
        ("by", options.by.clone()),
    ]);
    Ok(Plan {
        objects,
        intent,
        before: context.base.clone(),
        after: context.base.clone(),
        groups_before: None,
        groups_after: None,
        stamp: None,
    })
}

pub(crate) fn receipt(
    captured: &dyn Input,
    plan: &Plan,
    archive: &V,
    physical: &V,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
) -> Result<V> {
    let cap = F::capabilities(&plan.before, None)?;
    let mut intent = plan.intent.clone();
    map_mut(&mut intent)?.extend([
        ("archive".into(), archive.clone()),
        ("physical".into(), physical.clone()),
        ("baseline".into(), captured.baseline().clone()),
    ]);
    let mut before = A::evidence(
        &plan.before,
        &mut world(&plan.before, plan.groups_before.as_ref(), runtime)?,
        audit,
    )?;
    map_mut(&mut before)?.insert("hypothesis_authoring".into(), intent);
    let mut after = A::evidence(
        &plan.after,
        &mut world(&plan.after, plan.groups_after.as_ref(), runtime)?,
        audit,
    )?;
    let ids = plan
        .objects
        .iter()
        .map(|o| text(&map(o)?["id"]).map(str::to_owned))
        .collect::<Result<BTreeSet<_>>>()?;
    map_mut(&mut after)?.insert(
        "hypothesis_authoring".into(),
        obj([("objects", strings(ids))]),
    );
    T::semantic_receipt(text(&map(&cap)?["profile"])?, &cap, &before, &after)
}

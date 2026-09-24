//! Provider-neutral same/distinct semantic planning over a validated identity context.
use crate::history_identity::{Action, Intent};
use crate::{
    Result, history_authoring as A,
    history_authoring::Options,
    history_authoring_core::Input as BaseInput,
    history_contract::*,
    history_hypothesis_authoring as HA, history_identity_merge as M, history_identity_rewrite as R,
    history_reduce::claim_meaning,
    history_view::{list, map_mut, truth},
    history_yaml::SourceValue as Source,
    reasoning_fields as F,
    reasoning_runtime::Runtime,
    reasoning_snapshot::entries,
    require,
    value::{Date, TypedValue as V},
};
use A::{empty, n, obj, s, strings};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) trait Input: BaseInput {
    fn source_body(&self, object: &V) -> Result<Source>;
    fn template(&self) -> Result<V>;
}
pub(crate) type Key = (String, String);
pub(crate) type Own = BTreeMap<String, BTreeMap<String, Vec<String>>>;
#[derive(Clone)]
pub(crate) struct Spec {
    pub body: V,
    pub authored: V,
    pub prior: V,
    pub olds: Vec<String>,
}

fn select_prior<'a>(input: &'a dyn Input, ids: &[String]) -> Result<&'a V> {
    ids.iter()
        .min()
        .map(|id| input.object(id))
        .transpose()?
        .ok_or_else(|| error("incomplete_closure"))
}
fn own(input: &dyn Input, context: &HA::Context) -> Result<Own> {
    let mut base = BTreeMap::new();
    for subject in entries(&context.base)?.keys() {
        base.insert(
            subject.clone(),
            list(&map(&map(&map(input.state())?["subjects"])?[subject])?["heads"])?
                .iter()
                .map(|v| text(v).map(str::to_owned))
                .collect::<Result<_>>()?,
        );
    }
    let mut own = BTreeMap::from([(String::new(), base)]);
    for (name, members) in map(&map(&context.index)?["groups"])? {
        let mut group = BTreeMap::new();
        for (subject, ids) in map(members)? {
            group.insert(
                subject.clone(),
                list(ids)?
                    .iter()
                    .map(|v| text(v).map(str::to_owned))
                    .collect::<Result<_>>()?,
            );
        }
        own.insert(name.clone(), group);
    }
    Ok(own)
}
fn is_distinct(context: &HA::Context, a: &str, b: &str) -> Result<bool> {
    let mut raws = vec![
        entries(&context.base)?
            .into_iter()
            .map(|(k, (_, v))| (k, v))
            .collect::<Map>(),
    ];
    for group in map(&context.groups)?.values() {
        raws.push(map(&map(group)?["raw"])?.clone());
    }
    Ok(raws.iter().any(|raw| {
        [(a, b), (b, a)].iter().any(|(a, b)| {
            raw.get(*a)
                .and_then(|v| map(v).ok())
                .and_then(|m| m.get("distinct_from"))
                .is_some_and(|v| M::ids(v).iter().any(|id| id == b))
        })
    }))
}
fn errors(document: &V, runtime: Option<&Runtime>) -> Result<BTreeSet<(String, String)>> {
    let report = HA::world(document, None, runtime)?.assessment()?;
    let mut issues = BTreeSet::new();
    for (id, node) in map(&map(&report)?["nodes"])? {
        for issue in list(&map(&map(&map(node)?["state"])?["integrity"])?["issues"])? {
            let code = text(&map(issue)?["code"])?;
            if code != "missing_snapshot" {
                issues.insert((id.clone(), code.into()));
            }
        }
    }
    Ok(issues)
}
fn survivor(intent: &Intent) -> Result<(&str, &str)> {
    let Action::Same { keep } = &intent.action else {
        return Err(error("invalid_identity_receipt"));
    };
    match keep.as_deref() {
        None => Ok((&intent.a, &intent.b)),
        Some(k) if k == "a" || k == intent.a => Ok((&intent.a, &intent.b)),
        Some(k) if k == "b" || k == intent.b => Ok((&intent.b, &intent.a)),
        _ => Err(error("invalid_identity_keep")),
    }
}
fn put(document: &mut V, subject: &str, object: &V) -> Result<()> {
    let o = map(object)?;
    let collection = text(&map(&o["authored"])?["collection"])?;
    map_mut(
        map_mut(document)?
            .entry(collection.into())
            .or_insert_with(empty),
    )?
    .insert(subject.into(), o["body"].clone());
    Ok(())
}
fn remove(document: &mut V, subject: &str) -> Result<()> {
    for name in F::collections(document)?.keys() {
        map_mut(map_mut(document)?.get_mut(name).unwrap())?.remove(subject);
    }
    Ok(())
}
pub(crate) struct Builder<'a> {
    pub(crate) captured: &'a dyn Input,
    pub(crate) specs: &'a BTreeMap<Key, Spec>,
    pub(crate) mapped: &'a BTreeMap<Key, Key>,
    pub(crate) created: BTreeMap<Key, V>,
    pub(crate) visiting: BTreeSet<Key>,
    pub(crate) survivor: &'a str,
    pub(crate) retired: &'a str,
    pub(crate) options: &'a Options,
}
impl Builder<'_> {
    pub(crate) fn build(&mut self, key: &Key) -> Result<V> {
        let mut todo = vec![(key.clone(), false)];
        let mut visits = 0usize;
        while let Some((current, ready)) = todo.pop() {
            if self.created.contains_key(&current) {
                continue;
            }
            if ready {
                let object = self.construct(&current)?;
                self.created.insert(current.clone(), object);
                self.visiting.remove(&current);
                continue;
            }
            visits += 1;
            require(visits <= 100_000, "history_limit")?;
            require(self.visiting.insert(current.clone()), "identity_pin_cycle")?;
            todo.push((current.clone(), true));
            let pins = map(&map(&self.specs[&current].prior)?["pins"])?;
            for old in pins.values().rev() {
                let old = text(old)?;
                if let Some(mapped) = self
                    .mapped
                    .get(&(current.0.clone(), old.into()))
                    .or_else(|| self.mapped.get(&(String::new(), old.into())))
                {
                    todo.push((mapped.clone(), false));
                }
            }
        }
        self.created
            .get(key)
            .cloned()
            .ok_or_else(|| error("identity_pin_cycle"))
    }
    fn construct(&self, key: &Key) -> Result<V> {
        let spec = &self.specs[key];
        let prior = map(&spec.prior)?;
        let mut pins = Map::new();
        for (dep, version) in map(&prior["pins"])? {
            let target = if dep == self.retired {
                self.survivor
            } else {
                dep
            };
            if dep == self.retired
                && let Some(other) = map(&prior["pins"])?.get(self.survivor)
            {
                require(
                    claim_meaning(map(self.captured.object(text(other)?)?)?)?
                        == claim_meaning(map(self.captured.object(text(version)?)?)?)?,
                    "identity_pin_collision",
                )?;
            }
            let old = text(version)?;
            let mapped = self
                .mapped
                .get(&(key.0.clone(), old.into()))
                .or_else(|| self.mapped.get(&(String::new(), old.into())))
                .cloned();
            require(
                dep != self.retired || mapped.is_some(),
                "historical_alias_pin_requires_resolution",
            )?;
            let result = if let Some(mapped) = mapped {
                map(self
                    .created
                    .get(&mapped)
                    .ok_or_else(|| error("identity_pin_cycle"))?)?["id"]
                    .clone()
            } else {
                version.clone()
            };
            require(
                pins.get(target).is_none_or(|v| *v == result),
                "identity_pin_collision",
            )?;
            pins.insert(target.into(), result);
        }
        let mut gaps = Map::new();
        if let Some(v) = prior.get("pin_gaps") {
            for (dep, reason) in map(v)? {
                let target = if dep == self.retired {
                    self.survivor
                } else {
                    dep
                };
                require(
                    !pins.contains_key(target) && gaps.get(target).is_none_or(|v| v == reason),
                    "identity_pin_collision",
                )?;
                gaps.insert(target.into(), reason.clone());
            }
        }
        let mut body = spec.body.clone();
        let deps_field = text(&map(&map(&spec.authored)?["fields"])?["deps"])?;
        if let V::Map(b) = &mut body
            && let Some(deps) = b.get_mut(deps_field)
        {
            let self_dep = match deps {
                V::List(v) => v.contains(&s(&key.1)),
                V::Map(v) => v.contains_key(&key.1),
                _ => false,
            };
            require(!self_dep, "identity_self_dependency")?;
            if let V::Map(deps) = deps {
                for (dep, v) in deps {
                    if let Some(pin) = pins.get(dep) {
                        *v = pin.clone();
                    }
                }
            }
        }
        let object = A::make_object(
            &key.1,
            text(&prior["kind"])?,
            body,
            strings(self.captured.saw(&key.1)?),
            Some(spec.authored.clone()),
            V::Map(pins),
            V::Map(gaps),
            self.options,
        )?;
        Ok(object)
    }
}
fn same(
    captured: &dyn Input,
    context: &HA::Context,
    own: &Own,
    intent: &Intent,
    options: &Options,
    runtime: Option<&Runtime>,
) -> Result<(Vec<V>, V, V)> {
    let (survivor, retired) = survivor(intent)?;
    require(
        !is_distinct(context, &intent.a, &intent.b)?,
        "identities_declared_distinct",
    )?;
    let fields = F::snapshot_fields(&context.base)?;
    let template = captured.template()?;
    require(
        !R::mentions(&Source::from_typed(&template), retired, survivor, &fields)?,
        "identity_template_requires_migration",
    )?;
    let mut specs = BTreeMap::new();
    let mut mapped = BTreeMap::new();
    let mut worlds = BTreeMap::new();
    for (name, versions) in own {
        let document = if name.is_empty() {
            context.base.clone()
        } else {
            HA::layer(&context.base, &context.groups, std::slice::from_ref(name))?
        };
        let mut world = HA::world(&document, None, runtime)?;
        let changed_head = if name.is_empty() {
            None
        } else {
            Some(R::body(
                &Source::from_typed(&map(&map(&context.groups)?[name])?["head"]),
                retired,
                survivor,
                &fields,
            )?)
        };
        for (subject, ids) in versions {
            let mut prior = select_prior(captured, ids)?.clone();
            let target = if subject == retired {
                survivor
            } else {
                subject
            };
            if subject == survivor && versions.contains_key(retired) {
                continue;
            }
            let mut body = captured.source_body(&prior)?;
            let mut authored = map(&prior)?["authored"].clone();
            if subject == retired {
                let kept_ids = versions.get(survivor).or_else(|| own[""].get(survivor));
                let kept = kept_ids
                    .map(|ids| select_prior(captured, ids))
                    .transpose()?;
                let kept_body = kept
                    .map(|o| captured.source_body(o))
                    .transpose()?
                    .unwrap_or(Source::Map(vec![]));
                let (merged, taking) = M::merge(survivor, retired, &kept_body, &body, &mut world)?;
                body = merged;
                if let Some(kept) = kept {
                    if !taking {
                        prior = kept.clone();
                    }
                    authored = map(&prior)?["authored"].clone();
                    map_mut(&mut authored)?.insert(
                        "collection".into(),
                        map(&map(kept)?["authored"])?["collection"].clone(),
                    );
                }
            }
            let aliases = if target == survivor {
                body.get("also").map(Source::typed)
            } else {
                None
            };
            let mut rewritten = R::body(&body, retired, survivor, &fields)?;
            if target == survivor {
                let mut al = match aliases {
                    Some(V::Text(v)) => vec![s(&v)],
                    Some(V::List(v)) => v,
                    Some(V::Map(v)) => v.keys().map(|v| s(v)).collect(),
                    Some(V::Null) | None => vec![],
                    Some(v) if !truth(&v) => vec![],
                    _ => return Err(error("invalid_identity_aliases")),
                };
                al.retain(|v| !string_is(v, survivor));
                al.push(s(retired));
                let mut unique = vec![];
                for v in al {
                    require(
                        !matches!(v, V::Map(_) | V::List(_)),
                        "invalid_identity_aliases",
                    )?;
                    if !unique
                        .iter()
                        .any(|old| crate::source_clock::python_equal(old, &v))
                    {
                        unique.push(v);
                    }
                }
                map_mut(&mut rewritten)
                    .map_err(|_| error("identity_requires_mapping_body"))?
                    .insert("also".into(), V::List(unique));
            }
            if let Some(head) = &changed_head {
                map_mut(&mut authored)?.insert(
                    "hypothesis".into(),
                    obj([
                        ("version", n("1")),
                        ("name", s(name)),
                        ("head", head.clone()),
                    ]),
                );
            } else {
                map_mut(&mut authored)?.remove("hypothesis");
            }
            if target != subject
                || rewritten.digest()? != map(&prior)?["body"].digest()?
                || authored.digest()? != map(&prior)?["authored"].digest()?
            {
                let key = (name.clone(), target.into());
                let mut olds = ids.clone();
                if subject == retired {
                    olds.extend(versions.get(survivor).cloned().unwrap_or_default());
                    olds.sort();
                    olds.dedup();
                }
                for old in &olds {
                    mapped.insert((name.clone(), old.clone()), key.clone());
                }
                specs.insert(
                    key,
                    Spec {
                        body: rewritten,
                        authored,
                        prior,
                        olds,
                    },
                );
            }
        }
        worlds.insert(name.clone(), document);
    }
    loop {
        let mut changed = false;
        for (name, versions) in own {
            for (subject, ids) in versions {
                let key = (name.clone(), subject.clone());
                if subject == retired || specs.contains_key(&key) {
                    continue;
                }
                let prior = select_prior(captured, ids)?.clone();
                let m = map(&prior)?;
                if map(&m["pins"])?.values().any(|v| {
                    text(v).is_ok_and(|v| {
                        mapped.contains_key(&(name.clone(), v.into()))
                            || mapped.contains_key(&(String::new(), v.into()))
                    })
                }) {
                    specs.insert(
                        key.clone(),
                        Spec {
                            body: m["body"].clone(),
                            authored: m["authored"].clone(),
                            prior,
                            olds: ids.clone(),
                        },
                    );
                    for old in ids {
                        mapped.insert((name.clone(), old.clone()), key.clone());
                    }
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut builder = Builder {
        captured,
        specs: &specs,
        mapped: &mapped,
        created: BTreeMap::new(),
        visiting: BTreeSet::new(),
        survivor,
        retired,
        options,
    };
    for key in specs.keys() {
        builder.build(key)?;
    }
    let created = builder.created;
    let mut objects = created.values().cloned().collect::<Vec<_>>();
    for (key, object) in &created {
        let spec = &specs[key];
        let olds = spec
            .olds
            .iter()
            .filter(|id| {
                captured
                    .object(id)
                    .is_ok_and(|o| map(o).is_ok_and(|o| string_is(&o["subject"], &key.1)))
            })
            .cloned()
            .collect::<Vec<_>>();
        let extra = vec![text(&map(object)?["id"])?.into()];
        objects.push(act(
            captured,
            object,
            if key.0.is_empty() {
                "accept"
            } else {
                "propose"
            },
            &format!("explicit same {retired} as {survivor}"),
            strings(if key.0.is_empty() {
                olds.clone()
            } else {
                vec![]
            }),
            &extra,
            None,
            options,
        )?);
        if !key.0.is_empty() {
            for old in olds {
                objects.push(act(
                    captured,
                    captured.object(&old)?,
                    "retire",
                    "identity migration within named hypothesis",
                    strings(vec![]),
                    &extra,
                    None,
                    options,
                )?);
            }
        }
    }
    for versions in own.values() {
        if let Some(olds) = versions.get(retired) {
            for old in olds {
                objects.push(act(
                    captured,
                    captured.object(&old)?,
                    "retire",
                    &format!("explicit alias of {survivor}"),
                    strings(vec![]),
                    &[],
                    None,
                    options,
                )?);
            }
        }
    }
    let mut unique = BTreeMap::new();
    for o in objects {
        unique.insert(text(&map(&o)?["id"])?.to_owned(), o);
    }
    let mut after = context.base.clone();
    remove(&mut after, retired)?;
    for ((name, subject), o) in &created {
        if name.is_empty() {
            put(&mut after, subject, o)?;
        }
    }
    require(
        errors(&after, runtime)?.is_subset(&errors(&context.base, runtime)?),
        "identity_computation_regression",
    )?;
    for (name, before) in &worlds {
        if name.is_empty() {
            continue;
        }
        let mut candidate = before.clone();
        remove(&mut candidate, retired)?;
        for ((group, subject), o) in &created {
            if group.is_empty() || group == name {
                put(&mut candidate, subject, o)?;
            }
        }
        require(
            errors(&candidate, runtime)?.is_subset(&errors(before, runtime)?),
            "identity_computation_regression",
        )?;
    }
    Ok((unique.into_values().collect(), template, after))
}
fn distinct(
    captured: &dyn Input,
    context: &HA::Context,
    own: &Own,
    intent: &Intent,
    options: &Options,
    runtime: Option<&Runtime>,
) -> Result<(Vec<V>, V, V)> {
    let Action::Distinct { because } = &intent.action else {
        return Err(error("invalid_identity_receipt"));
    };
    require(!because.trim().is_empty(), "identity_reason_required")?;
    require(
        !is_distinct(context, &intent.a, &intent.b)?,
        "identities_already_distinct",
    )?;
    let mut objects = vec![];
    let mut after = context.base.clone();
    for (name, versions) in own {
        let Some(ids) = versions.get(&intent.a) else {
            continue;
        };
        let prior = select_prior(captured, ids)?;
        let m = map(prior)?;
        let mut body = m["body"].clone();
        let bodymap = map_mut(&mut body).map_err(|_| error("identity_requires_mapping_body"))?;
        let mut others = M::ids(bodymap.get("distinct_from").unwrap_or(&V::Null));
        others.push(intent.b.clone());
        let mut unique = vec![];
        for v in others {
            if !unique.contains(&v) {
                unique.push(v);
            }
        }
        bodymap.insert("distinct_from".into(), s(&unique.join(", ")));
        let object = A::make_object(
            &intent.a,
            text(&m["kind"])?,
            body,
            strings(captured.saw(&intent.a)?),
            Some(m["authored"].clone()),
            m["pins"].clone(),
            m.get("pin_gaps").cloned().unwrap_or_else(empty),
            options,
        )?;
        let extra = vec![text(&map(&object)?["id"])?.into()];
        objects.push(act(
            captured,
            &object,
            if name.is_empty() { "accept" } else { "propose" },
            because,
            strings(if name.is_empty() { ids.clone() } else { vec![] }),
            &extra,
            None,
            options,
        )?);
        if name.is_empty() {
            put(&mut after, &intent.a, &object)?;
        } else {
            for old in ids {
                objects.push(act(
                    captured,
                    captured.object(&old)?,
                    "retire",
                    "explicit distinct metadata update",
                    strings(vec![]),
                    &extra,
                    None,
                    options,
                )?);
            }
        }
        objects.push(object);
    }
    require(
        errors(&after, runtime)?.is_subset(&errors(&context.base, runtime)?),
        "identity_computation_regression",
    )?;
    Ok((objects, captured.template()?, after))
}

#[allow(clippy::too_many_arguments)]
fn act(
    input: &dyn Input,
    target: &V,
    kind: &str,
    because: &str,
    over: V,
    extra: &[String],
    read: Option<V>,
    options: &Options,
) -> Result<V> {
    let target = map(target)?;
    let subject = text(&target["subject"])?;
    let mut saw = input.saw(subject)?.into_iter().collect::<BTreeSet<_>>();
    saw.extend(extra.iter().cloned());
    let mut body = obj([
        ("act", s(kind)),
        ("of", target["id"].clone()),
        ("over", over),
        ("because", s(because)),
    ]);
    if let Some(read) = read {
        map_mut(&mut body)?.insert("read".into(), read);
    }
    A::make_object(
        subject,
        "act",
        body,
        strings(saw),
        None,
        empty(),
        empty(),
        options,
    )
}

pub(crate) fn validate(
    input: &dyn Input,
    context: &HA::Context,
    intent: &Intent,
    options: &Options,
) -> Result<()> {
    if let Some(day) = &intent.as_of {
        require(
            Date::new(day).is_ok() && day.len() == 10,
            "invalid_identity_as_of",
        )?;
        if !options.recording_day.is_empty() {
            require(day <= &options.recording_day, "invalid_identity_as_of")?;
        }
    }
    require(options.strict, "explicit_root_disposition_required")?;
    require(
        context.physical.is_empty(),
        "identity_physical_hypotheses_require_import",
    )?;
    let cap = F::capabilities(&context.base, None)?;
    let fields = F::snapshot_fields(&context.base)?;
    for (name, group) in map(&context.groups)? {
        require(
            !map(group)?.get("error").is_some_and(truth),
            "unresolved_history_hypothesis",
        )?;
        for versions in map(&map(&map(&context.index)?["groups"])?[name])?.values() {
            for version in list(versions)? {
                let authored = map(&map(input.object(text(version)?)?)?["authored"])?;
                require(
                    authored["profile"] == map(&cap)?["profile"],
                    "identity_mixed_group_profile",
                )?;
                let source = map(&authored["fields"])?;
                require(
                    ["deps", "snapshot", "predicate"]
                        .iter()
                        .all(|key| source.get(*key) == fields.get(*key)),
                    "identity_mixed_group_fields",
                )?;
            }
        }
    }
    require(
        intent.a != intent.b
            && !F::BUILTINS.contains(&intent.a.as_str())
            && !F::BUILTINS.contains(&intent.b.as_str()),
        "invalid_identity_subjects",
    )?;
    Ok(())
}

pub(crate) struct Plan {
    pub objects: Vec<V>,
    pub template: V,
    pub after_document: V,
}
pub(crate) fn plan(
    input: &dyn Input,
    context: &HA::Context,
    intent: &Intent,
    options: &Options,
    runtime: Option<&Runtime>,
) -> Result<Plan> {
    validate(input, context, intent, options)?;
    let own = own(input, context)?;
    require(
        [&intent.a, &intent.b]
            .iter()
            .all(|id| own.values().any(|m| m.contains_key(*id))),
        "identity_subject_unavailable",
    )?;
    let (objects, template, after_document) = match intent.action {
        Action::Same { .. } => same(input, context, &own, intent, options, runtime)?,
        Action::Distinct { .. } => distinct(input, context, &own, intent, options, runtime)?,
    };
    Ok(Plan {
        objects,
        template,
        after_document,
    })
}

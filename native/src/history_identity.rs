//! Explicit identity intents over immutable current claims and named proposals.
use crate::history_reduce::claim_meaning;
use crate::{
    Result, history_adapter as D,
    history_authoring::{self as A, Options, empty, n, obj, s, strings},
    history_authoring_audit::ReplayAudit,
    history_capture::Capture,
    history_contract::*,
    history_hypotheses as HH, history_hypothesis_authoring as HA, history_identity_merge as M,
    history_identity_rewrite as R, history_preparation as Preparation,
    history_store::Store,
    history_transaction::{self as T, FileImage, PreparedMutation},
    history_transaction_fs as FS,
    history_view::{self as View, list, map_mut, truth},
    history_yaml::{self as Y, SourceValue as Source},
    identity::sha256,
    reasoning_fields as F,
    reasoning_runtime::Runtime,
    reasoning_snapshot::entries,
    require,
    value::{Date, TypedValue as V},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub enum Action {
    Same { keep: Option<String> },
    Distinct { because: String },
}
#[derive(Clone)]
pub struct Intent {
    pub a: String,
    pub b: String,
    pub action: Action,
    pub as_of: Option<String>,
}
type Key = (String, String);
type Own = BTreeMap<String, BTreeMap<String, Vec<String>>>;
struct Spec {
    body: V,
    authored: V,
    prior: V,
    olds: Vec<String>,
}
fn source_body(capture: &Capture, object: &V) -> Result<Source> {
    let m = map(object)?;
    let key = (text(&m["subject"])?.into(), text(&m["id"])?.into());
    let raw = capture
        .object_bytes
        .get(&key)
        .ok_or_else(|| error("incomplete_closure"))?;
    Y::decode_source_document(raw)?
        .get("body")
        .cloned()
        .ok_or_else(|| error("invalid_object"))
}
fn prior<'a>(captured: &'a Capture, ids: &[String]) -> Result<&'a V> {
    ids.iter()
        .min()
        .and_then(|id| captured.objects.get(id))
        .ok_or_else(|| error("incomplete_closure"))
}
fn own(captured: &Capture, context: &HA::Context) -> Result<Own> {
    let mut base = BTreeMap::new();
    for subject in entries(&context.base)?.keys() {
        base.insert(
            subject.clone(),
            list(&map(&map(&map(&captured.state)?["subjects"])?[subject])?["heads"])?
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
fn context(
    store: &Store,
    captured: &Capture,
    intent: &Intent,
    options: &Options,
) -> Result<HA::Context> {
    if let Some(day) = &intent.as_of {
        require(
            Date::new(day).is_ok() && day.len() == 10,
            "invalid_identity_as_of",
        )?;
        // The operation boundary captures today's day independently of computation time.
        if !options.recording_day.is_empty() {
            require(day <= &options.recording_day, "invalid_identity_as_of")?;
        }
    }
    let context = HA::capture(store, captured, options)?;
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
                let authored = map(&map(&captured.objects[text(version)?])?["authored"])?;
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
    Ok(context)
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
struct Builder<'a> {
    captured: &'a Capture,
    specs: &'a BTreeMap<Key, Spec>,
    mapped: &'a BTreeMap<Key, Key>,
    created: BTreeMap<Key, V>,
    visiting: BTreeSet<Key>,
    survivor: &'a str,
    retired: &'a str,
    options: &'a Options,
}
impl Builder<'_> {
    fn build(&mut self, key: &Key) -> Result<V> {
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
                    claim_meaning(map(&self.captured.objects[text(other)?])?)?
                        == claim_meaning(map(&self.captured.objects[text(version)?])?)?,
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
            strings(A::saw(self.captured, &key.1)?),
            Some(spec.authored.clone()),
            V::Map(pins),
            V::Map(gaps),
            self.options,
        )?;
        Ok(object)
    }
}
fn same(
    captured: &Capture,
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
    let template = View::template(&captured.commits)?;
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
            let mut prior = prior(captured, ids)?.clone();
            let target = if subject == retired {
                survivor
            } else {
                subject
            };
            if subject == survivor && versions.contains_key(retired) {
                continue;
            }
            let mut body = source_body(captured, &prior)?;
            let mut authored = map(&prior)?["authored"].clone();
            if subject == retired {
                let kept_ids = versions.get(survivor).or_else(|| own[""].get(survivor));
                let kept = kept_ids.map(|ids| self::prior(captured, ids)).transpose()?;
                let kept_body = kept
                    .map(|o| source_body(captured, o))
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
                let prior = prior(captured, ids)?.clone();
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
                map(&captured.objects[*id]).is_ok_and(|o| string_is(&o["subject"], &key.1))
            })
            .cloned()
            .collect::<Vec<_>>();
        let extra = vec![text(&map(object)?["id"])?.into()];
        objects.push(HA::act(
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
                objects.push(HA::act(
                    captured,
                    &captured.objects[&old],
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
                objects.push(HA::act(
                    captured,
                    &captured.objects[old],
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
    captured: &Capture,
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
        let prior = prior(captured, ids)?;
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
            strings(A::saw(captured, &intent.a)?),
            Some(m["authored"].clone()),
            m["pins"].clone(),
            m.get("pin_gaps").cloned().unwrap_or_else(empty),
            options,
        )?;
        let extra = vec![text(&map(&object)?["id"])?.into()];
        objects.push(HA::act(
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
                objects.push(HA::act(
                    captured,
                    &captured.objects[old],
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
    Ok((objects, View::template(&captured.commits)?, after))
}
pub fn prepare(
    store: &Store,
    captured: &Capture,
    intent: &Intent,
    options: &Options,
    runtime: Option<&Runtime>,
) -> Result<PreparedMutation> {
    let mut options = options.clone();
    options.recording_day = crate::source_clock::latest_day();
    prepare_inner(store, captured, intent, &options, runtime, None, None, None)
}
#[allow(clippy::too_many_arguments)]
fn prepare_inner(
    store: &Store,
    captured: &Capture,
    intent: &Intent,
    options: &Options,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
    brief_before: Option<Option<&[u8]>>,
    receipt_version: Option<u8>,
) -> Result<PreparedMutation> {
    let context = context(store, captured, intent, options)?;
    let own = own(captured, &context)?;
    require(
        [&intent.a, &intent.b]
            .iter()
            .all(|id| own.values().any(|m| m.contains_key(*id))),
        "identity_subject_unavailable",
    )?;
    let (objects, mut template, after) = match intent.action {
        Action::Same { .. } => same(captured, &context, &own, intent, options, runtime)?,
        Action::Distinct { .. } => distinct(captured, &context, &own, intent, options, runtime)?,
    };
    let archive = A::archive(store)?;
    let physical = HA::physical_evidence(store)?;
    let observed = FS::read(&FS::target(&store.root, &store.layout.view)?)?;
    let view = brief_before.unwrap_or(observed.as_deref());
    let view_after = if matches!(intent.action, Action::Same { .. }) {
        let (survivor, retired) = survivor(intent)?;
        crate::history_identity_brief::rewrite(
            view,
            retired,
            survivor,
            &F::snapshot_fields(&context.base)?,
        )?
    } else {
        view.map(Vec::from)
    };
    let changed = view_after.as_deref() != view;
    require(
        receipt_version != Some(1) || !changed,
        "identity_brief_requires_migration",
    )?;
    require(
        receipt_version.is_none_or(|v| v == 1 || v == 2) && (receipt_version != Some(2) || changed),
        "invalid_identity_receipt",
    )?;
    let mut data = obj([
        ("version", n(if changed { "2" } else { "1" })),
        (
            "kind",
            s(if matches!(intent.action, Action::Same { .. }) {
                "same"
            } else {
                "distinct"
            }),
        ),
        ("a", s(&intent.a)),
        ("b", s(&intent.b)),
        ("by", options.by.clone()),
        ("operation", s(&options.operation)),
        ("recorded_at", s(&options.recorded_at)),
    ]);
    match &intent.action {
        Action::Same { keep } => {
            map_mut(&mut data)?.insert("keep".into(), keep.as_deref().map(s).unwrap_or(V::Null));
        }
        Action::Distinct { because } => {
            map_mut(&mut data)?.insert("because".into(), s(because));
        }
    }
    if let Some(day) = &intent.as_of {
        map_mut(&mut data)?.insert("as_of".into(), s(day));
    }
    let mut extra = vec![];
    if changed {
        let raw = view.ok_or_else(|| error("invalid_brief_encoding"))?;
        let next = view_after.clone().unwrap();
        let utf8 = std::str::from_utf8(raw).map_err(|_| error("invalid_brief_encoding"))?;
        map_mut(&mut data)?.insert(
            "brief".into(),
            obj([
                ("path", s(&store.layout.view)),
                ("before_utf8", s(utf8)),
                ("before_sha256", s(&sha256(raw))),
                ("after_sha256", s(&sha256(&next))),
            ]),
        );
        extra.push(FileImage {
            role: "view".into(),
            path: store.layout.view.clone(),
            before: Some(raw.to_vec()),
            after: Some(next),
        });
    }
    map_mut(&mut data)?.extend([
        ("baseline".into(), captured.baseline.clone()),
        ("archive".into(), archive.clone()),
        ("physical".into(), physical.clone()),
        (
            "view_sha256".into(),
            view.map(|v| s(&sha256(v))).unwrap_or(V::Null),
        ),
    ]);
    let mut before = A::evidence(
        &context.base,
        &mut HA::world(&context.base, None, runtime)?,
        audit,
    )?;
    map_mut(&mut before)?.insert("identity_authoring".into(), data);
    let mut after_evidence = A::evidence(&after, &mut HA::world(&after, None, runtime)?, audit)?;
    let mut output = obj([(
        "objects",
        strings(
            objects
                .iter()
                .map(|o| text(&map(o).unwrap()["id"]).unwrap().to_owned())
                .collect::<BTreeSet<_>>(),
        ),
    )]);
    if changed {
        map_mut(&mut output)?.insert(
            "view_sha256".into(),
            s(&sha256(view_after.as_ref().unwrap())),
        );
    }
    map_mut(&mut after_evidence)?.insert("identity_authoring".into(), output);
    let cap = F::capabilities(&context.base, None)?;
    let receipt = T::semantic_receipt(
        text(&map(&cap)?["profile"])?,
        &cap,
        &before,
        &after_evidence,
    )?;
    for o in &objects {
        let o = map(o)?;
        if !string_is(&o["kind"], "act") {
            map_mut(&mut template)?
                .entry(text(&map(&o["authored"])?["collection"])?.into())
                .or_insert_with(empty);
        }
    }
    let result = Preparation::prepare_commit_with_files(
        captured,
        &options.operation,
        &objects,
        &template,
        &receipt,
        options.requires().as_ref(),
        &extra,
    )?;
    let candidate = A::candidate(captured, &result)?;
    let adapted = D::from_store_capture(&candidate)?;
    HH::layers(adapted.projection(), adapted.document())?;
    require(
        A::archive(store)? == archive
            && HA::physical_evidence(store)? == physical
            && FS::read(&FS::target(&store.root, &store.layout.view)?)? == observed,
        "identity_source_changed",
    )?;
    Ok(result)
}
pub(crate) fn sources(store: &Store, mutation: &PreparedMutation) -> Result<()> {
    let data = mutation.to_data();
    let intent = map(field(
        map(&map(&map(&data)?["receipt"])?["before"])?,
        "identity_authoring",
    )?)?;
    require(
        A::archive(store)? == *field(intent, "archive")?
            && HA::physical_evidence(store)? == *field(intent, "physical")?,
        "identity_source_changed",
    )?;
    let view = FS::read(&FS::target(&store.root, &store.layout.view)?)?;
    let auxiliary = mutation.auxiliary_view()?;
    if is_int(field(intent, "version")?, "2") {
        let a = auxiliary.ok_or_else(|| error("invalid_identity_receipt"))?;
        require(
            view == a.before || view == a.after,
            "identity_source_changed",
        )?;
    } else {
        require(
            is_int(field(intent, "version")?, "1") && auxiliary.is_none(),
            "invalid_identity_receipt",
        )?;
        require(
            view.map(|v| s(&sha256(&v))).unwrap_or(V::Null) == *field(intent, "view_sha256")?,
            "identity_source_changed",
        )?;
    }
    Ok(())
}
pub fn verify_prepared(
    store: &Store,
    mutation: &PreparedMutation,
    runtime: Option<&Runtime>,
) -> Result<()> {
    sources(store, mutation)?;
    let (capture, mut options, intent, audit) =
        HA::replay_context(store, mutation, "identity_authoring", true)?;
    options.recording_day = crate::source_clock::latest_day();
    let i = map(&intent)?;
    let version = field(i, "version")?;
    require(
        is_int(version, "1") || is_int(version, "2"),
        "invalid_identity_receipt",
    )?;
    let action = match text(field(i, "kind")?)? {
        "same" => Action::Same {
            keep: match field(i, "keep")? {
                V::Null => None,
                v => Some(text(v)?.into()),
            },
        },
        "distinct" => Action::Distinct {
            because: text(field(i, "because")?)?.into(),
        },
        _ => return Err(error("invalid_identity_receipt")),
    };
    let intent = Intent {
        a: text(field(i, "a")?)?.into(),
        b: text(field(i, "b")?)?.into(),
        action,
        as_of: i.get("as_of").map(text).transpose()?.map(str::to_owned),
    };
    let brief = mutation.auxiliary_view()?.map(|v| v.before.as_deref());
    let expected = prepare_inner(
        store,
        &capture,
        &intent,
        &options,
        runtime,
        Some(&audit),
        brief,
        Some(if is_int(version, "2") { 2 } else { 1 }),
    )?;
    require(
        expected.to_bytes()? == mutation.to_bytes()?,
        "identity_receipt_mismatch",
    )
}
pub fn commit(
    store: &Store,
    mutation: &PreparedMutation,
    runtime: Option<&Runtime>,
    verify: FS::Verify<'_>,
) -> Result<V> {
    store.commit_identity(mutation, runtime, verify)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde_json::Value as J;
    use std::path::Path;
    fn runtime(cache: &Path) -> Runtime {
        Runtime::open(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/reasoning/native")
                .join(format!(
                    "{}.kpopper-runtime",
                    crate::reasoning_runtime::target_name().unwrap()
                )),
            cache,
            crate::reasoning_runtime::OperationalBounds::default(),
        )
        .unwrap()
    }
    fn corpus() -> J {
        let mut base: J =
            serde_json::from_str(include_str!("../tests/fixtures/history-identity.json")).unwrap();
        let candidate: J = serde_json::from_str(include_str!(
            "../tests/fixtures/history-identity-candidate.json"
        ))
        .unwrap();
        base["cases"]
            .as_array_mut()
            .unwrap()
            .extend(candidate["cases"].as_array().unwrap().iter().cloned());
        let edges: J = serde_json::from_str(include_str!(
            "../tests/fixtures/history-identity-edges.json"
        ))
        .unwrap();
        base["cases"]
            .as_array_mut()
            .unwrap()
            .extend(edges["cases"].as_array().unwrap().iter().cloned());
        base
    }
    fn write(root: &Path, case: &J) {
        for (path, raw) in case["files"].as_object().unwrap() {
            let p = root.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, STANDARD.decode(raw.as_str().unwrap()).unwrap()).unwrap();
        }
    }
    fn intent(case: &J) -> Intent {
        Intent {
            a: case["a"].as_str().unwrap().into(),
            b: case["b"].as_str().unwrap().into(),
            action: if case["kind"] == "same" {
                Action::Same {
                    keep: case["keep"].as_str().map(str::to_owned),
                }
            } else {
                Action::Distinct {
                    because: case["because"].as_str().unwrap().into(),
                }
            },
            as_of: case["as_of"].as_str().map(str::to_owned),
        }
    }
    fn options(case: &J) -> Options {
        Options {
            operation: case["operation"].as_str().unwrap().into(),
            recorded_at: case["recorded_at"].as_str().unwrap().into(),
            recording_day: case["today"].as_str().unwrap().into(),
            by: V::from_json(&case["by"]).unwrap(),
            strict: true,
            paths: crate::history_paths::Scheme::Hashed,
            receipt_version: None,
        }
    }
    #[test]
    fn identity_actions_match_python_mutation_bytes() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let mut failures = vec![];
        for case in corpus()["cases"].as_array().unwrap() {
            let root = tempfile::tempdir().unwrap();
            write(root.path(), case);
            let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
            let audit = case
                .get("receipt")
                .map(|r| ReplayAudit::oracle(&V::from_tagged(r).unwrap()).unwrap());
            let result = if !case["as_of"].is_null() && !case["as_of"].is_string() {
                Err(error("invalid_identity_as_of"))
            } else {
                store.capture().and_then(|c| {
                    prepare_inner(
                        &store,
                        &c,
                        &intent(case),
                        &options(case),
                        Some(&runtime),
                        audit.as_ref(),
                        None,
                        None,
                    )
                })
            };
            match (case.get("output"), result) {
                (Some(expected), Ok(actual)) => {
                    if actual.to_bytes().unwrap() != expected.as_str().unwrap().as_bytes() {
                        std::fs::write(
                            std::env::temp_dir().join(format!(
                                "identity-{}-actual.json",
                                case["operation"].as_str().unwrap()
                            )),
                            actual.to_bytes().unwrap(),
                        )
                        .unwrap();
                        failures.push(format!("{}: bytes differ", case["name"]));
                    }
                }
                (None, Err(_)) => {}
                (Some(_), Err(e)) => {
                    failures.push(format!("{}: unexpected refusal {}", case["name"], e))
                }
                (None, Ok(_)) => failures.push(format!("{}: unexpected acceptance", case["name"])),
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
    fn selected(prefix: &str) -> J {
        corpus()["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"].as_str().unwrap().starts_with(prefix))
            .unwrap()
            .clone()
    }
    #[test]
    fn identity_replay_retains_originals_and_requires_fresh_runtime() {
        let case = selected("same_rewrites");
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &case);
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
        let before = store.capture().unwrap();
        let mutation = prepare(
            &store,
            &before,
            &intent(&case),
            &options(&case),
            Some(&runtime),
        )
        .unwrap();
        assert_eq!(store.capture().unwrap().inventory, before.inventory);
        assert!(verify_prepared(&store, &mutation, None).is_err());
        assert_eq!(
            store.commit(&mutation, &mut |_| Ok(())).unwrap_err().0,
            "unsupported_identity_replay"
        );
        commit(&store, &mutation, Some(&runtime), &mut |_| Ok(())).unwrap();
        let after = store.capture().unwrap();
        for (k, v) in &before.object_bytes {
            assert_eq!(after.object_bytes.get(k), Some(v));
        }
        assert_eq!(
            map(&map(&map(&after.state).unwrap()["subjects"]).unwrap()["p.other"]).unwrap()["acceptance"],
            s("retired")
        );
        commit(&store, &mutation, Some(&runtime), &mut |_| Ok(())).unwrap();
        assert_eq!(
            store.capture().unwrap().commits.len(),
            before.commits.len() + 1
        );
    }
    #[test]
    fn identity_auxiliary_recovery_covers_each_publication_frontier() {
        let case = selected("brief_references_join");
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        for phase in 0..4 {
            let root = tempfile::tempdir().unwrap();
            write(root.path(), &case);
            let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
            let before = store.capture().unwrap();
            let mutation = prepare(
                &store,
                &before,
                &intent(&case),
                &options(&case),
                Some(&runtime),
            )
            .unwrap();
            let auxiliary = mutation.auxiliary_view().unwrap().unwrap();
            {
                let _lock = FS::DirectoryGuard::acquire(&store.root, true).unwrap();
                let _owner =
                    FS::auxiliary_owner(&store.root, &store.layout.journal, &mutation).unwrap();
                FS::publish_auxiliary_journal(&store.root, &store.layout.journal, &mutation)
                    .unwrap();
                FS::check_member_journals(std::slice::from_ref(&store.entry)).unwrap();
                if phase > 0 {
                    for f in mutation
                        .files()
                        .iter()
                        .filter(|f| f.role == "history_object" || f.role == "history_commit")
                    {
                        FS::publish_immutable(&store.root, &f.path, f.after.as_ref().unwrap())
                            .unwrap();
                    }
                }
                if phase > 1 {
                    let record = mutation
                        .files()
                        .iter()
                        .find(|f| f.role == "record")
                        .unwrap();
                    FS::replace(&store.entry, record.after.as_deref()).unwrap();
                }
                if phase > 2 {
                    FS::replace(
                        &store.root.join(&auxiliary.path),
                        auxiliary.after.as_deref(),
                    )
                    .unwrap();
                }
            }
            assert_eq!(store.capture().unwrap_err().0, "recovery_required");
            assert_eq!(
                FS::check_member_journals(std::slice::from_ref(&store.entry))
                    .unwrap_err()
                    .0,
                "recovery_required"
            );
            if phase == 0 {
                store
                    .recover_auxiliary("before", Some(&runtime), &mut |_| Ok(()))
                    .unwrap();
                assert_eq!(store.capture().unwrap().inventory, before.inventory);
            } else {
                assert_eq!(
                    store
                        .recover_auxiliary("before", Some(&runtime), &mut |_| Ok(()))
                        .unwrap_err()
                        .0,
                    "history_already_committed"
                );
                store
                    .recover_auxiliary("after", Some(&runtime), &mut |_| Ok(()))
                    .unwrap();
                assert_eq!(
                    std::fs::read(store.root.join(&auxiliary.path)).unwrap(),
                    *auxiliary.after.as_ref().unwrap()
                );
                assert_eq!(
                    store.capture().unwrap().commits.len(),
                    before.commits.len() + 1
                );
                verify_prepared(&store, &mutation, Some(&runtime)).unwrap();
            }
            assert!(!store.root.join(&store.layout.journal).exists());
        }
    }
    #[test]
    fn identity_final_callback_failure_keeps_recoverable_guard() {
        let case = selected("brief_references_join");
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &case);
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
        let before = store.capture().unwrap();
        let mutation = prepare(
            &store,
            &before,
            &intent(&case),
            &options(&case),
            Some(&runtime),
        )
        .unwrap();
        let mut calls = 0;
        assert_eq!(
            commit(&store, &mutation, Some(&runtime), &mut |_| {
                calls += 1;
                if calls == 2 {
                    Err(error("callback_failed"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err()
            .0,
            "callback_failed"
        );
        assert_eq!(store.capture().unwrap_err().0, "recovery_required");
        store
            .recover_auxiliary("after", Some(&runtime), &mut |_| Ok(()))
            .unwrap();
        assert_eq!(
            store.capture().unwrap().commits.len(),
            before.commits.len() + 1
        );
    }
    #[test]
    fn identity_storage_rejects_rehashed_receipts_and_late_source_changes() {
        let case = selected("brief_references_join");
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &case);
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
        let before = store.capture().unwrap();
        let mutation = prepare(
            &store,
            &before,
            &intent(&case),
            &options(&case),
            Some(&runtime),
        )
        .unwrap();
        let data = mutation.to_data();
        let data = map(&data).unwrap();
        let old = map(&data["receipt"]).unwrap();
        let mut after = old["after"].clone();
        map_mut(&mut after)
            .unwrap()
            .insert("forged_evidence".into(), V::Bool(true));
        let receipt =
            T::semantic_receipt("core/v1", &old["capabilities"], &old["before"], &after).unwrap();
        let mut files = mutation.files().to_vec();
        for f in &mut files {
            if f.role == "history_commit" {
                let mut m = Y::decode_document(f.after.as_ref().unwrap()).unwrap();
                map_mut(&mut m)
                    .unwrap()
                    .insert("receipt".into(), receipt.clone());
                f.after = Some(crate::history_emit::encode_document(&m).unwrap());
            }
        }
        let forged = PreparedMutation::prepare(
            &options(&case).operation,
            &data["authority"],
            &data["baseline"],
            files,
            &receipt,
            "GROUNDING.yaml",
            None,
        )
        .unwrap();
        assert_eq!(
            store
                .commit_identity(&forged, Some(&runtime), &mut |_| Ok(()))
                .unwrap_err()
                .0,
            "identity_receipt_mismatch"
        );
        let archive = store.root.join(&store.layout.replaced);
        assert_eq!(
            commit(&store, &mutation, Some(&runtime), &mut |_| {
                std::fs::write(&archive, b"changed archive")?;
                Ok(())
            })
            .unwrap_err()
            .0,
            "identity_source_changed"
        );
        assert_eq!(std::fs::read(&store.entry).unwrap(), before.entry_bytes);
        assert!(!store.root.join(&store.layout.journal).exists());
        std::fs::remove_file(archive).unwrap();
        let brief = store.root.join(&store.layout.view);
        assert_eq!(
            commit(&store, &mutation, Some(&runtime), &mut |_| {
                std::fs::write(&brief, b"changed brief")?;
                Ok(())
            })
            .unwrap_err()
            .0,
            "identity_source_changed"
        );
        assert_eq!(store.capture().unwrap().commits.len(), before.commits.len());
    }

    #[test]
    fn identity_clock_cannot_be_overridden_by_recording_metadata_or_replay() {
        let case = selected("same_rewrites");
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &case);
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
        let captured = store.capture().unwrap();
        let mut intent = intent(&case);
        intent.as_of = Some("9999-12-31".into());
        let mut options = options(&case);
        options.recording_day = "9999-12-31".into();
        assert_eq!(
            prepare(&store, &captured, &intent, &options, Some(&runtime))
                .unwrap_err()
                .0,
            "invalid_identity_as_of"
        );
        options.recording_day.clear();
        assert_eq!(
            prepare(&store, &captured, &intent, &options, Some(&runtime))
                .unwrap_err()
                .0,
            "invalid_identity_as_of"
        );
        // Manufacture a semantically consistent receipt under a false test clock;
        // current replay must reject it even after all envelope hashes are rebuilt.
        options.recording_day = "9999-12-31".into();
        let forged = prepare_inner(
            &store,
            &captured,
            &intent,
            &options,
            Some(&runtime),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            store
                .commit_identity(&forged, Some(&runtime), &mut |_| Ok(()))
                .unwrap_err()
                .0,
            "invalid_identity_as_of"
        );
        assert_eq!(store.capture().unwrap().inventory, captured.inventory);
    }
    #[test]
    fn inferred_judgments_cannot_use_reading_date_supersession() {
        for deps in [empty(), V::List(vec![])] {
            let kept = obj([("v", n("1")), ("of", s("2026-01-01")), ("rests_on", deps)]);
            let removed = obj([("v", n("2")), ("of", s("2026-01-02"))]);
            let document = obj([
                (
                    "meta",
                    obj([(
                        "reasoning",
                        obj([
                            ("version", n("2")),
                            ("profile", s("core/v1")),
                            ("requires", strings(vec!["arithmetic/v1".into()])),
                        ]),
                    )]),
                ),
                (
                    "schema",
                    obj([
                        ("deps", s("rests_on")),
                        ("snapshot", s("seen")),
                        ("predicate", s("wrong_if")),
                    ]),
                ),
                (
                    "readings",
                    obj([("p.left", kept.clone()), ("p.right", removed.clone())]),
                ),
            ]);
            let mut world = HA::world(&document, None, None).unwrap();
            assert_eq!(
                M::merge(
                    "p.left",
                    "p.right",
                    &Source::from_typed(&kept),
                    &Source::from_typed(&removed),
                    &mut world
                )
                .unwrap_err()
                .0,
                "identity_conflicting_readings"
            );
        }
    }

    #[test]
    fn identity_pin_rebuild_handles_long_chains_without_call_stack_recursion() {
        let case = selected("same_rewrites");
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &case);
        let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
        let mut captured = store.capture().unwrap();
        let options = options(&case);
        let authored = obj([
            ("profile", s("core/v1")),
            ("collection", s("decisions")),
            (
                "fields",
                obj([
                    ("deps", s("rests_on")),
                    ("snapshot", s("seen")),
                    ("predicate", s("wrong_if")),
                    ("value", s("v")),
                ]),
            ),
        ]);
        let mut specs = BTreeMap::new();
        let mut mapped = BTreeMap::new();
        let mut previous: Option<(String, String)> = None;
        for i in 0..300 {
            let subject = format!("d.chain{i:03}");
            let key = (String::new(), subject.clone());
            let (deps, pins) = if let Some((id, version)) = &previous {
                (
                    strings(vec![id.clone()]),
                    V::Map(Map::from([(id.clone(), s(version))])),
                )
            } else {
                (strings(vec![]), empty())
            };
            let body = obj([("verdict", s("old")), ("rests_on", deps)]);
            let prior = A::make_object(
                &subject,
                "judgment",
                body.clone(),
                strings(vec![]),
                Some(authored.clone()),
                pins,
                empty(),
                &options,
            )
            .unwrap();
            let id = text(&map(&prior).unwrap()["id"]).unwrap().to_owned();
            captured.objects.insert(id.clone(), prior.clone());
            let mut changed = body;
            map_mut(&mut changed)
                .unwrap()
                .insert("verdict".into(), s("new"));
            specs.insert(
                key.clone(),
                Spec {
                    body: changed,
                    authored: authored.clone(),
                    prior,
                    olds: vec![id.clone()],
                },
            );
            mapped.insert((String::new(), id.clone()), key);
            previous = Some((subject, id));
        }
        let last = specs.keys().last().unwrap().clone();
        let mut builder = Builder {
            captured: &captured,
            specs: &specs,
            mapped: &mapped,
            created: BTreeMap::new(),
            visiting: BTreeSet::new(),
            survivor: "p.input",
            retired: "p.other",
            options: &options,
        };
        builder.build(&last).unwrap();
        assert_eq!(builder.created.len(), 300);
        assert!(builder.visiting.is_empty());
        for (key, value) in &builder.created {
            let prior = map(&specs[key].prior).unwrap();
            assert_ne!(map(value).unwrap()["id"], prior["id"]);
            for old in map(&prior["pins"]).unwrap().values() {
                let dependency = &mapped[&(String::new(), text(old).unwrap().into())];
                assert!(
                    map(&map(value).unwrap()["pins"])
                        .unwrap()
                        .values()
                        .any(|v| *v == map(&builder.created[dependency]).unwrap()["id"])
                );
            }
        }
    }
}

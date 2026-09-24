//! Shared core authoring before storage-specific receipt and publication encoding.
//! The provider exposes only selected objects and exact observations; it need not expand history.
use crate::{
    Result,
    history_authoring::{Options, destination, empty, make_object, n, obj, s, strings},
    history_authoring_audit::ReplayAudit,
    history_contract::*,
    history_transaction as T,
    history_view::{list, map_mut, truth},
    reasoning_authoring as Authoring, reasoning_fields as F,
    reasoning_runtime::Runtime,
    reasoning_snapshot::entries,
    require,
    value::TypedValue as V,
};
use std::collections::BTreeSet;

pub(crate) trait Input {
    fn document(&self) -> Result<V>;
    fn state(&self) -> &V;
    fn object(&self, id: &str) -> Result<&V>;
    fn saw(&self, subject: &str) -> Result<Vec<String>>;
    fn baseline(&self) -> &V;

    fn head(&self, subject: &str) -> Result<&V> {
        let state = map(&map(self.state())?["subjects"])?
            .get(subject)
            .ok_or_else(|| error("unresolved_history_subject"))?;
        let state = map(state)?;
        require(
            string_is(&state["acceptance"], "accepted") && state.contains_key("head"),
            "unresolved_history_subject",
        )?;
        self.object(text(&state["head"])?)
    }
    fn pins(&self, deps: &[V], missing: bool) -> Result<(V, V)> {
        let mut pins = Map::new();
        let mut gaps = Map::new();
        for dep in deps {
            let name = text(dep)?;
            if !map(&map(self.state())?["subjects"])?.contains_key(name) && missing {
                gaps.insert(name.into(), s("unavailable"));
            } else {
                pins.insert(name.into(), map(self.head(name)?)?["id"].clone());
            }
        }
        Ok((V::Map(pins), V::Map(gaps)))
    }
    fn accepted_versions(&self) -> Result<Map> {
        let mut versions = Map::new();
        for (subject, state) in map(&map(self.state())?["subjects"])? {
            let state = map(state)?;
            if string_is(&state["acceptance"], "accepted") {
                versions.insert(subject.clone(), state["head"].clone());
            }
        }
        Ok(versions)
    }
}

pub(crate) struct Plan {
    pub objects: Vec<V>,
    pub document: V,
    before_document: V,
    intent: V,
    notes: Vec<String>,
    version: u8,
    capabilities: V,
}

pub(crate) fn prepare(
    input: &dyn Input,
    action: &V,
    options: &Options,
    runtime: Option<&Runtime>,
) -> Result<Plan> {
    require(
        options.receipt_version.is_none_or(|v| [1, 7].contains(&v)),
        "invalid_authoring_receipt",
    )?;
    let mut action = action.clone();
    let a = map_mut(&mut action)?;
    let kind = text(field(a, "kind")?)?.to_owned();
    require(
        ["add", "set", "review"].contains(&kind.as_str()),
        "unsupported_history_action",
    )?;
    require(
        !a.get("hypothesis").is_some_and(truth) && !a.get("section").is_some_and(truth),
        "history_hypothesis_write_unsupported",
    )?;
    if !a.get("as_of").is_some_and(truth) {
        require(!options.recording_day.is_empty(), "missing_recording_time")?;
        a.insert("as_of".into(), s(&options.recording_day));
    }
    let intent = action.clone();
    let before_doc = input.document()?;
    let mut profiles = BTreeSet::new();
    for state in map(&map(input.state())?["subjects"])?.values() {
        if let Some(h) = map(state)?.get("head") {
            let object = input.object(text(h)?)?;
            profiles.insert(text(field(
                map(field(map(object)?, "authored")?)?,
                "profile",
            )?)?);
        }
    }
    require(profiles.len() <= 1, "incompatible_authored_profiles")?;
    let cap = F::capabilities(&before_doc, profiles.first().copied())?;

    require(
        map(&action)?
            .get("profile")
            .is_none_or(|v| *v == V::Null || *v == map(&cap).unwrap()["profile"]),
        "history_profile_migration_required",
    )?;
    Authoring::validate_declared(&before_doc)?;
    let mut before_world =
        crate::history_authoring_reader::AuthoringReader::new(&before_doc, runtime)?;
    let (action, notes) = before_world.normalize(&action)?;
    let refusals = before_world.validate(&action)?;
    require(
        refusals.is_empty(),
        &format!("refused - {}", refusals.join("\n          ")),
    )?;
    let a = map(&action)?;
    let subject = text(field(a, "id")?)?;
    let old = if map(&map(input.state())?["subjects"])?.contains_key(subject) {
        Some(input.head(subject)?)
    } else {
        None
    };
    let saw = input.saw(subject)?;
    let mut new = vec![];
    let fields = before_world.fields().clone();
    let deps_field = text(&fields["deps"])?;
    let mut doc = before_doc.clone();
    let mut version = options.receipt_version.unwrap_or(1);
    if kind == "review" {
        let old = map(old.ok_or_else(|| error("invalid_review"))?)?;
        require(string_is(&old["kind"], "judgment"), "invalid_review")?;
        require(
            a.get("_record_scope").is_none_or(|v| {
                map(&old["body"])
                    .ok()
                    .and_then(|b| b.get("scope"))
                    .unwrap_or(&V::Null)
                    .digest()
                    .ok()
                    == v.digest().ok()
            }),
            "history_review_scope_change_requires_claim",
        )?;
        let deps = list(field(map(&old["body"])?, deps_field)?)?;
        let (pins, _) = input.pins(deps, !Authoring::blocked_text(&old["body"]).is_empty())?;
        let body = obj([
            ("act", s("review")),
            ("of", old["id"].clone()),
            ("over", V::List(vec![])),
            (
                "because",
                a.get("why")
                    .filter(|v| truth(v))
                    .map(|v| s(&Authoring::py(v)))
                    .unwrap_or_else(|| s("explicit review")),
            ),
            ("read", pins),
        ]);
        new.push(make_object(
            subject,
            "act",
            body,
            strings(saw),
            None,
            empty(),
            empty(),
            options,
        )?);
    } else {
        let candidate_doc = before_world.candidate_document(&action)?;
        let mut body = entries(&candidate_doc)?
            .get(subject)
            .ok_or_else(|| error("unresolved_history_subject"))?
            .1
            .clone();
        let collection = if let Some(old) = old {
            text(&map(&map(old)?["authored"])?["collection"])?.to_owned()
        } else {
            entries(&candidate_doc)?[subject].0.clone()
        };
        let authored = if kind == "set" {
            map(old.ok_or_else(|| error("unresolved_history_subject"))?)?["authored"].clone()
        } else {
            obj([
                ("collection", s(&collection)),
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
            ])
        };
        let judgment = map(&body).is_ok_and(|m| m.contains_key(deps_field));
        let deps = map(&body)
            .ok()
            .and_then(|b| b.get(deps_field))
            .map(list)
            .transpose()?
            .unwrap_or(&[])
            .to_vec();
        version = options
            .receipt_version
            .unwrap_or(if judgment && before_world.core() {
                7
            } else {
                1
            });
        require([1, 7].contains(&version), "invalid_authoring_receipt")?;
        if judgment && version == 7 && before_world.core() {
            let mut seen = Map::new();
            for dep in &deps {
                let name = text(dep)?;
                if before_world.raw().contains_key(name) {
                    seen.insert(name.into(), before_world.history(name)?);
                }
            }
            map_mut(&mut body)?.insert(text(&fields["snapshot"])?.into(), V::Map(seen));
        }
        let (pins, gaps) = input.pins(
            &deps,
            judgment
                && (version == 7 || !before_world.core())
                && !Authoring::blocked_text(&body).is_empty(),
        )?;
        let claim = make_object(
            subject,
            if judgment { "judgment" } else { "reading" },
            body.clone(),
            strings(saw.clone()),
            Some(authored),
            pins,
            gaps,
            options,
        )?;
        let claim_id = map(&claim)?["id"].clone();
        new.push(claim);
        map_mut(map_mut(&mut doc)?.entry(collection).or_insert_with(empty))?
            .insert(subject.into(), body);
        if old.is_some() || options.strict {
            let over = if old.is_some() {
                map(&map(&map(input.state())?["subjects"])?[subject])?["heads"].clone()
            } else {
                V::List(vec![])
            };
            let mut saw = saw;
            saw.push(text(&claim_id)?.into());
            saw.sort();
            // A correction marks the version it replaces as corrected rather than replaced;
            // an answer pins the exact version of the entry that answered the question.
            let amend = a
                .get("amend")
                .filter(|v| **v != V::Null)
                .map(text)
                .transpose()?;
            require(
                amend.is_none() || old.is_some(),
                "unresolved_history_subject",
            )?;
            let mut body = obj([
                (
                    "act",
                    s(if amend == Some("correct") {
                        "correct"
                    } else {
                        "accept"
                    }),
                ),
                ("of", claim_id),
                ("over", over),
                (
                    "because",
                    a.get("why")
                        .filter(|v| truth(v))
                        .map(|v| s(&Authoring::py(v)))
                        .unwrap_or_else(|| {
                            s(&format!("explicit {}", amend.unwrap_or(kind.as_str())))
                        }),
                ),
            ]);
            if amend == Some("answer")
                && let Some(by) = a.get("answer_by").filter(|v| **v != V::Null)
            {
                let (read, _) = input.pins(std::slice::from_ref(by), false)?;
                map_mut(&mut body)?.insert("read".into(), read);
            }
            new.push(make_object(
                subject,
                "act",
                body,
                strings(saw),
                None,
                empty(),
                empty(),
                options,
            )?);
        }
    }
    doc = destination(&doc)?;
    let cap = F::capabilities(&doc, Some(text(&map(&cap)?["profile"])?))?;
    let meta = map_mut(
        map_mut(&mut doc)?
            .get_mut("meta")
            .ok_or_else(|| error("invalid_document"))?,
    )?;
    if meta.contains_key("updated") {
        meta.insert("updated".into(), a["as_of"].clone());
    }
    Ok(Plan {
        objects: new,
        document: doc,
        before_document: before_doc,
        intent,
        notes,
        version,
        capabilities: cap,
    })
}

/// Legacy receipt materialization is intentionally separate from claim preparation.
/// The node provider can retain evidence by subject without constructing legacy file images.
pub(crate) fn receipt(
    input: &dyn Input,
    plan: &Plan,
    options: &Options,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
    frozen_archive: &V,
) -> Result<V> {
    let doc = &plan.document;
    let before_doc = &plan.before_document;
    let new = &plan.objects;
    let cap = &plan.capabilities;
    let version = plan.version;
    let intent = plan.intent.clone();
    let notes = plan.notes.clone();
    let kind = text(&map(&plan.intent)?["kind"])?;
    let subject = text(&map(&new[0])?["subject"])?;
    let mut before_world =
        crate::history_authoring_reader::AuthoringReader::new(before_doc, runtime)?;
    let mut after_world = crate::history_authoring_reader::AuthoringReader::new(&doc, runtime)?;
    let before_versions = input.accepted_versions()?;
    let mut after_versions = before_versions.clone();
    if kind != "review" {
        after_versions.insert(subject.into(), map(&new[0])?["id"].clone());
    }
    let mut before = before_world.evidence(&before_doc, audit, &before_versions)?;
    map_mut(&mut before)?.insert(
        "authoring".into(),
        obj([
            ("version", n(&version.to_string())),
            ("action", intent),
            ("by", options.by.clone()),
            ("recorded_at", s(&options.recorded_at)),
            ("archive", frozen_archive.clone()),
            ("baseline", input.baseline().clone()),
        ]),
    );
    let mut after = after_world.evidence(&doc, audit, &after_versions)?;
    let ids = new
        .iter()
        .map(|o| text(&map(o)?["id"]).map(str::to_owned))
        .collect::<Result<BTreeSet<_>>>()?;
    map_mut(&mut after)?.insert(
        "authoring".into(),
        obj([("objects", strings(ids)), ("notes", strings(notes))]),
    );
    let receipt = T::semantic_receipt(text(&map(&cap)?["profile"])?, &cap, &before, &after)?;
    Ok(receipt)
}

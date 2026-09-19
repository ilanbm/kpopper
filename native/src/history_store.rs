//! Committed history views and optimistic publication over captured evidence.
use crate::{
    Result,
    history_authority::{self as A, Files},
    history_capture::{self as H, Capture},
    history_contract::*,
    history_emit,
    history_transaction::{self as T, PreparedMutation},
    history_transaction_fs as F,
    history_view::{self as Vw, list, map_mut},
    history_yaml as Y,
    identity::sha256,
    require,
    value::{Integer, TypedValue as V},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
const MAX_RECONCILIATION_WORK: usize = 4_000_000;
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn object(fields: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn strings(values: impl IntoIterator<Item = String>) -> V {
    V::List(values.into_iter().map(V::Text).collect())
}
fn manifests(commits: &Files) -> Result<BTreeMap<String, V>> {
    commits
        .iter()
        .map(|(op, raw)| {
            let v = Y::decode_document(raw)?;
            A::validate_commit(&v)?;
            Ok((op.clone(), v))
        })
        .collect()
}
fn closure(
    manifests: &BTreeMap<String, V>,
    frontier: impl IntoIterator<Item = String>,
    work: &mut usize,
    limit: bool,
) -> Result<Vec<String>> {
    let mut result = BTreeSet::new();
    let mut pending: Vec<_> = frontier.into_iter().collect();
    while let Some(op) = pending.pop() {
        if result.insert(op.clone()) {
            *work += 1;
            require(!limit || *work <= MAX_RECONCILIATION_WORK, "history_limit")?;
            let m = manifests
                .get(&op)
                .ok_or_else(|| error("incomplete_view_baseline"))?;
            pending.extend(map(&map(m)?["parents"])?.keys().cloned());
        }
    }
    Ok(result.into_iter().collect())
}
fn subset(commits: &Files, ops: impl IntoIterator<Item = String>) -> Files {
    ops.into_iter()
        .map(|op| {
            let raw = commits[&op].clone();
            (op, raw)
        })
        .collect()
}
struct BaselineView {
    bound: V,
    document: V,
    objects: Map,
    state: V,
}
#[derive(Clone, Debug)]
pub struct Store {
    pub entry: PathBuf,
    pub root: PathBuf,
    pub layout: T::Layout,
}
impl Store {
    pub fn new(entry: &Path) -> Result<Self> {
        let entry = if entry.is_absolute() {
            entry.to_owned()
        } else {
            std::env::current_dir()?.join(entry)
        };
        let name = entry
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or_else(|| error("invalid_path"))?;
        let layout = T::Layout::for_entry(name)?;
        let root = entry
            .parent()
            .ok_or_else(|| error("invalid_path"))?
            .to_owned();
        Ok(Self {
            entry,
            root,
            layout,
        })
    }
    pub fn capture(&self) -> Result<Capture> {
        H::capture(&self.entry, None, None)
    }
    pub fn capture_reconciliation(&self, allow_conflicts: bool) -> Result<Capture> {
        H::capture_reconciliation(&self.entry, allow_conflicts)
    }
    pub fn render(&self, capture: &Capture) -> Result<Vec<u8>> {
        Vw::render(
            capture,
            &capture.objects,
            &capture.object_bytes,
            &capture.commits,
        )
    }
    pub(crate) fn known_view(capture: &Capture) -> Result<bool> {
        let manifests = manifests(&capture.commits)?;
        let hash = s(&sha256(&capture.entry_bytes));
        if manifests
            .values()
            .any(|v| map(v).unwrap()["view_sha256"] == hash)
        {
            return Ok(true);
        }
        let expected = map(&map(&capture.document)?["meta"])?["history"].digest()?;
        let mut examined = BTreeSet::new();
        let mut work = 0;
        for m in manifests.values() {
            let m = map(m)?;
            if !string_is(&m["baseline_digest"], &expected) {
                continue;
            }
            let parents = map(&m["parents"])?.keys().cloned().collect::<Vec<_>>();
            if !examined.insert(parents.clone()) {
                continue;
            }
            let commits = subset(
                &capture.commits,
                closure(&manifests, parents, &mut work, false)?,
            );
            if commits.is_empty() {
                continue;
            }
            let objects = A::committed_objects(&capture.marker, &commits, &capture.object_bytes)?;
            let state = Vw::selected_state(capture, &objects, &capture.object_bytes)?;
            if H::baseline(&capture.marker, &commits, &state)?.digest()? == expected
                && Vw::render(capture, &objects, &capture.object_bytes, &commits)?
                    == capture.entry_bytes
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn baseline_views(
        &self,
        capture: &Capture,
        wanted: &BTreeSet<String>,
        retained: &[Capture],
    ) -> Result<BTreeMap<String, BaselineView>> {
        let manifests = manifests(&capture.commits)?;
        let mut work = 0;
        // Resolve candidates lazily: a current view does not depend on unrelated
        // retained baselines or on work in historical candidates after its match.
        let candidates = std::iter::once((None, None))
            .chain(retained.iter().map(|old| (Some(old), None)))
            .chain(manifests.iter().flat_map(|(op, m)| {
                [
                    (None, Some(vec![op.clone()])),
                    (
                        None,
                        Some(
                            map(&map(m).unwrap()["parents"])
                                .unwrap()
                                .keys()
                                .cloned()
                                .collect(),
                        ),
                    ),
                ]
            }));
        let mut examined = BTreeSet::new();
        let mut found = BTreeMap::new();
        for (retained, frontier) in candidates {
            let ops = if let Some(old) = retained {
                require(
                    old.marker.digest()? == capture.marker.digest()?,
                    "authority_mismatch",
                )?;
                require(
                    old.commits
                        .iter()
                        .all(|(op, raw)| capture.commits.get(op) == Some(raw)),
                    "incomplete_view_baseline",
                )?;
                closure(&manifests, old.commits.keys().cloned(), &mut work, true)?
            } else if let Some(frontier) = frontier {
                closure(&manifests, frontier, &mut work, true)?
            } else {
                capture.commits.keys().cloned().collect()
            };
            if !examined.insert(ops.clone()) {
                continue;
            }
            work += ops.len();
            require(work <= MAX_RECONCILIATION_WORK, "history_limit")?;
            let commits = subset(&capture.commits, ops);
            let digest = A::committed_set_digest(&commits)?;
            if !wanted.contains(&digest) {
                continue;
            }
            let objects = A::committed_objects(&capture.marker, &commits, &capture.object_bytes)?;
            let state = Vw::selected_state(capture, &objects, &capture.object_bytes)?;
            let bound = H::baseline(&capture.marker, &commits, &state)?;
            let document = if !commits.is_empty() {
                Vw::render_document(capture, &objects, &capture.object_bytes, &commits)?
            } else {
                let mut templates = BTreeMap::new();
                for m in manifests.values() {
                    let m = map(m)?;
                    if map(&m["parents"])?.is_empty()
                        && string_is(&m["baseline_digest"], &bound.digest()?)
                        && let Some(t) = m.get("view_template")
                    {
                        templates.insert(t.digest()?, t.clone());
                    }
                }
                require(templates.len() == 1, "unresolved_template")?;
                let mut document = templates.into_values().next().unwrap();
                let meta = map_mut(&mut document)?
                    .entry("meta".into())
                    .or_insert_with(|| V::Map(Map::new()));
                map_mut(meta)?.insert("history".into(), bound.clone());
                document
            };
            found.insert(
                digest,
                BaselineView {
                    bound,
                    document,
                    objects,
                    state,
                },
            );
            if found.keys().cloned().collect::<BTreeSet<_>>() == *wanted {
                break;
            }
        }
        require(
            found.keys().cloned().collect::<BTreeSet<_>>() == *wanted,
            "incomplete_view_baseline",
        )?;
        Ok(found)
    }
    pub fn prepare_reconciliation(
        &self,
        capture: Option<&Capture>,
        allow_conflicts: bool,
        retained: &[Capture],
    ) -> Result<V> {
        let live = self.capture_reconciliation(allow_conflicts)?;
        if let Some(c) = capture {
            require(live.inventory == c.inventory, "stale_baseline")?;
        }
        let mut wanted = BTreeSet::new();
        for alternative in &live.view_alternatives {
            wanted.insert(text(&map(&map(&map(&alternative.document)?["meta"])?["history"])?["committed_set_digest"])?.to_owned());
        }
        let views = self.baseline_views(&live, &wanted, retained)?;
        let mut descriptions = Vec::new();
        let mut safe = true;
        for alternative in &live.view_alternatives {
            let observed = &map(&map(&alternative.document)?["meta"])?["history"];
            let base = &views[text(&map(observed)?["committed_set_digest"])?];
            require(
                observed.digest()? == base.bound.digest()?,
                "baseline_mismatch",
            )?;
            let original = &base.document;
            let edited = &alternative.document;
            let mut changes = Vw::changes(original, edited)?;
            let original_bytes = history_emit::encode_document(original)?;
            if changes.is_empty() && alternative.bytes != original_bytes {
                changes.push(object([
                    ("path", V::List(vec![])),
                    ("kind", s("text_edit")),
                    ("before_sha256", s(&sha256(&original_bytes))),
                    ("after_sha256", s(&sha256(&alternative.bytes))),
                ]));
            }
            let core = map(original)?
                .get("meta")
                .and_then(|v| map(v).ok())
                .and_then(|m| m.get("reasoning"))
                .and_then(|v| map(v).ok())
                .and_then(|m| m.get("profile"))
                .is_some_and(|v| string_is(v, "core/v1"));
            let collections=map(original)?.iter().filter(|(k,v)|! ["meta","schema","record","also"].contains(&k.as_str()) && matches!(v,V::Map(m) if !m.is_empty() && m.values().all(|v|core || !matches!(v,V::List(_))))).map(|(k,_)|k).collect::<BTreeSet<_>>();
            let mut bodies: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
            for (id, obj) in &base.objects {
                let obj = map(obj)?;
                if string_is(&obj["kind"], "act") {
                    continue;
                }
                let collection = obj
                    .get("authored")
                    .and_then(|v| map(v).ok())
                    .and_then(|m| m.get("collection"))
                    .and_then(|v| text(v).ok())
                    .unwrap_or("");
                bodies
                    .entry((
                        text(&obj["subject"])?.into(),
                        collection.into(),
                        obj["body"].digest()?,
                    ))
                    .or_default()
                    .push(id.clone());
            }
            for change in &mut changes {
                let change = map_mut(change)?;
                if change
                    .get("kind")
                    .is_some_and(|v| string_is(v, "text_edit"))
                {
                    continue;
                }
                let path = list(&change["path"])?;
                if path.len() >= 2 && collections.contains(&text(&path[0])?.to_owned()) {
                    let (collection, subject) =
                        (text(&path[0])?.to_owned(), text(&path[1])?.to_owned());
                    let body = map(edited)?
                        .get(&collection)
                        .and_then(|v| map(v).ok())
                        .and_then(|m| m.get(&subject))
                        .unwrap_or(&V::Null);
                    let matches = bodies
                        .get(&(subject.clone(), collection, body.digest()?))
                        .cloned()
                        .unwrap_or_default();
                    change.insert("kind".into(), s("proposal_candidate"));
                    change.insert("subject".into(), s(&subject));
                    change.insert(
                        "baseline_heads".into(),
                        map(&map(&base.bound)?["heads"])?
                            .get(&subject)
                            .cloned()
                            .unwrap_or(V::List(vec![])),
                    );
                    change.insert("recorded_claim_matches".into(), strings(matches));
                } else {
                    change.insert("kind".into(), s("header_edit"));
                }
            }
            safe &= changes.is_empty();
            let versions = map(&map(&base.state)?["subjects"])?
                .iter()
                .map(|(subject, entry)| {
                    let entry = map(entry)?;
                    let versions = list(&entry["heads"])?
                        .iter()
                        .chain(list(&entry["proposals"])?)
                        .map(|v| text(v).map(str::to_owned))
                        .collect::<Result<BTreeSet<_>>>()?;
                    Ok((subject.clone(), strings(versions)))
                })
                .collect::<Result<Map>>()?;
            let acts = base
                .objects
                .iter()
                .filter(|(_, v)| string_is(&map(v).unwrap()["kind"], "act"))
                .map(|(id, _)| id.clone());
            descriptions.push(object([
                ("name", s(&alternative.name)),
                ("entry_sha256", s(&sha256(&alternative.bytes))),
                ("baseline", base.bound.clone()),
                ("changes", V::List(changes)),
                ("original_view", original.clone()),
                ("edited_view", edited.clone()),
                ("original_versions", V::Map(versions)),
                ("recorded_acts", strings(acts)),
            ]));
        }
        let result = object([
            ("version", V::Integer(Integer::new("1")?)),
            ("authority", live.marker.clone()),
            ("entry_sha256", s(&sha256(&live.entry_bytes))),
            ("current_baseline", live.baseline.clone()),
            (
                "conflicted",
                V::Bool(live.view_alternatives[0].name != "view"),
            ),
            ("alternatives", V::List(descriptions)),
            ("rebuild_safe", V::Bool(safe)),
        ]);
        Y::validate_value(&result, 8 * 1024 * 1024)?;
        live.verify_current()?;
        Ok(result)
    }
    pub fn rebuild(
        &self,
        capture: Option<&Capture>,
        write: bool,
        allow_conflicts: bool,
        retained: &[Capture],
    ) -> Result<Vec<u8>> {
        let _lock = F::DirectoryGuard::acquire(&self.root, true)?;
        let live = self.capture_reconciliation(allow_conflicts)?;
        if let Some(c) = capture {
            require(live.inventory == c.inventory, "stale_baseline")?;
        }
        let rendered = self.render(&live)?;
        if allow_conflicts {
            let description = self.prepare_reconciliation(Some(&live), true, retained)?;
            require(
                map(&description)?["rebuild_safe"] == V::Bool(true),
                "unresolved_view_edit",
            )?;
        } else {
            require(
                Self::known_view(&live)?
                    || live.document.digest()? == Y::decode_document(&rendered)?.digest()?,
                "unresolved_view_edit",
            )?;
        }
        if write {
            live.verify_current()?;
            F::replace(&F::target(&self.root, &self.layout.entry)?, Some(&rendered))?;
        }
        Ok(rendered)
    }
    /// Mandatory verifier checks the operation's retained semantic and routing evidence.
    pub fn commit(&self, mutation: &PreparedMutation, verify: F::Verify<'_>) -> Result<V> {
        let _lock = F::DirectoryGuard::acquire(&self.root, true)?;
        // Identity, view-edit and adoption verification belongs to its replay
        // preparer. These are refused until that preparer is available.
        require(
            mutation.auxiliary_view()?.is_none(),
            "unsupported_identity_replay",
        )?;
        let data = mutation.to_data();
        let d = map(&data)?;
        require(string_is(&d["entry"], &self.layout.entry), "entry_mismatch")?;
        require(
            string_is(&map(&d["authority"])?["authority"], "history"),
            "history_not_active",
        )?;
        let live = self.capture()?;
        require(
            d["authority"].digest()? == live.marker.digest()?,
            "authority_mismatch",
        )?;
        let files = mutation.files();
        require(
            files.iter().all(|f| {
                [
                    "record",
                    "history_object",
                    "history_commit",
                    "history_evidence",
                    "view",
                ]
                .contains(&f.role.as_str())
            }),
            "unsupported_file_role",
        )?;
        let manifest = files
            .iter()
            .find(|f| f.role == "history_commit")
            .ok_or_else(|| error("invalid_mutation"))?;
        let record = files
            .iter()
            .find(|f| f.role == "record")
            .ok_or_else(|| error("invalid_mutation"))?;
        let op = text(&d["operation"])?;
        require(
            !live
                .inactive_generations
                .values()
                .any(|g| g.commits.contains_key(op)),
            "operation_collision",
        )?;
        let receipt = map(&d["receipt"])?;
        require(
            !map(&receipt["after"])?.contains_key("history_branch_adoption"),
            "unsupported_branch_adoption",
        )?;
        require(
            !map(&receipt["before"])?.contains_key("history_edit"),
            "unsupported_view_edit_replay",
        )?;
        let prior = live.commits.get(op);
        let after = manifest
            .after
            .as_ref()
            .ok_or_else(|| error("invalid_mutation"))?;
        if let Some(prior) = prior {
            require(prior == after, "operation_collision")?;
        } else {
            require(
                d["baseline"].digest()? == live.baseline.digest()?,
                "stale_baseline",
            )?;
            require(
                map(&map(&live.document)?["meta"])?["history"].digest()?
                    == live.baseline.digest()?,
                "stale_view",
            )?;
            require(
                record.before.as_ref() == Some(&live.entry_bytes),
                "concurrent_edit",
            )?;
            if !live.commits.is_empty() {
                require(
                    live.document.digest()?
                        == Y::decode_document(&self.render(&live)?)?.digest()?,
                    "unresolved_view_edit",
                )?;
            }
        }
        require(
            [record.before.as_ref(), record.after.as_ref()].contains(&Some(&live.entry_bytes)),
            "concurrent_edit",
        )?;
        let mut combined = live.commits.clone();
        combined.insert(op.into(), after.clone());
        let mut staged = live.object_bytes.clone();
        for f in files {
            let path = F::target(&self.root, &f.path)?;
            if f.role == "history_evidence" {
                require(
                    F::read(&path)?
                        .as_ref()
                        .is_none_or(|b| Some(b) == f.after.as_ref()),
                    "immutable_collision",
                )?;
            }
            if f.role == "history_object" {
                let bytes = f.after.as_ref().ok_or_else(|| error("invalid_mutation"))?;
                let obj = Y::decode_document(bytes)?;
                validate_object(&obj)?;
                let obj = map(&obj)?;
                let key = (
                    text(&obj["subject"])?.to_owned(),
                    text(&obj["id"])?.to_owned(),
                );
                require(
                    staged.get(&key).is_none_or(|b| b == bytes),
                    "immutable_collision",
                )?;
                staged.insert(key, bytes.clone());
            }
        }
        let selected = A::committed_objects(&live.marker, &combined, &staged)?;
        require(
            combined
                .values()
                .chain(staged.values())
                .map(Vec::len)
                .sum::<usize>()
                <= T::MAX_TRANSACTION_BYTES,
            "history_limit",
        )?;
        let expected = Vw::render(&live, &selected, &staged, &combined)?;
        require(
            record.after.as_ref() == Some(&expected),
            "view_projection_mismatch",
        )?;
        let commit = Y::decode_document(after)?;
        if prior.is_none() {
            let parents = A::commit_frontier(&live.commits)?;
            require(
                map(&commit)?["parents"] == V::Map(parents),
                "parent_baseline_mismatch",
            )?;
        }
        verify(&data)?;
        require(
            self.capture()?.inventory == live.inventory,
            "stale_baseline",
        )?;
        for f in files {
            if ["history_object", "history_evidence"].contains(&f.role.as_str()) {
                F::publish_immutable(&self.root, &f.path, f.after.as_ref().unwrap())?;
            }
        }
        F::publish_immutable(&self.root, &manifest.path, after)?;
        let path = F::target(&self.root, &self.layout.entry)?;
        let current = F::read(&path)?;
        require(
            current == record.before || current == record.after,
            "concurrent_edit",
        )?;
        if current != record.after {
            F::replace(&path, record.after.as_deref())?;
        }
        A::validate_commit(&commit)?;
        Ok(commit)
    }
}

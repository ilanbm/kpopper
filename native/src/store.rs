//! Opt-in linear history reader/writer. No evaluator or production route takeover.
use crate::{
    Error, Result,
    identity::{identity, object_identity, sha256, subject_path},
    require,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

pub const MAX_BYTES: usize = 1_048_576;
const MAX_STORE: usize = 16 * MAX_BYTES;
const MAX_COMMITS: usize = 1_000;
const OPT_IN: &str = ".kpopper/native-feasibility.json";
const CAPABILITIES: [&str; 2] = ["explicit-root-disposition/v1", "subject-paths/v2"];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub kind: String,
    pub subject: String,
    pub value: Value,
    pub source: Option<String>,
    pub operation: String,
    pub on: String,
}

#[derive(Clone, Default)]
struct Capture {
    authority: Value,
    template: Value,
    commits: BTreeMap<String, (Value, Vec<u8>)>,
    objects: BTreeMap<String, Value>,
    heads: BTreeMap<String, String>,
    // Latest disposition for every historical claim, including superseded claims.
    acts: BTreeMap<String, BTreeMap<String, String>>,
    tip: Option<String>,
    view_current: bool,
    total_bytes: usize,
    view_bytes: usize,
}

impl Capture {
    fn baseline(&self) -> Result<Value> {
        let mut effects = BTreeMap::new();
        for (op, (manifest, _)) in &self.commits {
            let mut effect = manifest.clone();
            effect.as_object_mut().unwrap().remove("receipt");
            effect.as_object_mut().unwrap().remove("view_sha256");
            effects.insert(op, effect);
        }
        Ok(
            json!({"version":1,"record_id":self.authority["record_id"],"authority_generation":1,
            "committed_set_digest":identity(&json!(effects))?,
            "heads":self.heads.iter().map(|(s,id)| (s,vec![id])).collect::<BTreeMap<_,_>>(),
            "open_acts":self.heads.keys().map(|s| (s,self.acts.get(s).into_iter().flat_map(|m|m.values()).collect::<BTreeSet<_>>())).collect::<BTreeMap<_,_>>() }),
        )
    }

    fn view(&self) -> Result<Value> {
        let mut doc = self.template.clone();
        doc["meta"]["history"] = self.baseline()?;
        doc["readings"] = json!(
            self.heads
                .iter()
                .map(|(s, id)| (s, self.objects[id]["body"].clone()))
                .collect::<BTreeMap<_, _>>()
        );
        Ok(doc)
    }

    fn frontier(&self) -> Value {
        json!(
            self.tip
                .iter()
                .map(|op| (op, sha256(&self.commits[op].1)))
                .collect::<BTreeMap<_, _>>()
        )
    }

    fn report(&self) -> Result<Value> {
        Ok(
            json!({"status":"found","profile":"native-feasibility/v1","scope":"linear-readings-only",
            "semantic_assessment":"not_performed","commits":self.commits.len(),"view_current":self.view_current,
            "document":self.view()?,"heads":self.heads,"revision":identity(&self.baseline()?)?}),
        )
    }
}

fn token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 160
        && s.bytes().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
}

fn mapping(value: &Value, keys: &[&str]) -> Result<()> {
    let map = value
        .as_object()
        .ok_or_else(|| Error("invalid_schema".into()))?;
    require(
        map.len() == keys.len() && keys.iter().all(|key| map.contains_key(*key)),
        "unsupported_schema",
    )
}

pub fn bytes(value: &Value) -> Result<Vec<u8>> {
    let typed = crate::value::TypedValue::from_json(value)?;
    let mut raw = serde_json::to_vec_pretty(value)?;
    raw.push(b'\n');
    // Retain existing byte identities wherever JSON also has the same YAML meaning.
    // YAML 1.1 reads e.g. 1e+20 as text; explicit tags preserve such floats.
    if crate::history_yaml::decode_document(&raw).ok().as_ref() != Some(&typed) {
        raw = crate::history_yaml::encode_document(&typed)?;
    }
    require(raw.len() <= MAX_BYTES, "byte_limit")?;
    Ok(raw)
}

// This visitor checks duplicate JSON keys before serde_json::Value can overwrite one.
struct Unique;
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Unique;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "unique JSON keys")
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> std::result::Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> std::result::Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> std::result::Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> std::result::Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> std::result::Result<Unique, E> {
                Ok(Unique)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Unique, A::Error> {
                while seq.next_element::<Unique>()?.is_some() {}
                Ok(Unique)
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Unique, A::Error> {
                let mut keys = BTreeSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !keys.insert(key) {
                        return Err(serde::de::Error::custom("duplicate key"));
                    }
                    map.next_value::<Unique>()?;
                }
                Ok(Unique)
            }
        }
        d.deserialize_any(Visitor)
    }
}

pub fn json_input(raw: &[u8]) -> Result<Value> {
    require(raw.len() <= MAX_BYTES, "byte_limit")?;
    serde_json::from_slice::<Unique>(raw)?;
    let value = serde_json::from_slice(raw)?;
    identity(&value)?; // includes explicit depth/work and finite-value bounds
    Ok(value)
}

fn parse(raw: &[u8]) -> Result<Value> {
    require(raw.len() <= MAX_BYTES, "byte_limit")?;
    // Storage is still the JSON-compatible subset. A date cannot become text here.
    crate::history_yaml::decode_document(raw)?.to_json()
}

pub struct Store {
    root: PathBuf,
    _lock: File,
}

impl Store {
    fn acquire(root: &Path, exclusive: bool) -> Result<Self> {
        require(root.is_absolute(), "absolute_workspace_required")?;
        let mut prefix = PathBuf::new();
        for component in root.components() {
            require(
                !matches!(component, Component::ParentDir),
                "invalid_workspace",
            )?;
            prefix.push(component);
            if let Ok(meta) = fs::symlink_metadata(&prefix) {
                require(!meta.file_type().is_symlink(), "symlink_refused")?;
            }
        }
        require(root.is_dir(), "workspace_missing")?;
        let root = root.canonicalize()?;
        let lock = File::open(&root)?;
        // Same directory-flock protocol as history_transaction._lock on POSIX.
        #[cfg(unix)]
        if exclusive {
            FileExt::try_lock_exclusive(&lock)?;
        } else {
            FileExt::try_lock_shared(&lock)?;
        }
        #[cfg(not(unix))]
        {
            let _ = exclusive;
            return Err(Error("platform_locking_unverified".into()));
        }
        Ok(Self { root, _lock: lock })
    }

    fn path(&self, relative: &str) -> Result<PathBuf> {
        let mut path = self.root.clone();
        for part in Path::new(relative).components() {
            require(matches!(part, Component::Normal(_)), "invalid_path")?;
            path.push(part);
            match fs::symlink_metadata(&path) {
                Ok(meta) => require(!meta.file_type().is_symlink(), "symlink_refused")?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(path)
    }

    fn read(&self, relative: &str) -> Result<Vec<u8>> {
        let path = self.path(relative)?;
        require(path.is_file(), "missing_history_file")?;
        let mut raw = Vec::new();
        File::open(path)?
            .take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut raw)?;
        require(raw.len() <= MAX_BYTES, "byte_limit")?;
        Ok(raw)
    }

    fn sync_dir(path: &Path) -> Result<()> {
        File::open(path)?.sync_all()?;
        Ok(())
    }

    fn publish(&self, relative: &str, raw: &[u8], immutable: bool) -> Result<()> {
        let path = self.path(relative)?;
        let parent = path.parent().unwrap();
        if !parent.exists() {
            fs::create_dir_all(parent)?;
            // Persist newly created history directories before publishing their contents.
            let mut ancestor = Some(parent);
            while let Some(p) = ancestor {
                Self::sync_dir(p)?;
                if p == self.root {
                    break;
                }
                ancestor = p.parent();
            }
        }
        self.path(relative)?;
        if immutable && path.exists() {
            return require(self.read(relative)? == raw, "immutable_collision");
        }
        let tmp = parent.join(format!(
            ".native-{}-{}.tmp",
            std::process::id(),
            sha256(raw)
        ));
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(raw)?;
        file.sync_all()?;
        drop(file);
        let result = if immutable {
            fs::hard_link(&tmp, &path)
        } else {
            fs::rename(&tmp, &path)
        };
        if immutable {
            let _ = fs::remove_file(&tmp);
        }
        result?;
        Self::sync_dir(parent)?;
        Ok(())
    }

    pub fn init(root: &Path, record_id: &str) -> Result<Value> {
        require(token(record_id), "invalid_record_id")?;
        let store = Self::acquire(root, true)?;
        require(
            fs::read_dir(&store.root)?.next().is_none(),
            "init_requires_empty_workspace",
        )?;
        let authority = json!({"version":1,"record_id":record_id,"authority":"history","generation":1,"profile":"history/v1"});
        let template = json!({"meta":{"scope":"Native runtime feasibility"},"schema":{"value":"v","deps":"rests_on","snapshot":"seen","predicate":"wrong_if"},"readings":{}});
        let capture = Capture {
            authority: authority.clone(),
            template,
            ..Default::default()
        };
        store.publish(".kpopper/history.yaml", &bytes(&authority)?, true)?;
        store.publish("GROUNDING.yaml", &bytes(&capture.view()?)?, true)?;
        store.publish(
            OPT_IN,
            &bytes(&json!({"version":1,"record_id":record_id,"profile":"native-feasibility/v1"}))?,
            true,
        )?;
        Ok(json!({"status":"initialized","record_id":record_id}))
    }

    fn load(&self) -> Result<Capture> {
        let opt = parse(&self.read(OPT_IN)?)?;
        mapping(&opt, &["version", "record_id", "profile"])?;
        require(
            opt["version"] == 1 && opt["profile"] == "native-feasibility/v1",
            "opt_in_required",
        )?;
        // Recovery of Python's retained transactions is outside this executable's scope.
        for name in [".history-local", ".kpopper/.history-local"] {
            let path = self.path(name)?;
            if path.exists() {
                require(
                    fs::read_dir(path)?.next().is_none(),
                    "pending_python_journal",
                )?;
            }
        }
        let authority = parse(&self.read(".kpopper/history.yaml")?)?;
        mapping(
            &authority,
            &["version", "record_id", "authority", "generation", "profile"],
        )?;
        require(
            authority
                == json!({"version":1,"record_id":opt["record_id"],"authority":"history","generation":1,"profile":"history/v1"}),
            "unsupported_authority",
        )?;
        require(
            authority["record_id"].as_str().is_some_and(token),
            "invalid_record_id",
        )?;
        let view_raw = self.read("GROUNDING.yaml")?;
        let mut template = parse(&view_raw)?;
        mapping(&template, &["meta", "schema", "readings"])?;
        let view_baseline = template["meta"]["history"].clone();
        require(
            view_baseline["record_id"] == authority["record_id"]
                && view_baseline["authority_generation"] == 1,
            "authority_mismatch",
        )?;
        let meta = template["meta"]
            .as_object_mut()
            .ok_or_else(|| Error("invalid_meta".into()))?;
        meta.remove("history");
        require(!meta.contains_key("reasoning"), "core_profile_unsupported")?;
        template["readings"] = json!({});
        let mut capture = Capture {
            authority,
            template,
            ..Default::default()
        };
        let mut pending = BTreeMap::new();
        let dir = self.path(".kpopper/history-commits")?;
        let mut total = view_raw.len();
        if dir.exists() {
            for item in fs::read_dir(dir)? {
                let item = item?;
                let name = item
                    .file_name()
                    .into_string()
                    .map_err(|_| Error("invalid_path".into()))?;
                if name.starts_with(".native-") && name.ends_with(".tmp") {
                    continue;
                }
                let op = name
                    .strip_suffix(".yaml")
                    .filter(|s| token(s))
                    .ok_or_else(|| Error("invalid_commit_path".into()))?;
                let raw = self.read(&format!(".kpopper/history-commits/{name}"))?;
                total += raw.len();
                require(
                    total <= MAX_STORE && pending.len() < MAX_COMMITS,
                    "store_limit",
                )?;
                pending.insert(op.to_owned(), (parse(&raw)?, raw));
            }
        }
        let initial = capture.baseline()?;
        let mut known_views =
            BTreeMap::from([(identity(&initial)?, sha256(&bytes(&capture.view()?)?))]);
        while !pending.is_empty() {
            let frontier = capture.frontier();
            let ready = pending
                .iter()
                .filter(|(_, (m, _))| m["parents"] == frontier)
                .map(|(op, _)| op.clone())
                .collect::<Vec<_>>();
            require(ready.len() == 1, "unsupported_or_incomplete_commit_graph")?;
            let op = &ready[0];
            let (manifest, raw) = pending.remove(op).unwrap();
            mapping(
                &manifest,
                &[
                    "version",
                    "record_id",
                    "authority_generation",
                    "operation",
                    "parents",
                    "baseline_digest",
                    "objects",
                    "receipt",
                    "view_sha256",
                    "view_template",
                    "requires",
                ],
            )?;
            require(
                manifest["version"] == 1
                    && manifest["record_id"] == capture.authority["record_id"]
                    && manifest["authority_generation"] == 1,
                "authority_mismatch",
            )?;
            require(
                manifest["operation"] == *op
                    && manifest["baseline_digest"] == identity(&capture.baseline()?)?,
                "baseline_mismatch",
            )?;
            require(
                manifest["requires"] == json!(CAPABILITIES)
                    && manifest["view_template"] == capture.template,
                "unsupported_capability_or_template",
            )?;
            require(
                manifest["receipt"]
                    .as_object()
                    .is_some_and(|r| !r.is_empty()),
                "missing_receipt",
            )?;
            require(
                manifest["view_sha256"].as_str().is_some_and(|s| {
                    s.len() == 64
                        && s.bytes()
                            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
                }),
                "invalid_view_hash",
            )?;
            let items = manifest["objects"]
                .as_array()
                .ok_or_else(|| Error("invalid_inventory".into()))?;
            require(items.len() == 2, "unsupported_operation")?;
            let mut objects = Vec::new();
            let mut ids = Vec::new();
            for item in items {
                mapping(item, &["subject", "id", "sha256"])?;
                let subject = item["subject"]
                    .as_str()
                    .ok_or_else(|| Error("invalid_subject".into()))?;
                let id = item["id"]
                    .as_str()
                    .ok_or_else(|| Error("invalid_id".into()))?;
                let object_raw =
                    self.read(&format!(".kpopper/history/{}", subject_path(subject, id)?))?;
                total += object_raw.len();
                require(total <= MAX_STORE, "store_limit")?;
                require(
                    sha256(&object_raw) == item["sha256"],
                    "object_bytes_mismatch",
                )?;
                let object = parse(&object_raw)?;
                require(
                    object["subject"] == subject && object["id"] == id && object["op"] == *op,
                    "object_binding_mismatch",
                )?;
                require(
                    object_identity(&object)? == id,
                    "identity_mismatch_or_unsupported_yaml_type",
                )?;
                ids.push(id.to_owned());
                objects.push(object);
            }
            require(ids[0] < ids[1], "invalid_inventory_order")?;
            Self::apply_objects(&mut capture, &objects)?;
            capture.commits.insert(op.clone(), (manifest.clone(), raw));
            capture.tip = Some(op.clone());
            known_views.insert(
                identity(&capture.baseline()?)?,
                manifest["view_sha256"].as_str().unwrap().to_owned(),
            );
        }
        let view_id = identity(&view_baseline)?;
        require(
            known_views.get(&view_id) == Some(&sha256(&view_raw)),
            "edited_or_unknown_view",
        )?;
        capture.view_current = view_baseline == capture.baseline()?;
        capture.total_bytes = total;
        capture.view_bytes = view_raw.len();
        Ok(capture)
    }

    fn apply_objects(capture: &mut Capture, objects: &[Value]) -> Result<()> {
        let claim = objects
            .iter()
            .find(|o| o["kind"] == "reading")
            .ok_or_else(|| Error("reading_required".into()))?;
        let act = objects
            .iter()
            .find(|o| o["kind"] == "act")
            .ok_or_else(|| Error("accept_act_required".into()))?;
        mapping(
            claim,
            &[
                "schema_version",
                "id_scheme",
                "subject",
                "kind",
                "by",
                "on",
                "op",
                "body",
                "saw",
                "authored",
                "pins",
                "id",
            ],
        )?;
        mapping(
            act,
            &[
                "schema_version",
                "id_scheme",
                "subject",
                "kind",
                "by",
                "on",
                "op",
                "body",
                "saw",
                "id",
            ],
        )?;
        let subject = claim["subject"]
            .as_str()
            .ok_or_else(|| Error("invalid_subject".into()))?;
        let id = claim["id"]
            .as_str()
            .ok_or_else(|| Error("invalid_id".into()))?;
        require(
            claim["schema_version"] == 2
                && act["schema_version"] == 2
                && act["id_scheme"] == "typed-history/v2",
            "unsupported_identity",
        )?;
        require(
            claim["authored"] == authored() && claim["pins"] == json!({}),
            "unsupported_authored_mapping",
        )?;
        mapping(&claim["body"], &["v", "from"])?;
        require(claim["body"]["from"].is_string(), "invalid_source")?;
        for obj in [claim, act] {
            require(
                obj["by"].is_null() || obj["by"].is_string(),
                "invalid_actor",
            )?;
            require(
                obj["on"].as_str().is_some_and(|s| !s.is_empty()),
                "invalid_recorded_time",
            )?;
        }
        require(
            act["subject"] == subject && act["on"] == claim["on"] && act["by"] == claim["by"],
            "invalid_act_context",
        )?;
        let mut saw = capture
            .objects
            .iter()
            .filter(|(_, o)| o["subject"] == subject)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        require(claim["saw"] == json!(saw), "unsupported_claim_context")?;
        saw.push(id.into());
        saw.sort();
        require(act["saw"] == json!(saw), "unsupported_act_context")?;
        mapping(&act["body"], &["act", "of", "over", "because"])?;
        require(
            act["body"]["act"] == "accept"
                && act["body"]["of"] == id
                && act["body"]["over"]
                    == json!(capture.heads.get(subject).into_iter().collect::<Vec<_>>())
                && act["body"]["because"]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty()),
            "unsupported_disposition",
        )?;
        let current_acts = capture.acts.entry(subject.into()).or_default();
        let act_id = act["id"].as_str().unwrap().to_owned();
        current_acts.insert(id.into(), act_id.clone());
        if let Some(old) = capture.heads.get(subject) {
            current_acts.insert(old.clone(), act_id);
        }
        capture.heads.insert(subject.into(), id.into());
        for object in objects {
            capture
                .objects
                .insert(object["id"].as_str().unwrap().into(), object.clone());
        }
        Ok(())
    }

    pub fn open(root: &Path) -> Result<Value> {
        Self::acquire(root, false)?.load()?.report()
    }

    pub fn history(root: &Path, subject: Option<&str>) -> Result<Value> {
        let store = Self::acquire(root, false)?;
        let capture = store.load()?;
        Ok(
            json!({"profile":"native-feasibility/v1","heads":capture.heads,"objects":capture.objects.values().filter(|o|subject.is_none_or(|s|o["subject"]==s)).collect::<Vec<_>>(),"commits":capture.commits.len()}),
        )
    }

    pub fn write(root: &Path, request: Request, expected: Option<&str>) -> Result<Value> {
        require(
            token(&request.operation)
                && !request.on.is_empty()
                && request.subject.len() <= MAX_BYTES,
            "invalid_request",
        )?;
        require(
            matches!(request.kind.as_str(), "add" | "set"),
            "unsupported_operation",
        )?;
        let request_value = serde_json::to_value(&request)?;
        let request_id = identity(&request_value)?;
        let store = Self::acquire(root, true)?;
        let mut capture = store.load()?;
        if let Some((prior, _)) = capture.commits.get(&request.operation) {
            require(
                prior["receipt"]["request_digest"] == request_id,
                "operation_collision",
            )?;
            store.publish("GROUNDING.yaml", &bytes(&capture.view()?)?, false)?;
            return Ok(
                json!({"status":"replayed","operation":request.operation,"revision":identity(&capture.baseline()?)?}),
            );
        }
        if let Some(expected) = expected {
            require(
                identity(&capture.baseline()?)? == expected,
                "stale_revision",
            )?;
        }
        require(capture.commits.len() < MAX_COMMITS, "store_limit")?;
        let prior = capture.heads.get(&request.subject);
        require(
            (request.kind == "add") == prior.is_none(),
            "subject_presence_mismatch",
        )?;
        let source = request
            .source
            .clone()
            .or_else(|| {
                prior.and_then(|id| {
                    capture.objects[id]["body"]["from"]
                        .as_str()
                        .map(String::from)
                })
            })
            .ok_or_else(|| Error("source_required".into()))?;
        let saw = capture
            .objects
            .iter()
            .filter(|(_, o)| o["subject"] == request.subject)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let mut claim = json!({"schema_version":2,"id_scheme":"typed-history/v2","subject":request.subject,"kind":"reading","by":null,"on":request.on,"op":request.operation,"body":{"v":request.value,"from":source},"saw":saw,"authored":authored(),"pins":{}});
        claim["id"] = json!(object_identity(&claim)?);
        let mut act_saw = saw;
        act_saw.push(claim["id"].as_str().unwrap().into());
        act_saw.sort();
        let mut act = json!({"schema_version":2,"id_scheme":"typed-history/v2","subject":request.subject,"kind":"act","by":null,"on":request.on,"op":request.operation,"body":{"act":"accept","of":claim["id"],"over":prior.into_iter().collect::<Vec<_>>(),"because":"Explicit native CLI write"},"saw":act_saw});
        act["id"] = json!(object_identity(&act)?);
        let mut objects = vec![claim, act];
        objects.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        let pairs = objects
            .iter()
            .map(|o| Ok((o.clone(), bytes(o)?)))
            .collect::<Result<Vec<_>>>()?;
        let inventory = pairs
            .iter()
            .map(|(o, raw)| json!({"subject":o["subject"],"id":o["id"],"sha256":sha256(raw)}))
            .collect::<Vec<_>>();
        let mut manifest = json!({"version":1,"record_id":capture.authority["record_id"],"authority_generation":1,"operation":request.operation,
            "parents":capture.frontier(),"baseline_digest":identity(&capture.baseline()?)?,"objects":inventory,
            "receipt":{"scope":"native-feasibility/storage-only","semantic_assessment":"not_performed","request_digest":request_id},
            "view_sha256":sha256(b""),"view_template":capture.template,"requires":CAPABILITIES});
        Self::apply_objects(&mut capture, &objects)?;
        capture.commits.insert(
            request.operation.clone(),
            (manifest.clone(), bytes(&manifest)?),
        );
        let view = bytes(&capture.view()?)?;
        manifest["view_sha256"] = json!(sha256(&view));
        let manifest_bytes = bytes(&manifest)?;
        // Validate all serialized inputs/budgets before the first persistent effect.
        let new_bytes = pairs.iter().map(|(_, raw)| raw.len()).sum::<usize>()
            + manifest_bytes.len()
            + view.len();
        require(
            capture.total_bytes - capture.view_bytes + new_bytes <= MAX_STORE,
            "store_limit",
        )?;
        for (object, raw) in &pairs {
            store.publish(
                &format!(
                    ".kpopper/history/{}",
                    subject_path(
                        object["subject"].as_str().unwrap(),
                        object["id"].as_str().unwrap()
                    )?
                ),
                raw,
                true,
            )?;
        }
        failpoint("objects")?;
        store.publish(
            &format!(".kpopper/history-commits/{}.yaml", request.operation),
            &manifest_bytes,
            true,
        )?;
        failpoint("commit")?;
        store.publish("GROUNDING.yaml", &view, false)?;
        let after = store.load()?;
        Ok(
            json!({"status":"committed","operation":request.operation,"revision":identity(&after.baseline()?)?}),
        )
    }

    pub fn recover(root: &Path) -> Result<Value> {
        let store = Self::acquire(root, true)?;
        let capture = store.load()?;
        store.publish("GROUNDING.yaml", &bytes(&capture.view()?)?, false)?;
        Ok(json!({"status":"recovered","revision":identity(&capture.baseline()?)?}))
    }
}

fn authored() -> Value {
    json!({"collection":"readings","profile":"ordinary-reader/v1","fields":{"value":"v","deps":"rests_on","snapshot":"seen","predicate":"wrong_if"}})
}
fn failpoint(name: &str) -> Result<()> {
    require(
        std::env::var("KPOP_NATIVE_FAIL_AFTER").ok().as_deref() != Some(name),
        &format!("injected_after_{name}"),
    )
}

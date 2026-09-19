//! Detached prepared mutations. Decoding verifies evidence; it grants no write authority.
use crate::{
    Result, history_authority as A,
    history_contract::*,
    history_paths as HP, history_yaml as Y,
    identity::sha256,
    require,
    value::{Integer, TypedValue as V},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use num_bigint::BigInt;
use serde::Deserialize;
use std::path::Path;

pub const MAX_TRANSACTION_BYTES: usize = 64 * 1024 * 1024;
const AUXILIARY_KIND: &str = "history-auxiliary/v1";
const ROLES: &[&str] = &[
    "record",
    "record_member",
    "hypothesis",
    "view",
    "replaced",
    "history_authority",
    "history_object",
    "history_commit",
    "history_retained",
    "history_evidence",
];
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn n(v: &str) -> V {
    V::Integer(Integer::new(v).expect("integer constant"))
}
fn object(fields: &[(&str, V)]) -> V {
    V::Map(
        fields
            .iter()
            .map(|(k, v)| ((*k).into(), v.clone()))
            .collect(),
    )
}
fn hash(v: &V) -> Result<()> {
    require(
        matches!(v,V::Text(t) if t.len()==64 && HP::object_id(t)),
        "invalid_identifier",
    )
}
fn bounded(v: &V) -> Result<()> {
    v.validate().map_err(|_| error("history_limit"))?;
    require(
        v.canonical_bytes()?.len() <= MAX_TRANSACTION_BYTES,
        "history_limit",
    )
}
fn get<'a>(v: &'a V, key: &str) -> Option<&'a V> {
    if let V::Map(m) = v { m.get(key) } else { None }
}
fn mapping<'a>(v: &'a V, code: &str) -> Result<&'a Map> {
    map(v).map_err(|_| error(code))
}
fn mapping_child<'a>(value: Option<&'a V>, key: &str) -> Result<Option<&'a V>> {
    value
        .map(|v| mapping(v, "invalid_journal").map(|m| m.get(key)))
        .transpose()
        .map(Option::flatten)
}
fn list(v: &V) -> Result<&Vec<V>> {
    if let V::List(a) = v {
        Ok(a)
    } else {
        Err(error("invalid_journal"))
    }
}
fn with_digest(mut value: V) -> Result<V> {
    let digest = value.digest()?;
    if let V::Map(m) = &mut value {
        m.insert("digest".into(), s(&digest));
    }
    Ok(value)
}
pub(crate) fn blob(raw: Option<&[u8]>) -> V {
    raw.map_or(V::Null, |raw| {
        object(&[
            ("sha256", s(&sha256(raw))),
            ("data", s(&STANDARD.encode(raw))),
        ])
    })
}
pub(crate) fn unblob(v: &V) -> Result<Option<Vec<u8>>> {
    if *v == V::Null {
        return Ok(None);
    }
    let m = schema(v, &["sha256", "data"], &[])?;
    let encoded = text(&m["data"]).map_err(|_| error("invalid_bytes"))?;
    // Python accepts nonzero padding bits, then rejects them by exact re-encoding.
    let config = base64::engine::general_purpose::GeneralPurposeConfig::new()
        .with_decode_allow_trailing_bits(true);
    let engine = base64::engine::GeneralPurpose::new(&base64::alphabet::STANDARD, config);
    let raw = engine.decode(encoded).map_err(|_| error("invalid_bytes"))?;
    require(blob(Some(&raw)) == *v, "byte_hash_mismatch")?;
    Ok(Some(raw))
}

pub fn semantic_receipt(profile: &str, capabilities: &V, before: &V, after: &V) -> Result<V> {
    require(
        ["ordinary-reader/v1", "checked-reader/v1", "core/v1"].contains(&profile),
        "unsupported_profile",
    )?;
    mapping(capabilities, "invalid_capabilities")?;
    mapping(before, "invalid_evidence")?;
    mapping(after, "invalid_evidence")?;
    let payload = object(&[
        ("version", n("1")),
        ("profile", s(profile)),
        ("capabilities", capabilities.clone()),
        ("before", before.clone()),
        ("after", after.clone()),
    ]);
    bounded(&payload)?;
    with_digest(payload)
}
pub fn validate_receipt(value: &V) -> Result<V> {
    let m = schema(
        value,
        &[
            "version",
            "profile",
            "capabilities",
            "before",
            "after",
            "digest",
        ],
        &[],
    )?;
    require(is_int(&m["version"], "1"), "invalid_receipt")?;
    let profile = text(&m["profile"]).map_err(|_| error("unsupported_profile"))?;
    let expected = semantic_receipt(profile, &m["capabilities"], &m["before"], &m["after"])?;
    require(expected.digest()? == value.digest()?, "invalid_receipt")?;
    Ok(expected)
}

#[derive(Clone, Debug)]
pub struct Layout {
    pub entry: String,
    pub home: String,
    pub hypotheses: String,
    pub view: String,
    pub replaced: String,
    pub authority: String,
    pub objects: String,
    pub commits: String,
    pub cancellations: String,
    pub retained: String,
    pub journal: String,
}
impl Layout {
    pub fn for_entry(entry: &str) -> Result<Self> {
        A::relative_path(entry)?;
        let (parent, name) = entry.rsplit_once('/').map_or(("", entry), |(p, n)| (p, n));
        let join = |name: &str| {
            if parent.is_empty() {
                name.into()
            } else {
                format!("{parent}/{name}")
            }
        };
        let modern = name == "GROUNDING.yaml";
        let home = if modern {
            join(".kpopper")
        } else {
            parent.into()
        };
        let home_path = |name: &str| {
            if home.is_empty() {
                name.into()
            } else {
                format!("{home}/{name}")
            }
        };
        let objects = if modern {
            home_path("history")
        } else {
            join("PROVENANCE.history")
        };
        let view = if modern {
            home_path("view.yaml")
        } else {
            let name = entry
                .strip_suffix(".yaml")
                .or_else(|| entry.strip_suffix(".yml"))
                .unwrap_or(entry);
            format!("{name}.view.yaml")
        };
        Ok(Self {
            entry: entry.into(),
            hypotheses: if modern {
                home_path("hypotheses")
            } else {
                join("PROVENANCE.d")
            },
            view,
            replaced: if modern {
                home_path("replaced.yaml")
            } else {
                join("PROVENANCE.replaced.yaml")
            },
            authority: format!("{objects}.yaml"),
            commits: format!("{objects}-commits"),
            cancellations: format!("{objects}-cancellations"),
            objects,
            retained: join(".kpopper-history-migration"),
            journal: home_path(&format!(
                ".history-local/{}.json",
                &sha256(name.as_bytes())[..24]
            )),
            home,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileImage {
    pub path: String,
    pub role: String,
    pub before: Option<Vec<u8>>,
    pub after: Option<Vec<u8>>,
}
impl FileImage {
    fn encoded(&self) -> V {
        object(&[
            ("path", s(&self.path)),
            ("role", s(&self.role)),
            ("before", blob(self.before.as_deref())),
            ("after", blob(self.after.as_deref())),
        ])
    }
    fn decode(v: &V) -> Result<Self> {
        let m = schema(v, &["path", "role", "before", "after"], &[])?;
        Ok(Self {
            path: text(&m["path"]).map_err(|_| error("invalid_path"))?.into(),
            role: text(&m["role"])
                .map_err(|_| error("invalid_file_role"))?
                .into(),
            before: unblob(&m["before"])?,
            after: unblob(&m["after"])?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct PreparedMutation {
    data: V,
    files: Vec<FileImage>,
}
impl PreparedMutation {
    pub fn prepare(
        operation: &str,
        authority: &V,
        baseline: &V,
        files: Vec<FileImage>,
        receipt: &V,
        entry: &str,
        transition: Option<&V>,
    ) -> Result<Self> {
        token(&s(operation))?;
        let layout = Layout::for_entry(entry)?;
        A::validate_authority(authority)?;
        let marker = map(authority)?;
        let base = mapping(baseline, "invalid_baseline")?;
        let legacy = string_is(&marker["authority"], "legacy");
        let next = if let Some(t) = transition {
            let t = schema(t, &["version", "after"], &[])?;
            require(is_int(&t["version"], "1"), "invalid_authority_transition")?;
            A::validate_authority(&t["after"])?;
            let next = map(&t["after"])?;
            let generation = |v: &V| -> Result<BigInt> {
                text_integer(v)?
                    .parse()
                    .map_err(|_| error("invalid_generation"))
            };
            require(
                next["record_id"] == marker["record_id"]
                    && generation(&next["generation"])? == generation(&marker["generation"])? + 1
                    && next["authority"] != marker["authority"]
                    && base.get("kind") == Some(&s("history-authority-transition/v1"))
                    && base.get("direction")
                        == Some(&s(if string_is(&next["authority"], "history") {
                            "activate"
                        } else {
                            "deactivate"
                        })),
                "invalid_authority_transition",
            )?;
            Some(&t["after"])
        } else {
            None
        };
        let members = base
            .get("record_members")
            .map(|v| mapping(v, "invalid_record_members"))
            .transpose()?;
        let absent = base.get("source_absent") == Some(&V::Bool(true));
        if let Some(members) = members {
            require(members.contains_key(entry), "invalid_record_members")?;
            for (path, digest) in members {
                A::relative_path(path)?;
                if *digest == V::Null {
                    require(path == entry && absent && legacy, "invalid_record_members")?;
                } else {
                    hash(digest)?;
                }
            }
        }
        if absent {
            require(
                members == Some(&Map::from([(entry.into(), V::Null)])) && legacy,
                "invalid_record_members",
            )?;
        }
        let empty = Map::new();
        let hypotheses = base
            .get("hypothesis_members")
            .map(|v| mapping(v, "invalid_hypothesis_members"))
            .transpose()?
            .unwrap_or(&empty);
        for (path, digest) in hypotheses {
            A::relative_path(path)?;
            require(
                child_file(path, &layout.hypotheses, &["yaml", "yml"]),
                "invalid_hypothesis_members",
            )?;
            if *digest != V::Null {
                hash(digest)?;
            }
        }
        let receipt = validate_receipt(receipt)?;
        require(!files.is_empty(), "empty_mutation")?;
        let before = get(&receipt, "before").unwrap();
        let after = get(&receipt, "after").unwrap();
        for item in &files {
            A::relative_path(&item.path)?;
            require(ROLES.contains(&item.role.as_str()), "invalid_file_role")?;
            let expected = match item.role.as_str() {
                "record_member" => {
                    require(legacy, "authority_transition_required")?;
                    require(item.path != entry, "role_path_mismatch")?;
                    let digest = members
                        .and_then(|m| m.get(&item.path))
                        .ok_or_else(|| error("invalid_record_members"))?;
                    require(
                        item.before
                            .as_ref()
                            .is_some_and(|raw| s(&sha256(raw)) == *digest),
                        "record_member_mismatch",
                    )?;
                    &item.path
                }
                "hypothesis" => {
                    require(
                        legacy && hypotheses.contains_key(&item.path),
                        "invalid_hypothesis_members",
                    )?;
                    require(
                        image_hash(&item.before) == hypotheses[&item.path],
                        "hypothesis_member_mismatch",
                    )?;
                    &item.path
                }
                "history_object" => {
                    let obj = decode_image(&item.after)?;
                    validate_object(&obj)?;
                    let obj = map(&obj)?;
                    let suffix = item
                        .path
                        .strip_prefix(&(layout.objects.clone() + "/"))
                        .ok_or_else(|| error("role_path_mismatch"))?;
                    HP::validate_object_path(suffix, text(&obj["subject"])?, text(&obj["id"])?)
                        .map_err(|_| error("role_path_mismatch"))?;
                    &item.path
                }
                "history_commit" => {
                    require(
                        item.path == format!("{}/{operation}.yaml", layout.commits),
                        "role_path_mismatch",
                    )?;
                    &item.path
                }
                "history_evidence" => {
                    let reports = join(&layout.home, "evidence/reports");
                    let edits = join(&layout.home, "evidence/view-edits");
                    let branches = join(&layout.home, "evidence/branches");
                    let stem = Path::new(&item.path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    require(
                        !legacy
                            && next.is_none()
                            && (child_file(&item.path, &reports, &["txt"])
                                || (child_file(&item.path, &edits, &["yaml"])
                                    && view_edit_intent(get(before, "history_edit"))
                                    && mapping_child(get(before, "authoring"), "kind")?
                                        == Some(&s("view-edit-proposals")))
                                || (child_file(&item.path, &branches, &["json"])
                                    && stem.len() == 64
                                    && HP::object_id(stem)
                                    && get(after, "history_branch_adoption").is_some())),
                        "invalid_history_evidence",
                    )?;
                    token(&s(stem))?;
                    &item.path
                }
                "history_retained" => {
                    let declared = base.get("retained_files").and_then(|v| get(v, &item.path));
                    require(
                        next.and_then(|v| get(v, "authority")) == Some(&s("history"))
                            && item.before.is_none()
                            && item.after.is_some()
                            && declared == Some(&image_hash(&item.after)),
                        "invalid_retained_file",
                    )?;
                    require(
                        item.path.starts_with(&(layout.retained.clone() + "/")),
                        "invalid_retained_file",
                    )?;
                    &item.path
                }
                "record" => &layout.entry,
                "view" => &layout.view,
                "replaced" => &layout.replaced,
                "history_authority" => &layout.authority,
                _ => return Err(error("invalid_file_role")),
            };
            require(item.path == *expected, "role_path_mismatch")?;
            if item.role == "record"
                && let Some(members) = members
            {
                require(
                    image_hash(&item.before) == members[entry],
                    "record_member_mismatch",
                )?;
            }
            if immutable(&item.role) {
                require(
                    item.before.is_none() && item.after.is_some(),
                    "immutable_mutation",
                )?;
            }
        }
        let mut files = files;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        require(
            files.windows(2).all(|p| p[0].path != p[1].path),
            "duplicate_path",
        )?;
        // Source-capsule auditing is a distinct downstream contract; never treat its
        // presence as a validated adoption while that validator is unavailable.
        require(
            get(after, "history_branch_adoption").is_none(),
            "unsupported_branch_adoption",
        )?;
        if let Some(next) = next {
            require(
                files.iter().all(|i| {
                    [
                        "record",
                        "history_authority",
                        "history_object",
                        "history_commit",
                        "history_retained",
                    ]
                    .contains(&i.role.as_str())
                }),
                "invalid_transition_role",
            )?;
            let authorities: Vec<_> = files
                .iter()
                .filter(|i| i.role == "history_authority")
                .collect();
            require(authorities.len() == 1, "missing_authority_transition")?;
            let image = authorities[0];
            let decoded = decode_image(&image.after)?;
            A::validate_authority(&decoded)?;
            require(decoded == *next, "authority_transition_mismatch")?;
            if image.before.is_none() {
                require(
                    legacy && is_int(&marker["generation"], "0"),
                    "authority_transition_mismatch",
                )?;
            } else {
                let decoded = decode_image(&image.before)?;
                A::validate_authority(&decoded)?;
                require(decoded == *authority, "authority_transition_mismatch")?;
            }
            require(
                files.iter().filter(|i| i.role == "record").count() == 1,
                "missing_transition_record",
            )?;
            require(
                inventory(&files, "history_retained")
                    == base
                        .get("retained_files")
                        .cloned()
                        .unwrap_or(V::Map(Map::new())),
                "invalid_retained_file",
            )?;
        }
        let history = get(next.unwrap_or(authority), "authority") == Some(&s("history"));
        if history {
            if let Some(authoring) = get(before, "authoring") {
                mapping(authoring, "invalid_journal")?;
            }
            let declared = get(before, "authoring")
                .and_then(|v| get(v, "evidence"))
                .cloned()
                .unwrap_or(V::Map(Map::new()));
            require(
                matches!(declared, V::Map(_)) && inventory(&files, "history_evidence") == declared,
                "history_evidence_mismatch",
            )?;
            let commit_marker = next.unwrap_or(authority);
            let commit_baseline = if next.is_some() {
                base.get("history_baseline").unwrap_or(&V::Null)
            } else {
                baseline
            };
            A::bind_authority(commit_marker, commit_baseline)?;
            let commits: Vec<_> = files
                .iter()
                .filter(|i| i.role == "history_commit")
                .collect();
            require(commits.len() == 1, "missing_commit")?;
            require(
                next.is_some()
                    || !files.iter().any(|i| {
                        ["replaced", "history_authority", "history_retained"]
                            .contains(&i.role.as_str())
                    }),
                "authority_transition_required",
            )?;
            let commit = decode_image(&commits[0].after)?;
            A::validate_commit(&commit)?;
            let commit = map(&commit)?;
            let paths = files
                .iter()
                .filter(|i| i.role == "history_object")
                .map(|i| i.path[layout.objects.len() + 1..].to_owned())
                .collect();
            let capabilities: Vec<String> = commit
                .get("requires")
                .map(|v| {
                    list(v).and_then(|a| a.iter().map(|v| text(v).map(str::to_owned)).collect())
                })
                .transpose()?
                .unwrap_or_default();
            HP::validate_path_capability(&paths, &capabilities)?;
            let cm = map(commit_marker)?;
            require(
                commit["operation"] == s(operation)
                    && commit["record_id"] == cm["record_id"]
                    && commit["authority_generation"] == cm["generation"]
                    && commit["baseline_digest"] == s(&commit_baseline.digest()?)
                    && commit["receipt"].digest()? == receipt.digest()?,
                "commit_mismatch",
            )?;
            let mut objects = A::ObjectBytes::new();
            let mut inv = Vec::new();
            for i in files.iter().filter(|i| i.role == "history_object") {
                let obj = decode_image(&i.after)?;
                let m = map(&obj)?;
                inv.push(object(&[
                    ("subject", m["subject"].clone()),
                    ("id", m["id"].clone()),
                    ("sha256", image_hash(&i.after)),
                ]));
                objects.insert(
                    (text(&m["subject"])?.into(), text(&m["id"])?.into()),
                    i.after.clone().unwrap(),
                );
            }
            inv.sort_by(|a, b| {
                text(get(a, "id").unwrap())
                    .unwrap()
                    .cmp(text(get(b, "id").unwrap()).unwrap())
            });
            require(
                V::List(inv) == commit["objects"],
                "commit_inventory_mismatch",
            )?;
            let views: Vec<_> = files.iter().filter(|i| i.role == "record").collect();
            require(
                views.len() == 1
                    && views[0].after.is_some()
                    && image_hash(&views[0].after) == commit["view_sha256"],
                "commit_view_mismatch",
            )?;
            if next.is_some() {
                require(
                    map(&commit["parents"])?.is_empty(),
                    "transition_requires_initial_commit",
                )?;
                A::committed_objects(
                    commit_marker,
                    &A::Files::from([(operation.into(), commits[0].after.clone().unwrap())]),
                    &objects,
                )?;
            }
        } else {
            require(
                !files.iter().any(|i| {
                    ["history_object", "history_commit", "history_retained"]
                        .contains(&i.role.as_str())
                        || i.role == "history_authority" && next.is_none()
                }),
                "authority_transition_required",
            )?;
        }
        let mut payload = object(&[
            ("version", n(if next.is_some() { "2" } else { "1" })),
            ("operation", s(operation)),
            ("entry", s(entry)),
            ("authority", authority.clone()),
            ("baseline", baseline.clone()),
            (
                "files",
                V::List(files.iter().map(FileImage::encoded).collect()),
            ),
            ("receipt", receipt),
        ]);
        if let Some(next) = next
            && let V::Map(m) = &mut payload
        {
            m.insert(
                "transition".into(),
                object(&[("version", n("1")), ("after", next.clone())]),
            );
        }
        bounded(&payload)?;
        let result = Self {
            data: with_digest(payload)?,
            files,
        };
        if history && next.is_none() {
            result.auxiliary_view()?;
        }
        Ok(result)
    }
    pub fn to_data(&self) -> V {
        self.data.clone()
    }
    pub fn files(&self) -> &[FileImage] {
        &self.files
    }
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let raw = self.data.canonical_bytes()?;
        require(raw.len() <= MAX_TRANSACTION_BYTES, "history_limit")?;
        Ok(raw)
    }
    pub fn from_bytes(raw: &[u8]) -> Result<Self> {
        let value = parse_journal(raw, "invalid_journal")?;
        if get(&value, "kind") == Some(&s("history-authority-group-guard/v1")) {
            return Err(error("group_recovery_required"));
        }
        if get(&value, "kind") == Some(&s(AUXILIARY_KIND)) {
            return Err(error("history_auxiliary_recovery_required"));
        }
        let m = schema(
            &value,
            &[
                "version",
                "operation",
                "entry",
                "authority",
                "baseline",
                "files",
                "receipt",
                "digest",
            ],
            &["transition"],
        )?;
        require(
            (is_int(&m["version"], "1") || is_int(&m["version"], "2"))
                && m.contains_key("transition") == is_int(&m["version"], "2"),
            "invalid_journal",
        )?;
        let empty = Vec::new();
        let items = match &m["files"] {
            V::Map(m) if m.is_empty() => &empty,
            V::Text(s) if s.is_empty() => &empty,
            v => list(v)?,
        };
        let files = items
            .iter()
            .map(FileImage::decode)
            .collect::<Result<Vec<_>>>()?;
        let operation = text(&m["operation"])?;
        let entry = text(&m["entry"]).map_err(|_| error("invalid_path"))?;
        let result = Self::prepare(
            operation,
            &m["authority"],
            &m["baseline"],
            files,
            &m["receipt"],
            entry,
            m.get("transition"),
        )?;
        require(result.data.digest()? == value.digest()?, "invalid_journal")?;
        Ok(result)
    }
    pub fn auxiliary_view(&self) -> Result<Option<&FileImage>> {
        let receipt = get(&self.data, "receipt").unwrap();
        let before = get(receipt, "before").unwrap();
        let after = get(receipt, "after").unwrap();
        let empty = V::Map(Map::new());
        let intent = get(before, "identity_authoring").unwrap_or(&empty);
        mapping(intent, "invalid_journal")?;
        let items: Vec<_> = self.files.iter().filter(|i| i.role == "view").collect();
        if items.is_empty() {
            require(
                !numeric_two(get(intent, "version")),
                "missing_identity_brief",
            )?;
            return Ok(None);
        }
        require(
            get(get(&self.data, "authority").unwrap(), "authority") == Some(&s("history"))
                && get(&self.data, "transition").is_none()
                && items.len() == 1
                && numeric_two(get(intent, "version"))
                && get(intent, "kind") == Some(&s("same")),
            "invalid_history_auxiliary",
        )?;
        let item = items[0];
        let brief = schema(
            get(intent, "brief").unwrap_or(&V::Null),
            &["path", "before_utf8", "before_sha256", "after_sha256"],
            &[],
        )?;
        require(
            item.before.is_some()
                && item.after.is_some()
                && item.before != item.after
                && matches!(brief["before_utf8"], V::Text(_)),
            "invalid_history_auxiliary",
        )?;
        require(
            brief["path"] == s(&item.path)
                && text(&brief["before_utf8"])?.as_bytes() == item.before.as_ref().unwrap()
                && brief["before_sha256"] == image_hash(&item.before)
                && get(intent, "view_sha256") == Some(&brief["before_sha256"])
                && brief["after_sha256"] == image_hash(&item.after),
            "identity_brief_mismatch",
        )?;
        let after_intent = get(after, "identity_authoring").unwrap_or(&empty);
        mapping(after_intent, "invalid_journal")?;
        require(
            get(after_intent, "view_sha256") == Some(&brief["after_sha256"]),
            "identity_brief_mismatch",
        )?;
        Ok(Some(item))
    }
    pub fn auxiliary_envelope(&self) -> Result<Vec<u8>> {
        require(
            self.auxiliary_view()?.is_some(),
            "invalid_history_auxiliary",
        )?;
        let body = object(&[
            ("version", n("1")),
            ("kind", s(AUXILIARY_KIND)),
            ("mutation", blob(Some(&self.to_bytes()?))),
        ]);
        let raw = with_digest(body)?.canonical_bytes()?;
        require(raw.len() <= MAX_TRANSACTION_BYTES, "history_limit")?;
        Ok(raw)
    }
}
// This one Python receipt comparison uses ordinary equality (1 == True == 1.0),
// while the enclosing receipt digest still retains the exact authored type.
fn view_edit_intent(v: Option<&V>) -> bool {
    let Some(V::Map(m)) = v else {
        return false;
    };
    let version = m.get("version");
    m.len() == 2
        && m.get("kind") == Some(&s("view-edit-proposals"))
        && (matches!(version,Some(V::Integer(i)) if i.as_str()=="1")
            || version == Some(&V::Bool(true))
            || matches!(version,Some(V::Float(f)) if f.get()==1.0))
}
fn numeric_two(v: Option<&V>) -> bool {
    matches!(v,Some(V::Integer(i)) if i.as_str()=="2")
        || matches!(v,Some(V::Float(f)) if f.get()==2.0)
}
fn text_integer(v: &V) -> Result<&str> {
    if let V::Integer(i) = v {
        Ok(i.as_str())
    } else {
        Err(error("invalid_generation"))
    }
}
fn join(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.into()
    } else {
        format!("{parent}/{child}")
    }
}
fn child_file(path: &str, parent: &str, extensions: &[&str]) -> bool {
    let path = Path::new(path);
    path.parent() == Some(Path::new(parent))
        && path
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| extensions.contains(&x))
        && path.file_stem().is_some_and(|x| !x.is_empty())
}
fn image_hash(raw: &Option<Vec<u8>>) -> V {
    raw.as_ref().map_or(V::Null, |raw| s(&sha256(raw)))
}
fn immutable(role: &str) -> bool {
    [
        "history_object",
        "history_commit",
        "history_retained",
        "history_evidence",
    ]
    .contains(&role)
}
fn inventory(files: &[FileImage], role: &str) -> V {
    V::Map(
        files
            .iter()
            .filter(|i| i.role == role)
            .map(|i| (i.path.clone(), image_hash(&i.after)))
            .collect(),
    )
}
fn decode_image(raw: &Option<Vec<u8>>) -> Result<V> {
    Y::decode_document(raw.as_deref().ok_or_else(|| error("history_limit"))?)
}

pub(crate) fn parse_journal(raw: &[u8], code: &str) -> Result<V> {
    require(raw.len() <= MAX_TRANSACTION_BYTES, "history_limit")?;
    // Bound syntax depth before disabling serde's smaller default recursion limit.
    // A typed map adds three array levels per semantic value level.
    let mut depth = 0usize;
    let mut string = false;
    let mut escaped = false;
    for &byte in raw {
        if string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                string = false;
            }
        } else if byte == b'"' {
            string = true;
        } else if byte == b'[' || byte == b'{' {
            depth += 1;
            require(depth <= 520, code)?;
        } else if byte == b']' || byte == b'}' {
            depth = depth.saturating_sub(1);
        }
    }
    let mut de = serde_json::Deserializer::from_slice(raw);
    de.disable_recursion_limit();
    let encoded = serde_json::Value::deserialize(&mut de).map_err(|_| error(code))?;
    de.end().map_err(|_| error(code))?;
    V::from_tagged(&encoded).map_err(|_| error(code))
}
pub fn decode_auxiliary_envelope(raw: &[u8]) -> Result<PreparedMutation> {
    let value = parse_journal(raw, "invalid_auxiliary_journal")?;
    let m = schema(&value, &["version", "kind", "mutation", "digest"], &[])?;
    require(
        is_int(&m["version"], "1") && m["kind"] == s(AUXILIARY_KIND),
        "invalid_auxiliary_journal",
    )?;
    let payload = V::Map(
        m.iter()
            .filter(|(k, _)| *k != "digest")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );
    require(
        m["digest"] == s(&payload.digest()?),
        "invalid_auxiliary_journal",
    )?;
    let bytes = unblob(&m["mutation"])?.ok_or_else(|| error("history_limit"))?;
    let mutation = PreparedMutation::from_bytes(&bytes)?;
    require(
        raw == mutation.auxiliary_envelope()?,
        "invalid_auxiliary_journal",
    )?;
    Ok(mutation)
}

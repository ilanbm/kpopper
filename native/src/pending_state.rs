//! Read-only pinned pending ledgers and cached publisher observations. These
//! values describe local evidence and never grant remote publication authority.
use crate::{
    Result, history_authority::Files, history_contract::*, history_view::map_mut, identity::sha256,
    pending_bundle, project_modes::Project, require, value::TypedValue as V,
};
use sha1::Digest;
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
pub const REF: &str = "refs/kpopper/pending_grounding";
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_FILE: usize = 16 * 1024 * 1024;
const MAX_FILES: usize = 100_000;
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn empty() -> V {
    V::Map(Map::new())
}
fn obj(items: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(items.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
pub(crate) fn git(
    root: &Path,
    args: &[&str],
    maximum: usize,
    missing: bool,
) -> Result<Option<Vec<u8>>> {
    let mut cmd = Command::new("git");
    cmd.args([
        "--no-pager",
        "--no-replace-objects",
        "--no-lazy-fetch",
        "-C",
    ])
    .arg(root)
    .args(args);
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        cmd.env_remove(key);
    }
    for (key, value) in [
        ("GIT_NO_LAZY_FETCH", "1"),
        ("GIT_NO_REPLACE_OBJECTS", "1"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_OPTIONAL_LOCKS", "0"),
    ] {
        cmd.env(key, value);
    }
    match crate::reasoning_runtime::run_command_bounded(
        &mut cmd,
        vec![],
        Duration::from_secs(10),
        maximum,
    ) {
        Ok(raw) => Ok(Some(raw)),
        Err(e) if missing && e.0 == "native reasoning process failed" => Ok(None),
        Err(e) => Err(e),
    }
}
fn string(raw: Vec<u8>) -> Result<String> {
    String::from_utf8(raw).map_err(|_| error("invalid_pending_ledger"))
}
fn oid(value: &str) -> bool {
    [40, 64].contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) fn verify_blob(id: &str, raw: &[u8]) -> Result<()> {
    require(oid(id), "invalid_git_object")?;
    let mut preimage = format!("blob {}\0", raw.len()).into_bytes();
    preimage.extend(raw);
    let actual = if id.len() == 40 {
        format!("{:x}", sha1::Sha1::digest(&preimage))
    } else {
        sha256(&preimage)
    };
    require(id == actual, "pending_object_mismatch")
}
pub(crate) fn resolve(root: &Path, reference: &str) -> Result<Option<String>> {
    require(
        !reference.is_empty() && !reference.contains(['\0', '\n', '\r']),
        "invalid_git_ref",
    )?;
    let raw = git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
        1024,
        true,
    )?;
    raw.map(|raw| {
        let value = string(raw)?.trim().to_owned();
        require(oid(&value), "invalid_pending_ledger")?;
        Ok(value)
    })
    .transpose()
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bundle {
    pub value: V,
    pub files: Files,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ledger {
    pub head: Option<String>,
    pub events: Vec<V>,
    pub bundles: BTreeMap<String, Bundle>,
}
struct Reader<'a> {
    root: &'a Path,
    tree: BTreeMap<String, String>,
    cache: Files,
    total: usize,
}
impl Reader<'_> {
    fn read(&mut self, path: &str) -> Result<Vec<u8>> {
        let id = self
            .tree
            .get(path)
            .ok_or_else(|| error("missing_pending_evidence"))?
            .clone();
        if let Some(raw) = self.cache.get(&id) {
            return Ok(raw.clone());
        }
        let raw = git(self.root, &["cat-file", "blob", &id], MAX_FILE, false)?.unwrap();
        self.total = self.total.saturating_add(raw.len());
        require(self.total <= MAX_BYTES, "pending_limit")?;
        verify_blob(&id, &raw)?;
        self.cache.insert(id, raw.clone());
        Ok(raw)
    }
}
fn json(raw: &[u8]) -> Result<V> {
    require(raw.len() <= MAX_FILE, "pending_limit")?;
    let value: serde_json::Value = serde_json::from_slice(raw)?;
    V::from_json(&value)
}
fn typed_json(raw: &[u8]) -> Result<V> {
    require(raw.len() <= MAX_FILE, "pending_limit")?;
    crate::history_transaction::parse_journal(raw, "invalid_pending_manifest")
}
impl Ledger {
    pub fn capture(project: &Project) -> Result<Self> {
        require(project.is_git(), "pending_requires_git")?;
        Self::at(&project.root, resolve(&project.root, REF)?)
    }
    pub fn at(root: &Path, head: Option<String>) -> Result<Self> {
        let Some(id) = &head else {
            return Ok(Self {
                head,
                events: vec![],
                bundles: BTreeMap::new(),
            });
        };
        require(oid(id), "invalid_pending_ledger")?;
        let raw = git(root, &["ls-tree", "-r", "-z", id], MAX_FILE, false)?.unwrap();
        let mut reader = Reader {
            root,
            tree: BTreeMap::new(),
            cache: Files::new(),
            total: 0,
        };
        for row in raw.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let split = row
                .iter()
                .position(|b| *b == b'\t')
                .ok_or_else(|| error("invalid_pending_ledger"))?;
            let info = std::str::from_utf8(&row[..split])
                .map_err(|_| error("invalid_pending_ledger"))?
                .split(' ')
                .collect::<Vec<_>>();
            require(
                info.len() == 3 && info[0] == "100644" && info[1] == "blob" && oid(info[2]),
                "invalid_pending_ledger",
            )?;
            let path = std::str::from_utf8(&row[split + 1..])
                .map_err(|_| error("invalid_pending_ledger"))?;
            crate::history_branch::portable_path(path)?;
            require(
                reader.tree.insert(path.into(), info[2].into()).is_none()
                    && reader.tree.len() <= MAX_FILES,
                "pending_limit",
            )?;
        }
        let event_paths = reader
            .tree
            .keys()
            .filter(|p| p.starts_with("events/"))
            .cloned()
            .collect::<Vec<_>>();
        let mut events = vec![];
        for path in event_paths {
            let event = json(&reader.read(&path)?)?;
            let fields = map(&event)?;
            require(
                matches!(fields.get("sequence"), Some(V::Integer(_))),
                "invalid_pending_event",
            )?;
            let revision = text(
                fields
                    .get("revision")
                    .ok_or_else(|| error("invalid_pending_event"))?,
            )?;
            require(
                revision.len() == 64 && oid(revision),
                "invalid_pending_event",
            )?;
            events.push(event);
        }
        events.sort_by(|a, b| {
            let V::Integer(a) = &map(a).unwrap()["sequence"] else {
                unreachable!()
            };
            let V::Integer(b) = &map(b).unwrap()["sequence"] else {
                unreachable!()
            };
            let a = a.as_str().parse::<num_bigint::BigInt>().unwrap();
            let b = b.as_str().parse::<num_bigint::BigInt>().unwrap();
            a.cmp(&b)
        });
        let mut bundles = BTreeMap::new();
        for event in &events {
            let revision = text(&map(event)?["revision"])?;
            if bundles.contains_key(revision) {
                continue;
            }
            let prefix = format!("contributions/{revision}/");
            let manifest = typed_json(&reader.read(&format!("{prefix}manifest.json"))?)?;
            let evidence = map(map(&manifest)?
                .get("evidence")
                .ok_or_else(|| error("invalid_pending_manifest"))?)?;
            let mut files = Files::new();
            for path in evidence.keys() {
                crate::history_branch::portable_path(path)?;
                files.insert(
                    path.clone(),
                    reader.read(&format!("{prefix}evidence/{path}"))?,
                );
            }
            let value = obj([("revision", s(revision)), ("manifest", manifest)]);
            pending_bundle::validate_archival(&value, &files)?;
            bundles.insert(revision.into(), Bundle { value, files });
        }
        Ok(Self {
            head,
            events,
            bundles,
        })
    }
    pub fn portable(&self) -> V {
        obj([
            ("ref", self.head.as_ref().map(|s0| s(s0)).unwrap_or(V::Null)),
            ("events", V::List(self.events.clone())),
            (
                "bundles",
                V::Map(
                    self.bundles
                        .iter()
                        .map(|(revision, bundle)| {
                            let mut value = bundle.value.clone();
                            map_mut(&mut value).unwrap().insert(
                                "files".into(),
                                V::Map(
                                    bundle
                                        .files
                                        .iter()
                                        .map(|(p, raw)| {
                                            (
                                                p.clone(),
                                                obj([
                                                    ("encoding", s("hex")),
                                                    (
                                                        "data",
                                                        s(&raw
                                                            .iter()
                                                            .map(|b| format!("{b:02x}"))
                                                            .collect::<String>()),
                                                    ),
                                                    ("sha256", s(&sha256(raw))),
                                                ]),
                                            )
                                        })
                                        .collect(),
                                ),
                            );
                            (revision.clone(), value)
                        })
                        .collect(),
                ),
            ),
        ])
    }
}
pub(crate) fn read_private(path: &Path) -> Result<Option<Vec<u8>>> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    require(file.metadata()?.is_file(), "invalid_pending_state")?;
    let mut raw = vec![];
    file.take((MAX_FILE + 1) as u64).read_to_end(&mut raw)?;
    require(raw.len() <= MAX_FILE, "pending_limit")?;
    Ok(Some(raw))
}
pub fn publication_status(raw: Option<&[u8]>, ledger: &Ledger) -> Result<V> {
    let state = raw.map(json).transpose()?.unwrap_or_else(|| {
        obj([
            (
                "version",
                V::Integer(crate::value::Integer::new("1").unwrap()),
            ),
            ("states", empty()),
            ("decisions", empty()),
            ("paused", V::Bool(false)),
            ("pr", V::Null),
            ("expected_head", V::Null),
            (
                "retry_at",
                V::Integer(crate::value::Integer::new("0").unwrap()),
            ),
            (
                "failures",
                V::Integer(crate::value::Integer::new("0").unwrap()),
            ),
            ("intent", V::Null),
        ])
    });
    let state = map(&state)?;
    require(
        state.get("version").is_some_and(|v| is_int(v, "1")),
        "unsupported_publication_state",
    )?;
    let field = |key: &str| {
        state
            .get(key)
            .cloned()
            .ok_or_else(|| error("invalid_publication_state"))
    };
    let states = map(state
        .get("states")
        .ok_or_else(|| error("invalid_publication_state"))?)?;
    let decisions = map(state
        .get("decisions")
        .ok_or_else(|| error("invalid_publication_state"))?)?;
    let mut out = obj([
        (
            "states",
            V::Map(
                ledger
                    .bundles
                    .keys()
                    .map(|revision| {
                        (
                            revision.clone(),
                            states
                                .get(revision)
                                .cloned()
                                .or_else(|| {
                                    decisions
                                        .get(revision)
                                        .and_then(|v| map(v).ok())
                                        .and_then(|m| m.get("state"))
                                        .cloned()
                                })
                                .unwrap_or_else(|| s("captured")),
                        )
                    })
                    .collect(),
            ),
        ),
        ("decisions", V::Map(decisions.clone())),
        (
            "ledger_ref",
            ledger.head.as_ref().map(|h| s(h)).unwrap_or(V::Null),
        ),
        ("verified", V::Bool(false)),
        (
            "last_verified",
            state.get("verified").cloned().unwrap_or(V::Null),
        ),
    ]);
    for key in [
        "paused",
        "pr",
        "expected_head",
        "retry_at",
        "failures",
        "intent",
    ] {
        map_mut(&mut out)?.insert(key.into(), field(key)?);
    }
    Ok(out)
}
pub fn scope_identity(publication: &V) -> Result<String> {
    let p = map(publication)?;
    let value = V::Map(
        ["remote", "repository", "target", "branch"]
            .iter()
            .map(|k| {
                Ok((
                    (*k).into(),
                    p.get(*k)
                        .cloned()
                        .ok_or_else(|| error("invalid_publication_scope"))?,
                ))
            })
            .collect::<Result<Map>>()?,
    );
    Ok(sha256(&serde_json::to_vec(&value.to_json()?)?))
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub ledger: Ledger,
    pub publication: V,
    pub target: Option<V>,
    pub publication_bytes: Option<Vec<u8>>,
    path: PathBuf,
}
impl Observation {
    pub fn capture(project: &Project, config: &V) -> Result<Self> {
        let path = project.state.join("publication.json");
        let publication_bytes = read_private(&path)?;
        let ledger = Ledger::capture(project)?;
        let publication = publication_status(publication_bytes.as_deref(), &ledger)?;
        let mut target = None;
        if let Some(scope) = map(config)?.get("publication").filter(|v| **v != V::Null) {
            let p = map(scope)?;
            let mut reference = format!(
                "refs/remotes/{}/{}",
                text(&p["remote"])?,
                text(&p["target"])?
            );
            if let Some(observed) = map(&publication)?
                .get("last_verified")
                .and_then(|v| map(v).ok())
                && observed.get("scope") == Some(&s(&scope_identity(scope)?))
                && let Some(V::Text(value)) = observed
                    .get("target")
                    .filter(|v| crate::history_view::truth(v))
            {
                reference = value.clone();
            }
            target = Some(obj([
                ("ref", s(&reference)),
                (
                    "revision",
                    resolve(&project.root, &reference)?
                        .map(|h| s(&h))
                        .unwrap_or(V::Null),
                ),
            ]));
        }
        require(
            publication_bytes == read_private(&path)?
                && ledger.head == resolve(&project.root, REF)?,
            "snapshot_changed",
        )?;
        Ok(Self {
            ledger,
            publication,
            target,
            publication_bytes,
            path,
        })
    }
}

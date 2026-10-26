//! Local project discovery and policy-bound record routing. Discovery and reads
//! never configure publication, migrate a record or change Git configuration.
use crate::{
    Result,
    history_authoring::{n, obj, s},
    history_branch::portable_path,
    history_contract::*,
    history_view::map_mut,
    require,
    value::TypedValue as V,
};
use fs2::FileExt;
use serde_json::json;
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Component, Path, PathBuf},
    process::Command,
    time::Duration,
};
const CONFIG_LIMIT: usize = 1024 * 1024;
const OWNER_RECEIPT: &str = ".kpopper/owner.json";
fn git(root: &Path, args: &[&str], missing: bool) -> Result<Option<Vec<u8>>> {
    let mut command = Command::new("git");
    command
        .args([
            "--no-pager",
            "--no-replace-objects",
            "--no-lazy-fetch",
            "-C",
        ])
        .arg(root)
        .args(args);
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(name);
    }
    for (name, value) in [
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("GIT_NO_LAZY_FETCH", "1"),
        ("GIT_NO_REPLACE_OBJECTS", "1"),
    ] {
        command.env(name, value);
    }
    match crate::reasoning_runtime::run_command_bounded(
        &mut command,
        Vec::new(),
        Duration::from_secs(10),
        16 * 1024 * 1024,
    ) {
        Ok(raw) => Ok(Some(raw)),
        Err(e) if missing && e.0 == "native reasoning process failed" => Ok(None),
        Err(e) => Err(e),
    }
}
fn string(raw: Vec<u8>) -> Result<String> {
    String::from_utf8(raw).map_err(|_| error("nonportable_project_path"))
}
fn expanded(path: &Path) -> Result<PathBuf> {
    if let Ok(tail) = path.strip_prefix("~") {
        let home = std::env::var_os("HOME").ok_or_else(|| error("home_directory_unavailable"))?;
        Ok(PathBuf::from(home).join(tail))
    } else {
        require(
            !path.to_string_lossy().starts_with('~'),
            "unsupported_named_home",
        )?;
        Ok(path.into())
    }
}
/// Resolve existing links while allowing a configured destination to be absent.
pub(crate) fn resolved(path: &Path) -> Result<PathBuf> {
    let path = expanded(path)?;
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    };
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => {
                result.push(other);
                if result.exists() {
                    result = result.canonicalize()?;
                }
            }
        }
    }
    Ok(result)
}
fn bounded(path: &Path) -> Result<Option<Vec<u8>>> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    require(
        file.metadata()?.is_file() && file.metadata()?.len() <= CONFIG_LIMIT as u64,
        "project_config_limit",
    )?;
    let mut raw = Vec::new();
    file.take((CONFIG_LIMIT + 1) as u64).read_to_end(&mut raw)?;
    require(raw.len() <= CONFIG_LIMIT, "project_config_limit")?;
    Ok(Some(raw))
}
fn private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mut b = fs::DirBuilder::new();
        b.recursive(true).mode(0o700);
        b.create(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(path)?;
    Ok(())
}
fn atomic_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| error("invalid_path"))?;
    private_directory(parent)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("kpopper"),
        std::process::id()
    ));
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut f = o.open(&temporary)?;
        std::io::Write::write_all(&mut f, bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
fn central_root() -> Result<PathBuf> {
    if let Some(raw) = std::env::var_os("KPOPPER_PROJECTS_DIR") {
        let p = PathBuf::from(raw);
        require(
            p.is_absolute() && !p.as_os_str().is_empty(),
            "KPOPPER_PROJECTS_DIR must be an absolute non-empty path",
        )?;
        return Ok(resolved(&p)?);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| error("HOME is unavailable; set absolute KPOPPER_PROJECTS_DIR"))?;
    let home = PathBuf::from(home);
    require(
        home.is_absolute(),
        "HOME must be an absolute non-empty path",
    )?;
    resolved(&home.join(".kpopper/projects"))
}
fn slug(path: &Path) -> String {
    let raw = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("project")
        .to_ascii_lowercase();
    let mut out = String::new();
    for c in raw.bytes() {
        if c.is_ascii_alphanumeric() {
            out.push(c as char)
        } else if !out.ends_with('-') {
            out.push('-')
        }
    }
    let out = out.trim_matches('-').chars().take(48).collect::<String>();
    if out.is_empty() {
        "project".into()
    } else {
        out
    }
}
fn provisional_hash(anchor: &Path) -> String {
    crate::identity::sha256(anchor.to_string_lossy().as_bytes())[..32].into()
}
fn project_slug(project: &Project) -> String {
    if project.is_git() {
        if let Ok(roots) = project.worktrees() {
            if let Some(primary) = roots.into_iter().find(|root| {
                fs::symlink_metadata(root.join(".git")).is_ok_and(|meta| meta.is_dir())
            }) {
                return slug(&primary);
            }
        }
    }
    slug(&project.root)
}
fn json_line(value: &V) -> Result<Vec<u8>> {
    let mut b = serde_json::to_vec(&value.to_json()?)?;
    b.push(b'\n');
    Ok(b)
}
#[derive(Clone, Debug)]
pub struct Project {
    pub cwd: PathBuf,
    pub root: PathBuf,
    pub common: Option<PathBuf>,
    pub state: PathBuf,
    pub config_path: PathBuf,
}
impl Project {
    pub fn open(cwd: &Path) -> Result<Self> {
        let cwd = resolved(cwd)?;
        require(cwd.is_dir(), "project_directory_unavailable")?;
        let (root, common, state) =
            if let Some(raw) = git(&cwd, &["rev-parse", "--show-toplevel"], true)? {
                let root = resolved(Path::new(string(raw)?.trim()))?;
                let raw = git(&root, &["rev-parse", "--git-common-dir"], false)?
                    .ok_or_else(|| error("project_git_unavailable"))?;
                let common = resolved(&root.join(string(raw)?.trim()))?;
                let state = common.join("kpopper/project");
                (root, Some(common), state)
            } else {
                let mut root = cwd.clone();
                let home = std::env::var_os("HOME").map(PathBuf::from);
                for path in cwd.ancestors() {
                    if path.parent().is_none() || home.as_deref() == Some(path) {
                        break;
                    }
                    if [".kpopper/project.json", "GROUNDING.yaml", "PROVENANCE.yaml"]
                        .iter()
                        .any(|name| path.join(name).exists())
                    {
                        root = path.into();
                        break;
                    }
                }
                let state = root.join(".kpopper");
                (root, None, state)
            };
        let config_path = state.join("project.json");
        Ok(Self {
            cwd,
            root,
            common,
            state,
            config_path,
        })
    }
    pub fn is_git(&self) -> bool {
        self.common.is_some()
    }
    pub fn worktrees(&self) -> Result<Vec<PathBuf>> {
        if !self.is_git() {
            return Ok(vec![self.root.clone()]);
        }
        let raw = git(
            &self.root,
            &["worktree", "list", "--porcelain", "-z"],
            false,
        )?
        .ok_or_else(|| error("project_git_unavailable"))?;
        let mut paths = vec![];
        for field in raw.split(|b| *b == 0) {
            if let Some(path) = field.strip_prefix(b"worktree ") {
                let path = PathBuf::from(string(path.to_vec())?);
                if path.is_dir() {
                    paths.push(resolved(&path)?);
                }
            }
        }
        require(!paths.is_empty(), "project_worktrees_unavailable")?;
        Ok(paths)
    }
    pub fn defaults(&self) -> Result<V> {
        let roots = self.worktrees()?;
        let name = ["GROUNDING.yaml", "PROVENANCE.yaml"]
            .into_iter()
            .find(|name| roots.iter().any(|r| r.join(name).exists()))
            .unwrap_or("GROUNDING.yaml");
        let mut record = name.to_owned();
        let mut mode = if self.is_git() { "advanced" } else { "simple" };
        if let Some(common) = &self.common {
            if let Some(raw) = bounded(&common.join("kpopper-record"))? {
                record = string(raw)?.trim().into();
                require(!record.is_empty(), "registered_record_empty")?;
                let registered = resolved(&self.root.join(expanded(Path::new(&record))?))?;
                for root in &roots {
                    let candidate = root.join(name);
                    require(
                        !candidate.exists() || resolved(&candidate)? == registered,
                        "ambiguous_registered_record",
                    )?;
                }
                if expanded(Path::new(&record))?.is_absolute() {
                    record = registered
                        .to_str()
                        .ok_or_else(|| error("nonportable_project_path"))?
                        .into();
                    mode = "simple";
                } else {
                    portable_path(&record)?;
                }
            } else if !roots.iter().any(|r| r.join(name).exists()) {
                let anchor = self.common.as_ref().unwrap_or(&self.root);
                mode = "simple";
                let record_path = central_root()?.join(format!(
                    "{}-unbound-{}/GROUNDING.yaml",
                    project_slug(self),
                    provisional_hash(anchor)
                ));
                record = record_path
                    .to_str()
                    .ok_or_else(|| error("nonportable_project_path"))?
                    .into();
            }
        } else if roots.iter().any(|r| r.join(name).exists()) {
            record = resolved(&self.root.join(record))?
                .to_str()
                .ok_or_else(|| error("nonportable_project_path"))?
                .into();
        } else {
            let anchor = self.common.as_ref().unwrap_or(&self.root);
            mode = "simple";
            let record_path = central_root()?.join(format!(
                "{}-unbound-{}/GROUNDING.yaml",
                project_slug(self),
                provisional_hash(anchor)
            ));
            record = record_path
                .to_str()
                .ok_or_else(|| error("nonportable_project_path"))?
                .into();
        }
        Ok(obj([
            ("version", n("1")),
            ("mode", s(mode)),
            ("record", s(&record)),
            ("publication", V::Null),
            ("generation", n("0")),
        ]))
    }
    fn decode_config(&self, raw: Option<&[u8]>) -> Result<V> {
        let Some(raw) = raw else {
            return self.defaults();
        };
        let json =
            crate::json_ingress::parse_slice(raw, crate::json_ingress::DuplicateKeys::Reject)?;
        let value = V::from_json(&json)?;
        let m = map(&value)?;
        require(
            m.get("version").is_some_and(|v| is_int(v, "1"))
                && m.get("mode")
                    .is_some_and(|v| string_is(v, "simple") || string_is(v, "advanced"))
                && m.get("generation")
                    .is_some_and(|v| matches!(v,V::Integer(i) if !i.as_str().starts_with('-')))
                && m.get("record")
                    .and_then(|v| text(v).ok())
                    .is_some_and(|v| !v.trim().is_empty()),
            "invalid_project_config",
        )?;
        if string_is(&m["mode"], "advanced") {
            require(self.is_git(), "advanced_requires_git")?;
            portable_path(text(&m["record"])?)?;
        }
        if m.contains_key("default_identity") {
            require(string_is(&m["mode"], "simple"), "generated_binding_invalid")?;
            let i = map(&m["default_identity"])?;
            require(
                is_int(
                    i.get("version")
                        .ok_or_else(|| error("generated_binding_invalid"))?,
                    "1",
                ),
                "generated_binding_invalid",
            )?;
            require(
                uuid::Uuid::parse_str(text(
                    i.get("token")
                        .ok_or_else(|| error("generated_binding_invalid"))?,
                )?)
                .is_ok(),
                "generated_binding_invalid",
            )?;
            require(
                Path::new(text(
                    i.get("owner_anchor")
                        .ok_or_else(|| error("generated_binding_invalid"))?,
                )?)
                .is_absolute(),
                "generated_binding_invalid",
            )?;
            require(
                matches!(i.get("kind"),Some(v) if string_is(v,"git")||string_is(v,"directory")),
                "generated_binding_invalid",
            )?;
            require(
                Path::new(text(&m["record"])?).is_absolute(),
                "generated_binding_invalid",
            )?;
        }
        if let Some(publication) = m.get("publication").filter(|v| **v != V::Null) {
            let p = schema(
                publication,
                &[
                    "remote",
                    "repository",
                    "target",
                    "branch",
                    "standing_permission",
                ],
                &[],
            )?;
            require(
                matches!(p["standing_permission"], V::Bool(_))
                    && ["remote", "repository", "target", "branch"]
                        .iter()
                        .all(|k| text(&p[*k]).is_ok_and(|v| !v.trim().is_empty())),
                "invalid_publication_config",
            )?;
        }
        Ok(value)
    }
    pub fn config(&self) -> Result<V> {
        self.decode_config(bounded(&self.config_path)?.as_deref())
    }
    pub fn generated_default(&self, config: Option<&V>) -> Result<bool> {
        let c = config.cloned().unwrap_or(self.config()?);
        let m = map(&c)?;
        Ok(string_is(m.get("mode").unwrap_or(&V::Null), "simple")
            && m.contains_key("default_identity"))
    }
    fn owner_receipt_path(record: &Path) -> PathBuf {
        record.parent().unwrap_or(record).join(OWNER_RECEIPT)
    }
    fn generated_binding<'a>(&self, config: &'a V) -> Result<(&'a Map, &'a str)> {
        let i = map(map(config)?
            .get("default_identity")
            .ok_or_else(|| error("generated_binding_missing"))?)?;
        require(
            is_int(
                i.get("version")
                    .ok_or_else(|| error("generated_binding_invalid"))?,
                "1",
            ),
            "generated_binding_invalid",
        )?;
        let token = text(
            i.get("token")
                .ok_or_else(|| error("generated_binding_invalid"))?,
        )?;
        require(
            uuid::Uuid::parse_str(token).is_ok(),
            "generated_binding_invalid",
        )?;
        Ok((i, token))
    }
    fn verify_generated_owner(&self, config: &V, mutation: bool) -> Result<()> {
        let (_, token) = self.generated_binding(config)?;
        let receipt = Self::owner_receipt_path(&self.record(Some(config))?);
        let raw = bounded(&receipt)?.ok_or_else(|| error("generated_owner_receipt_missing"))?;
        let value: serde_json::Value = serde_json::from_slice(&raw)?;
        require(
            value["token"].as_str() == Some(token),
            "generated_owner_receipt_mismatch",
        )?;
        if let Some(path) = value["owner_path"].as_str().map(PathBuf::from) {
            let same_git_owner = self.common.as_ref().is_some_and(|common| {
                value["owner_anchor"]
                    .as_str()
                    .is_some_and(|anchor| Path::new(anchor) == common)
            });
            if path != self.root && path.exists() && !same_git_owner {
                return Err(error(if mutation {
                    "generated_binding_copy_requires_recovery: run kpop config --new-project"
                } else {
                    "generated_binding_copy_conflict: run kpop config --new-project"
                }));
            }
        }
        Ok(())
    }
    fn finish_generated_binding(&self, config: &V, allow_nonempty: bool) -> Result<()> {
        let (identity, token) = self.generated_binding(config)?;
        let record = self.record(Some(config))?;
        let expected_anchor = self.common.as_ref().unwrap_or(&self.root);
        require(
            text(
                identity
                    .get("owner_anchor")
                    .ok_or_else(|| error("generated_binding_invalid"))?,
            )? == expected_anchor.to_string_lossy(),
            "generated_binding_owner_conflict",
        )?;
        let parent = record.parent().ok_or_else(|| error("invalid_path"))?;
        if let Ok(meta) = fs::symlink_metadata(parent) {
            require(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "central_record_parent_unavailable",
            )?;
        } else {
            private_directory(parent)?;
        }
        let receipt = Self::owner_receipt_path(&record);
        if let Some(raw) = bounded(&receipt)? {
            let old: serde_json::Value = serde_json::from_slice(&raw)?;
            require(
                old["token"].as_str() == Some(token),
                "generated_owner_receipt_conflict",
            )?;
        } else {
            // A process can die after creating the candidate directory but
            // before the receipt rename. Only an empty candidate (apart from
            // harmless empty control directories) is safe to repair.
            let clean = fs::read_dir(parent)?.try_fold(true, |clean, entry| -> Result<bool> {
                let entry = entry?;
                if !clean {
                    return Ok(false);
                }
                let name = entry.file_name();
                if name == ".kpopper" && entry.file_type()?.is_dir() {
                    Ok(fs::read_dir(entry.path())?.next().is_none())
                } else {
                    Ok(false)
                }
            })?;
            if !clean && !allow_nonempty {
                return Err(error("generated_binding_candidate_conflict"));
            }
        }
        let value = json!({
            "version": 1,
            "token": token,
            "record": record,
            "owner_anchor": self.common.as_ref().unwrap_or(&self.root),
            "owner_path": self.root,
            "config_path": self.config_path,
        });
        atomic_bytes(&receipt, serde_json::to_string(&value)?.as_bytes())
    }
    pub fn bind_for_mutation(&self) -> Result<()> {
        let guard = self.lock()?;
        if guard.raw.is_some() {
            if self.generated_default(Some(&guard.config))? {
                let receipt = Self::owner_receipt_path(&self.record(Some(&guard.config))?);
                if bounded(&receipt)?.is_some() {
                    self.verify_generated_owner(&guard.config, true)?;
                }
                return self.finish_generated_binding(&guard.config, false);
            }
            return Ok(());
        }
        let proposal = self.defaults()?;
        let proposal_record = PathBuf::from(text(field(map(&proposal)?, "record")?)?);
        // A pre-existing in-tree or registered record is legacy state. It must
        // remain in place and must never acquire a generated central identity.
        let candidate = resolved(&self.root.join(expanded(&proposal_record)?))?;
        let registered = self
            .common
            .as_ref()
            .is_some_and(|common| common.join("kpopper-record").is_file());
        if candidate.is_file() || registered {
            return Ok(());
        }
        require(
            self.record(Some(&guard.config))? == proposal_record,
            "default_binding_changed",
        )?;
        if std::env::var_os("CLAUDE_CODE_REMOTE").is_some_and(|v| v == "true")
            && std::env::var_os("KPOPPER_PROJECTS_DIR").is_none()
        {
            return Err(error(
                "fresh_default_requires_absolute_KPOPPER_PROJECTS_DIR",
            ));
        }
        let token = uuid::Uuid::new_v4().to_string();
        let anchor = self.common.as_ref().unwrap_or(&self.root);
        let final_record =
            central_root()?.join(format!("{}-{token}/GROUNDING.yaml", project_slug(self)));
        let mut next = guard.config.clone();
        let m = map_mut(&mut next)?;
        m.insert(
            "record".into(),
            V::Text(final_record.to_string_lossy().into_owned()),
        );
        m.insert(
            "default_identity".into(),
            obj([
                ("version", n("1")),
                ("token", s(&token)),
                ("owner_anchor", s(&anchor.to_string_lossy())),
                ("kind", s(if self.is_git() { "git" } else { "directory" })),
            ]),
        );
        atomic_bytes(&self.config_path, &json_line(&next)?)?;
        self.finish_generated_binding(&next, false)
    }

    /// Validate and repair a generated binding explicitly, including a
    /// completed record whose receipt was lost. This never migrates history.
    pub fn rebind_generated(&self) -> Result<()> {
        let guard = self.lock()?;
        require(
            self.generated_default(Some(&guard.config))?,
            "generated_binding_required",
        )?;
        let record = self.record(Some(&guard.config))?;
        require(
            record.is_absolute() && record.parent().is_some(),
            "generated_binding_invalid",
        )?;
        let (_, token) = self.generated_binding(&guard.config)?;
        require(
            record
                .parent()
                .unwrap()
                .file_name()
                .is_some_and(|name| name.to_string_lossy().contains(token)),
            "generated_binding_invalid",
        )?;
        require(record.is_file(), "generated_binding_record_missing")?;
        if bounded(&Self::owner_receipt_path(&record))?.is_some() {
            self.verify_generated_owner(&guard.config, true)?;
        }
        self.finish_generated_binding(&guard.config, true)
    }

    /// Start a fresh generated identity for a genuine copied workspace. The
    /// original owner and record are left untouched; no data is copied.
    pub fn new_generated_project(&self) -> Result<()> {
        let guard = self.lock()?;
        require(
            self.generated_default(Some(&guard.config))?,
            "generated_binding_required",
        )?;
        let old_record = self.record(Some(&guard.config))?;
        let receipt = Self::owner_receipt_path(&old_record);
        let raw = bounded(&receipt)?.ok_or_else(|| error("generated_owner_receipt_missing"))?;
        let value: serde_json::Value = serde_json::from_slice(&raw)?;
        let owner = PathBuf::from(
            value["owner_path"]
                .as_str()
                .ok_or_else(|| error("generated_owner_receipt_invalid"))?,
        );
        let same_git_owner = self.common.as_ref().is_some_and(|common| {
            value["owner_anchor"]
                .as_str()
                .is_some_and(|anchor| Path::new(anchor) == common)
        });
        require(
            owner != self.root && owner.exists() && !same_git_owner,
            "generated_binding_not_copy",
        )?;
        let token = uuid::Uuid::new_v4().to_string();
        let anchor = self.common.as_ref().unwrap_or(&self.root);
        let record = central_root()?.join(format!("{}-{token}/GROUNDING.yaml", project_slug(self)));
        let mut next = guard.config.clone();
        let m = map_mut(&mut next)?;
        m.insert(
            "record".into(),
            V::Text(record.to_string_lossy().into_owned()),
        );
        m.insert(
            "default_identity".into(),
            obj([
                ("version", n("1")),
                ("token", s(&token)),
                ("owner_anchor", s(&anchor.to_string_lossy())),
                ("kind", s(if self.is_git() { "git" } else { "directory" })),
            ]),
        );
        atomic_bytes(&self.config_path, &json_line(&next)?)?;
        self.finish_generated_binding(&next, false)
    }

    /// Return the current workspace root only when generated ownership is
    /// validated. This is read-only and intentionally returns `None` for
    /// copied or unreachable owner contexts.
    pub fn generated_source_workspace(&self, cwd: &Path) -> Result<Option<PathBuf>> {
        let current = Project::open(cwd)?;
        if !current.config_path.exists() || !current.generated_default(None)? {
            return Ok(None);
        }
        let config = current.config()?;
        let (identity, token) = current.generated_binding(&config)?;
        let anchor = PathBuf::from(text(
            identity
                .get("owner_anchor")
                .ok_or_else(|| error("generated_binding_invalid"))?,
        )?);
        let expected = current.common.as_ref().unwrap_or(&current.root);
        if anchor != *expected {
            return Ok(None);
        }
        let record = current.record(Some(&config))?;
        let receipt = Self::owner_receipt_path(&record);
        if let Some(raw) = bounded(&receipt)? {
            let value: serde_json::Value = serde_json::from_slice(&raw)?;
            if value["token"].as_str() != Some(token) {
                return Ok(None);
            }
            if let Some(owner) = value["owner_path"].as_str() {
                let owner = Path::new(owner);
                if owner.exists() && owner != current.root && current.common.is_none() {
                    return Ok(None);
                }
            }
        } else if !record.is_file() {
            return Ok(None);
        }
        Ok(Some(current.root))
    }
    pub fn record(&self, config: Option<&V>) -> Result<PathBuf> {
        let config = if let Some(v) = config {
            v.clone()
        } else {
            self.config()?
        };
        let name = text(field(map(&config)?, "record")?)?;
        resolved(&self.root.join(expanded(Path::new(name))?))
    }
    /// Hold through final publication and call verify to reject policy changes.
    pub fn lock(&self) -> Result<PolicyGuard> {
        let (anchor, relative) = if let Some(common) = &self.common {
            (common.as_path(), "kpopper/project/project.lock")
        } else {
            (self.root.as_path(), ".kpopper/project.lock")
        };
        crate::history_transaction_fs::target(anchor, relative)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true).mode(0o700);
            builder.create(&self.state)?;
        }
        #[cfg(not(unix))]
        fs::create_dir_all(&self.state)?;
        let path = crate::history_transaction_fs::target(anchor, relative)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        FileExt::lock_exclusive(&file)?;
        let raw = bounded(&self.config_path)?;
        let config = self.decode_config(raw.as_deref())?;
        Ok(PolicyGuard {
            file,
            project: self.clone(),
            raw,
            config,
        })
    }
}
pub struct PolicyGuard {
    file: File,
    project: Project,
    raw: Option<Vec<u8>>,
    config: V,
}
impl PolicyGuard {
    pub fn config(&self) -> &V {
        &self.config
    }
    pub fn verify(&self) -> Result<()> {
        let actual = Project::open(&self.project.cwd)?;
        require(
            actual.root == self.project.root
                && actual.common == self.project.common
                && actual.state == self.project.state,
            "project_policy_changed",
        )?;
        require(
            bounded(&self.project.config_path)? == self.raw,
            "project_policy_changed",
        )?;
        require(
            self.project.config()? == self.config,
            "project_policy_changed",
        )
    }
}
impl Drop for PolicyGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}
/// The explicitly configured caller may own an external shared record.
pub fn project_for(paths: &[PathBuf], cwd: &Path) -> Result<Project> {
    let first = resolved(paths.first().ok_or_else(|| error("missing_record_path"))?)?;
    let parent = first.parent().ok_or_else(|| error("invalid_path"))?;
    let current = Project::open(cwd)?;
    let current_matches = if current.config_path.exists() {
        let configured = current.record(None)?;
        let generated_alias = current.generated_default(None)?
            && (first == current.root.join("GROUNDING.yaml")
                || first == current.root.join("PROVENANCE.yaml")
                || current
                    .defaults()
                    .ok()
                    .and_then(|v| map(&v).ok().and_then(|m| m.get("record").cloned()))
                    .and_then(|v| text(&v).ok().map(PathBuf::from))
                    .is_some_and(|proposal| proposal == first));
        configured == first || generated_alias
    } else {
        let matches = current.defaults().ok().is_some_and(|value| {
            map(&value)
                .ok()
                .and_then(|m| m.get("record"))
                .and_then(|value| text(value).ok())
                .is_some_and(|path| Path::new(path) == first)
        });
        matches
    };
    if current_matches {
        return Ok(current);
    }
    if let Some(raw) = bounded(&Project::owner_receipt_path(&first))? {
        let v: serde_json::Value = serde_json::from_slice(&raw)?;
        if let (Some(owner), Some(config)) = (v["owner_path"].as_str(), v["config_path"].as_str()) {
            if let Ok(p) = Project::open(Path::new(owner)) {
                if p.config_path == Path::new(config) && p.record(None)? == first {
                    return Ok(p);
                }
            }
        }
    }
    Ok(Project::open(parent)?)
}
pub fn write_paths(paths: &[PathBuf], cwd: &Path) -> Result<Vec<PathBuf>> {
    let mut result = paths.to_vec();
    if let Some(record) = simple_record(paths, cwd)? {
        result[0] = record;
    }
    Ok(result)
}
/// The one record a configured Simple project reads in place of the first of `paths`,
/// when that path is an entry name at the project's root or the record itself.
pub(crate) fn simple_record(paths: &[PathBuf], cwd: &Path) -> Result<Option<PathBuf>> {
    let project = project_for(paths, cwd)?;
    let first = resolved(&paths[0])?;
    if project.config_path.exists() {
        let config = project.config()?;
        let record = project.record(Some(&config))?;
        if string_is(&map(&config)?["mode"], "simple")
            && [
                project.root.join("GROUNDING.yaml"),
                project.root.join("PROVENANCE.yaml"),
                record.clone(),
            ]
            .contains(&first)
        {
            return Ok(Some(record));
        }
        if project.generated_default(Some(&config))? {
            let proposal = project.defaults()?;
            if text(field(map(&proposal)?, "record")?).is_ok_and(|path| Path::new(path) == first) {
                return Ok(Some(record));
            }
        }
    }
    Ok(None)
}

/// A captured write route holds the project policy lock until its caller has
/// finished publication. Pending overlay capture remains required when indicated.
pub struct WriteRoute {
    policy: PolicyGuard,
    original: Vec<PathBuf>,
    selected: Vec<PathBuf>,
    cwd: PathBuf,
}
impl WriteRoute {
    pub fn capture_mutation(paths: &[PathBuf], cwd: &Path) -> Result<Self> {
        let project = project_for(paths, cwd)?;
        let first = resolved(paths.first().ok_or_else(|| error("missing_record_path"))?)?;
        let owned = if project.config_path.exists() {
            let config = project.config()?;
            let record = project.record(Some(&config))?;
            first == record
                || first == project.root.join("GROUNDING.yaml")
                || first == project.root.join("PROVENANCE.yaml")
        } else {
            let proposal = project.defaults().ok().and_then(|value| {
                map(&value)
                    .ok()
                    .and_then(|m| m.get("record"))
                    .and_then(|value| text(value).ok())
                    .map(PathBuf::from)
            });
            first == project.root.join("GROUNDING.yaml")
                || first == project.root.join("PROVENANCE.yaml")
                || proposal.is_some_and(|path| path == first)
        };
        require(owned, "mutation_record_not_owned")?;
        project.bind_for_mutation()?;
        Self::capture(paths, cwd)
    }
    pub fn capture(paths: &[PathBuf], cwd: &Path) -> Result<Self> {
        let project = project_for(paths, cwd)?;
        let policy = project.lock()?;
        let selected = write_paths(paths, cwd)?;
        let route = Self {
            policy,
            original: paths.to_vec(),
            selected,
            cwd: resolved(cwd)?,
        };
        route.verify()?;
        Ok(route)
    }
    pub fn paths(&self) -> &[PathBuf] {
        &self.selected
    }
    pub fn config(&self) -> &V {
        self.policy.config()
    }
    pub fn project(&self) -> &Project {
        &self.policy.project
    }
    pub fn pending_required(&self) -> Result<bool> {
        Ok(self.project().is_git()
            && string_is(&map(self.config())?["mode"], "advanced")
            && resolved(&self.selected[0])? == self.project().record(Some(self.config()))?)
    }
    pub fn verify(&self) -> Result<()> {
        self.policy.verify()?;
        let current = project_for(&self.original, &self.cwd)?;
        require(
            current.root == self.project().root
                && current.state == self.project().state
                && write_paths(&self.original, &self.cwd)? == self.selected,
            "project_route_changed",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value as J, json};
    fn replace(v: &mut J, root: &str) {
        match v {
            J::String(s) => {
                *s = if let Some(relative) = s.strip_prefix("$ROOT/") {
                    Path::new(root)
                        .join(relative)
                        .to_string_lossy()
                        .into_owned()
                } else {
                    s.replace("$ROOT", root)
                }
            }
            J::Array(a) => a.iter_mut().for_each(|v| replace(v, root)),
            J::Object(m) => m.values_mut().for_each(|v| replace(v, root)),
            _ => {}
        }
    }
    fn command(root: &Path, args: &[&str]) {
        let o = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
    fn write(path: &Path, raw: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, raw).unwrap();
    }
    #[test]
    fn project_discovery_configuration_and_alias_routes_match_python() {
        let cases: J =
            serde_json::from_str(include_str!("../tests/fixtures/project-modes.json")).unwrap();
        for case in cases.as_array().unwrap() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            fs::create_dir(root.join("nested")).unwrap();
            let mut c = case.clone();
            replace(&mut c, root.to_str().unwrap());
            if c["git"] == true {
                command(&root, &["init", "-b", "main"]);
            }
            for (path, raw) in c["files"].as_object().unwrap() {
                write(&root.join(path), raw.as_str().unwrap().as_bytes());
            }
            let project = Project::open(&root).unwrap();
            if !c["config"].is_null() {
                write(
                    &project.config_path,
                    &serde_json::to_vec(&c["config"]).unwrap(),
                );
            }
            let result = (|| -> Result<J> {
                let project = Project::open(&root.join(c["cwd"].as_str().unwrap_or(".")))?;
                let config = project.config()?;
                let record = project.record(None)?;
                let paths = write_paths(&[root.join("GROUNDING.yaml")], &root)?;
                Ok(
                    json!({"root":project.root,"common":project.common,"state":project.state,"config_path":project.config_path,"config":config.to_json()?,"record":record,"worktrees":project.worktrees()?,"write_paths":paths}),
                )
            })();
            if let Some(expected) = c.get("output") {
                let actual = result.unwrap_or_else(|e| panic!("{}: {e}", c["name"]));
                if c["name"] == "plain" || c["name"] == "git-default" {
                    assert_eq!(actual["config"]["mode"], "simple");
                    assert!(
                        actual["config"]["record"]
                            .as_str()
                            .unwrap()
                            .contains("-unbound-")
                    );
                    assert_eq!(actual["record"], actual["config"]["record"]);
                    assert!(!Path::new(actual["record"].as_str().unwrap()).exists());
                    if c["name"] == "git-default" {
                        assert!(actual["common"].as_str().unwrap().ends_with("/.git"));
                        assert!(
                            actual["state"]
                                .as_str()
                                .unwrap()
                                .ends_with("/.git/kpopper/project")
                        );
                    }
                } else {
                    assert_eq!(actual, *expected, "{}", c["name"]);
                }
            } else {
                assert!(result.is_err(), "accepted {}", c["name"]);
            }
        }
    }

    #[test]
    fn private_number_object_cannot_forge_project_config_version() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let project = Project::open(&root).unwrap();
        let raw = br#"{"version":{"$serde_json::private::Number":"1"},"mode":"simple","record":"GROUNDING.yaml","publication":null,"generation":0}"#;
        assert_eq!(
            project.decode_config(Some(raw)).unwrap_err().0,
            "invalid_project_config"
        );
    }

    #[test]
    fn fresh_defaults_are_provisional_and_do_not_create_state() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let project = Project::open(&root).unwrap();
        let config = project.config().unwrap();
        assert!(project.generated_default(Some(&config)).unwrap() == false);
        let record = project.record(Some(&config)).unwrap();
        assert!(record.to_string_lossy().contains("-unbound-"));
        assert!(!record.exists());
        assert!(!project.config_path.exists());
    }

    #[test]
    fn legacy_record_is_not_rebound_to_central_storage() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        write(&root.join("GROUNDING.yaml"), b"known: {}\n");
        let project = Project::open(&root).unwrap();
        project.bind_for_mutation().unwrap();
        assert!(!project.config_path.exists());
        assert_eq!(project.record(None).unwrap(), root.join("GROUNDING.yaml"));
    }

    #[test]
    fn legacy_git_record_and_missing_registration_are_not_bound() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir(&root).unwrap();
        command(&root, &["init", "-b", "main"]);
        write(&root.join("GROUNDING.yaml"), b"known: {}\n");
        let project = Project::open(&root).unwrap();
        project.bind_for_mutation().unwrap();
        assert!(!project.config_path.exists());
        fs::remove_file(root.join("GROUNDING.yaml")).unwrap();
        let missing = dir.path().join("missing.yaml");
        write(
            &project.common.as_ref().unwrap().join("kpopper-record"),
            missing.to_str().unwrap().as_bytes(),
        );
        project.bind_for_mutation().unwrap();
        assert!(!project.config_path.exists());
    }

    #[test]
    fn first_mutation_persists_token_and_receipt_then_reuses_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let project = Project::open(&root).unwrap();
        project.bind_for_mutation().unwrap();
        let first = project.config().unwrap();
        let record = project.record(Some(&first)).unwrap();
        let identity = map(&first).unwrap().get("default_identity").unwrap();
        let token = text(map(identity).unwrap().get("token").unwrap()).unwrap();
        assert!(uuid::Uuid::parse_str(token).is_ok());
        assert!(
            record
                .parent()
                .unwrap()
                .join(".kpopper/owner.json")
                .is_file()
        );
        project.bind_for_mutation().unwrap();
        assert_eq!(project.record(None).unwrap(), record);
    }

    #[test]
    fn interrupted_empty_binding_repairs_same_token_but_nonempty_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let project = Project::open(&root).unwrap();
        project.bind_for_mutation().unwrap();
        let config = project.config().unwrap();
        let record = project.record(Some(&config)).unwrap();
        let parent = record.parent().unwrap();
        let receipt = parent.join(".kpopper/owner.json");
        let token = text(
            map(map(&config).unwrap().get("default_identity").unwrap())
                .unwrap()
                .get("token")
                .unwrap(),
        )
        .unwrap()
        .to_owned();
        fs::remove_file(&receipt).unwrap();
        project.bind_for_mutation().unwrap();
        let repaired: serde_json::Value =
            serde_json::from_slice(&fs::read(&receipt).unwrap()).unwrap();
        assert_eq!(repaired["token"].as_str(), Some(token.as_str()));
        fs::remove_file(&receipt).unwrap();
        fs::write(parent.join("unexpected"), b"foreign").unwrap();
        assert_eq!(
            project.bind_for_mutation().unwrap_err().0,
            "generated_binding_candidate_conflict"
        );
    }

    #[test]
    fn explicit_rebind_repairs_completed_record_and_copy_has_no_source_context() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let project = Project::open(&root).unwrap();
        project.bind_for_mutation().unwrap();
        let config = project.config().unwrap();
        let record = project.record(Some(&config)).unwrap();
        fs::write(&record, b"known: {}\n").unwrap();
        let receipt = Project::owner_receipt_path(&record);
        fs::remove_file(&receipt).unwrap();
        project.rebind_generated().unwrap();
        assert!(receipt.is_file());
        assert_eq!(
            project.generated_source_workspace(&root).unwrap(),
            Some(root.clone())
        );

        let copy = dir.path().join("copy");
        fs::create_dir(&copy).unwrap();
        fs::create_dir(copy.join(".kpopper")).unwrap();
        fs::copy(&project.config_path, copy.join(".kpopper/project.json")).unwrap();
        let copied = Project::open(&copy).unwrap();
        assert_eq!(copied.generated_source_workspace(&copy).unwrap(), None);
    }

    #[test]
    fn common_policy_and_registered_record_are_shared_across_worktrees() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir(&root).unwrap();
        command(&root, &["init", "-b", "main"]);
        command(
            &root,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "fixture",
            ],
        );
        let other = dir.path().join("other");
        command(
            &root,
            &["worktree", "add", "--detach", other.to_str().unwrap()],
        );
        write(&other.join("PROVENANCE.yaml"), b"known: {}\n");
        let main = Project::open(&root).unwrap();
        let linked = Project::open(&other).unwrap();
        assert_eq!(main.state, linked.state);
        assert_eq!(
            map(&main.config().unwrap()).unwrap()["record"],
            s("PROVENANCE.yaml")
        );
        let registered = dir
            .path()
            .canonicalize()
            .unwrap()
            .join("absent-shared.yaml");
        write(
            &main.common.as_ref().unwrap().join("kpopper-record"),
            registered.to_str().unwrap().as_bytes(),
        );
        assert!(main.config().is_err());
        fs::remove_file(other.join("PROVENANCE.yaml")).unwrap();
        assert_eq!(main.record(None).unwrap(), registered);
        assert_eq!(linked.record(None).unwrap(), registered);
        assert_eq!(map(&main.config().unwrap()).unwrap()["mode"], s("simple"));
    }
    #[test]
    fn policy_guard_rechecks_bytes_and_rejects_symlinked_lock_state() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("GROUNDING.yaml"), b"known: {}\n");
        let project = Project::open(dir.path()).unwrap();
        let guard = project.lock().unwrap();
        guard.verify().unwrap();
        write(
            &project.config_path,
            b"{\"version\":1,\"mode\":\"simple\",\"record\":\"other.yaml\",\"generation\":1}",
        );
        assert!(guard.verify().is_err());
        drop(guard);
        #[cfg(unix)]
        {
            let second = tempfile::tempdir().unwrap();
            let external = tempfile::tempdir().unwrap();
            write(&second.path().join("GROUNDING.yaml"), b"known: {}\n");
            std::os::unix::fs::symlink(external.path(), second.path().join(".kpopper")).unwrap();
            assert!(Project::open(second.path()).unwrap().lock().is_err());
            assert!(!external.path().join("project.lock").exists());
        }
    }

    #[test]
    fn write_route_keeps_policy_lock_and_identifies_pending_overlay_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        command(&root, &["init", "-b", "main"]);
        let paths = vec![root.join("GROUNDING.yaml")];
        let route = WriteRoute::capture(&paths, &root).unwrap();
        assert!(!route.pending_required().unwrap());
        route.verify().unwrap();
        let competing = OpenOptions::new()
            .read(true)
            .write(true)
            .open(route.project().state.join("project.lock"))
            .unwrap();
        assert!(FileExt::try_lock_exclusive(&competing).is_err());
        drop(route);
        FileExt::try_lock_exclusive(&competing).unwrap();
        FileExt::unlock(&competing).unwrap();
        let unrelated = WriteRoute::capture(&[root.join("artifact.yaml")], &root).unwrap();
        assert!(!unrelated.pending_required().unwrap());
        unrelated.verify().unwrap();
    }
}

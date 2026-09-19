//! Local project discovery and policy-bound record routing. Discovery and reads
//! never configure publication, migrate a record or change Git configuration.
use crate::{
    Result,
    history_authoring::{n, obj, s},
    history_branch::portable_path,
    history_contract::*,
    require,
    value::TypedValue as V,
};
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Component, Path, PathBuf},
    process::Command,
    time::Duration,
};
const CONFIG_LIMIT: usize = 1024 * 1024;
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
                    paths.push(path);
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
            }
        } else {
            record = resolved(&self.root.join(record))?
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
    let project = Project::open(parent)?;
    if project.is_git() {
        return Ok(project);
    }
    let current = Project::open(cwd)?;
    if current.is_git() && current.config_path.exists() && current.record(None)? == first {
        Ok(current)
    } else {
        Ok(project)
    }
}
pub fn write_paths(paths: &[PathBuf], cwd: &Path) -> Result<Vec<PathBuf>> {
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
            let mut result = paths.to_vec();
            result[0] = record;
            return Ok(result);
        }
    }
    Ok(paths.to_vec())
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
            J::String(s) => *s = s.replace("$ROOT", root),
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
                assert_eq!(
                    result.unwrap_or_else(|e| panic!("{}: {e}", c["name"])),
                    *expected,
                    "{}",
                    c["name"]
                );
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
        assert!(route.pending_required().unwrap());
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

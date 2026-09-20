//! Durable watch configuration, queued work and native processor coordination.
use crate::{Error, Result, require, value::TypedValue as V};
use fs2::FileExt;
use serde_json::{Value as J, json};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) fn error(message: impl Into<String>) -> Error {
    Error(message.into())
}
pub(crate) fn truth(value: &J) -> bool {
    match value {
        J::Null => false,
        J::Bool(v) => *v,
        J::String(v) => !v.is_empty(),
        J::Array(v) => !v.is_empty(),
        J::Object(v) => !v.is_empty(),
        J::Number(v) => v.as_f64() != Some(0.0),
    }
}
pub(crate) fn typed(value: &J) -> Result<V> {
    V::from_json(value)
}
pub fn digest(value: &J) -> Result<String> {
    digest_value(&typed(value)?)
}
pub(crate) fn digest_value(value: &V) -> Result<String> {
    Ok(crate::identity::sha256(&crate::history_emit::encode_value(
        value,
    )?))
}
pub(crate) fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
pub(crate) fn load(path: &Path) -> Result<Option<J>> {
    match fs::read(path) {
        Ok(raw) => Ok(Some(serde_json::from_slice(&raw)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub(crate) fn bytes(value: &J) -> Result<Vec<u8>> {
    let mut raw = crate::ordinary_assessment_report::compact_json(&typed(value)?)?.into_bytes();
    raw.push(b'\n');
    Ok(raw)
}
pub(crate) fn atomic(path: &Path, raw: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| error("invalid watch state path"))?;
    private_dir(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(raw)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| error(e.error.to_string()))?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
pub(crate) fn save(path: &Path, value: &J) -> Result<()> {
    atomic(path, &bytes(value)?)
}
pub(crate) fn lock(path: &Path, blocking: bool) -> Result<Option<File>> {
    private_dir(
        path.parent()
            .ok_or_else(|| error("invalid watch lock path"))?,
    )?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    if blocking {
        file.lock_exclusive()?;
    } else {
        match file.try_lock_exclusive() {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(Some(file))
}
pub(crate) fn json_files(folder: &Path) -> Result<Vec<PathBuf>> {
    if !folder.is_dir() {
        return Ok(vec![]);
    }
    let mut paths = fs::read_dir(folder)?
        .map(|item| item.map(|item| item.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.retain(|path| path.extension().is_some_and(|v| v == "json"));
    paths.sort();
    Ok(paths)
}
pub(crate) fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    git_with_program("git", cwd, args, std::time::Duration::from_secs(10))
}

fn git_with_program(
    program: &str,
    cwd: &Path,
    args: &[&str],
    timeout: std::time::Duration,
) -> Result<String> {
    let mut command = Command::new(program);
    command.arg("-C").arg(cwd).args(args);
    let out = crate::reasoning_runtime::run_command_capture(
        &mut command, Vec::new(), timeout, 4 * 1024 * 1024,
    ).map_err(|e| match e.0.as_str() {
        "runtime_timeout" => error(format!("Git command timed out after {} seconds", timeout.as_secs())),
        "output_limit" => error("Git command output exceeded 4194304 bytes"),
        _ => e,
    })?;
    require(
        out.status.success(),
        &format!(
            "Git could not read {}: {}",
            args.iter().take(2).copied().collect::<Vec<_>>().join(" "),
            String::from_utf8_lossy(&out.stderr)
                .trim()
                .chars()
                .take(300)
                .collect::<String>()
        ),
    )?;
    Ok(String::from_utf8(out.stdout)
        .map_err(|e| error(e.to_string()))?
        .trim()
        .into())
}

#[cfg(all(test, unix))]
mod git_tests {
    use super::git_with_program;
    use std::{fs, os::unix::fs::PermissionsExt, path::Path, time::Duration};

    #[test]
    fn wedged_git_is_bounded_without_a_repository() {
        let temp = tempfile::tempdir().unwrap();
        let fake = temp.path().join("git");
        fs::write(&fake, b"#!/bin/sh\n/bin/sleep 1\n").unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let started = std::time::Instant::now();
        assert_eq!(
            git_with_program(
                &fake.to_string_lossy(),
                Path::new("."),
                &["status"],
                Duration::from_millis(20)
            )
            .unwrap_err()
            .0,
            "Git command timed out after 0 seconds"
        );
        assert!(started.elapsed() < Duration::from_millis(700));
    }

    #[test]
    fn exited_git_with_inherited_pipes_still_obeys_timeout() {
        let temp = tempfile::tempdir().unwrap();
        let fake = temp.path().join("git");
        fs::write(&fake, b"#!/bin/sh\ntrap '' HUP\n/bin/sleep 1 &\nexit 0\n").unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let started = std::time::Instant::now();
        let result = git_with_program(&fake.to_string_lossy(), Path::new("."),
            &["status"], Duration::from_millis(20));
        assert!(result.is_err(), "incomplete pipe capture cannot be success");
        assert!(started.elapsed() < Duration::from_millis(700));
    }

    #[test]
    fn oversized_git_output_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let fake = temp.path().join("git");
        fs::write(&fake, b"#!/bin/sh\nyes x | head -c 5000000\nexit 0\n").unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let result = git_with_program(&fake.to_string_lossy(), Path::new("."),
            &["status"], Duration::from_secs(3));
        assert!(result.is_err(), "oversized output must not be returned as a complete result");
        assert_eq!(result.err().unwrap().0, "Git command output exceeded 4194304 bytes");
    }

    #[test]
    fn failed_git_keeps_its_stderr_diagnostic() {
        let temp = tempfile::tempdir().unwrap();
        let fake = temp.path().join("git");
        fs::write(&fake, b"#!/bin/sh\nprintf 'fixture refusal' >&2\nexit 7\n").unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let result = git_with_program(&fake.to_string_lossy(), Path::new("."),
            &["status"], Duration::from_secs(3));
        assert_eq!(result.unwrap_err().0, "Git could not read status: fixture refusal");
    }

    #[test]
    fn git_output_is_drained_beyond_pipe_capacity() {
        let temp = tempfile::tempdir().unwrap();
        let fake = temp.path().join("git");
        fs::write(&fake, b"#!/bin/sh\nyes x | head -c 200000\n").unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            git_with_program(
                &fake.to_string_lossy(),
                Path::new("."),
                &["status"],
                Duration::from_secs(3)
            )
            .unwrap()
            .len(),
            199999
        );
    }
}
fn absolute(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    let parent = path.parent().ok_or_else(|| error("invalid watch path"))?;
    Ok(absolute(parent)?.join(
        path.file_name()
            .ok_or_else(|| error("invalid watch path"))?,
    ))
}
#[derive(Clone)]
pub struct Clock {
    seconds: Arc<dyn Fn() -> f64 + Send + Sync>,
    nanos: Arc<dyn Fn() -> u128 + Send + Sync>,
}
impl Default for Clock {
    fn default() -> Self {
        Self {
            seconds: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs_f64()
            }),
            nanos: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            }),
        }
    }
}
impl Clock {
    pub fn at(seconds: f64) -> Self {
        Self {
            seconds: Arc::new(move || seconds),
            nanos: Arc::new(move || (seconds * 1e9) as u128),
        }
    }
    pub fn seconds(&self) -> f64 {
        (self.seconds)()
    }
    pub fn nanos(&self) -> u128 {
        (self.nanos)()
    }
}
#[derive(Clone)]
pub struct Watch {
    pub cwd: PathBuf,
    pub tree: PathBuf,
    pub common: PathBuf,
    pub record: PathBuf,
    pub entry: String,
    pub config_path: PathBuf,
    pub key: String,
    pub project_state: PathBuf,
    pub state: PathBuf,
    pub state_home: PathBuf,
    pub clock: Clock,
}
impl Watch {
    pub fn open(directory: &Path) -> Result<Self> {
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
            });
        Self::in_state(directory, &state, Clock::default())
    }
    pub fn in_state(directory: &Path, state: &Path, clock: Clock) -> Result<Self> {
        let cwd = absolute(directory)?;
        let tree = PathBuf::from(git(&cwd, &["rev-parse", "--show-toplevel"])?).canonicalize()?;
        let common = absolute(&cwd.join(git(&cwd, &["rev-parse", "--git-common-dir"])?))?;
        let found = crate::public_workspace::locate(&cwd, crate::source_capture::ReadMode::Live)?;
        let mut record = absolute(&found.record)?;
        let mut entry = record
            .strip_prefix(&tree)
            .map_err(|_| error("watch record must be inside its checkout"))?
            .to_string_lossy()
            .replace('\\', "/");
        let mut candidates = vec![entry.clone()];
        if record
            .file_name()
            .is_some_and(|n| n == "GROUNDING.yaml" || n == "PROVENANCE.yaml")
        {
            for name in ["GROUNDING.yaml", "PROVENANCE.yaml"] {
                let candidate = Path::new(&entry)
                    .with_file_name(name)
                    .to_string_lossy()
                    .replace('\\', "/");
                if candidate != entry {
                    candidates.push(candidate);
                }
            }
        }
        let mut paths = vec![];
        for candidate in candidates {
            paths.push((
                candidate.clone(),
                common
                    .join("kpopper-watch")
                    .join(format!("{}.json", digest(&json!(candidate))?)),
            ));
        }
        let existing = paths
            .iter()
            .filter(|(_, path)| path.exists())
            .collect::<Vec<_>>();
        require(
            existing.len() <= 1,
            &format!(
                "multiple watch configurations exist for supported record names ({}); reconcile their configuration and retained state before using watch",
                existing
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )?;
        let (configured_entry, config_path) =
            existing.first().copied().unwrap_or(&paths[0]).clone();
        if found.status != "found" {
            require(
                config_path.exists(),
                "watch needs an existing project record",
            )?;
            record = tree.join(&configured_entry);
            entry = configured_entry.clone();
        }
        let key = digest(&json!(configured_entry))?;
        let project_state = state
            .join("kpopper/watch")
            .join(digest(&json!([common, configured_entry]))?);
        let private = project_state.join("worktrees").join(digest(&json!(tree))?);
        Ok(Self {
            cwd,
            tree,
            common,
            record,
            entry,
            config_path,
            key,
            project_state,
            state: private,
            state_home: state.to_owned(),
            clock,
        })
    }
    pub fn config(&self) -> Result<Option<J>> {
        let mut config = load(&self.config_path)?;
        if let Some(value) = config.as_mut().filter(|v| truth(v)) {
            value
                .as_object_mut()
                .ok_or_else(|| error("watch configuration must be an object"))?
                .insert("entry".into(), json!(self.entry));
        }
        Ok(config)
    }
    pub fn enabled(&self) -> Result<bool> {
        Ok(self
            .config()?
            .is_some_and(|config| truth(&config["enabled"])))
    }
    pub fn setup(
        &self,
        base_ref: Option<&str>,
        shared_record: Option<&Path>,
        shared_private: bool,
    ) -> Result<J> {
        require(
            cfg!(unix),
            "watch configuration requires POSIX file locking",
        )?;
        require(
            self.record.is_file(),
            "configured branch record is unavailable; it was not recreated",
        )?;
        let config = self.config()?.unwrap_or(json!({}));
        let mut base = base_ref
            .map(str::to_owned)
            .or_else(|| config["base_ref"].as_str().map(str::to_owned));
        if base.is_none() {
            for candidate in [
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
                "refs/heads/main",
            ] {
                if git(
                    &self.tree,
                    &["rev-parse", "--verify", &format!("{candidate}^{{commit}}")],
                )
                .is_ok()
                {
                    base = Some(candidate.into());
                    break;
                }
            }
        }
        let base = base
            .filter(|v| !v.is_empty() && !v.starts_with('-'))
            .ok_or_else(|| error("choose an existing base ref with --base-ref"))?;
        let resolved = git(
            &self.tree,
            &["rev-parse", "--symbolic-full-name", "--verify", &base],
        )?;
        require(
            resolved.starts_with("refs/"),
            "base must name a branch or tracking ref, not a fixed commit",
        )?;
        git(
            &self.tree,
            &["rev-parse", "--verify", &format!("{resolved}^{{commit}}")],
        )?;
        require(
            !(shared_record.is_some() && shared_private),
            "choose one shared record destination",
        )?;
        let mut shared = shared_record
            .map(|p| {
                absolute(
                    &if let Some(rest) = p.to_str().and_then(|p| p.strip_prefix("~/")) {
                        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest)
                    } else {
                        self.cwd.join(p)
                    },
                )
            })
            .transpose()?
            .map(|p| json!(p))
            .unwrap_or_else(|| config["shared_record"].clone());
        if shared_private {
            shared = json!(self.project_state.join("shared/PROVENANCE.yaml"));
        }
        if let Some(name) = shared.as_str() {
            let path = Path::new(name);
            require(
                path != self.record
                    && !path.starts_with(&self.tree)
                    && !path.starts_with(&self.common),
                "shared facts must live outside Git checkouts and Git metadata",
            )?;
            if truth(&config["shared_record"]) {
                require(
                    config["shared_record"] == shared,
                    "shared destination is already pinned; reconcile its queued reports before relocating",
                )?;
                require(
                    path.is_file(),
                    "configured shared record is unavailable; it was not recreated",
                )?;
            }
            if shared_record.is_some() {
                require(
                    path.is_file(),
                    "the selected shared record must already exist; use --shared-private for a new destination",
                )?;
            }
            fs::create_dir_all(path.parent().unwrap())?;
            if let Ok(common) = git(path.parent().unwrap(), &["rev-parse", "--git-common-dir"]) {
                require(
                    absolute(&path.parent().unwrap().join(common))? != self.common,
                    "shared facts must live outside this project's checkouts",
                )?;
            }
            let _guard = crate::history_transaction_fs::DirectoryGuard::acquire(
                path.parent().unwrap(),
                true,
            )?;
            require(
                path.is_file() || (!truth(&config["shared_record"]) && shared_private),
                "configured shared record is unavailable; it was not recreated",
            )?;
            if !path.exists() && shared_private {
                atomic(
                    path,
                    b"meta:\n  name: Shared external facts\nsources: {}\nknown: {}\n",
                )?;
            }
        }
        let _guard = lock(&self.config_path.with_extension("lock"), true)?;
        let mut config = self.config()?.unwrap_or(json!({}));
        let object = config
            .as_object_mut()
            .ok_or_else(|| error("watch configuration must be an object"))?;
        for (key, value) in [
            ("base_ref", json!(resolved)),
            ("shared_record", shared),
            ("enabled", json!(true)),
            ("entry", json!(self.entry)),
            ("schema", json!(1)),
        ] {
            object.insert(key.into(), value);
        }
        save(&self.config_path, &config)?;
        config["configured"] = json!(true);
        config["freshness"] = json!("local Git objects; no implicit fetch");
        Ok(config)
    }
    pub fn pause(&self) -> Result<J> {
        let _guard = lock(&self.config_path.with_extension("lock"), true)?;
        let mut config = self.config()?.unwrap_or(json!({}));
        config["enabled"] = json!(false);
        save(&self.config_path, &config)?;
        Ok(json!({"state":"disabled"}))
    }
    pub fn launch(&self) -> Result<()> {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("--workspace")
            .arg(&self.cwd)
            .args(["watch", "_process"])
            .env("XDG_STATE_HOME", &self.state_home)
            .env("KPOPPER_WATCH_DETACH", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x00000008 | 0x00000200);
        }
        command.spawn()?;
        Ok(())
    }
    pub fn request(&self) -> Result<J> {
        self.request_with(&|watch| watch.launch())
    }
    pub fn request_with(&self, launch: &dyn Fn(&Watch) -> Result<()>) -> Result<J> {
        if !self.enabled()? {
            return Ok(json!({"state":"disabled"}));
        }
        let _guard = lock(&self.state.join("request.lock"), true)?;
        save(
            &self.state.join("request.json"),
            &json!({"id":uuid::Uuid::new_v4().simple().to_string(),"at":self.clock.seconds()}),
        )?;
        save(
            &self.state.join("location.json"),
            &json!({"cwd":self.cwd,"tree":self.tree,"entry":self.entry}),
        )?;
        let active = load(&self.state.join("active.json"))?.unwrap_or(json!({}));
        if active["until"].as_f64().unwrap_or(0.0) <= self.clock.seconds() {
            save(
                &self.state.join("active.json"),
                &json!({"until":self.clock.seconds()+30.0}),
            )?;
            launch(self)?;
        }
        Ok(json!({"state":"pending","workspace":self.cwd}))
    }
    pub fn request_all(&self) -> Result<J> {
        self.request_all_with(&|watch| watch.launch())
    }
    pub fn request_all_with(&self, launch: &dyn Fn(&Watch) -> Result<()>) -> Result<J> {
        let mut results = vec![self.request_with(launch)?];
        let root = self.project_state.join("worktrees");
        if root.is_dir() {
            for entry in fs::read_dir(root)? {
                let path = entry?.path().join("location.json");
                if !path.is_file() {
                    continue;
                }
                let process = || -> Result<Option<J>> {
                    let Some(location) = load(&path)?.filter(J::is_object) else {
                        return Ok(None);
                    };
                    if location["tree"] == json!(self.tree) {
                        return Ok(None);
                    }
                    let cwd = location["cwd"]
                        .as_str()
                        .ok_or_else(|| error("missing registered worktree cwd"))?;
                    let peer =
                        Self::in_state(Path::new(cwd), &self.state_home, self.clock.clone())?;
                    if peer.common == self.common && peer.config_path == self.config_path {
                        Ok(Some(peer.request_with(launch)?))
                    } else {
                        Ok(None)
                    }
                };
                match process(){Ok(Some(result))=>results.push(result),Ok(None)=>{},Err(e)=>results.push(json!({"state":"unavailable","location":path,"reason":e.to_string().chars().take(300).collect::<String>()}))}
            }
        }
        Ok(json!({"worktrees":results}))
    }
    pub fn snapshot(&self) -> Result<crate::watch_capture::Snapshot> {
        crate::watch_capture::snapshot(self)
    }
    pub fn process(&self) -> Result<J> {
        self.process_with(&|watch| watch.launch())
    }
    pub fn process_with(&self, launch: &dyn Fn(&Watch) -> Result<()>) -> Result<J> {
        if !self.enabled()? {
            return Ok(json!({"state":"disabled"}));
        }
        let Some(_guard) = lock(&self.state.join("processor.lock"), false)? else {
            return Ok(json!({"state":"busy"}));
        };
        for _ in 0..3 {
            let ticket = {
                let _request = lock(&self.state.join("request.lock"), true)?;
                let ticket = load(&self.state.join("request.json"))?;
                save(
                    &self.state.join("active.json"),
                    &json!({"until":self.clock.seconds()+30.0}),
                )?;
                ticket
            };
            let mut before = None;
            let computed = (|| -> Result<Option<J>> {
                let shared = truth(&self.config()?.unwrap_or(J::Null)["shared_record"]);
                if shared && crate::watch_shared::process(self)? {
                    self.request_all_with(launch)?;
                }
                let observed = self.snapshot()?;
                before = Some(observed.clone());
                let existing = load(&self.state.join("result.json"))?.unwrap_or(json!({}));
                let mut result = if existing["identity"] == observed["identity"]
                    && matches!(existing["state"].as_str(), Some("clear" | "attention"))
                {
                    existing
                } else {
                    crate::watch_compare::compare(&observed)?
                };
                if shared {
                    let findings = result["findings"]
                        .as_array_mut()
                        .ok_or_else(|| error("invalid watch findings"))?;
                    findings.retain(|f| f["kind"] != "shared_review");
                    for report in crate::watch_shared::receipts(self)? {
                        if report["state"] == "needs_review" && report["origin"] == json!(self.tree)
                        {
                            let mut finding = json!({"kind":"shared_review","id":report["target"],"reason":report["reason"],"report":report["id"]});
                            finding["fingerprint"] = json!(digest(&finding)?);
                            if !findings.contains(&finding) {
                                findings.push(finding);
                            }
                        }
                    }
                    result["state"] = json!(if findings.is_empty() {
                        "clear"
                    } else {
                        "attention"
                    });
                }
                if self.snapshot()?["identity"] != observed["identity"] {
                    return Ok(None);
                }
                Ok(Some(result))
            })();
            let mut result = match computed {
                Ok(Some(result)) => result,
                Ok(None) => continue,
                Err(e) => {
                    json!({"state":"unavailable","reason":e.to_string().chars().take(1000).collect::<String>(),"findings":[],"identity":before.as_ref().map(|v|v["identity"].clone()),"versions":before.as_ref().map(|v|v["versions"].clone())})
                }
            };
            let previous = load(&self.state.join("result.json"))?.unwrap_or(json!({}));
            result["episode"] = if previous["state"] == result["state"] {
                previous["episode"].clone()
            } else {
                json!(uuid::Uuid::new_v4().simple().to_string())
            };
            let mut stored = result.clone();
            stored["checked_at"] = json!(self.clock.seconds());
            save(&self.state.join("result.json"), &stored)?;
            let _request = lock(&self.state.join("request.lock"), true)?;
            if load(&self.state.join("request.json"))? == ticket {
                save(&self.state.join("active.json"), &json!({"until":0}))?;
                return Ok(result);
            }
        }
        save(&self.state.join("active.json"), &json!({"until":0}))?;
        Ok(json!({"state":"pending"}))
    }
    pub fn status(&self) -> Result<J> {
        if !self.enabled()? {
            return Ok(json!({"state":"disabled"}));
        }
        let result = load(&self.state.join("result.json"))?.unwrap_or(json!({"state":"pending"}));
        let current = (|| -> Result<Option<crate::watch_capture::Snapshot>> {
            if truth(&self.config()?.unwrap_or(J::Null)["shared_record"])
                && crate::watch_shared::receipts(self)?
                    .iter()
                    .any(|r| r["state"] == "captured")
            {
                return Ok(None);
            }
            Ok(Some(self.snapshot()?))
        })();
        match current {
            Ok(None) => {
                Ok(json!({"state":"pending","reason":"shared reports are still being processed"}))
            }
            Ok(Some(current)) if result["identity"] != current["identity"] => {
                Ok(json!({"state":"pending","versions":current["versions"]}))
            }
            Ok(Some(_)) => Ok(result),
            Err(e) => Ok(
                json!({"state":"unavailable","reason":e.to_string().chars().take(1000).collect::<String>()}),
            ),
        }
    }
    pub fn poll_result(&self, previous: Option<J>) -> Result<(Option<J>, Option<J>)> {
        let raw = load(&self.state.join("result.json"))?;
        let signature = raw.and_then(|v| v.get("checked_at").cloned());
        if signature.is_some() && signature != previous {
            Ok((signature, Some(self.status()?)))
        } else {
            Ok((previous, None))
        }
    }
    pub fn offer(&self, session: &str, consume: bool, result: Option<&J>) -> Result<Option<J>> {
        let result = result.cloned().map(Ok).unwrap_or_else(|| self.status())?;
        let path = self
            .state
            .join("delivery")
            .join(format!("{}.json", digest(&json!(session))?));
        if !matches!(result["state"].as_str(), Some("attention" | "unavailable")) {
            if result["state"] == "clear" && consume {
                let _guard = lock(&self.state.join("delivery.lock"), true)?;
                save(&path, &json!({}))?;
            }
            return Ok(None);
        }
        let findings=result["findings"].as_array().filter(|v|!v.is_empty()).cloned().unwrap_or(vec![json!({"kind":"unavailable","reason":result["reason"],"fingerprint":digest(&result["reason"])?})]);
        let _guard = lock(&self.state.join("delivery.lock"), true)?;
        let receipt = load(&path)?.unwrap_or(json!({}));
        let offered = if receipt["episode"] == result["episode"] {
            receipt["ids"].as_array().cloned().unwrap_or_default()
        } else {
            vec![]
        };
        let fresh = findings
            .iter()
            .filter(|f| !offered.contains(&f["fingerprint"]))
            .cloned()
            .collect::<Vec<_>>();
        let current_ids = findings
            .iter()
            .filter_map(|f| f["fingerprint"].as_str())
            .collect::<BTreeSet<_>>();
        let delivered = offered
            .iter()
            .filter_map(J::as_str)
            .filter(|v| current_ids.contains(v))
            .chain(
                fresh
                    .iter()
                    .take(8)
                    .filter_map(|f| f["fingerprint"].as_str()),
            )
            .collect::<BTreeSet<_>>();
        if consume {
            save(&path, &json!({"ids":delivered,"episode":result["episode"]}))?;
        }
        if fresh.is_empty() {
            return Ok(None);
        }
        Ok(Some(
            json!({"workspace":self.cwd,"versions":result["versions"],"findings":fresh.iter().take(8).collect::<Vec<_>>(),"remaining":fresh.len().saturating_sub(8)}),
        ))
    }
}

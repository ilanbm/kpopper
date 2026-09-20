//! Durable workspace-local followups, independent of the canonical task store.
use crate::{
    Error, Result, followup_triggers as triggers, history_contract as H, identity::sha256,
    ordinary_reader::Reader, require, source_capture::ReadMode, value::TypedValue as V,
};
use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;
use serde_json::{Map, Value, json};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    process::Command,
};
use tempfile::NamedTempFile;
use uuid::Uuid;

pub const MAX_BYTES: usize = 8 * 1024 * 1024;
const OBSERVATION_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug)]
struct Location {
    key: String,
    workspace: PathBuf,
    record: PathBuf,
}

#[derive(Clone, Debug, Default)]
struct Graph {
    values: Map<String, Value>,
    core: BTreeSet<String>,
    maintenance: Vec<Value>,
}

pub struct Store {
    location: Location,
    pub root: PathBuf,
    pub path: PathBuf,
    registered: bool,
    now: Box<dyn Fn() -> DateTime<Utc> + Send + Sync>,
}

fn error(message: impl Into<String>) -> Error {
    Error(message.into())
}

fn state_home() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path);
        require(path.is_absolute(), "XDG_STATE_HOME must be absolute")?;
        return Ok(path);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".local/state"))
        .ok_or_else(|| error("home_directory_unavailable"))
}

pub(crate) fn typed_json(value: &V) -> Result<Value> {
    Ok(match value {
        V::Null => Value::Null,
        V::Bool(value) => json!(value),
        V::Integer(value) => Value::Number(value.as_str().parse()?),
        V::Float(value) => Value::Number(crate::identity::python_float(value.get()).parse()?),
        V::Text(value) => json!(value),
        V::Date(value) => json!(value.as_str()),
        V::DateTime(value) => json!(triggers::stamp(triggers::parse_time(
            value.as_str(),
            "UTC"
        )?)),
        V::List(values) => Value::Array(values.iter().map(typed_json).collect::<Result<_>>()?),
        V::Map(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| Ok((key.clone(), typed_json(value)?)))
                .collect::<Result<_>>()?,
        ),
    })
}

fn read_value(path: &Path) -> Result<Value> {
    let metadata = path
        .symlink_metadata()
        .map_err(|e| error(format!("Unreadable YAML at {}: {e}", path.display())))?;
    require(
        metadata.is_file(),
        &format!("Unreadable YAML at {}", path.display()),
    )?;
    require(
        metadata.len() <= MAX_BYTES as u64,
        &format!("File is too large: {}", path.display()),
    )?;
    let raw = fs::read(path)?;
    parse_input(&raw).map_err(|e| error(format!("Unreadable YAML at {}: {e}", path.display())))
}

pub fn parse_input(raw: &[u8]) -> Result<Value> {
    require(raw.len() <= MAX_BYTES, "byte_limit")?;
    let first = raw.iter().copied().find(|byte| !byte.is_ascii_whitespace());
    if matches!(first, Some(b'{' | b'[')) {
        let value = crate::json_ingress::parse_slice_bounded(
            raw,
            crate::json_ingress::DuplicateKeys::Reject,
            128,
        )
        .map_err(|e| error(format!("invalid_json: {e}")))?;
        return canonical(&value);
    }
    typed_json(&crate::history_yaml::decode_document(raw)?)
}

fn canonical(value: &Value) -> Result<Value> {
    fn visit(value: &Value, depth: usize, count: &mut usize) -> Result<Value> {
        *count += 1;
        require(
            depth <= 100 && *count <= 100_000,
            "value exceeds normalization bounds",
        )?;
        Ok(match value {
            Value::Number(number) => {
                let spelling = number.to_string();
                let normalized = if spelling.contains(['.', 'e', 'E']) {
                    let float = spelling
                        .parse::<f64>()
                        .ok()
                        .filter(|v| v.is_finite())
                        .ok_or_else(|| error("nonfinite numbers are not supported"))?;
                    crate::identity::python_float(float)
                } else {
                    require(
                        spelling.trim_start_matches('-').len() <= 4300,
                        "integer exceeds 4300 digits",
                    )?;
                    spelling
                        .parse::<num_bigint::BigInt>()
                        .map_err(|_| error("invalid integer"))?
                        .to_string()
                };
                Value::Number(normalized.parse()?)
            }
            Value::Array(values) => Value::Array(
                values
                    .iter()
                    .map(|v| visit(v, depth + 1, count))
                    .collect::<Result<_>>()?,
            ),
            Value::Object(values) => Value::Object(
                values
                    .iter()
                    .map(|(key, v)| Ok((key.clone(), visit(v, depth + 1, count)?)))
                    .collect::<Result<_>>()?,
            ),
            value => value.clone(),
        })
    }
    visit(value, 0, &mut 0)
}

pub fn digest(value: &Value) -> Result<String> {
    Ok(sha256(&serde_json::to_vec(&canonical(value)?)?))
}

fn text(value: Option<&Value>, field: &str) -> Result<String> {
    let value = value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.chars().count() <= 12_000)
        .ok_or_else(|| {
            error(format!(
                "{field} must be nonempty text (at most 12000 characters)"
            ))
        })?;
    Ok(value.to_owned())
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

fn git(path: &Path, argument: &str) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["-C", path.to_str()?, "rev-parse", argument])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let found = PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());
    let found = if found.is_absolute() {
        found
    } else {
        path.join(found)
    };
    found.canonicalize().ok()
}

fn local_absolute(value: &str) -> Result<PathBuf> {
    let expanded = value.strip_prefix("~/").map_or_else(
        || PathBuf::from(value),
        |suffix| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(suffix)
        },
    );
    require(expanded.is_absolute(), "Task files must use absolute paths")?;
    if expanded.exists() {
        return expanded.canonicalize().map_err(Error::from);
    }
    if let Some(parent) = expanded.parent()
        && let Ok(parent) = parent.canonicalize()
    {
        return Ok(parent.join(
            expanded
                .file_name()
                .ok_or_else(|| error("invalid task path"))?,
        ));
    }
    let mut normalized = PathBuf::new();
    for component in expanded.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}

fn task_reference(value: &str) -> Result<String> {
    let value = text(Some(&json!(value)), "task reference")?;
    if let Some(authority) = value.strip_prefix("https://") {
        let host = authority.split('/').next().unwrap_or("");
        require(
            !host.is_empty() && !host.contains('@'),
            "External task reference must be an HTTPS URL without credentials",
        )?;
        return Ok(value);
    }
    Ok(local_absolute(&value)?.to_string_lossy().into_owned())
}

fn fingerprint(reference: &str) -> Result<Option<String>> {
    if reference.starts_with("https://") {
        return Ok(None);
    }
    let path = Path::new(reference);
    let metadata = path
        .metadata()
        .map_err(|_| error(format!("Task file is unavailable: {reference}")))?;
    require(
        metadata.is_file() && metadata.len() <= MAX_BYTES as u64,
        &format!("Task file is unavailable: {reference}"),
    )?;
    Ok(Some(sha256(&fs::read(path)?)))
}

fn atomic(path: &Path, raw: &[u8]) -> Result<()> {
    require(
        raw.len() <= MAX_BYTES,
        "Followups ledger is too large; retain a backup before reducing history",
    )?;
    let parent = path
        .parent()
        .ok_or_else(|| error("invalid followups path"))?;
    if parent.exists() {
        require(
            parent.symlink_metadata()?.file_type().is_dir(),
            "followups state directory must not be a symlink",
        )?;
    }
    fs::create_dir_all(parent)?;
    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(raw)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|e| Error::from(e.error))?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn atomic_value(path: &Path, value: &Value) -> Result<()> {
    atomic(path, &serialized_value(value)?)
}

fn serialized_value(value: &Value) -> Result<Vec<u8>> {
    // This shared ledger is also read as YAML 1.1. JSON exponent tokens without
    // a decimal point would be read as strings by the Python reader.
    let raw = serde_json::to_string_pretty(&canonical(value)?)?;
    let mut safe = String::with_capacity(raw.len());
    let mut quoted = false;
    let mut escaped = false;
    let bytes = raw.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        let ch = raw[at..].chars().next().unwrap();
        if !quoted && (ch == '-' || ch.is_ascii_digit()) {
            let end = at
                + bytes[at..]
                    .iter()
                    .take_while(|b| b.is_ascii_digit() || b"-+.eE".contains(b))
                    .count();
            let token = &raw[at..end];
            if let Some(exp) = token.find(['e', 'E'])
                && !token[..exp].contains('.')
            {
                safe.push_str(&token[..exp]);
                safe.push_str(".0");
                safe.push_str(&token[exp..]);
            } else {
                safe.push_str(token);
            }
            at = end;
            continue;
        }
        safe.push(ch);
        if quoted {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
        } else if ch == '"' {
            quoted = true;
        }
        at += ch.len_utf8();
    }
    let mut raw = safe.into_bytes();
    raw.push(b'\n');
    require(
        raw.len() <= MAX_BYTES,
        "Followups ledger is too large; retain a backup before reducing history",
    )?;
    Ok(raw)
}

fn python_json(value: &Value) -> Result<String> {
    let compact = serde_json::to_string(value)?;
    let mut output = String::with_capacity(compact.len() + compact.len() / 8);
    let mut quoted = false;
    let mut escaped = false;
    for character in compact.chars() {
        output.push(character);
        if quoted {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                quoted = false;
            }
        } else if character == '"' {
            quoted = true;
        } else if character == ',' || character == ':' {
            output.push(' ');
        }
    }
    Ok(output)
}

impl Store {
    pub fn open(workspace: &Path) -> Result<Self> {
        Self::at(workspace, Utc::now())
    }

    pub fn at(workspace: &Path, now: DateTime<Utc>) -> Result<Self> {
        Self::with_clock(workspace, move || now)
    }

    pub fn at_in_state(workspace: &Path, state: &Path, now: DateTime<Utc>) -> Result<Self> {
        Self::with_clock_and_state(workspace, state.to_path_buf(), move || now)
    }

    pub fn with_clock(
        workspace: &Path,
        now: impl Fn() -> DateTime<Utc> + Send + Sync + 'static,
    ) -> Result<Self> {
        Self::with_clock_and_state(workspace, state_home()?, now)
    }

    fn with_clock_and_state(
        workspace: &Path,
        state: PathBuf,
        now: impl Fn() -> DateTime<Utc> + Send + Sync + 'static,
    ) -> Result<Self> {
        let requested = workspace.canonicalize()?;
        let located = crate::public_workspace::locate(&requested, ReadMode::Live)?;
        let base = state.join("kpopper/followups");
        let git_root = git(&requested, "--show-toplevel");
        let common = git(&requested, "--git-common-dir");
        let mut candidates = vec![];
        if base.is_dir() {
            for entry in fs::read_dir(&base)? {
                let entry = entry?;
                let path = entry.path().join("location.yaml");
                if !path.is_file() {
                    continue;
                }
                let hint = read_value(&path)?;
                let object = hint.as_object().ok_or_else(|| {
                    error(format!(
                        "Invalid followup location index: {}",
                        path.display()
                    ))
                })?;
                let key = object.get("key").and_then(Value::as_str).unwrap_or("");
                require(
                    key == entry.file_name().to_string_lossy(),
                    &format!("Invalid followup location index: {}", path.display()),
                )?;
                let anchor = if common.as_ref().is_some_and(|path| {
                    object.get("git_common").and_then(Value::as_str)
                        == Some(path.to_string_lossy().as_ref())
                }) {
                    let relative = object
                        .get("relative")
                        .and_then(Value::as_str)
                        .ok_or_else(|| error("invalid followup location index"))?;
                    git_root.as_ref().unwrap().join(relative)
                } else if common.is_none() && object.get("git_common").is_none_or(Value::is_null) {
                    PathBuf::from(
                        object
                            .get("workspace")
                            .and_then(Value::as_str)
                            .ok_or_else(|| error("invalid followup location index"))?,
                    )
                } else {
                    continue;
                };
                if requested == anchor || requested.starts_with(&anchor) {
                    candidates.push((anchor.components().count(), key.to_owned(), anchor));
                }
            }
        }
        candidates.sort_by(|a, b| b.cmp(a));
        if candidates
            .get(1)
            .is_some_and(|other| other.0 == candidates[0].0)
        {
            return Err(error(
                "More than one followup store owns this workspace; reconcile its location indexes",
            ));
        }
        let registered = !candidates.is_empty();
        let (key, workspace) = candidates
            .into_iter()
            .next()
            .map(|(_, key, workspace)| (key, workspace))
            .unwrap_or((located.key, located.workspace));
        let root = base.join(&key);
        Ok(Self {
            location: Location {
                key,
                workspace,
                record: located.record,
            },
            path: root.join("followups.yaml"),
            root,
            registered,
            now: Box::new(now),
        })
    }

    fn now(&self) -> DateTime<Utc> {
        (self.now)()
    }

    fn lock(&self) -> Result<File> {
        if self.root.exists() {
            require(
                self.root.symlink_metadata()?.file_type().is_dir(),
                "followups state directory must not be a symlink",
            )?;
        }
        fs::create_dir_all(&self.root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))?;
        }
        let lock_path = self.root.join("ledger.lock");
        if lock_path.exists() {
            require(
                lock_path.symlink_metadata()?.file_type().is_file(),
                "followups lock must be a regular file",
            )?;
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        FileExt::lock_exclusive(&lock)?;
        Ok(lock)
    }

    fn register(&self) -> Result<()> {
        let git_root = git(&self.location.workspace, "--show-toplevel");
        let common = git(&self.location.workspace, "--git-common-dir");
        let relative = git_root
            .as_ref()
            .and_then(|root| self.location.workspace.strip_prefix(root).ok())
            .map(|path| {
                if path.as_os_str().is_empty() {
                    ".".to_owned()
                } else {
                    path.to_string_lossy().into_owned()
                }
            });
        atomic_value(
            &self.root.join("location.yaml"),
            &json!({
                "key":self.location.key,
                "workspace":self.location.workspace,
                "git_common":common,
                "relative":relative,
            }),
        )
    }

    pub fn load(&self, required: bool) -> Result<Option<Value>> {
        self.load_path(required, &self.path)
    }

    fn load_path(&self, required: bool, path: &Path) -> Result<Option<Value>> {
        if !path.exists() {
            if self.registered {
                return Err(error(format!(
                    "Registered followups ledger is unavailable: {}; restore it from backup",
                    self.path.display()
                )));
            }
            if required {
                return Err(error(
                    "Followups are not configured. Run `kpop followups setup`.",
                ));
            }
            return Ok(None);
        }
        let data = read_value(path)?;
        self.validate_ledger(&data)?;
        Ok(Some(data))
    }

    fn validate_ledger(&self, data: &Value) -> Result<()> {
        let object = data.as_object().ok_or_else(|| error("Unknown or mismatched followups ledger; restore it instead of creating a replacement"))?;
        require(
            object.get("version") == Some(&json!(1))
                && object.get("workspace_key") == Some(&json!(self.location.key)),
            "Unknown or mismatched followups ledger; restore it instead of creating a replacement",
        )?;
        for section in ["config", "items", "observations", "daily"] {
            require(
                object.get(section).is_some_and(Value::is_object),
                &format!("Invalid ledger section: {section}"),
            )?;
        }
        let config = object["config"].as_object().unwrap();
        for field in ["record", "workspace", "timezone", "store"] {
            text(config.get(field), field)?;
        }
        config["timezone"]
            .as_str()
            .unwrap()
            .parse::<chrono_tz::Tz>()
            .map_err(|e| error(format!("invalid time or timezone: {e}")))?;
        for (key, item) in object["items"].as_object().unwrap() {
            let item = item
                .as_object()
                .ok_or_else(|| error(format!("Invalid followup: {key}")))?;
            require(
                identifier(key)
                    && item.get("id") == Some(&json!(key))
                    && matches!(
                        item.get("state").and_then(Value::as_str),
                        Some("waiting" | "needs_user" | "done" | "cancelled")
                    )
                    && item.get("attempts").is_some_and(Value::is_array),
                &format!("Invalid followup: {key}"),
            )?;
            self.validate_spec(&item["spec"], data, Some(key))?;
            require(
                item.get("baseline").is_some_and(Value::is_object),
                &format!("Invalid followup: {key}"),
            )?;
            require(
                item.get("task").is_some_and(Value::is_string)
                    && item.get("executor").is_some_and(Value::is_string)
                    && item.get("generation").is_some_and(Value::is_u64),
                &format!("Invalid followup: {key}"),
            )?;
            if let Some(next_at) = item.get("next_at").filter(|value| !value.is_null()) {
                triggers::parse_time(
                    next_at
                        .as_str()
                        .ok_or_else(|| error(format!("Invalid followup: {key}")))?,
                    "UTC",
                )?;
            }
            if let Some(marked) = item.get("core_baseline") {
                let marked = marked
                    .as_array()
                    .ok_or_else(|| error("invalid core followup baseline marker"))?;
                let baseline = item["baseline"].as_object().unwrap();
                require(
                    marked
                        .iter()
                        .all(|id| id.as_str().is_some_and(|id| baseline.contains_key(id)))
                        && marked
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<BTreeSet<_>>()
                            .len()
                            == marked.len(),
                    "invalid core followup baseline marker",
                )?;
            }
            if let Some(claim) = item.get("claim").filter(|value| !value.is_null()) {
                let claim = claim
                    .as_object()
                    .ok_or_else(|| error(format!("Invalid followup claim: {key}")))?;
                for field in ["token", "owner", "started_at", "expires_at", "occurrence"] {
                    text(claim.get(field), field)?;
                }
                triggers::parse_time(claim["started_at"].as_str().unwrap(), "UTC")?;
                triggers::parse_time(claim["expires_at"].as_str().unwrap(), "UTC")?;
                require(
                    claim.get("baseline").is_some_and(Value::is_object)
                        && claim.get("baseline_events").is_some_and(Value::is_object),
                    &format!("Invalid followup claim: {key}"),
                )?;
            }
        }
        for (reference, observation) in object["observations"].as_object().unwrap() {
            text(Some(&json!(reference)), "ref")?;
            let observation = observation
                .as_object()
                .ok_or_else(|| error("Invalid followup observation"))?;
            require(
                ["value", "observed_at", "evidence"]
                    .iter()
                    .all(|field| observation.contains_key(*field)),
                "Invalid followup observation",
            )?;
            text(observation.get("evidence"), "evidence")?;
            triggers::parse_time(
                observation
                    .get("observed_at")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                "UTC",
            )?;
        }
        require(
            object["daily"].get("receipts").is_some_and(Value::is_array),
            "Invalid ledger section: daily",
        )?;
        Ok(())
    }

    fn transaction<T>(&self, mutation: impl FnOnce(&mut Value) -> Result<T>) -> Result<T> {
        self.load(true)?;
        let _lock = self.lock()?;
        let mut data = self.load(true)?.unwrap();
        let before = digest(&data)?;
        let result = mutation(&mut data)?;
        if digest(&data)? != before {
            let serialized = serialized_value(&data)?;
            atomic(
                &self.root.join("followups.previous.yaml"),
                &fs::read(&self.path)?,
            )?;
            atomic(&self.path, &serialized)?;
        }
        Ok(result)
    }

    pub fn suggested_store(&self) -> Option<PathBuf> {
        let main = git(&self.location.workspace, "--git-common-dir")
            .and_then(|common| {
                (common.file_name().is_some_and(|name| name == ".git"))
                    .then(|| common.parent().unwrap().to_path_buf())
            })
            .unwrap_or_else(|| self.location.workspace.clone());
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| {
                home.join("docs")
                    .join(main.file_name().unwrap_or_default())
                    .join("followups")
            })
            .filter(|path| path.is_dir())
    }

    pub fn setup(
        &self,
        store: Option<&Path>,
        timezone: &str,
        record: Option<&Path>,
        private: bool,
    ) -> Result<Value> {
        timezone
            .parse::<chrono_tz::Tz>()
            .map_err(|e| error(format!("invalid time or timezone: {e}")))?;
        let record = record.unwrap_or(&self.location.record);
        let record = record.canonicalize().map_err(|_| {
            error("Create or locate the knowledge record before enabling followups")
        })?;
        require(
            record.is_file(),
            "Create or locate the knowledge record before enabling followups",
        )?;
        require(
            !(private && store.is_some()),
            "Choose either an existing store or the private fallback",
        )?;
        if store.is_none() && !private && self.suggested_store().is_some() && !self.path.exists() {
            return Err(error(format!(
                "An existing followups directory was found: {}. Confirm its project identity with --store, or deliberately choose --private.",
                self.suggested_store().unwrap().display()
            )));
        }
        let destination = if let Some(store) = store {
            require(
                store.is_absolute() && store.is_dir(),
                "An existing task directory must already exist",
            )?;
            store.canonicalize()?
        } else {
            self.root.join("items")
        };
        require(
            destination != self.location.workspace
                && !destination.starts_with(&self.location.workspace),
            "Choose a followups store outside the product workspace",
        )?;
        let config = json!({"workspace":self.location.workspace,"record":record,"store":destination,"timezone":timezone});
        let _lock = self.lock()?;
        if let Some(existing) = self.load(false)? {
            require(
                existing["config"] == config,
                "Already configured; preserve the existing store and record. Use `status` to inspect them.",
            )?;
            self.register()?;
            return Ok(existing["config"].clone());
        }
        let data = json!({"version":1,"workspace_key":self.location.key,"config":config,"items":{},"observations":{},"daily":{"binding":null,"claim":null,"receipts":[]}});
        atomic_value(&self.path, &data)?;
        self.register()?;
        Ok(data["config"].clone())
    }

    fn validate_spec(&self, spec: &Value, data: &Value, item_id: Option<&str>) -> Result<String> {
        let spec = spec
            .as_object()
            .ok_or_else(|| error("A followup specification must be a mapping"))?;
        let allowed = [
            "id", "title", "why", "how", "task", "related", "when", "scope", "executor",
        ];
        let unknown = spec
            .keys()
            .filter(|key| !allowed.contains(&key.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        require(
            unknown.is_empty(),
            &format!("Unknown followup fields: {}", unknown.join(", ")),
        )?;
        let key = spec.get("id").and_then(Value::as_str).unwrap_or("");
        require(
            identifier(key),
            "id must contain 1–100 letters, numbers, underscores or dashes",
        )?;
        text(spec.get("scope"), "authorized scope")?;
        let executor = text(spec.get("executor").or(Some(&json!("kpopper"))), "executor")?;
        let related = spec
            .get("related")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                error("related must be a nonempty unique list of at most 100 graph ids")
            })?;
        let ids = related
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        require(
            !ids.is_empty()
                && ids.len() <= 100
                && ids.len() == related.len()
                && ids.iter().all(|id| !id.is_empty() && id.len() <= 200)
                && ids.iter().collect::<BTreeSet<_>>().len() == ids.len(),
            "related must be a nonempty unique list of at most 100 graph ids",
        )?;
        triggers::validate(spec.get("when").unwrap_or(&Value::Null), &ids)?;
        if let Some(task) = spec.get("task") {
            task_reference(task.as_str().unwrap_or(""))?;
        } else {
            for field in ["title", "why", "how"] {
                text(spec.get(field), field)?;
            }
        }
        let references = triggers::referenced_tasks(&spec["when"])?;
        let items = data["items"].as_object().unwrap();
        let missing = references
            .iter()
            .filter(|key| !items.contains_key(*key))
            .cloned()
            .collect::<Vec<_>>();
        require(
            missing.is_empty(),
            &format!("Unknown prerequisite followups: {}", missing.join(", ")),
        )?;
        let mut done = BTreeSet::new();
        let mut active = BTreeSet::new();
        let mut stack = references
            .into_iter()
            .map(|id| (id, false))
            .collect::<Vec<_>>();
        while let Some((node, leaving)) = stack.pop() {
            if leaving {
                active.remove(&node);
                done.insert(node);
                continue;
            }
            require(
                item_id != Some(&node) && !active.contains(&node),
                "Followup prerequisites cannot contain a cycle",
            )?;
            if done.contains(&node) {
                continue;
            }
            active.insert(node.clone());
            stack.push((node.clone(), true));
            stack.extend(
                triggers::referenced_tasks(&items[&node]["spec"]["when"])?
                    .into_iter()
                    .map(|id| (id, false)),
            );
        }
        Ok(executor)
    }

    fn graph(&self, data: &Value) -> Result<Graph> {
        self.graph_inner(data)
            .map_err(|reason| error(format!("Knowledge record unavailable: {reason}")))
    }

    fn graph_inner(&self, data: &Value) -> Result<Graph> {
        let record = PathBuf::from(data["config"]["record"].as_str().unwrap());
        let workspace = PathBuf::from(data["config"]["workspace"].as_str().unwrap());
        let paths = vec![record];
        let runtime = crate::public_workspace::runtime_for_paths(&paths, &workspace, None)?;
        let capture = crate::source_capture::capture_source_with_runtime(
            &paths,
            &workspace,
            ReadMode::Live,
            None,
            runtime.as_ref(),
        )?;
        let capabilities =
            crate::reasoning_fields::capabilities(capture.ordinary_document(), None)?;
        let core = H::string_is(&H::map(&capabilities)?["profile"], "core/v1");
        if core {
            let context = crate::reasoning_context::CapturedAssessment::from_snapshot(
                capture.snapshot()?.clone(),
                None,
                "focused-review/v1",
                runtime.as_ref(),
                crate::reasoning_runtime::OperationalBounds::default(),
                None,
            )?;
            let projected = crate::followup_core::project(
                context.assessment(),
                context.snapshot_id(),
                context.findings_revision(),
            )?;
            let graph = Graph {
                core: projected.values.keys().cloned().collect(),
                values: projected.values,
                maintenance: projected.maintenance,
            };
            capture.verify()?;
            return Ok(graph);
        }
        let reader = Reader::for_followups(capture.ordinary_document(), runtime.as_ref())?;
        let mut graph = Graph::default();
        for id in &reader.ids {
            let Some(body) = reader.raw().get(id) else {
                continue;
            };
            let value = if id.starts_with("page.")
                && crate::reasoning_fields::BUILTINS.contains(&id.as_str())
            {
                Err(error(
                    "page inputs are unavailable; build the canonical page again",
                ))
            } else {
                crate::ordinary_assessment::snapshot_value(&reader, id)
                    .and_then(|value| crate::history_yaml::strict_ordinary_projection(&value))
                    .and_then(|value| typed_json(&value))
            };
            graph.values.insert(
                id.clone(),
                match value {
                    Ok(value) => value,
                    Err(reason) => json!({"unavailable":reason.to_string()}),
                },
            );
            if H::map(body).is_ok_and(|body| {
                H::text(&reader.fields()["deps"]).is_ok_and(|key| body.contains_key(key))
            }) {
                let reasons = crate::ordinary_counts::flags(&reader, body)?
                    .into_iter()
                    .collect::<Vec<_>>();
                if !reasons.is_empty() {
                    graph.maintenance.push(json!({"id":id,"reasons":reasons}));
                }
            }
        }
        capture.verify()?;
        Ok(graph)
    }

    fn event_values(spec: &Value, data: &Value) -> Result<Value> {
        let external = triggers::referenced_external(&spec["when"])?.into_iter().map(|reference| {
            let observation = data["observations"].get(&reference);
            (reference, json!({"available":observation.is_some(),"value":observation.and_then(|v|v.get("value")).cloned()}))
        }).collect::<Map<_,_>>();
        let completed = triggers::referenced_tasks(&spec["when"])?
            .into_iter()
            .map(|key| {
                let done = data["items"]
                    .get(&key)
                    .and_then(|v| v.get("state"))
                    .and_then(Value::as_str)
                    == Some("done");
                (key, json!(done))
            })
            .collect::<Map<_, _>>();
        Ok(json!({"external":external,"completed":completed}))
    }

    fn baseline(spec: &Value, graph: &Graph) -> Value {
        Value::Object(
            spec["related"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .map(|key| {
                    (
                        key.to_owned(),
                        graph.values.get(key).cloned().unwrap_or(Value::Null),
                    )
                })
                .collect(),
        )
    }

    fn mark_core(holder: &mut Map<String, Value>, spec: &Value, graph: &Graph) {
        let marked = spec["related"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .filter(|key| graph.core.contains(*key))
            .map(|key| json!(key))
            .collect::<Vec<_>>();
        if marked.is_empty() {
            holder.remove("core_baseline");
        } else {
            holder.insert("core_baseline".into(), Value::Array(marked));
        }
    }

    pub fn add(&self, supplied: Value) -> Result<Value> {
        let supplied = canonical(&supplied)?;
        self.transaction(|data| {
            let executor = self.validate_spec(&supplied, data, None)?;
            let key = supplied["id"].as_str().unwrap().to_owned();
            if let Some(existing) = data["items"].get(&key) {
                if existing["spec"] == supplied {
                    return Ok(existing.clone());
                }
                return Err(error("Followup id already exists; use refresh to change it"));
            }
            let graph = self.graph(data)?;
            let related = supplied["related"].as_array().unwrap();
            require(related.iter().all(|key| key.as_str().is_some_and(|key| graph.values.contains_key(key))), "Some related graph ids do not exist")?;
            let reference = if let Some(task) = supplied.get("task") {
                let reference = task_reference(task.as_str().unwrap_or(""))?;
                let duplicate = data["items"].as_object().unwrap().values().any(|item| item["task"] == reference);
                require(!duplicate, "This canonical task is already linked; refresh its existing followup")?;
                reference
            } else {
                let destination = data["config"]["store"].as_str().unwrap();
                require(!destination.starts_with("https://"), "Create the task through the existing task-system connector, then add its task URL")?;
                let root = PathBuf::from(destination);
                if root == self.root.join("items") {
                    fs::create_dir_all(&root)?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
                    }
                } else {
                    require(root.is_dir(), &format!("The configured task store is unavailable; restore its location: {destination}"))?;
                }
                let target = root.join(format!("kp-{key}.md"));
                let content = format!(
                    "# {}\n\n- **When**: {}\n- **Why**: {}\n- **How**: {}\n- **Routine**: Managed by kpop followups {}; use its scan/claim/finish commands.\n",
                    supplied["title"].as_str().unwrap(), python_json(&supplied["when"])? ,
                    supplied["why"].as_str().unwrap(), supplied["how"].as_str().unwrap(), key,
                );
                if target.exists() {
                    require(
                        target.symlink_metadata()?.file_type().is_file(),
                        &format!("Task filename is already occupied: {}", target.display()),
                    )?;
                    require(fs::read_to_string(&target)? == content, &format!("Task filename is already occupied: {}", target.display()))?;
                } else {
                    let mut file = OpenOptions::new().write(true).create_new(true).open(&target)?;
                    file.write_all(content.as_bytes())?;
                    file.sync_all()?;
                }
                target.to_string_lossy().into_owned()
            };
            let mut item = json!({
                "id":key,"spec":supplied,"task":reference,"task_fingerprint":fingerprint(&reference)?,
                "executor":executor,"state":"waiting","created_at":triggers::stamp(self.now()),
                "baseline":Self::baseline(&supplied,&graph),"next_at":null,
                "baseline_events":Self::event_values(&supplied,data)?,"claim":null,"attempts":[],"generation":1,
            });
            Self::mark_core(item.as_object_mut().unwrap(), &supplied, &graph);
            data["items"].as_object_mut().unwrap().insert(key, item.clone());
            Ok(item)
        })
    }

    fn item<'a>(data: &'a Value, key: &str) -> Result<&'a Value> {
        data["items"]
            .get(key)
            .ok_or_else(|| error(format!("Unknown followup: {key}")))
    }

    fn row(
        &self,
        item: &Value,
        data: &Value,
        graph: &Graph,
        graph_error: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Value> {
        let spec = &item["spec"];
        let completed = data["items"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(_, item)| item["state"] == "done")
            .map(|(key, _)| key.clone())
            .collect::<BTreeSet<_>>();
        let baseline = item["baseline"]
            .as_object()
            .ok_or_else(|| error("invalid followup baseline"))?;
        let baseline_core = item
            .get("core_baseline")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let mut assessment = triggers::evaluate(
            &spec["when"],
            &graph.values,
            baseline,
            &completed,
            data["observations"].as_object().unwrap(),
            now,
            data["config"]["timezone"].as_str().unwrap(),
            &graph.core,
            &baseline_core,
        )?;
        let mut state = match assessment.value {
            Some(true) => "ready",
            Some(false) => "waiting",
            None => "unknown",
        }
        .to_owned();
        if let Some(next_at) = item["next_at"].as_str() {
            if triggers::parse_time(next_at, "UTC")? <= now {
                if assessment.value.is_some() {
                    state = "ready".into();
                }
                assessment
                    .reasons
                    .push("The explicitly requested followup check is due".into());
            } else {
                let related = Value::Object(
                    spec["related"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|key| {
                            (
                                key.to_owned(),
                                graph.values.get(key).cloned().unwrap_or(Value::Null),
                            )
                        })
                        .collect(),
                );
                if digest(&related)? == digest(&item["baseline"])?
                    && digest(&Self::event_values(spec, data)?)?
                        == digest(item.get("baseline_events").unwrap_or(&json!({})))?
                {
                    if state != "unknown" {
                        state = "waiting".into();
                    }
                    assessment
                        .reasons
                        .push(format!("Next justified check: {next_at}"));
                }
            }
        }
        let fp = match fingerprint(item["task"].as_str().unwrap()) {
            Ok(value) => {
                if value.as_ref().map_or(Value::Null, |value| json!(value))
                    != item["task_fingerprint"]
                {
                    state = "unknown".into();
                    assessment.reasons.push(
                        "Task content changed; reread it and explicitly refresh the followup"
                            .into(),
                    );
                }
                value.map_or(Value::Null, |value| json!(value))
            }
            Err(reason) => {
                state = "unknown".into();
                assessment.reasons.push(reason.to_string());
                json!("unavailable")
            }
        };
        if item["task"].as_str().unwrap().starts_with("https://") {
            let remote =
                json!({"external":{"ref":item["task"],"equals":"open","max_age_hours":24}});
            let availability = triggers::evaluate(
                &remote,
                &graph.values,
                &Map::new(),
                &completed,
                data["observations"].as_object().unwrap(),
                now,
                "UTC",
                &graph.core,
                &BTreeSet::new(),
            )?;
            if availability.value != Some(true) {
                state = "unknown".into();
                assessment.reasons.push("Reread the canonical remote task and observe its current open status before acting".into());
            }
            assessment.inputs.insert(
                "task_observation".into(),
                Value::Object(availability.inputs),
            );
        }
        if let Some(reason) = graph_error {
            state = "unknown".into();
            assessment.reasons.push(reason.into());
        } else if spec["related"].as_array().unwrap().iter().any(|key| {
            key.as_str()
                .is_some_and(|key| !graph.values.contains_key(key))
        }) {
            state = "unknown".into();
            assessment
                .reasons
                .push("A related graph entry is missing".into());
        }
        let unavailable = spec["related"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .filter(|key| triggers::unavailable(graph.values.get(*key), graph.core.contains(*key)))
            .collect::<Vec<_>>();
        if !unavailable.is_empty() {
            assessment.reasons.push(format!(
                "Related values are unavailable: {}",
                unavailable.join(", ")
            ));
        }
        if let Some(claim) = item.get("claim").filter(|claim| !claim.is_null()) {
            state = if triggers::parse_time(claim["expires_at"].as_str().unwrap(), "UTC")? > now {
                "claimed"
            } else {
                "interrupted"
            }
            .into();
        } else if matches!(
            item["state"].as_str(),
            Some("done" | "cancelled" | "needs_user")
        ) {
            state = item["state"].as_str().unwrap().into();
        } else if item["executor"] != "kpopper" {
            state = "delegated".into();
            assessment.reasons.push(format!(
                "Execution belongs to {}",
                item["executor"].as_str().unwrap()
            ));
        }
        let mut external = triggers::referenced_external(&spec["when"])?;
        if item["task"].as_str().unwrap().starts_with("https://") {
            external.insert(item["task"].as_str().unwrap().into());
        }
        let inputs = json!({
            "generation":item["generation"],"task":fp,
            "related":spec["related"].as_array().unwrap().iter().filter_map(Value::as_str).map(|key|(key.to_owned(),graph.values.get(key).cloned().unwrap_or(Value::Null))).collect::<Map<_,_>>(),
            "completed":triggers::referenced_tasks(&spec["when"])?.into_iter().map(|key|{let done=completed.contains(&key);(key,json!(done))}).collect::<Map<_,_>>(),
            "external":external.into_iter().map(|reference|{let observation=data["observations"].get(&reference);(reference,json!({"available":observation.is_some(),"value":observation.and_then(|v|v.get("value")).cloned()}))}).collect::<Map<_,_>>(),
            "next_at":item["next_at"],
        });
        Ok(json!({
            "id":item["id"],"task":item["task"],"state":state,"executor":item["executor"],
            "title":spec.get("title").and_then(Value::as_str).unwrap_or(item["id"].as_str().unwrap()).chars().take(160).collect::<String>(),
            "reasons":assessment.reasons.into_iter().take(5).map(|reason|reason.chars().take(400).collect::<String>()).collect::<Vec<_>>(),
            "occurrence":digest(&inputs)?,"next_at":item["next_at"],"wake_hint":assessment.next_at.map(triggers::stamp),
            "scope":spec["scope"].as_str().unwrap().chars().take(600).collect::<String>(),"related":spec["related"],
        }))
    }

    pub fn scan(&self, limit: usize) -> Result<Value> {
        require(
            (1..=100).contains(&limit),
            "limit must be between 1 and 100",
        )?;
        let data = self.load(true)?.unwrap();
        let (graph, graph_error) = match self.graph(&data) {
            Ok(graph) => (graph, None),
            Err(reason) => (Graph::default(), Some(reason.to_string())),
        };
        let now = self.now();
        let mut rows = data["items"]
            .as_object()
            .unwrap()
            .values()
            .map(|item| self.row(item, &data, &graph, graph_error.as_deref(), now))
            .collect::<Result<Vec<_>>>()?;
        let order = [
            "interrupted",
            "unknown",
            "needs_user",
            "ready",
            "claimed",
            "delegated",
            "waiting",
            "done",
            "cancelled",
        ];
        rows.sort_by_key(|row| {
            (
                order
                    .iter()
                    .position(|state| row["state"] == *state)
                    .unwrap(),
                row["next_at"].as_str().unwrap_or("").to_owned(),
                row["id"].as_str().unwrap().to_owned(),
            )
        });
        let counts = order
            .into_iter()
            .map(|state| {
                (
                    state.to_owned(),
                    json!(rows.iter().filter(|row| row["state"] == state).count()),
                )
            })
            .collect::<Map<_, _>>();
        let omitted = rows.len().saturating_sub(limit);
        let maintenance_omitted = graph.maintenance.len().saturating_sub(1);
        Ok(json!({
            "workspace":data["config"]["workspace"],"record":data["config"]["record"],"ledger":self.path,
            "counts":counts,"items":rows.into_iter().take(limit).collect::<Vec<_>>(),"omitted":omitted,
            "maintenance":graph.maintenance.into_iter().take(1).collect::<Vec<_>>(),"maintenance_omitted":maintenance_omitted,
            "graph_error":graph_error,"daily":data["daily"]["binding"],
            "notification":graph_error.is_some() || ["interrupted","unknown","needs_user","ready"].iter().any(|state|counts[*state].as_u64().unwrap_or(0)>0),
        }))
    }

    pub fn observe(&self, report: Value) -> Result<Value> {
        let report = canonical(&report)?;
        let object = report
            .as_object()
            .ok_or_else(|| error("An observation needs ref, value, observed_at and evidence"))?;
        require(
            object.len() == 4
                && ["ref", "value", "observed_at", "evidence"]
                    .iter()
                    .all(|key| object.contains_key(*key)),
            "An observation needs ref, value, observed_at and evidence",
        )?;
        let reference = text(object.get("ref"), "ref")?;
        let evidence = text(object.get("evidence"), "evidence")?;
        let observed_text = object["observed_at"]
            .as_str()
            .filter(|value| value.contains(['T', 't', ' ']))
            .ok_or_else(|| error("Observation time must be a timestamp with an explicit offset"))?;
        let observed = triggers::parse_time(observed_text, "UTC")?;
        require(
            observed <= self.now(),
            "An observation cannot come from the future",
        )?;
        require(
            serde_json::to_vec(&object["value"])?.len() <= OBSERVATION_BYTES,
            "Observation value exceeds 64 KiB; retain a small state or reading and link the full evidence",
        )?;
        let value = json!({"value":object["value"],"observed_at":triggers::stamp(observed),"evidence":evidence});
        digest(&value)?;
        self.transaction(|data| {
            if let Some(old) = data["observations"].get(&reference) {
                let old_time = triggers::parse_time(old["observed_at"].as_str().unwrap(),"UTC")?;
                require(old_time <= observed && (old_time != observed || old == &value), "Observation is older than, or conflicts with, the retained observation")?;
            }
            data["observations"].as_object_mut().unwrap().insert(reference.clone(),value.clone());
            Ok(json!({"ref":reference,"value":value["value"],"observed_at":value["observed_at"],"evidence":value["evidence"]}))
        })
    }

    pub fn claim(
        &self,
        key: &str,
        occurrence: &str,
        owner: &str,
        daily_token: Option<&str>,
    ) -> Result<Value> {
        text(Some(&json!(owner)), "session owner")?;
        self.transaction(|data| {
            let graph = self.graph(data)?;
            let row = self.row(Self::item(data,key)?,data,&graph,None,self.now())?;
            require(row["state"]=="ready" && row["occurrence"]==occurrence, &format!("Followup is not ready or the scan is stale; scan again ({})",row["state"].as_str().unwrap()))?;
            if let Some(token)=daily_token {
                let daily=&data["daily"]["claim"];
                require(!daily.is_null() && daily["token"]==token && triggers::parse_time(daily["expires_at"].as_str().unwrap(),"UTC")?>self.now(), "The daily review is not live or owned by this token")?;
                require(daily["actions"].as_array().is_some_and(|a|a.len()<3), "Daily review's three-action budget is exhausted")?;
                data["daily"]["claim"]["actions"].as_array_mut().unwrap().push(json!(key));
            }
            let spec=Self::item(data,key)?["spec"].clone();
            let mut claim=json!({"token":Uuid::new_v4().simple().to_string(),"owner":owner,"started_at":triggers::stamp(self.now()),"expires_at":triggers::stamp(self.now()+Duration::minutes(30)),"occurrence":occurrence,"baseline":Self::baseline(&spec,&graph),"daily_token":daily_token,"baseline_events":Self::event_values(&spec,data)?});
            Self::mark_core(claim.as_object_mut().unwrap(),&spec,&graph);
            data["items"][key]["claim"]=claim.clone();
            let mut result=row;
            result["claim"]=claim;
            result["spec"]=spec;
            result["record"]=data["config"]["record"].clone();
            Ok(result)
        })
    }

    fn owned_claim<'a>(&self, item: &'a Value, token: &str, live: bool) -> Result<&'a Value> {
        let claim = item
            .get("claim")
            .filter(|v| !v.is_null())
            .ok_or_else(|| error("This run token does not own the followup"))?;
        require(
            claim["token"] == token,
            "This run token does not own the followup",
        )?;
        if live {
            require(
                triggers::parse_time(claim["expires_at"].as_str().unwrap(), "UTC")? > self.now(),
                "Run expired; reconcile its effects and use recover",
            )?;
        }
        Ok(claim)
    }

    pub fn renew(&self, key: &str, token: &str) -> Result<Value> {
        self.transaction(|data| {
            self.owned_claim(Self::item(data, key)?, token, true)?;
            data["items"][key]["claim"]["expires_at"] =
                json!(triggers::stamp(self.now() + Duration::minutes(30)));
            Ok(data["items"][key]["claim"].clone())
        })
    }

    pub fn finish(
        &self,
        key: &str,
        token: &str,
        outcome: &str,
        evidence: &str,
        next_at: Option<&str>,
    ) -> Result<Value> {
        text(Some(&json!(evidence)), "outcome evidence")?;
        require(
            ["checked", "done", "cancelled", "needs_user", "released"].contains(&outcome),
            "Unknown outcome",
        )?;
        self.transaction(|data|{
            let request=json!({"token":token,"outcome":outcome,"evidence":evidence,"next_at":next_at});
            let item=Self::item(data,key)?;
            if item["attempts"].as_array().unwrap().iter().any(|attempt|attempt.get("request")==Some(&request)) {
                return Ok(json!({"id":key,"state":item["state"],"already_recorded":true}));
            }
            let claim=self.owned_claim(item,token,true)?.clone();
            let normalized_next=if outcome=="checked" {
                let next=next_at.ok_or_else(||error("A completed check needs a justified future next_at"))?;
                let parsed=triggers::parse_time(next,data["config"]["timezone"].as_str().unwrap())?;
                require(parsed>self.now(),"A completed check needs a justified future next_at")?;
                Some(triggers::stamp(parsed))
            } else { require(next_at.is_none(),"Only checked outcomes take next_at")?; None };
            let graph_result=self.graph(data);
            let stale=match graph_result {
                Ok(ref graph)=>self.row(Self::item(data,key)?,data,graph,None,self.now())?["occurrence"]!=claim["occurrence"],
                Err(_)=>true,
            };
            let effective=if stale && outcome!="released"{"needs_user"}else{outcome};
            let attempt=json!({"run":claim,"finished_at":triggers::stamp(self.now()),"request":request,"outcome":effective,"evidence":evidence,"inputs_changed":stale});
            let item=data["items"][key].as_object_mut().unwrap();
            item["attempts"].as_array_mut().unwrap().push(attempt);
            item.insert("claim".into(),Value::Null);
            item.insert("state".into(),json!(if ["checked","released"].contains(&effective){"waiting"}else{effective}));
            if effective=="checked" {
                item.insert("baseline".into(),claim["baseline"].clone());
                if let Some(core)=claim.get("core_baseline"){item.insert("core_baseline".into(),core.clone());}else{item.remove("core_baseline");}
                item.insert("baseline_events".into(),claim["baseline_events"].clone());
                item.insert("next_at".into(),json!(normalized_next));
            }
            Ok(json!({"id":key,"state":item["state"],"outcome":effective,"inputs_changed":stale}))
        })
    }

    pub fn recover(&self, key: &str, evidence: &str) -> Result<Value> {
        text(Some(&json!(evidence)), "reconciliation evidence")?;
        self.transaction(|data|{
            let item=Self::item(data,key)?;
            let claim=item.get("claim").filter(|v|!v.is_null()).ok_or_else(||error("Only an interrupted run can be recovered"))?.clone();
            require(triggers::parse_time(claim["expires_at"].as_str().unwrap(),"UTC")?<=self.now(),"Only an interrupted run can be recovered")?;
            let item=data["items"][key].as_object_mut().unwrap();
            item["attempts"].as_array_mut().unwrap().push(json!({"run":claim,"finished_at":triggers::stamp(self.now()),"outcome":"recovered","evidence":evidence}));
            item.insert("claim".into(),Value::Null); item.insert("state".into(),json!("waiting"));
            Ok(json!({"id":key,"state":"waiting"}))
        })
    }

    pub fn refresh(&self, key: &str, supplied: Value, evidence: &str) -> Result<Value> {
        let supplied = canonical(&supplied)?;
        text(Some(&json!(evidence)), "reread evidence")?;
        self.transaction(|data|{
            let old=Self::item(data,key)?.clone();
            require(old["claim"].is_null() && !matches!(old["state"].as_str(),Some("done"|"cancelled")),"An active or closed followup cannot be refreshed")?;
            require(supplied["id"]==key,"Refresh must preserve the followup id")?;
            let executor=self.validate_spec(&supplied,data,Some(key))?;
            let reference=task_reference(supplied.get("task").unwrap_or(&old["task"]).as_str().unwrap_or(""))?;
            require(!data["items"].as_object().unwrap().iter().any(|(other_id,item)|other_id!=key && item["task"]==reference),"This canonical task is already linked")?;
            let graph=self.graph(data)?;
            require(supplied["related"].as_array().unwrap().iter().all(|id|id.as_str().is_some_and(|id|graph.values.contains_key(id))),"Some related graph ids do not exist")?;
            let mut updated=old.clone();
            let item=updated.as_object_mut().unwrap();
            item["attempts"].as_array_mut().unwrap().push(json!({"outcome":"refreshed","finished_at":triggers::stamp(self.now()),"evidence":evidence,"previous_spec":old["spec"]}));
            item.insert("spec".into(),supplied.clone()); item.insert("executor".into(),json!(executor)); item.insert("task".into(),json!(reference)); item.insert("task_fingerprint".into(),json!(fingerprint(&reference)?)); item.insert("state".into(),json!("waiting")); item.insert("next_at".into(),Value::Null); item.insert("baseline".into(),Self::baseline(&supplied,&graph)); item.insert("baseline_events".into(),Self::event_values(&supplied,data)?); item.insert("generation".into(),json!(old["generation"].as_u64().unwrap()+1));
            Self::mark_core(item,&supplied,&graph);
            data["items"][key]=updated.clone(); Ok(updated)
        })
    }

    pub fn resolve(&self, key: &str, outcome: &str, evidence: &str) -> Result<Value> {
        text(Some(&json!(evidence)), "external outcome evidence")?;
        require(
            ["done", "cancelled"].contains(&outcome),
            "Resolution must be done or cancelled",
        )?;
        self.transaction(|data|{
            let item=Self::item(data,key)?;
            require(item["claim"].is_null(),"Reconcile the active or interrupted run before resolving its task")?;
            if matches!(item["state"].as_str(),Some("done"|"cancelled")) {
                require(item["state"]==outcome,"The obligation is already closed with another outcome")?;
                return Ok(json!({"id":key,"state":outcome,"already_recorded":true}));
            }
            let item=data["items"][key].as_object_mut().unwrap();
            item["attempts"].as_array_mut().unwrap().push(json!({"outcome":outcome,"evidence":evidence,"finished_at":triggers::stamp(self.now()),"reconciled_external":true}));
            item.insert("state".into(),json!(outcome)); Ok(json!({"id":key,"state":outcome}))
        })
    }

    pub fn resume(&self, key: &str, evidence: &str) -> Result<Value> {
        text(Some(&json!(evidence)), "resume evidence")?;
        self.transaction(|data| {
            let item = Self::item(data, key)?;
            require(
                item["claim"].is_null() && item["state"] == "needs_user",
                "Only parked, unclaimed work can be resumed",
            )?;
            let item = data["items"][key].as_object_mut().unwrap();
            item["attempts"].as_array_mut().unwrap().push(
                json!({"outcome":"resumed","evidence":evidence,"at":triggers::stamp(self.now())}),
            );
            item.insert("state".into(), json!("waiting"));
            Ok(json!({"id":key,"state":"waiting","next_at":item["next_at"]}))
        })
    }

    pub fn relocate(&self, record: &Path, evidence: &str) -> Result<Value> {
        text(Some(&json!(evidence)), "relocation evidence")?;
        let record = record.canonicalize()?;
        let _lock = self.lock()?;
        let mut data = self.load(true)?.unwrap();
        let mut probe = data.clone();
        probe["config"]["record"] = json!(record);
        let graph = self.graph(&probe)?;
        require(
            data["daily"]["claim"].is_null()
                && data["items"]
                    .as_object()
                    .unwrap()
                    .values()
                    .all(|item| item["claim"].is_null()),
            "Reconcile all active/interrupted runs before relocating the record",
        )?;
        let related = data["items"]
            .as_object()
            .unwrap()
            .values()
            .flat_map(|item| item["spec"]["related"].as_array().unwrap())
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>();
        require(
            related.iter().all(|id| graph.values.contains_key(*id)),
            "The new record does not contain the existing related graph ids",
        )?;
        let previous = data["config"].clone();
        data.as_object_mut()
            .unwrap()
            .entry("relocations")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap()
            .push(
                json!({"previous":previous,"at":triggers::stamp(self.now()),"evidence":evidence}),
            );
        data["config"]["record"] = json!(record);
        data["config"]["workspace"] = json!(self.location.workspace);
        atomic(
            &self.root.join("followups.previous.yaml"),
            &fs::read(&self.path)?,
        )?;
        atomic_value(&self.path, &data)?;
        self.register()?;
        Ok(data["config"].clone())
    }

    pub fn restore(&self, backup: &Path, evidence: &str) -> Result<Value> {
        text(Some(&json!(evidence)), "restore evidence")?;
        let _lock = self.lock()?;
        let mut recovered = self.load_path(true, backup)?.unwrap();
        let past = triggers::stamp(self.now() - Duration::seconds(1));
        for item in recovered["items"].as_object_mut().unwrap().values_mut() {
            if !item["claim"].is_null() {
                item["claim"]["expires_at"] = json!(past);
            } else if !matches!(item["state"].as_str(), Some("done" | "cancelled")) {
                item["state"] = json!("needs_user");
            }
        }
        if !recovered["daily"]["claim"].is_null() {
            recovered["daily"]["claim"]["expires_at"] = json!(past);
        }
        let quarantine = if self.path.exists() {
            let path = self
                .root
                .join(format!("quarantine-{}.yaml", Uuid::new_v4().simple()));
            atomic(&path, &fs::read(&self.path)?)?;
            Some(path)
        } else {
            None
        };
        recovered.as_object_mut().unwrap().entry("restorations").or_insert_with(||json!([])).as_array_mut().unwrap().push(json!({"at":triggers::stamp(self.now()),"backup":backup.canonicalize()?,"evidence":evidence,"quarantine":quarantine}));
        atomic_value(&self.path, &recovered)?;
        self.register()?;
        Ok(
            json!({"restored":self.path,"quarantine":quarantine,"message":"Unfinished work requires reconciliation; interrupted claims remain interrupted."}),
        )
    }

    pub fn status(&self) -> Result<Value> {
        let data = self.load(false)?;
        if let Some(data) = data {
            let daily = &data["daily"];
            let run = if let Some(claim) = daily.get("claim").filter(|v| !v.is_null()) {
                if triggers::parse_time(claim["expires_at"].as_str().unwrap(), "UTC")? > self.now()
                {
                    "running"
                } else {
                    "interrupted"
                }
            } else {
                "idle"
            };
            Ok(
                json!({"configured":true,"ledger":self.path,"suggested_store":null,"config":data["config"],"daily":{"state":if daily["binding"].is_null(){"proposed"}else{daily["binding"]["state"].as_str().unwrap_or("unknown")},"timezone":data["config"]["timezone"],"binding":daily["binding"],"run":run,"claim":daily["claim"],"last_review":daily["receipts"].as_array().and_then(|a|a.last()).cloned()}}),
            )
        } else {
            Ok(
                json!({"configured":false,"ledger":self.path,"suggested_store":self.suggested_store()}),
            )
        }
    }

    pub fn list(&self) -> Result<Value> {
        let data = self.load(true)?.unwrap();
        Ok(
            json!({"items":data["items"].as_object().unwrap().values().map(|item|json!({"id":item["id"],"task":item["task"],"state":item["state"]})).collect::<Vec<_>>()}),
        )
    }
    pub fn show(&self, key: &str) -> Result<Value> {
        Ok(Self::item(&self.load(true)?.unwrap(), key)?.clone())
    }
}

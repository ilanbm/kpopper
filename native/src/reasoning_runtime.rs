//! Verified offline Lean runtime and bounded process I/O; no semantic fallback.
use crate::{Error, Result, identity::sha256, reasoning_transport, require};
use serde_json::{Value as J, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;
const MAX_ARCHIVE_MEMBER: u64 = 128 * 1024 * 1024;
const PROTOCOLS: [&str; 3] = ["KP2", "KP3", "KP4"];
const MODULES: [&str; 3] = ["arithmetic/v1", "composition/v1", "query/v1"];
#[derive(Clone, Debug)]
pub struct OperationalBounds {
    pub timeout: Duration,
    pub batch_requests: usize,
    pub input_bytes: usize,
    pub output_bytes: usize,
}
impl Default for OperationalBounds {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            batch_requests: 1000,
            input_bytes: MAX_REQUEST_BYTES,
            output_bytes: 64 * 1024 * 1024,
        }
    }
}
impl OperationalBounds {
    pub fn validate(&self) -> Result<()> {
        let maximum = Self::default();
        require(
            !self.timeout.is_zero() && self.timeout <= maximum.timeout,
            "invalid operational limit: timeout_seconds",
        )?;
        for (key, v, max) in [
            (
                "batch_requests",
                self.batch_requests,
                maximum.batch_requests,
            ),
            ("input_bytes", self.input_bytes, maximum.input_bytes),
            ("output_bytes", self.output_bytes, maximum.output_bytes),
        ] {
            require(
                v > 0 && v <= max,
                &format!("invalid operational limit: {key}"),
            )?;
        }
        Ok(())
    }
}
fn terminate(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        use nix::{
            sys::signal::{Signal, killpg},
            unistd::Pid,
        };
        if killpg(Pid::from_raw(child.id() as i32), Signal::SIGKILL).is_ok() {
            return;
        }
    }
    let _ = child.kill();
}
/// Drain stderr under the same hard cap as stdout. A deadline also bounds pipes
/// inherited by another process; detached readers retain no unbounded buffer.
pub fn run_bounded(
    command: &Path,
    args: &[&str],
    payload: Vec<u8>,
    timeout: Duration,
    output_limit: usize,
) -> Result<Vec<u8>> {
    let mut cmd = Command::new(command);
    cmd.args(args);
    run_command_bounded(&mut cmd, payload, timeout, output_limit)
}
pub(crate) fn run_command_bounded(
    cmd: &mut Command,
    payload: Vec<u8>,
    timeout: Duration,
    output_limit: usize,
) -> Result<Vec<u8>> {
    let (status, output) = run_command_bounded_with_status(cmd, payload, timeout, output_limit)?;
    require(status.success(), "native reasoning process failed")?;
    Ok(output)
}

/// Run one bounded process while retaining its exit status. Callers must
/// validate the status together with the complete response schema.
pub(crate) fn run_command_bounded_with_status(
    cmd: &mut Command,
    payload: Vec<u8>,
    timeout: Duration,
    output_limit: usize,
) -> Result<(std::process::ExitStatus, Vec<u8>)> {
    let deadline = Instant::now() + timeout;
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn()?;
    struct Output {
        bytes: Vec<u8>,
        used: usize,
        exceeded: bool,
        failed: bool,
    }
    let shared = Arc::new(Mutex::new(Output {
        bytes: Vec::new(),
        used: 0,
        exceeded: false,
        failed: false,
    }));
    let (tx, rx) = mpsc::channel();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    for (mut stream, retain) in [
        (Box::new(stdout) as Box<dyn Read + Send>, true),
        (Box::new(stderr) as Box<dyn Read + Send>, false),
    ] {
        let state = shared.clone();
        let done = tx.clone();
        thread::spawn(move || {
            let mut buf = [0; 8192];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut state = state.lock().unwrap();
                        state.used = state.used.saturating_add(n);
                        if state.used > output_limit {
                            state.exceeded = true;
                            break;
                        }
                        if retain {
                            state.bytes.extend_from_slice(&buf[..n]);
                        }
                    }
                    Err(_) => {
                        state.lock().unwrap().failed = true;
                        break;
                    }
                }
            }
            let _ = done.send(());
        });
    }
    let state = shared.clone();
    thread::spawn(move || {
        if let Err(e) = stdin.write_all(&payload).and_then(|_| stdin.flush())
            && e.kind() != std::io::ErrorKind::BrokenPipe
        {
            state.lock().unwrap().failed = true;
        }
        drop(stdin);
        let _ = tx.send(());
    });
    let mut completed = 0;
    let mut status = None;
    let mut timeout_hit = false;
    let mut polling_error = None;
    loop {
        while rx.try_recv().is_ok() {
            completed += 1;
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(s) => status = s,
                Err(e) => {
                    polling_error = Some(e);
                    break;
                }
            }
        }
        if status.is_some() && completed == 3 {
            break;
        }
        if shared.lock().unwrap().exceeded {
            terminate(&mut child);
            break;
        }
        if Instant::now() >= deadline {
            timeout_hit = true;
            terminate(&mut child);
            break;
        }
        if rx
            .recv_timeout(
                Duration::from_millis(5).min(deadline.saturating_duration_since(Instant::now())),
            )
            .is_ok()
        {
            completed += 1;
        }
    }
    if polling_error.is_some() {
        terminate(&mut child);
    }
    let status = child.wait()?;
    let mut state = shared.lock().unwrap();
    if let Some(e) = polling_error {
        return Err(e.into());
    }
    require(!timeout_hit, "runtime_timeout")?;
    require(!state.exceeded, "output_limit")?;
    require(!state.failed, "native reasoning process failed")?;
    Ok((status, std::mem::take(&mut state.bytes)))
}
pub fn target_name() -> Result<String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let name = match (os, arch) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        ("linux", "x86_64") => "linux-x86_64",
        ("windows", "x86_64") => "windows-x86_64",
        _ => return Err(Error(format!("unsupported runtime platform: {os}-{arch}"))),
    };
    Ok(name.into())
}
fn relative(name: &str) -> Result<()> {
    require(
        !name.is_empty() && !name.contains(['\\', ':']),
        "invalid runtime archive path",
    )?;
    require(
        !name.starts_with('/')
            && !name
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == ".."),
        "runtime archive path escapes its directory",
    )
}
fn hash_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0; 8192];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}
/// Resolve only regular resource files beneath an already selected root. Missing
/// paths are allowed for diagnostics, but symlinks and non-directory ancestors
/// never select another deployment implicitly.
pub(crate) fn resource_path(root: &Path, name: &str) -> Result<PathBuf> {
    relative(name)?;
    let parts = name.split('/').collect::<Vec<_>>();
    let mut path = root.to_path_buf();
    for (index, part) in parts.iter().enumerate() {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(meta) => require(
                !meta.file_type().is_symlink() && (index + 1 == parts.len() || meta.is_dir()),
                "runtime_symlink",
            )?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(path)
}
pub(crate) fn resource_read(root: &Path, name: &str, maximum: usize) -> Result<Vec<u8>> {
    let path = resource_path(root, name)?;
    let meta = fs::metadata(&path)?;
    require(
        meta.is_file() && meta.len() <= maximum as u64,
        "runtime_size_limit",
    )?;
    let mut raw = Vec::new();
    File::open(&path)?
        .take(maximum as u64 + 1)
        .read_to_end(&mut raw)?;
    require(raw.len() <= maximum, "runtime_size_limit")?;
    resource_path(root, name)?;
    Ok(raw)
}
pub(crate) fn resource_hash(root: &Path, name: &str, maximum: usize) -> Result<String> {
    use sha2::{Digest, Sha256};
    let path = resource_path(root, name)?;
    let meta = fs::metadata(&path)?;
    require(
        meta.is_file() && meta.len() <= maximum as u64,
        "runtime_size_limit",
    )?;
    let mut file = File::open(&path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    let mut total = 0usize;
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n);
        require(total <= maximum, "runtime_size_limit")?;
        digest.update(&buffer[..n]);
    }
    resource_path(root, name)?;
    Ok(format!("{:x}", digest.finalize()))
}
/// Validate the existing archive contract without extracting, executing or
/// creating a cache. Declarations re-read this observation before returning.
pub(crate) fn inspect_archive(root: &Path, name: &str, target: &str) -> Result<J> {
    let raw = resource_read(root, name, 32 * 1024 * 1024)?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&raw))
        .map_err(|_| Error("invalid_runtime_archive".into()))?;
    require(zip.len() <= 1024, "runtime_size_limit")?;
    let mut names = BTreeSet::new();
    let mut expanded = 0u64;
    for i in 0..zip.len() {
        let file = zip
            .by_index(i)
            .map_err(|_| Error("invalid_runtime_archive".into()))?;
        relative(file.name())?;
        expanded = expanded.saturating_add(file.size());
        require(
            names.insert(file.name().to_owned())
                && !file.is_dir()
                && !file.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
                && file.size() <= MAX_ARCHIVE_MEMBER
                && expanded <= 256 * 1024 * 1024,
            "invalid_runtime_archive",
        )?;
    }
    require(names.contains("manifest.json"), "invalid_runtime_archive")?;
    let mut manifest_bytes = Vec::new();
    zip.by_name("manifest.json")
        .map_err(|_| Error("invalid_runtime_archive".into()))?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut manifest_bytes)?;
    require(manifest_bytes.len() <= 1024 * 1024, "runtime_size_limit")?;
    let manifest = crate::json_ingress::parse_slice(
        &manifest_bytes,
        crate::json_ingress::DuplicateKeys::Reject,
    )
    .map_err(|_| Error("invalid_runtime_json".into()))?;
    Runtime::validate_manifest(&manifest, target)?;
    let files = manifest["files"].as_object().unwrap();
    require(
        names
            == files
                .keys()
                .cloned()
                .chain(["manifest.json".into()])
                .collect(),
        "invalid_runtime_archive",
    )?;
    for (name, hash) in files {
        let mut bytes = Vec::new();
        zip.by_name(name)
            .map_err(|_| Error("invalid_runtime_archive".into()))?
            .take(MAX_ARCHIVE_MEMBER + 1)
            .read_to_end(&mut bytes)?;
        require(
            bytes.len() as u64 <= MAX_ARCHIVE_MEMBER && hash == &J::String(sha256(&bytes)),
            "runtime_archive_checksum_mismatch",
        )?;
    }
    Ok(
        json!({"status":"archive_validated", "path":name, "sha256":sha256(&raw), "manifest":manifest}),
    )
}
#[derive(Debug)]
pub struct Runtime {
    ordinary: Option<crate::ordinary_runtime::Program>,
    pub root: PathBuf,
    pub binary: PathBuf,
    pub manifest: J,
    pub implementation: J,
    bounds: OperationalBounds,
    observed: BTreeMap<String, String>,
}
impl Runtime {
    pub fn with_ordinary_program(mut self, mut program: crate::ordinary_runtime::Program) -> Self {
        program.limit(&self.bounds);
        self.ordinary = Some(program);
        self
    }
    pub(crate) fn ordinary_program(&self) -> Option<&crate::ordinary_runtime::Program> {
        self.ordinary.as_ref()
    }
    pub fn open(archive: &Path, cache: &Path, bounds: OperationalBounds) -> Result<Self> {
        bounds.validate()?;
        let target = target_name()?;
        let archive_digest = hash_file(archive)?;
        let mut zip = zip::ZipArchive::new(File::open(archive)?)
            .map_err(|_| Error("invalid packaged reasoning runtime".into()))?;
        let mut names = BTreeSet::new();
        for i in 0..zip.len() {
            let file = zip
                .by_index(i)
                .map_err(|_| Error("invalid packaged reasoning runtime".into()))?;
            require(
                names.insert(file.name().to_owned()),
                "invalid runtime archive inventory",
            )?;
            relative(file.name())?;
            require(
                !file.is_dir()
                    && !file.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
                    && file.size() <= MAX_ARCHIVE_MEMBER,
                "unsupported runtime archive member",
            )?;
        }
        require(
            names.contains("manifest.json"),
            "invalid runtime archive inventory",
        )?;
        let mut manifest_file = zip
            .by_name("manifest.json")
            .map_err(|_| Error("invalid runtime archive inventory".into()))?;
        let mut manifest_raw = Vec::with_capacity(manifest_file.size() as usize);
        Read::by_ref(&mut manifest_file)
            .take(MAX_ARCHIVE_MEMBER + 1)
            .read_to_end(&mut manifest_raw)?;
        drop(manifest_file);
        require(
            manifest_raw.len() as u64 <= MAX_ARCHIVE_MEMBER,
            "unsupported runtime archive member",
        )?;
        let manifest = crate::json_ingress::parse_slice(
            &manifest_raw,
            crate::json_ingress::DuplicateKeys::LastWins,
        )
        .map_err(|_| Error("invalid packaged reasoning runtime".into()))?;
        Self::validate_manifest(&manifest, &target)?;
        let files = manifest["files"].as_object().unwrap();
        require(
            names
                == files
                    .keys()
                    .cloned()
                    .chain(["manifest.json".into()])
                    .collect(),
            "runtime archive inventory disagrees with manifest",
        )?;
        let root = cache.join("kpopper/reasoning").join(&archive_digest);
        if !root.exists() {
            let parent = root.parent().unwrap();
            fs::create_dir_all(parent)?;
            let temporary = tempfile::Builder::new()
                .prefix(".extract-")
                .tempdir_in(parent)?;
            let stage = temporary.path().join("runtime");
            fs::create_dir(&stage)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&stage, fs::Permissions::from_mode(0o700))?;
            }
            for (name, expected) in files {
                let mut member = zip
                    .by_name(name)
                    .map_err(|_| Error("invalid packaged reasoning runtime".into()))?;
                let mut data = Vec::new();
                member.read_to_end(&mut data)?;
                require(
                    Some(sha256(&data).as_str()) == expected.as_str(),
                    &format!("runtime archive checksum mismatch: {name}"),
                )?;
                let path = stage.join(name);
                fs::create_dir_all(path.parent().unwrap())?;
                fs::write(&path, data)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(
                        path,
                        fs::Permissions::from_mode(
                            if Some(name.as_str()) == manifest["executable"].as_str() {
                                0o755
                            } else {
                                0o644
                            },
                        ),
                    )?;
                }
            }
            if let Err(e) = fs::rename(&stage, &root)
                && !root.is_dir()
            {
                return Err(e.into());
            }
        }
        let binary = root.join(manifest["executable"].as_str().unwrap());
        let mut runtime = Self {
            ordinary: None,
            root,
            binary,
            manifest,
            implementation: J::Null,
            bounds,
            observed: BTreeMap::new(),
        };
        runtime.observed = runtime.verify_files()?;
        let libraries = runtime.manifest["libraries"].as_array().unwrap();
        let library_hashes = libraries
            .iter()
            .map(|n| {
                let name = n.as_str().unwrap();
                (name.to_owned(), runtime.observed[name].clone())
            })
            .collect::<BTreeMap<_, _>>();
        let modified = libraries
            .iter()
            .filter(|n| {
                runtime.observed[n.as_str().unwrap()]
                    != runtime.manifest["files"][n.as_str().unwrap()]
                        .as_str()
                        .unwrap()
            })
            .cloned()
            .collect::<Vec<_>>();
        runtime.implementation = json!({"protocol":"KP2","lean_version":runtime.manifest["lean_version"],"adapter_source_sha256":env!("KPOP_REASONING_ADAPTER_SHA256"),"source_sha256":runtime.manifest["source_sha256"],"archive_sha256":archive_digest,"binary_sha256":runtime.observed[runtime.manifest["executable"].as_str().unwrap()],"target":target,"libraries":library_hashes,"modified_libraries":modified});
        Ok(runtime)
    }
    pub(crate) fn validate_manifest(d: &J, target: &str) -> Result<()> {
        let required = [
            "version",
            "protocols",
            "target",
            "min_os",
            "lean_version",
            "source_sha256",
            "files",
            "executable",
            "libraries",
            "modules",
        ];
        require(
            d.as_object().is_some_and(|m| {
                m.len() == required.len() && required.iter().all(|k| m.contains_key(*k))
            }) && d["version"].as_u64() == Some(3)
                && d["protocols"] == json!(PROTOCOLS)
                && d["target"] == target
                && d["min_os"].as_str().is_some_and(|s| !s.trim().is_empty())
                && d["lean_version"] == "4.33.1"
                && d["modules"] == json!(MODULES)
                && d["files"].is_object()
                && d["libraries"].is_array(),
            "unsupported runtime manifest",
        )?;
        require(
            d["source_sha256"] == env!("KPOP_REASONING_SOURCE_SHA256"),
            "packaged runtime does not match its source revision",
        )?;
        let files = d["files"].as_object().unwrap();
        for (name, expected) in files {
            relative(name)?;
            require(
                expected.as_str().is_some_and(|s| {
                    s.len() == 64
                        && s.bytes()
                            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                }),
                "invalid runtime file digest",
            )?;
        }
        let executable = d["executable"]
            .as_str()
            .ok_or_else(|| Error("invalid runtime executable/library inventory".into()))?;
        let libraries = d["libraries"].as_array().unwrap();
        let names = libraries.iter().map(J::as_str).collect::<Vec<_>>();
        require(
            files.contains_key(executable)
                && names
                    .iter()
                    .all(|n| n.is_some_and(|n| files.contains_key(n) && n != executable))
                && names.iter().collect::<BTreeSet<_>>().len() == names.len(),
            "invalid runtime executable/library inventory",
        )
    }
    fn verify_files(&self) -> Result<BTreeMap<String, String>> {
        let libraries = self.manifest["libraries"].as_array().unwrap();
        let mut observed = BTreeMap::new();
        for (name, expected) in self.manifest["files"].as_object().unwrap() {
            let path = self.root.join(name);
            require(
                path.is_file() && !fs::symlink_metadata(&path)?.file_type().is_symlink(),
                &format!("runtime member is unavailable: {name}"),
            )?;
            let actual = hash_file(&path)?;
            require(
                libraries.contains(&J::String(name.clone()))
                    || Some(actual.as_str()) == expected.as_str(),
                &format!("runtime member checksum changed: {name}"),
            )?;
            observed.insert(name.clone(), actual);
        }
        Ok(observed)
    }
    pub fn implementation_for(&self, request: &J) -> Result<J> {
        let m = request
            .as_object()
            .ok_or_else(|| Error("invalid reasoning request".into()))?;
        let protocol = match m.get("protocol") {
            None | Some(J::Null) => {
                if m.get("version").and_then(J::as_f64) == Some(4.0) {
                    "KP4"
                } else {
                    "KP2"
                }
            }
            Some(J::String(s)) => s,
            _ => return Err(Error("invalid reasoning request".into())),
        };
        require(
            !(protocol == "KP2" && m.contains_key("protocol")),
            "invalid explicit reasoning protocol",
        )?;
        require(
            PROTOCOLS.contains(&protocol),
            "runtime does not support request protocol or modules",
        )?;
        if protocol == "KP4" {
            let raw = m.get("request").unwrap_or(request);
            let modules = raw["required_modules"].as_array().ok_or_else(|| {
                Error("runtime does not support request protocol or modules".into())
            })?;
            let names = modules.iter().map(J::as_str).collect::<Vec<_>>();
            require(
                names.contains(&Some("query/v1"))
                    && names
                        .iter()
                        .all(|n| n.is_some_and(|n| MODULES.contains(&n)))
                    && names.windows(2).all(|w| w[0] < w[1]),
                "runtime does not support request protocol or modules",
            )?;
        }
        let mut implementation = self.implementation.clone();
        implementation["protocol"] = json!(protocol);
        Ok(implementation)
    }
    pub fn request_many(&self, requests: &[J]) -> Result<Vec<J>> {
        self.request_many_bounded(requests, &self.bounds)
    }
    pub fn bounds(&self) -> &OperationalBounds {
        &self.bounds
    }
    pub fn request_many_bounded(
        &self,
        requests: &[J],
        requested: &OperationalBounds,
    ) -> Result<Vec<J>> {
        requested.validate()?;
        let bounds = OperationalBounds {
            timeout: requested.timeout.min(self.bounds.timeout),
            batch_requests: requested.batch_requests.min(self.bounds.batch_requests),
            input_bytes: requested.input_bytes.min(self.bounds.input_bytes),
            output_bytes: requested.output_bytes.min(self.bounds.output_bytes),
        };
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        require(
            self.verify_files()? == self.observed,
            "runtime changed; reopen before evaluating",
        )?;
        let mut payload = Vec::new();
        let mut protocols = Vec::new();
        for (request_index, request) in requests.iter().enumerate() {
            require(request_index < bounds.batch_requests, "batch_request_limit")?;
            let implementation = self.implementation_for(request)?;
            let encoded = reasoning_transport::encode_request(request)?;
            let mut line = encoded.into_bytes();
            if implementation["protocol"] != "KP4" {
                line.push(b'\n');
            }
            require(
                line.len() <= MAX_REQUEST_BYTES && payload.len() + line.len() <= bounds.input_bytes,
                "batch_input_limit",
            )?;
            payload.extend(line);
            protocols.push(if implementation["protocol"] == "KP3" {
                "KR3"
            } else if implementation["protocol"] == "KP4" {
                "KR4"
            } else {
                "KR2"
            });
        }
        let output = run_bounded(
            &self.binary,
            &[],
            payload,
            bounds.timeout,
            bounds.output_bytes,
        )?;
        require(
            self.verify_files()? == self.observed,
            "runtime changed during evaluation",
        )?;
        let mut position = 0;
        let mut decoded = Vec::new();
        for protocol in protocols {
            let size = output[position..]
                .iter()
                .position(|b| *b == b'\n')
                .ok_or_else(|| Error("native response count does not match requests".into()))?;
            if protocol == "KR4" {
                let header = &output[position..position + size];
                require(
                    header.starts_with(b"KR4 "),
                    "native response protocol does not match request",
                )?;
                let length = std::str::from_utf8(&header[4..])
                    .map_err(|_| Error("invalid native KR4 frame length".into()))?;
                require(
                    !length.is_empty()
                        && length.bytes().all(|b| b.is_ascii_digit())
                        && (!length.starts_with('0') || length == "0"),
                    "invalid native KR4 frame length",
                )?;
                let length = length
                    .parse::<usize>()
                    .map_err(|_| Error("truncated native KR4 frame".into()))?;
                let stop = (position + size + 1)
                    .checked_add(length)
                    .ok_or_else(|| Error("truncated native KR4 frame".into()))?;
                require(
                    stop < output.len() && output[stop] == b'\n',
                    "truncated native KR4 frame",
                )?;
                decoded.push(reasoning_transport::decode_response(
                    &output[position..=stop],
                )?);
                position = stop + 1;
                continue;
            }
            let frame = &output[position..position + size];
            require(
                frame.split(|b| *b == b'\t').next() == Some(protocol.as_bytes()),
                "native response protocol does not match request",
            )?;
            decoded.push(reasoning_transport::decode_response(frame)?);
            position += size + 1;
        }
        require(
            position == output.len(),
            "native response count does not match requests",
        )?;
        for value in &decoded {
            let status = value
                .as_object()
                .and_then(|m| m.get("status"))
                .ok_or_else(|| Error("invalid native response".into()))?;
            if status == "ok" {
                crate::reasoning_query::typed_value(
                    value
                        .get("value")
                        .ok_or_else(|| Error("invalid native response".into()))?,
                )?;
            }
        }
        Ok(decoded)
    }
}

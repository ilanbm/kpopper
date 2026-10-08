//! Isolated consumer runtime for verified model bundles.
use crate::{
    manifest::{Descriptor, PackageLock, Smoke, ENGINE_SHA256, ENGINE_SIZE, ENGINE_VERSION},
    Result,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const MAX_ARCHIVE_ENTRIES: usize = 2_000;
const MAX_ARCHIVE_TOTAL: u64 = 256 * 1024 * 1024;
const MAX_ARCHIVE_MEMBER: u64 = 128 * 1024 * 1024;
const MAX_OUTPUT: usize = 8 * 1024 * 1024;
const MAX_READ_IDS: usize = 64;
const MAX_QUERY_BYTES: usize = 16 * 1024;
const MAX_CASE_BYTES: u64 = 16 * 1024 * 1024;
const PROCESS_TIMEOUT: Duration = Duration::from_secs(120);
static CANCELLED_BY: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

pub fn cancellation_exit_code() -> Option<i32> {
    let signal = CANCELLED_BY.load(std::sync::atomic::Ordering::Relaxed);
    (signal != 0).then_some(128 + signal)
}

pub fn install_signal_handlers() {
    #[cfg(unix)]
    {
        extern "C" fn cancelled(signal: nix::libc::c_int) {
            CANCELLED_BY.store(signal, std::sync::atomic::Ordering::Relaxed);
        }
        unsafe {
            for signal in [nix::libc::SIGTERM, nix::libc::SIGINT, nix::libc::SIGHUP] {
                nix::libc::signal(signal, cancelled as *const () as nix::libc::sighandler_t);
            }
        }
    }
}

fn require_not_cancelled() -> Result<()> {
    if cancellation_exit_code().is_some() {
        Err(err("operation cancelled"))
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum Operation {
    Setup,
    Doctor,
    Versions,
    Activate {
        digest: String,
    },
    Pull {
        ids: Vec<String>,
    },
    Context {
        ids: Vec<String>,
    },
    Search {
        query: String,
    },
    Run {
        case: PathBuf,
        as_of: String,
        policy_overlay: Option<PathBuf>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    format: String,
    origin: String,
    id: String,
    version: String,
    digest: String,
    descriptor_sha256: String,
    source_commit: String,
    engine_version: String,
    engine_sha256: String,
    files: BTreeMap<String, crate::manifest::FileIdentity>,
    engine_files: BTreeMap<String, crate::manifest::FileIdentity>,
}

/// Validate a complete bundle with the pinned engine in disposable state only.
pub(crate) fn validate_bundle_runtime(bundle: &Path) -> Result<()> {
    require_not_cancelled()?;
    let bundle = bundle.canonicalize()?;
    let (descriptor, lock, _digest) = crate::package::verify_bundle(&bundle)?;
    let scratch = tempfile::tempdir()?;
    let stage = scratch.path().join("validation");
    fs::create_dir(&stage)?;
    let payload = stage.join("payload");
    fs::create_dir(&payload)?;
    for rel in lock.files.keys() {
        require_not_cancelled()?;
        let from = safe_join(&bundle, rel)?;
        let to = safe_join(&payload, rel)?;
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        copy_checked(&from, &to)?;
    }
    copy_checked(
        &bundle.join("model-package.lock.json"),
        &payload.join("model-package.lock.json"),
    )?;
    let (checked_descriptor, checked_lock, _) = crate::package::verify_bundle(&payload)?;
    if checked_descriptor.id != descriptor.id || checked_lock.files != lock.files {
        return Err(err(
            "disposable package snapshot differs from the verified bundle",
        ));
    }
    let archive = payload.join("assets/kpopper.tar.gz");
    let inventory = payload.join("engine-members.json");
    if fs::metadata(&archive)?.len() != ENGINE_SIZE || digest_path(&archive)? != ENGINE_SHA256 {
        return Err(err("bundled engine archive does not match pinned identity"));
    }
    if fs::read(&inventory)? != crate::package::engine_inventory(&archive)? {
        return Err(err("engine member inventory differs from pinned archive"));
    }
    let engine = stage.join("engine");
    fs::create_dir(&engine)?;
    extract_engine(&archive, &engine)?;
    let engine_root = engine.join("kpopper-0.15.1-darwin-arm64");
    validate_expanded_inventory(&fs::read(&inventory)?, &engine)?;
    let binary = engine_root.join("bin/kpop");
    let version = bounded_command(&binary, &["--version".into()], &engine)?;
    if !version.status.success()
        || String::from_utf8_lossy(&version.stdout)
            .split_whitespace()
            .last()
            != Some(ENGINE_VERSION)
    {
        return Err(err("bundled native engine failed exact version check"));
    }
    validate_model_sources(&stage, &descriptor, &binary, &lock.files)?;
    run_smoke(&stage, &descriptor, &binary, &lock.files)?;
    require_not_cancelled()?;
    Ok(())
}

pub fn execute(bundle: &Path, cache: Option<&Path>, operation: Operation) -> Result<Value> {
    require_not_cancelled()?;
    let bundle = bundle.canonicalize()?;
    let (descriptor, lock, digest) = crate::package::verify_bundle(&bundle)?;
    let root = match cache {
        Some(p) if p.is_absolute() => p.to_path_buf(),
        Some(_) => return Err(err("--cache must be an absolute directory")),
        None => default_cache()?,
    };
    ensure_private_dir(&root)?;
    let install_key = hex(&Sha256::digest(
        format!("{}\0{}", bundle.display(), descriptor.id).as_bytes(),
    ));
    let namespace = root.join(install_key);
    ensure_private_dir(&namespace)?;
    let origin = bundle.to_string_lossy().into_owned();
    let _guard = lock_namespace(&namespace)?;
    match operation {
        Operation::Setup => {
            let generation =
                ensure_generation(&bundle, &namespace, &descriptor, &lock, &digest, &origin)?;
            require_not_cancelled()?;
            activate_pointer(&namespace, &digest)?;
            Ok(envelope(
                &descriptor,
                &lock,
                &digest,
                "setup",
                json!({"active":digest,"generation":generation}),
            ))
        }
        Operation::Doctor => {
            let active = active_digest(&namespace)?;
            let g = active
                .as_ref()
                .ok_or_else(|| err("no active generation; run setup"))?;
            if g != &digest && !valid_rollback(&namespace, &digest, g)? {
                return Err(err("active model differs from this bundle; run setup or explicitly activate a generation"));
            }
            let receipt = validate_generation(&namespace, g, &origin, &descriptor.id)?;
            let (selected_descriptor, selected_lock, selected_digest) =
                crate::package::verify_bundle(
                    &namespace.join("generations").join(g).join("payload"),
                )?;
            let selected =
                selected_identity(&selected_descriptor, &selected_lock, &selected_digest);
            Ok(envelope(
                &descriptor,
                &lock,
                &digest,
                "doctor",
                json!({"active":g,"generation":receipt,"selected":selected,"healthy":true}),
            ))
        }
        Operation::Versions => {
            let mut generations = Vec::new();
            let dir = namespace.join("generations");
            if dir.exists() {
                for ent in fs::read_dir(dir)? {
                    require_not_cancelled()?;
                    let ent = ent?;
                    if ent.file_type()?.is_dir() {
                        let d = ent.file_name().to_string_lossy().into_owned();
                        let _receipt =
                            match validate_generation(&namespace, &d, &origin, &descriptor.id) {
                                Ok(r) => r,
                                Err(_) => {
                                    require_not_cancelled()?;
                                    continue;
                                }
                            };
                        let (selected_descriptor, selected_lock, selected_digest) =
                            crate::package::verify_bundle(
                                &namespace.join("generations").join(&d).join("payload"),
                            )?;
                        generations.push(json!({"digest":selected_digest,"version":selected_descriptor.version,"model":selected_descriptor.model,"skill":selected_descriptor.skill,"selected":selected_identity(&selected_descriptor,&selected_lock,&selected_digest),"active":active_digest(&namespace)?.as_deref()==Some(&d)}));
                    }
                }
            }
            Ok(envelope(
                &descriptor,
                &lock,
                &digest,
                "versions",
                json!({"generations":generations}),
            ))
        }
        Operation::Activate { digest: target } => {
            validate_hex(&target)?;
            validate_generation(&namespace, &target, &origin, &descriptor.id)?;
            let (selected_descriptor, selected_lock, selected_digest) =
                crate::package::verify_bundle(
                    &namespace.join("generations").join(&target).join("payload"),
                )?;
            let selected =
                selected_identity(&selected_descriptor, &selected_lock, &selected_digest);
            let record = json!({"format":"kpop-model-rollback/v1","invoking_digest":digest,"target_digest":target});
            atomic_json(&namespace.join(format!("rollback-{digest}.json")), &record)?;
            activate_pointer(&namespace, &target)?;
            Ok(envelope(
                &descriptor,
                &lock,
                &digest,
                "activate",
                json!({"active":target,"version":selected_descriptor.version,"selected":selected,"rollback":true}),
            ))
        }
        op => {
            let active =
                active_digest(&namespace)?.ok_or_else(|| err("no active generation; run setup"))?;
            if active != digest && !valid_rollback(&namespace, &digest, &active)? {
                return Err(err("active model differs from this bundle; run setup or explicitly activate a generation"));
            }
            let receipt = validate_generation(&namespace, &active, &origin, &descriptor.id)?;
            let gen = namespace.join("generations").join(&active);
            let (selected_descriptor, selected_lock, _) =
                crate::package::verify_bundle(&gen.join("payload"))?;
            // Pin the immutable generation, then allow independent reads/updates.
            drop(_guard);
            let scratch = tempfile::tempdir_in(&namespace)?;
            let model_dst = scratch.path().join("model");
            let model_files = selected_model_files(&receipt.files, &selected_descriptor.model)?;
            copy_tree_limited(&gen.join("payload"), &model_dst, &model_files)?;
            let workspace = scratch.path().join("workspace");
            fs::create_dir(&workspace)?;
            let state = scratch.path().join("state");
            fs::create_dir(&state)?;
            let engine = gen
                .join("engine")
                .join("kpopper-0.15.1-darwin-arm64/bin/kpop");
            let mut result = match op {
                Operation::Pull { ids } => native_read(
                    &engine,
                    &workspace,
                    &model_dst.join(&selected_descriptor.model),
                    &state,
                    "pull",
                    ids,
                )?,
                Operation::Context { ids } => native_read(
                    &engine,
                    &workspace,
                    &model_dst.join(&selected_descriptor.model),
                    &state,
                    "context",
                    ids,
                )?,
                Operation::Search { query } => native_search(
                    &engine,
                    &workspace,
                    &model_dst.join(&selected_descriptor.model),
                    &state,
                    &query,
                )?,
                Operation::Run {
                    case,
                    as_of,
                    policy_overlay,
                } => run_adapter(
                    &gen,
                    &selected_descriptor,
                    &engine,
                    &model_dst,
                    &case,
                    &as_of,
                    policy_overlay.as_deref(),
                )?,
                _ => unreachable!(),
            };
            validate_generation(&namespace, &active, &origin, &descriptor.id)?;
            require_not_cancelled()?;
            result["selected"] = selected_identity(&selected_descriptor, &selected_lock, &active);
            let mut response = envelope(
                &selected_descriptor,
                &selected_lock,
                &active,
                "execute",
                result,
            );
            response["invoking_package_digest"] = json!(digest);
            Ok(response)
        }
    }
}

fn ensure_generation(
    bundle: &Path,
    ns: &Path,
    d: &Descriptor,
    l: &PackageLock,
    digest: &str,
    origin: &str,
) -> Result<PathBuf> {
    let base = ns.join("generations");
    ensure_private_dir(&base)?;
    let dest = base.join(digest);
    let repair_existing = match fs::symlink_metadata(&dest) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e.into()),
        Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
            return Err(err(&format!(
                "refusing to replace unmanaged generation path {}",
                dest.display()
            )));
        }
        Ok(_) => match validate_generation(ns, digest, origin, &d.id) {
            Ok(_) => return Ok(dest),
            Err(validation) => {
                if !managed_generation_lock(&dest, digest) {
                    return Err(err(&format!(
                        "refusing to replace unmanaged generation at {}: {validation}",
                        dest.display()
                    )));
                }
                true
            }
        },
    };
    let stage = tempfile::Builder::new()
        .prefix(".stage-")
        .tempdir_in(&base)?;
    let payload = stage.path().join("payload");
    fs::create_dir(&payload)?;
    for rel in l.files.keys() {
        let p = safe_join(bundle, rel)?;
        let q = safe_join(&payload, rel)?;
        if let Some(parent) = q.parent() {
            fs::create_dir_all(parent)?;
        }
        copy_checked(&p, &q)?;
    }
    copy_checked(
        &bundle.join("model-package.lock.json"),
        &payload.join("model-package.lock.json"),
    )?;
    let archive = bundle.join("assets/kpopper.tar.gz");
    if !archive.is_file() {
        return Err(err("bundle missing assets/kpopper.tar.gz"));
    }
    if archive.metadata()?.len() != ENGINE_SIZE || digest_path(&archive)? != ENGINE_SHA256 {
        return Err(err(
            "bundled engine archive does not match pinned kpopper 0.15.1 darwin-arm64",
        ));
    }
    let engine = stage.path().join("engine");
    fs::create_dir(&engine)?;
    let inventory = fs::read(bundle.join("engine-members.json"))?;
    if inventory != crate::package::engine_inventory(&archive)? {
        return Err(err("engine member inventory differs from pinned archive"));
    }
    extract_engine(&archive, &engine)?;
    let engine_root = engine.join("kpopper-0.15.1-darwin-arm64");
    validate_expanded_inventory(&inventory, &engine)?;
    for required in [
        "bin/kpop",
        "bin/resources/ordinary/darwin-arm64",
        "bin/resources/reasoning/native",
    ] {
        if !engine_root.join(required).exists() {
            return Err(err("required native engine resource is missing"));
        }
    }
    let engine_files = tree_identity(&engine_root)?;
    let binary = engine_root.join("bin/kpop");
    let out = bounded_command(&binary, &["--version".to_string()], &engine)?;
    let version_text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || version_text.split_whitespace().last() != Some(ENGINE_VERSION) {
        return Err(err("bundled native engine failed exact version check"));
    }
    validate_model_sources(stage.path(), d, &binary, &l.files)?;
    run_smoke(stage.path(), d, &binary, &l.files)?;
    let receipt = Receipt {
        format: "kpop-model-install/v1".into(),
        origin: origin.into(),
        id: d.id.clone(),
        version: d.version.clone(),
        digest: digest.into(),
        descriptor_sha256: l.descriptor_sha256.clone(),
        source_commit: l.source_commit.clone(),
        engine_version: ENGINE_VERSION.into(),
        engine_sha256: ENGINE_SHA256.into(),
        files: l.files.clone(),
        engine_files,
    };
    atomic_json(&stage.path().join("receipt.json"), &receipt)?;
    freeze_tree(stage.path())?;
    let quarantine = if repair_existing {
        Some(quarantine_generation(&base, &dest, digest)?)
    } else {
        None
    };
    if let Err(rename_error) =
        rename_managed_directory(stage.path(), &dest, "activate staged generation")
    {
        if let Some(old) = &quarantine {
            rename_managed_directory(old, &dest, "restore quarantined generation").map_err(
                |restore_error| {
                    err(&format!(
                        "{rename_error}; additionally failed to restore {} to {}: {restore_error}",
                        old.display(),
                        dest.display()
                    ))
                },
            )?;
        }
        return Err(rename_error);
    }
    Ok(dest)
}
fn managed_generation_lock(generation: &Path, expected_digest: &str) -> bool {
    let path = generation.join("payload/model-package.lock.json");
    let Ok(root_meta) = fs::symlink_metadata(generation) else {
        return false;
    };
    let Ok(payload_meta) = fs::symlink_metadata(generation.join("payload")) else {
        return false;
    };
    let Ok(lock_meta) = fs::symlink_metadata(&path) else {
        return false;
    };
    if root_meta.file_type().is_symlink()
        || payload_meta.file_type().is_symlink()
        || lock_meta.file_type().is_symlink()
        || !root_meta.is_dir()
        || !payload_meta.is_dir()
        || !lock_meta.is_file()
    {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if lock_meta.nlink() != 1 {
            return false;
        }
    }
    let Ok(raw) = fs::read(&path) else {
        return false;
    };
    if hex(&Sha256::digest(&raw)) != expected_digest {
        return false;
    }
    let Ok(lock) = serde_json::from_slice::<PackageLock>(&raw) else {
        return false;
    };
    let Ok(canonical) = serde_json::to_vec(&lock) else {
        return false;
    };
    canonical == raw
        && lock.format == crate::manifest::LOCK_FORMAT
        && lock.engine.version == ENGINE_VERSION
        && lock.engine.sha256 == ENGINE_SHA256
        && lock.engine.size == ENGINE_SIZE
}
fn quarantine_generation(base: &Path, generation: &Path, digest: &str) -> Result<PathBuf> {
    if !managed_generation_lock(generation, digest) {
        return Err(err(&format!(
            "refusing to quarantine unmanaged generation at {}",
            generation.display()
        )));
    }
    let reservation = tempfile::Builder::new()
        .prefix(&format!(".quarantine-{digest}-"))
        .tempdir_in(base)?;
    let target = reservation.path().to_path_buf();
    reservation.close()?;
    rename_managed_directory(generation, &target, "quarantine damaged generation")?;
    Ok(target)
}

fn rename_managed_directory(from: &Path, to: &Path, action: &str) -> Result<()> {
    rename_managed_directory_with(from, to, action, |source, destination| {
        fs::rename(source, destination)
    })
}

#[cfg(unix)]
fn rename_managed_directory_with(
    from: &Path,
    to: &Path,
    action: &str,
    rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let meta = fs::symlink_metadata(from).map_err(|e| {
        err(&format!(
            "{action}: cannot inspect source directory {}: {e}",
            from.display()
        ))
    })?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(err(&format!(
            "{action}: refusing non-directory or symlink source {}",
            from.display()
        )));
    }
    let original_mode = meta.permissions().mode() & 0o777;
    fs::set_permissions(from, fs::Permissions::from_mode(original_mode | 0o200)).map_err(|e| {
        err(&format!(
            "{action}: cannot temporarily enable owner write on {}: {e}",
            from.display()
        ))
    })?;

    if let Err(rename_error) = rename(from, to) {
        return match fs::set_permissions(from, fs::Permissions::from_mode(original_mode)) {
            Ok(()) => Err(err(&format!("{action}: rename {} to {} failed: {rename_error}", from.display(), to.display()))),
            Err(restore_error) => Err(err(&format!("{action}: rename {} to {} failed: {rename_error}; restoring source permissions also failed: {restore_error}", from.display(), to.display()))),
        };
    }

    let destination = fs::symlink_metadata(to).map_err(|e| {
        err(&format!(
            "{action}: renamed destination {} cannot be inspected: {e}",
            to.display()
        ))
    })?;
    if destination.file_type().is_symlink() || !destination.is_dir() {
        return Err(err(&format!(
            "{action}: renamed destination {} is not a managed directory",
            to.display()
        )));
    }
    fs::set_permissions(to, fs::Permissions::from_mode(original_mode)).map_err(|e| {
        err(&format!("{action}: rename {} to {} succeeded, but restoring destination permissions failed: {e}", from.display(), to.display()))
    })?;
    Ok(())
}

#[cfg(not(unix))]
fn rename_managed_directory_with(
    from: &Path,
    to: &Path,
    action: &str,
    rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    let meta = fs::symlink_metadata(from).map_err(|e| {
        err(&format!(
            "{action}: cannot inspect source directory {}: {e}",
            from.display()
        ))
    })?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(err(&format!(
            "{action}: refusing non-directory or symlink source {}",
            from.display()
        )));
    }
    rename(from, to).map_err(|e| {
        err(&format!(
            "{action}: rename {} to {} failed: {e}",
            from.display(),
            to.display()
        ))
    })
}

fn validate_model_sources(
    generation: &Path,
    d: &Descriptor,
    engine: &Path,
    files: &BTreeMap<String, crate::manifest::FileIdentity>,
) -> Result<()> {
    let entry = generation.join("payload").join(&d.model);
    let input = fs::read(&entry)?;
    let scratch = tempfile::tempdir()?;
    let workspace = scratch.path().join("workspace");
    fs::create_dir(&workspace)?;
    let state = scratch.path().join("state");
    fs::create_dir(&state)?;
    let mut c = Command::new(engine);
    sanitize_native_environment(&mut c);
    c.args([
        "--workspace",
        workspace
            .to_str()
            .ok_or_else(|| err("workspace path is not UTF-8"))?,
        "--no-cache",
        "--frozen",
        "--json",
        "history-codec",
    ])
    .current_dir(&workspace)
    .env("HOME", scratch.path())
    .env("XDG_CONFIG_HOME", &state)
    .env("XDG_CACHE_HOME", &state)
    .env("KPOPPER_NATIVE_CACHE", &state)
    .env_remove("KPOPPER_HOME")
    .env_remove("KPOPPER_CONFIG");
    let out = bounded_output_with_input(c, Some(input))?;
    if !out.status.success() {
        return Err(err(&format!(
            "native history-codec rejected the model entry: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    let decoded: Value = serde_json::from_slice(&out.stdout)?;
    let typed = decoded
        .get("typed")
        .ok_or_else(|| err("native history-codec did not return typed model data"))?;
    let mut refs = Vec::new();
    collect_source_file_refs(typed, false, &mut refs)?;
    let model_root = entry
        .parent()
        .ok_or_else(|| err("model entry has no model root"))?;
    let root = model_root.canonicalize()?;
    for reference in refs {
        require_not_cancelled()?;
        validate_relative(Path::new(&reference))?;
        let mut p = root.clone();
        for part in Path::new(&reference).components() {
            require_not_cancelled()?;
            p.push(part.as_os_str());
            if fs::symlink_metadata(&p)?.file_type().is_symlink() {
                return Err(err("model source reference crosses a symlink"));
            }
        }
        let canonical = p.canonicalize()?;
        if !canonical.starts_with(&root) || !canonical.is_file() {
            return Err(err(
                "model source reference escapes the model root or is not a file",
            ));
        }
        let rel = relative_packaged_source(generation, &canonical)?;
        if !files.contains_key(&rel) {
            return Err(err("model source reference is not packaged"));
        }
    }
    Ok(())
}
fn relative_packaged_source(generation: &Path, canonical_source: &Path) -> Result<String> {
    let payload_root = generation.join("payload").canonicalize()?;
    Ok(canonical_source
        .strip_prefix(payload_root)?
        .to_string_lossy()
        .replace('\\', "/"))
}

fn collect_source_file_refs(value: &Value, in_sources: bool, out: &mut Vec<String>) -> Result<()> {
    match value {
        Value::Array(a) if a.len() == 2 && a[0].as_str() == Some("map") => {
            let pairs = a[1]
                .as_array()
                .ok_or_else(|| err("native typed map has invalid shape"))?;
            for pair in pairs {
                require_not_cancelled()?;
                let pair = pair
                    .as_array()
                    .ok_or_else(|| err("native typed map entry has invalid shape"))?;
                if pair.len() != 2 {
                    return Err(err("native typed map entry has invalid arity"));
                }
                let key = pair[0]
                    .as_str()
                    .ok_or_else(|| err("native typed map key is not text"))?;
                let source_scope = in_sources || key == "sources" || key.starts_with("source.");
                if source_scope && key == "file" {
                    let v = &pair[1];
                    if v[0].as_str() != Some("text") {
                        return Err(err("source file reference is not text"));
                    }
                    out.push(
                        v[1].as_str()
                            .ok_or_else(|| err("source file reference is not text"))?
                            .to_owned(),
                    );
                }
                collect_source_file_refs(&pair[1], source_scope, out)?;
            }
        }
        Value::Array(a) if a.len() == 2 && a[0].as_str() == Some("list") => {
            for v in a[1]
                .as_array()
                .ok_or_else(|| err("native typed list has invalid shape"))?
            {
                require_not_cancelled()?;
                collect_source_file_refs(v, in_sources, out)?;
            }
        }
        Value::Array(a) => {
            for v in a {
                require_not_cancelled()?;
                collect_source_file_refs(v, in_sources, out)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn run_smoke(
    generation: &Path,
    d: &Descriptor,
    engine: &Path,
    files: &BTreeMap<String, crate::manifest::FileIdentity>,
) -> Result<()> {
    let payload = generation.join("payload");
    crate::package::verify_bundle(&payload)?;
    let scratch = tempfile::tempdir()?;
    let model = scratch.path().join("model");
    let model_files = selected_model_files(files, &d.model)?;
    copy_tree_limited(&payload, &model, &model_files)?;
    let work = scratch.path().join("workspace");
    fs::create_dir(&work)?;
    let state = scratch.path().join("state");
    fs::create_dir(&state)?;
    match &d.smoke {
        Smoke::Read { id } => {
            let _ = native_read(
                engine,
                &work,
                &model.join(&d.model),
                &state,
                "pull",
                vec![id.clone()],
            )?;
        }
        Smoke::Application {
            case,
            as_of,
            expect,
        } => {
            let case_path = payload.join(case);
            let result = run_adapter(generation, d, engine, &model, &case_path, as_of, None)?;
            for (pointer, want) in expect {
                require_not_cancelled()?;
                if result.pointer(pointer) != Some(want) {
                    return Err(err(&format!("application smoke mismatch at {pointer}")));
                }
            }
        }
    }
    crate::package::verify_bundle(&payload)?;
    Ok(())
}
fn verify_payload(
    payload: &Path,
    files: &BTreeMap<String, crate::manifest::FileIdentity>,
) -> Result<()> {
    let mut observed = BTreeMap::<String, (u64, String, bool)>::new();
    walk_payload(payload, payload, &mut observed)?;
    if observed.len() != files.len() {
        return Err(err("installed payload has missing or extra files"));
    }
    for (k, v) in files {
        require_not_cancelled()?;
        let Some((size, hash, executable)) = observed.get(k) else {
            return Err(err("installed payload is missing a locked file"));
        };
        if *size != v.size || hash != &v.sha256 || *executable != v.executable {
            return Err(err("installed payload failed integrity or mode check"));
        }
    }
    Ok(())
}
fn walk_payload(
    root: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, (u64, String, bool)>,
) -> Result<()> {
    for ent in fs::read_dir(dir)? {
        require_not_cancelled()?;
        let ent = ent?;
        let path = ent.path();
        let ty = ent.file_type()?;
        if ty.is_symlink() {
            return Err(err("installed payload contains a symlink"));
        }
        if ty.is_dir() {
            walk_payload(root, &path, out)?;
        } else if ty.is_file() {
            let rel = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            let size = ent.metadata()?.len();
            let hash = digest_path(&path)?;
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt;
                ent.metadata()?.permissions().mode() & 0o111 != 0
            };
            #[cfg(not(unix))]
            let executable = false;
            out.insert(rel, (size, hash, executable));
        } else {
            return Err(err("installed payload contains a special file"));
        }
    }
    Ok(())
}

fn freeze_tree(root: &Path) -> Result<()> {
    for ent in fs::read_dir(root)? {
        require_not_cancelled()?;
        let ent = ent?;
        let p = ent.path();
        let ty = ent.file_type()?;
        if ty.is_symlink() {
            return Err(err("staged generation contains a symlink"));
        }
        if ty.is_dir() {
            freeze_tree(&p)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = ent.metadata()?.permissions().mode() & 0o777;
                fs::set_permissions(&p, fs::Permissions::from_mode(mode & !0o222))?;
            }
        } else if ty.is_file() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = ent.metadata()?.permissions().mode() & 0o777;
                fs::set_permissions(&p, fs::Permissions::from_mode(mode & !0o222))?;
            }
        } else {
            return Err(err("staged generation contains a special file"));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(root)?.permissions().mode() & 0o777;
        fs::set_permissions(root, fs::Permissions::from_mode(mode & !0o222))?;
    }
    Ok(())
}

fn validate_expanded_inventory(raw: &[u8], engine_dir: &Path) -> Result<()> {
    #[derive(Deserialize)]
    struct Member {
        path: String,
        size: u64,
        #[serde(rename = "type")]
        kind: String,
    }
    #[derive(Deserialize)]
    struct Inventory {
        archive_sha256: String,
        members: Vec<Member>,
    }
    let inventory: Inventory = serde_json::from_slice(raw)?;
    if inventory.archive_sha256 != ENGINE_SHA256 {
        return Err(err("expanded inventory archive identity mismatch"));
    }
    let expected = inventory.members;
    let mut want = BTreeMap::new();
    for m in expected {
        require_not_cancelled()?;
        validate_relative(Path::new(&m.path))?;
        // Tar preserves a trailing slash on directory names; filesystem walks
        // return the same directory without it. Bind the normalized path once.
        let name = if m.kind == "dir" {
            m.path.trim_end_matches('/').to_owned()
        } else {
            m.path
        };
        if want.insert(name, (m.size, m.kind)).is_some() {
            return Err(err("duplicate engine inventory member"));
        }
    }
    let mut got = BTreeMap::new();
    walk_engine(engine_dir, engine_dir, &mut got)?;
    if want != got {
        return Err(err("expanded engine members differ from locked inventory"));
    }
    Ok(())
}
fn walk_engine(root: &Path, dir: &Path, out: &mut BTreeMap<String, (u64, String)>) -> Result<()> {
    for ent in fs::read_dir(dir)? {
        require_not_cancelled()?;
        let ent = ent?;
        let p = ent.path();
        let ty = ent.file_type()?;
        if ty.is_symlink() {
            return Err(err("expanded engine contains a symlink"));
        }
        let rel = p.strip_prefix(root)?.to_string_lossy().replace('\\', "/");
        if ty.is_dir() {
            out.insert(rel.clone(), (0, "dir".into()));
            walk_engine(root, &p, out)?;
        } else if ty.is_file() {
            out.insert(rel, (ent.metadata()?.len(), "file".into()));
        } else {
            return Err(err("expanded engine contains a special file"));
        }
    }
    Ok(())
}

fn tree_identity(root: &Path) -> Result<BTreeMap<String, crate::manifest::FileIdentity>> {
    let mut observed = BTreeMap::new();
    let mut flat = BTreeMap::new();
    walk_payload(root, root, &mut flat)?;
    for (path, (size, sha256, executable)) in flat {
        observed.insert(
            path,
            crate::manifest::FileIdentity {
                sha256,
                size,
                executable,
            },
        );
    }
    Ok(observed)
}
fn extract_engine(archive: &Path, dest: &Path) -> Result<()> {
    extract_engine_with(archive, dest, require_not_cancelled)
}
fn extract_engine_with(
    archive: &Path,
    dest: &Path,
    mut check: impl FnMut() -> Result<()>,
) -> Result<()> {
    let file = File::open(archive)?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut ar = tar::Archive::new(gz);
    let entries = ar.entries()?;
    let mut seen = std::collections::BTreeSet::new();
    let mut count = 0;
    let mut total = 0u64;
    for e in entries {
        check()?;
        let mut e = e?;
        let p = e.path()?.into_owned();
        validate_relative(&p)?;
        let name = p.to_string_lossy().replace('\\', "/");
        if !seen.insert(name.clone()) {
            return Err(err("duplicate engine archive path"));
        }
        count += 1;
        if count > MAX_ARCHIVE_ENTRIES {
            return Err(err("engine archive entry limit exceeded"));
        }
        let ty = e.header().entry_type();
        if ty.is_dir() {
            fs::create_dir_all(safe_join(dest, &name)?)?;
            continue;
        }
        if !ty.is_file() {
            return Err(err("engine archive contains a link or special file"));
        }
        let n = e.size();
        total = total
            .checked_add(n)
            .ok_or_else(|| err("archive size overflow"))?;
        if n > MAX_ARCHIVE_MEMBER || total > MAX_ARCHIVE_TOTAL {
            return Err(err("engine archive size limit exceeded"));
        }
        let out = safe_join(dest, &name)?;
        if let Some(pa) = out.parent() {
            fs::create_dir_all(pa)?;
        }
        let mode = e.header().mode()?;
        if mode & !0o755 != 0 || mode & 0o022 != 0 {
            return Err(err("unsafe engine member mode"));
        }
        let mut f = OpenOptions::new().write(true).create_new(true).open(out)?;
        let copied = copy_stream_with_cancel(&mut e, &mut f, Some(n), &mut check)?;
        if copied != n {
            return Err(err("engine member length differs from archive header"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                safe_join(dest, &name)?,
                fs::Permissions::from_mode(mode & 0o755 | (mode & 0o044)),
            )?;
        }
    }
    let root = dest.join("kpopper-0.15.1-darwin-arm64/bin/kpop");
    if !root.is_file() {
        return Err(err("engine archive root or direct bin/kpop is missing"));
    }
    Ok(())
}
fn locked_engine_files(path: &Path) -> Result<BTreeMap<String, crate::manifest::FileIdentity>> {
    let inventory: Value = serde_json::from_slice(&fs::read(path)?)?;
    if inventory["archive_sha256"].as_str() != Some(ENGINE_SHA256) {
        return Err(err("engine inventory archive identity mismatch"));
    }
    let mut files = BTreeMap::new();
    for member in inventory["members"]
        .as_array()
        .ok_or_else(|| err("missing engine members"))?
    {
        if member["type"].as_str() != Some("file") {
            continue;
        }
        let path = member["path"]
            .as_str()
            .ok_or_else(|| err("bad member path"))?;
        let relative = path
            .strip_prefix("kpopper-0.15.1-darwin-arm64/")
            .ok_or_else(|| err("wrong engine root"))?;
        validate_relative(Path::new(relative))?;
        let sha256 = member["sha256"]
            .as_str()
            .ok_or_else(|| err("missing engine member digest"))?
            .to_owned();
        validate_hex(&sha256)?;
        let mode = member["mode"]
            .as_str()
            .and_then(|s| s.strip_prefix("0o"))
            .and_then(|s| u32::from_str_radix(s, 8).ok())
            .ok_or_else(|| err("bad engine member mode"))?;
        let identity = crate::manifest::FileIdentity {
            sha256,
            size: member["size"]
                .as_u64()
                .ok_or_else(|| err("bad member size"))?,
            executable: mode & 0o111 != 0,
        };
        if files.insert(relative.to_owned(), identity).is_some() {
            return Err(err("duplicate engine member"));
        }
    }
    Ok(files)
}

fn validate_generation(ns: &Path, digest: &str, origin: &str, id: &str) -> Result<Receipt> {
    validate_hex(digest)?;
    let p = ns.join("generations").join(digest);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&p)?.permissions().mode() & 0o222 != 0 {
            return Err(err("installed generation is writable"));
        }
    }
    let raw = fs::read(p.join("receipt.json"))?;
    let r: Receipt = serde_json::from_slice(&raw)?;
    if r.format != "kpop-model-install/v1"
        || r.origin != origin
        || r.id != id
        || r.digest != digest
        || r.engine_sha256 != ENGINE_SHA256
        || r.engine_version != ENGINE_VERSION
    {
        return Err(err("installed generation receipt identity mismatch"));
    }
    let (descriptor, locked, actual_digest) = crate::package::verify_bundle(&p.join("payload"))?;
    if actual_digest != digest
        || descriptor.id != r.id
        || descriptor.version != r.version
        || locked.files != r.files
        || locked.source_commit != r.source_commit
        || locked.descriptor_sha256 != r.descriptor_sha256
    {
        return Err(err(
            "installed receipt does not match its original package lock",
        ));
    }
    let expected_engine = locked_engine_files(&p.join("payload/engine-members.json"))?;
    if expected_engine != r.engine_files {
        return Err(err(
            "installed engine receipt is not bound to the pinned archive members",
        ));
    }
    verify_payload(
        &p.join("engine/kpopper-0.15.1-darwin-arm64"),
        &r.engine_files,
    )?;
    let bin = p.join("engine/kpopper-0.15.1-darwin-arm64/bin/kpop");
    if !bin.is_file() {
        return Err(err("installed engine missing"));
    }
    Ok(r)
}
fn native_read(
    engine: &Path,
    work: &Path,
    model: &Path,
    state: &Path,
    cmd: &str,
    ids: Vec<String>,
) -> Result<Value> {
    if ids.is_empty() || ids.len() > MAX_READ_IDS {
        return Err(err("read operation requires 1 to 64 ids"));
    }
    let mut out = Vec::new();
    for id in ids {
        require_not_cancelled()?;
        if id.is_empty()
            || id.len() > 512
            || id.starts_with('-')
            || id.chars().any(char::is_control)
        {
            return Err(err("invalid record id"));
        }
        let args = if cmd == "pull" {
            vec![cmd.to_owned(), id, model.display().to_string()]
        } else {
            vec![
                cmd.to_owned(),
                "--input".into(),
                model.display().to_string(),
                id,
            ]
        };
        out.push(run_json(engine, &args, work, state)?);
    }
    Ok(json!({"operation":cmd,"results":out}))
}
fn native_search(engine: &Path, work: &Path, model: &Path, state: &Path, q: &str) -> Result<Value> {
    if q.trim().is_empty()
        || q.len() > MAX_QUERY_BYTES
        || q.starts_with('-')
        || q.chars().any(char::is_control)
    {
        return Err(err("invalid search query"));
    }
    run_json(
        engine,
        &[
            "search".into(),
            "--record".into(),
            model.display().to_string(),
            q.into(),
        ],
        work,
        state,
    )
}
fn sanitize_native_environment(command: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if key
            .to_str()
            .is_some_and(|s| s.starts_with("KPOPPER_") && s != "KPOPPER_AGENT_SESSION")
        {
            command.env_remove(key);
        }
    }
    for key in [
        "KPOP_BIN",
        "LEAN_PATH",
        "LEAN_SRC_PATH",
        "DYLD_LIBRARY_PATH",
        "DYLD_FALLBACK_LIBRARY_PATH",
        "DYLD_INSERT_LIBRARIES",
        "LD_LIBRARY_PATH",
        "LD_PRELOAD",
    ] {
        command.env_remove(key);
    }
}

fn run_json(engine: &Path, args: &[String], work: &Path, state: &Path) -> Result<Value> {
    let mut c = Command::new(engine);
    sanitize_native_environment(&mut c);
    c.args([
        "--workspace",
        work.to_str()
            .ok_or_else(|| err("workspace path is not UTF-8"))?,
        "--no-cache",
        "--frozen",
        "--json",
    ])
    .args(args)
    .current_dir(work)
    .env("HOME", work)
    .env("XDG_CONFIG_HOME", state)
    .env("XDG_CACHE_HOME", state)
    .env("KPOPPER_NATIVE_CACHE", state)
    .env("KPOPPER_STATE_DIR", state)
    .env_remove("KPOPPER_HOME")
    .env_remove("KPOPPER_CONFIG");
    let o = bounded_output(c)?;
    if o.stdout.len() > MAX_OUTPUT {
        return Err(err("native output exceeded limit"));
    }
    decode_native_read(&args[0], o.status.success(), &o.stdout, &o.stderr)
}
fn decode_native_read(
    command: &str,
    process_ok: bool,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<Value> {
    if command == "pull" {
        return unwrap_native_output(process_ok, stdout, stderr);
    }
    if !process_ok {
        return Err(err(&format!(
            "native {command} failed: {}",
            String::from_utf8_lossy(stderr)
        )));
    }
    // These public readers emit direct revision-bound JSON, whereas pull emits
    // the command receipt containing JSON text. Keep that protocol distinction.
    let evidence: Value = serde_json::from_slice(stdout)?;
    if evidence.get("error").is_some() {
        return Err(err(&format!(
            "native {command} rejected the read: {}",
            evidence["error"]
        )));
    }
    let valid = match command {
        "context" => evidence["reads"].is_array() && evidence["revision"].is_string(),
        "search" => {
            evidence["results"].is_array()
                && evidence["corpus_revision"].is_string()
                && evidence["record_sha256"].is_string()
        }
        _ => false,
    };
    if !valid {
        return Err(err(
            "native reader did not return its declared evidence shape",
        ));
    }
    Ok(
        json!({"process_status":{"command":command,"exit_code":0,"output_format":"direct-json"}, "evidence":evidence}),
    )
}
fn unwrap_native_output(process_ok: bool, stdout: &[u8], stderr: &[u8]) -> Result<Value> {
    let parsed = serde_json::from_slice::<Value>(stdout);
    let receipt = parsed.as_ref().ok();
    if !process_ok {
        return Err(err(&format!(
            "native reader process failed: {}",
            native_failure_detail(receipt, stderr)
        )));
    }
    let receipt = parsed?;
    if receipt.get("exit_code").and_then(Value::as_i64) != Some(0)
        || !receipt
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("")
            .is_empty()
    {
        return Err(err(&format!(
            "native reader rejected the request: {}",
            native_failure_detail(Some(&receipt), stderr)
        )));
    }
    let output = receipt
        .get("output")
        .and_then(Value::as_str)
        .ok_or_else(|| err("native receipt has no textual output"))?;
    let evidence: Value = serde_json::from_str(output)
        .map_err(|_| err("native receipt output is not JSON evidence"))?;
    let mut status = receipt.clone();
    status
        .as_object_mut()
        .ok_or_else(|| err("native receipt is not an object"))?
        .remove("output");
    status["output_sha256"] = json!(hex(&Sha256::digest(output.as_bytes())));
    Ok(json!({"native_status":status,"evidence":evidence}))
}
fn native_failure_detail(receipt: Option<&Value>, stderr: &[u8]) -> String {
    let from_receipt = receipt
        .and_then(|r| r.get("error"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            receipt
                .and_then(|r| r.get("output"))
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
        });
    let detail = match from_receipt {
        Some(s) => s.to_owned(),
        None => {
            let s = String::from_utf8_lossy(stderr);
            if s.trim().is_empty() {
                "native read failed".to_owned()
            } else {
                s.into_owned()
            }
        }
    };
    let detail = detail.trim();
    let mut end = detail.len().min(4096);
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    detail[..end].to_owned()
}

fn run_adapter(
    gen: &Path,
    d: &Descriptor,
    engine: &Path,
    model: &Path,
    case: &Path,
    as_of: &str,
    overlay: Option<&Path>,
) -> Result<Value> {
    if d.capability != crate::manifest::Capability::Application {
        return Err(err("run requires application capability"));
    }
    if !valid_date(as_of) {
        return Err(err(
            "as_of must be a valid calendar date in YYYY-MM-DD form",
        ));
    }
    let adapter = gen.join("payload/bin/model-adapter");
    if !adapter.is_file() {
        return Err(err("application adapter missing"));
    }
    let case = case.canonicalize()?;
    if !case.is_file() || fs::metadata(&case)?.len() > MAX_CASE_BYTES {
        return Err(err("case must be a regular file no larger than 16 MiB"));
    }
    let mut args = vec![
        "--model".into(),
        model.join(&d.model).display().to_string(),
        "--kpop".into(),
        engine.display().to_string(),
        "--case".into(),
        case.display().to_string(),
        "--as-of".into(),
        as_of.into(),
    ];
    if let Some(p) = overlay {
        let p = p.canonicalize()?;
        if !p.is_file() || fs::metadata(&p)?.len() > MAX_CASE_BYTES {
            return Err(err(
                "policy overlay must be a regular file no larger than 16 MiB",
            ));
        }
        args.extend(["--policy-overlay".into(), p.display().to_string()]);
    }
    args.push("--json".into());
    let work = tempfile::tempdir()?;
    let (packaged, lock, package_digest) = crate::package::verify_bundle(&gen.join("payload"))?;
    if packaged.id != d.id || packaged.model != d.model || packaged.version != d.version {
        return Err(err(
            "selected adapter context differs from its verified package",
        ));
    }
    let prefix = Path::new(&d.model)
        .parent()
        .ok_or_else(|| err("model has no root"))?;
    let mut model_files = serde_json::Map::new();
    for (path, identity) in &lock.files {
        if let Ok(relative) = Path::new(path).strip_prefix(prefix) {
            model_files.insert(
                relative.to_string_lossy().replace('\\', "/"),
                json!({"sha256":identity.sha256,"size":identity.size}),
            );
        }
    }
    let context = json!({"format":"kpop-model-context/v1", "package_digest":package_digest,
        "source_commit":lock.source_commit,"model_files":model_files,
        "runtime_sha256":digest_path(engine)?});
    let context_path = work.path().join("package-context.json");
    fs::write(&context_path, serde_json::to_vec(&context)?)?;
    args.extend([
        "--package-receipt".into(),
        context_path.display().to_string(),
    ]);
    let mut command = Command::new(&adapter);
    sanitize_native_environment(&mut command);
    command
        .args(&args)
        .current_dir(work.path())
        .env("HOME", work.path())
        .env("XDG_CONFIG_HOME", work.path())
        .env("XDG_CACHE_HOME", work.path())
        .env_remove("KPOPPER_HOME")
        .env_remove("KPOPPER_CONFIG");
    let o = bounded_output(command)?;
    let v: Value = serde_json::from_slice(&o.stdout)?;
    if !o.status.success() {
        return Err(err(&format!(
            "adapter failed: {}",
            String::from_utf8_lossy(&o.stderr)
        )));
    }
    Ok(v)
}
fn bounded_command(exe: &Path, args: &[String], cwd: &Path) -> Result<std::process::Output> {
    let mut c = Command::new(exe);
    c.args(args).current_dir(cwd);
    bounded_output(c)
}
fn bounded_output(c: Command) -> Result<std::process::Output> {
    bounded_output_with_input(c, None)
}
fn bounded_output_with_input(
    mut c: Command,
    input: Option<Vec<u8>>,
) -> Result<std::process::Output> {
    // Files keep a detached stdout-holding descendant from blocking a reader
    // thread after its parent exits. All reads and file growth remain bounded.
    let stdout = tempfile::NamedTempFile::new()?;
    let stderr = tempfile::NamedTempFile::new()?;
    let mut stdin = tempfile::tempfile()?;
    if let Some(bytes) = input {
        stdin.write_all(&bytes)?;
        stdin.seek(SeekFrom::Start(0))?;
        c.stdin(Stdio::from(stdin));
    } else {
        c.stdin(Stdio::null());
    }
    c.stdout(Stdio::from(stdout.reopen()?))
        .stderr(Stdio::from(stderr.reopen()?));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    let mut child = c.spawn()?;
    let start = Instant::now();
    let mut failure = None;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= PROCESS_TIMEOUT {
            failure = Some("subprocess timed out");
        }
        if cancellation_exit_code().is_some() {
            failure = Some("subprocess cancelled");
        }
        if stdout.as_file().metadata()?.len() > MAX_OUTPUT as u64
            || stderr.as_file().metadata()?.len() > MAX_OUTPUT as u64
        {
            failure = Some("subprocess output exceeded limit");
        }
        if failure.is_some() {
            #[cfg(unix)]
            {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(child.id() as i32),
                    nix::sys::signal::Signal::SIGTERM,
                );
            }
            let grace = Instant::now();
            let mut exited = None;
            while grace.elapsed() < Duration::from_secs(2) {
                if let Some(status) = child.try_wait()? {
                    exited = Some(status);
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if let Some(status) = exited {
                break status;
            }
            #[cfg(unix)]
            {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(child.id() as i32),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
            let _ = child.kill();
            break child.wait()?;
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    // A normally exited parent must not leave helpers sharing its process group.
    #[cfg(unix)]
    {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(child.id() as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    if let Some(message) = failure {
        return Err(err(message));
    }
    fn read_output(file: &tempfile::NamedTempFile) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        file.reopen()?
            .take((MAX_OUTPUT + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_OUTPUT {
            return Err(err("subprocess output exceeded limit"));
        }
        Ok(bytes)
    }
    Ok(std::process::Output {
        status,
        stdout: read_output(&stdout)?,
        stderr: read_output(&stderr)?,
    })
}

fn default_cache() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| err("HOME is unset; pass --cache"))?;
    Ok(PathBuf::from(home).join("Library/Caches/kpopper-models"))
}
fn ensure_private_dir(p: &Path) -> Result<()> {
    if !p.is_absolute() {
        return Err(err("cache path must be absolute"));
    }
    let mut cur = PathBuf::new();
    for component in p.components() {
        cur.push(component.as_os_str());
        if let Ok(meta) = fs::symlink_metadata(&cur) {
            if meta.file_type().is_symlink() {
                return Err(err("cache path ancestors may not be symlinks"));
            }
        }
    }
    fs::create_dir_all(p)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(p, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn lock_namespace(ns: &Path) -> Result<File> {
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(ns.join(".lock"))?;
    f.lock_exclusive()?;
    Ok(f)
}
fn active_digest(ns: &Path) -> Result<Option<String>> {
    let p = ns.join("active.json");
    if !p.exists() {
        return Ok(None);
    }
    let v: Value = serde_json::from_slice(&fs::read(p)?)?;
    let d = v
        .get("digest")
        .and_then(Value::as_str)
        .ok_or_else(|| err("invalid active pointer"))?;
    validate_hex(d)?;
    Ok(Some(d.into()))
}
fn valid_rollback(ns: &Path, invoking: &str, active: &str) -> Result<bool> {
    let p = ns.join(format!("rollback-{invoking}.json"));
    if !p.is_file() {
        return Ok(false);
    }
    let v: Value = serde_json::from_slice(&fs::read(p)?)?;
    Ok(
        v.get("format").and_then(Value::as_str) == Some("kpop-model-rollback/v1")
            && v.get("invoking_digest").and_then(Value::as_str) == Some(invoking)
            && v.get("target_digest").and_then(Value::as_str) == Some(active),
    )
}

fn activate_pointer(ns: &Path, d: &str) -> Result<()> {
    validate_hex(d)?;
    atomic_json(
        &ns.join("active.json"),
        &json!({"format":"kpop-model-active/v1","digest":d}),
    )
}
fn atomic_json<T: Serialize>(p: &Path, v: &T) -> Result<()> {
    let parent = p.parent().ok_or_else(|| err("missing parent"))?;
    let mut t = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut t, v)?;
    t.write_all(b"\n")?;
    t.as_file().sync_all()?;
    t.persist(p)?;
    Ok(())
}
fn selected_model_files(
    files: &BTreeMap<String, crate::manifest::FileIdentity>,
    entry: &str,
) -> Result<BTreeMap<String, crate::manifest::FileIdentity>> {
    let root = Path::new(entry)
        .parent()
        .ok_or_else(|| err("model entry has no model directory"))?;
    let selected: BTreeMap<_, _> = files
        .iter()
        .filter(|(path, _)| Path::new(path).starts_with(root))
        .map(|(p, i)| (p.clone(), i.clone()))
        .collect();
    if !selected.contains_key(entry) {
        return Err(err("verified package is missing its model entry"));
    }
    Ok(selected)
}

fn copy_tree_limited(
    src: &Path,
    dst: &Path,
    files: &BTreeMap<String, crate::manifest::FileIdentity>,
) -> Result<()> {
    fs::create_dir_all(dst)?;
    for k in files.keys() {
        let a = safe_join(src, k)?;
        let b = safe_join(dst, k)?;
        if let Some(p) = b.parent() {
            fs::create_dir_all(p)?;
        }
        copy_checked(&a, &b)?;
    }
    Ok(())
}
fn safe_join(base: &Path, rel: impl AsRef<Path>) -> Result<PathBuf> {
    let rel = rel.as_ref();
    validate_relative(rel)?;
    Ok(base.join(rel))
}
fn validate_relative(p: &Path) -> Result<()> {
    if p.as_os_str().is_empty() || p.is_absolute() {
        return Err(err("unsafe relative path"));
    }
    for c in p.components() {
        if !matches!(c, Component::Normal(_)) {
            return Err(err("unsafe relative path component"));
        }
    }
    let raw = p.to_string_lossy();
    let drive = raw.as_bytes().get(1) == Some(&b':')
        && raw.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
    if raw.contains('\\') || drive || raw.chars().any(char::is_control) {
        return Err(err("unsafe relative path"));
    }
    Ok(())
}
fn digest_path(p: &Path) -> Result<String> {
    digest_reader(File::open(p)?, require_not_cancelled)
}
fn digest_reader(mut reader: impl Read, mut check: impl FnMut() -> Result<()>) -> Result<String> {
    let mut h = Sha256::new();
    let mut b = [0u8; 65536];
    loop {
        check()?;
        let n = reader.read(&mut b)?;
        if n == 0 {
            break;
        }
        h.update(&b[..n]);
    }
    Ok(hex(&h.finalize()))
}
fn copy_checked(src: &Path, dst: &Path) -> Result<u64> {
    require_not_cancelled()?;
    let meta = fs::symlink_metadata(src)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(err("copy source is not a regular file"));
    }
    let mut input = File::open(src)?;
    let mut output = OpenOptions::new().write(true).create_new(true).open(dst)?;
    let copied = copy_stream_with_cancel(
        &mut input,
        &mut output,
        Some(meta.len()),
        require_not_cancelled,
    )?;
    output.sync_all()?;
    if copied != meta.len() {
        return Err(err("copied file length differs from source"));
    }
    fs::set_permissions(dst, meta.permissions())?;
    Ok(copied)
}
fn copy_stream_with_cancel(
    reader: &mut impl Read,
    writer: &mut impl Write,
    expected: Option<u64>,
    mut check: impl FnMut() -> Result<()>,
) -> Result<u64> {
    let mut total = 0u64;
    let mut buf = [0u8; 65536];
    loop {
        check()?;
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total = total
            .checked_add(n as u64)
            .ok_or_else(|| err("copy size overflow"))?;
        if expected.is_some_and(|limit| total > limit) {
            return Err(err("copy exceeded declared size"));
        }
        writer.write_all(&buf[..n])?;
    }
    check()?;
    Ok(total)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn validate_hex(s: &str) -> Result<()> {
    if s.len() != 64
        || !s
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(err("digest must be 64 lowercase hex characters"));
    }
    Ok(())
}
fn valid_date(s: &str) -> bool {
    if s.len() != 10
        || s.as_bytes()[4] != b'-'
        || s.as_bytes()[7] != b'-'
        || !s
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
    {
        return false;
    }
    let year = s[0..4].parse::<u32>().ok();
    let month = s[5..7].parse::<u32>().ok();
    let day = s[8..10].parse::<u32>().ok();
    let (Some(y), Some(m), Some(d)) = (year, month, day) else {
        return false;
    };
    if m == 0 || m > 12 {
        return false;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    };
    d > 0 && d <= days
}
fn selected_identity(d: &Descriptor, l: &PackageLock, digest: &str) -> Value {
    json!({"id":d.id,"version":d.version,"digest":digest,"model":d.model,"skill":d.skill,"source_commit":l.source_commit,"engine_version":l.engine.version,"engine_sha256":l.engine.sha256})
}
fn envelope(
    d: &Descriptor,
    l: &PackageLock,
    digest: &str,
    operation: &str,
    result: Value,
) -> Value {
    json!({"format":"kpop-model-result/v1","operation":operation,"package":{"id":d.id,"version":d.version,"digest":digest,"source_commit":l.source_commit,"engine_version":l.engine.version,"engine_sha256":l.engine.sha256},"result":result})
}
fn err(s: &str) -> Box<dyn std::error::Error + Send + Sync> {
    std::io::Error::other(s.to_owned()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_validation_gate_checks_a_real_package_with_the_pinned_engine() {
        use std::process::Command;
        let archive = PathBuf::from(
            std::env::var_os("KPOP_MODEL_ENGINE_ARCHIVE")
                .expect("set KPOP_MODEL_ENGINE_ARCHIVE to the pinned native release"),
        );
        let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/model-plugin");
        let t = tempfile::tempdir().unwrap();
        let source = t.path().join("source");
        fn copy_tree(from: &Path, to: &Path) {
            std::fs::create_dir_all(to).unwrap();
            for entry in std::fs::read_dir(from).unwrap() {
                let entry = entry.unwrap();
                let src = entry.path();
                let dst = to.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy_tree(&src, &dst)
                } else if entry.file_type().unwrap().is_file() {
                    std::fs::copy(src, dst).unwrap();
                }
            }
        }
        copy_tree(&example, &source);
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(&source)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success());
        };
        git(&["init", "-q"]);
        git(&["add", "."]);
        let status = Command::new("git")
            .arg("-C")
            .arg(&source)
            .args([
                "-c",
                "user.name=Runtime Fixture",
                "-c",
                "user.email=runtime@example.invalid",
                "commit",
                "-qm",
                "fixture",
            ])
            .status()
            .unwrap();
        assert!(status.success());
        let bundle = t.path().join("bundle");
        crate::package::build(
            &source,
            Path::new("model-package.json"),
            &archive,
            None,
            &bundle,
        )
        .unwrap();
        validate_bundle_runtime(&bundle).unwrap();
    }
    #[test]
    fn runtime_validation_gate_rejects_an_unverified_directory() {
        let temp = tempfile::tempdir().unwrap();
        assert!(validate_bundle_runtime(temp.path()).is_err());
    }
    #[test]
    fn rejects_unsafe_relative_paths_and_bad_digests() {
        for p in ["../escape", "/absolute", "a/../../b", "a\\b", "C:/drive"] {
            assert!(validate_relative(Path::new(p)).is_err());
        }
        assert!(validate_relative(Path::new("engine/bin/kpop")).is_ok());
        assert!(validate_hex(&"a".repeat(64)).is_ok());
        assert!(validate_hex(&"A".repeat(64)).is_err());
    }
    #[test]
    fn payload_integrity_detects_mutation_and_unexpected_files() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("payload");
        std::fs::create_dir(&root).unwrap();
        let f = root.join("a.txt");
        std::fs::write(&f, b"original").unwrap();
        let identity = crate::manifest::FileIdentity {
            sha256: digest_path(&f).unwrap(),
            size: 8,
            executable: false,
        };
        let files = BTreeMap::from([("a.txt".to_string(), identity)]);
        assert!(verify_payload(&root, &files).is_ok());
        std::fs::write(root.join("extra"), b"x").unwrap();
        assert!(verify_payload(&root, &files).is_err());
        std::fs::remove_file(root.join("extra")).unwrap();
        std::fs::write(&f, b"modified").unwrap();
        assert!(verify_payload(&root, &files).is_err());
    }
    fn test_package_lock() -> (PackageLock, Vec<u8>, String) {
        let lock = PackageLock {
            format: crate::manifest::LOCK_FORMAT.into(),
            descriptor_sha256: "a".repeat(64),
            source_commit: "deadbeef".into(),
            engine: crate::manifest::Engine {
                version: ENGINE_VERSION.into(),
                sha256: ENGINE_SHA256.into(),
                size: ENGINE_SIZE,
            },
            files: BTreeMap::new(),
        };
        let bytes = serde_json::to_vec(&lock).unwrap();
        let digest = hex(&Sha256::digest(&bytes));
        (lock, bytes, digest)
    }
    #[test]
    fn damaged_managed_generation_is_quarantined_without_losing_active_or_old_bytes() {
        let t = tempfile::tempdir().unwrap();
        let namespace = t.path();
        let base = namespace.join("generations");
        let (lock, bytes, digest) = test_package_lock();
        let generation = base.join(&digest);
        std::fs::create_dir_all(generation.join("payload/skills/rates")).unwrap();
        std::fs::write(generation.join("payload/model-package.lock.json"), bytes).unwrap();
        std::fs::write(
            generation.join("payload/skills/rates/SKILL.md"),
            b"corrupted",
        )
        .unwrap();
        activate_pointer(namespace, &digest).unwrap();
        assert!(managed_generation_lock(&generation, &digest));

        let quarantined = quarantine_generation(&base, &generation, &digest).unwrap();
        assert!(!generation.exists());
        assert_eq!(
            std::fs::read(quarantined.join("payload/skills/rates/SKILL.md")).unwrap(),
            b"corrupted"
        );
        assert_eq!(
            active_digest(namespace).unwrap().as_deref(),
            Some(digest.as_str())
        );

        std::fs::create_dir_all(generation.join("payload/skills/rates")).unwrap();
        std::fs::write(
            generation.join("payload/model-package.lock.json"),
            serde_json::to_vec(&lock).unwrap(),
        )
        .unwrap();
        std::fs::write(generation.join("payload/skills/rates/SKILL.md"), b"rebuilt").unwrap();
        assert!(managed_generation_lock(&generation, &digest));
        assert_eq!(
            std::fs::read(generation.join("payload/skills/rates/SKILL.md")).unwrap(),
            b"rebuilt"
        );
        assert_eq!(
            active_digest(namespace).unwrap().as_deref(),
            Some(digest.as_str())
        );
    }
    #[test]
    fn unmanaged_generation_is_refused_and_kept_in_place() {
        let t = tempfile::tempdir().unwrap();
        let base = t.path().join("generations");
        std::fs::create_dir(&base).unwrap();
        let digest = "b".repeat(64);
        let generation = base.join(&digest);
        std::fs::create_dir(&generation).unwrap();
        std::fs::write(generation.join("user-data"), b"keep").unwrap();
        let error = quarantine_generation(&base, &generation, &digest)
            .unwrap_err()
            .to_string();
        assert!(error.contains(&generation.display().to_string()));
        assert_eq!(
            std::fs::read(generation.join("user-data")).unwrap(),
            b"keep"
        );
    }
    #[cfg(unix)]
    #[test]
    fn managed_rename_temporarily_enables_only_root_and_restores_permissions_on_failure() {
        use std::os::unix::fs::PermissionsExt;

        let t = tempfile::tempdir().unwrap();
        let source = t.path().join("stage");
        let nested = source.join("payload");
        std::fs::create_dir_all(&nested).unwrap();
        let file = nested.join("read-only.txt");
        std::fs::write(&file, b"preserve me").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o500)).unwrap();
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o500)).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o400)).unwrap();
        let destination = t.path().join("generation");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(destination.join("existing"), b"keep").unwrap();
        let mut observed_writable_root = false;
        let mut reached_injected_collision = false;

        let error =
            rename_managed_directory_with(&source, &destination, "test rename", |from, to| {
                let mode = std::fs::symlink_metadata(from)?.permissions().mode();
                if mode & 0o200 == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "HFS-style rename rejection for read-only source root",
                    ));
                }
                observed_writable_root = true;
                if to.exists() {
                    reached_injected_collision = true;
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        "injected destination collision",
                    ));
                }
                std::fs::rename(from, to)
            })
            .unwrap_err()
            .to_string();

        assert!(observed_writable_root);
        assert!(reached_injected_collision);
        assert!(error.contains("test rename"));
        assert!(error.contains(&source.display().to_string()));
        assert!(error.contains(&destination.display().to_string()));
        assert_eq!(
            std::fs::symlink_metadata(&source)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o500
        );
        assert_eq!(
            std::fs::symlink_metadata(&nested)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o500
        );
        assert_eq!(
            std::fs::symlink_metadata(&file)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o400
        );
        assert_eq!(std::fs::read(&file).unwrap(), b"preserve me");
        assert_eq!(
            std::fs::read(destination.join("existing")).unwrap(),
            b"keep"
        );
    }

    #[cfg(unix)]
    #[test]
    fn managed_rename_succeeds_and_restores_read_only_tree_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let t = tempfile::tempdir().unwrap();
        let source = t.path().join("quarantine");
        let nested = source.join("payload");
        std::fs::create_dir_all(&nested).unwrap();
        let file = nested.join("read-only.txt");
        std::fs::write(&file, b"preserve me").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o500)).unwrap();
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o500)).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o400)).unwrap();
        let destination = t.path().join("renamed");

        rename_managed_directory(&source, &destination, "test rename").unwrap();

        assert!(!source.exists());
        assert_eq!(
            std::fs::symlink_metadata(&destination)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o500
        );
        assert_eq!(
            std::fs::symlink_metadata(destination.join("payload"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o500
        );
        assert_eq!(
            std::fs::symlink_metadata(destination.join("payload/read-only.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o400
        );
        assert_eq!(
            std::fs::read(destination.join("payload/read-only.txt")).unwrap(),
            b"preserve me"
        );
    }
    #[test]
    fn shared_archive_inventory_validates_after_extraction() {
        let archive = PathBuf::from(
            std::env::var_os("KPOP_MODEL_ENGINE_ARCHIVE")
                .expect("set KPOP_MODEL_ENGINE_ARCHIVE to the pinned native release"),
        );
        assert_eq!(digest_path(&archive).unwrap(), ENGINE_SHA256);
        let inventory = crate::package::engine_inventory(&archive).unwrap();
        let temp = tempfile::tempdir().unwrap();
        extract_engine(&archive, temp.path()).unwrap();
        validate_expanded_inventory(&inventory, temp.path()).unwrap();
        std::fs::write(temp.path().join("extra"), b"unlisted").unwrap();
        assert!(validate_expanded_inventory(&inventory, temp.path()).is_err());
    }

    #[test]
    fn expanded_engine_inventory_rejects_extra_members() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("engine");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin/kpop"), b"binary").unwrap();
        let inventory = serde_json::to_vec(&json!({"archive_sha256": ENGINE_SHA256,
            "members": [{"path":"bin","size":0,"mode":"0o755","type":"dir"},
                {"path":"bin/kpop","size":6,"mode":"0o755","type":"file"}]}))
        .unwrap();
        assert!(validate_expanded_inventory(&inventory, &root).is_ok());
        std::fs::write(root.join("extra"), b"x").unwrap();
        assert!(validate_expanded_inventory(&inventory, &root).is_err());
    }
    #[test]
    fn invalid_activation_cannot_replace_the_last_good_pointer() {
        let t = tempfile::tempdir().unwrap();
        let digest = "a".repeat(64);
        activate_pointer(t.path(), &digest).unwrap();
        assert!(activate_pointer(t.path(), "invalid").is_err());
        assert_eq!(
            active_digest(t.path()).unwrap().as_deref(),
            Some(digest.as_str())
        );
    }
    #[test]
    fn exited_parent_does_not_wait_for_stdout_holding_child() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 5 & printf done"]);
        let start = Instant::now();
        let result = bounded_output(command).unwrap();
        assert!(result.status.success());
        assert_eq!(result.stdout, b"done");
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn cancellation_interrupts_hash_copy_and_archive_extraction_loops() {
        let data = vec![7u8; 200_000];
        let mut calls = 0;
        let mut writer = Vec::new();
        let copy =
            copy_stream_with_cancel(&mut std::io::Cursor::new(&data), &mut writer, None, || {
                calls += 1;
                if calls > 1 {
                    Err(err("operation cancelled"))
                } else {
                    Ok(())
                }
            });
        assert!(copy.unwrap_err().to_string().contains("cancelled"));
        assert_eq!(writer.len(), 65_536);
        let mut calls = 0;
        let hash = digest_reader(std::io::Cursor::new(data.clone()), || {
            calls += 1;
            if calls > 1 {
                Err(err("operation cancelled"))
            } else {
                Ok(())
            }
        });
        assert!(hash.unwrap_err().to_string().contains("cancelled"));

        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("cancel.tar.gz");
        let file = File::create(&archive).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut tar = tar::Builder::new(encoder);
        let bytes = b"x";
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(
            &mut header,
            "kpopper-0.15.1-darwin-arm64/bin/kpop",
            &bytes[..],
        )
        .unwrap();
        let encoder = tar.into_inner().unwrap();
        encoder.finish().unwrap();
        let out = temp.path().join("expanded");
        std::fs::create_dir(&out).unwrap();
        let mut checks = 0;
        let result = extract_engine_with(&archive, &out, || {
            checks += 1;
            if checks == 1 {
                Ok(())
            } else {
                Err(err("operation cancelled"))
            }
        });
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        let partial = out.join("kpopper-0.15.1-darwin-arm64/bin/kpop");
        assert_eq!(std::fs::metadata(partial).unwrap().len(), 0);
    }
    #[test]
    fn direct_readers_preserve_revision_and_exact_numbers() {
        let context = br#"{"reads":[{"value":922337203685477580812345678901}],"revision":"abc"}"#;
        let result = decode_native_read("context", true, context, b"").unwrap();
        assert_eq!(
            result["evidence"]["reads"][0]["value"].to_string(),
            "922337203685477580812345678901"
        );
        assert!(decode_native_read("context", true, br#"{"exit_code":0}"#, b"").is_err());
        assert!(decode_native_read("search", true, br#"{"error":"refused"}"#, b"").is_err());
        assert!(decode_native_read(
            "search",
            true,
            br#"{"results":[],"corpus_revision":"a","record_sha256":"b"}"#,
            b""
        )
        .is_ok());
    }

    #[test]
    fn rollback_selection_reports_the_target_version_and_paths() {
        let selected_descriptor = Descriptor {
            format: crate::manifest::PACKAGE_FORMAT.into(),
            id: "rates".into(),
            version: "0.1.0".into(),
            target: crate::manifest::TARGET.into(),
            capability: crate::manifest::Capability::Read,
            model: "knowledge/old/GROUNDING.yaml".into(),
            skill: "skills/old/SKILL.md".into(),
            include: vec![],
            engine: crate::manifest::Engine {
                version: ENGINE_VERSION.into(),
                sha256: ENGINE_SHA256.into(),
                size: ENGINE_SIZE,
            },
            adapter: None,
            smoke: Smoke::Read {
                id: "pricing.hourly".into(),
            },
        };
        let lock = PackageLock {
            format: crate::manifest::LOCK_FORMAT.into(),
            descriptor_sha256: "a".repeat(64),
            source_commit: "commit".into(),
            engine: crate::manifest::Engine {
                version: ENGINE_VERSION.into(),
                sha256: ENGINE_SHA256.into(),
                size: ENGINE_SIZE,
            },
            files: BTreeMap::new(),
        };
        let selected = selected_identity(&selected_descriptor, &lock, &"b".repeat(64));
        assert_eq!(selected["version"], "0.1.0");
        assert_eq!(selected["model"], "knowledge/old/GROUNDING.yaml");
        assert_eq!(selected["skill"], "skills/old/SKILL.md");
    }
    #[test]
    fn reader_requires_process_and_receipt_success_and_preserves_evidence() {
        let ok = br#"{"command":"pull","error":"","exit_code":0,"output":"{\"nodes\":[1]}"}"#;
        let value = unwrap_native_output(true, ok, b"").unwrap();
        assert_eq!(value["evidence"]["nodes"][0], 1);
        assert_eq!(value["native_status"]["command"], "pull");
        let bad=br#"{"command":"pull","error":"nothing matching","exit_code":1,"output":"nothing matching"}"#;
        let error = unwrap_native_output(false, bad, b"")
            .unwrap_err()
            .to_string();
        assert!(error.contains("nothing matching"));
        let fallback = unwrap_native_output(false, b"not json", b"reader crashed")
            .unwrap_err()
            .to_string();
        assert!(fallback.contains("reader crashed"));
        let fallback = unwrap_native_output(true, bad, b"reader crashed")
            .unwrap_err()
            .to_string();
        assert!(fallback.contains("nothing matching"));
        let missing=br#"{"command":"pull","error":"","exit_code":1,"output":"nothing matching missing.id\n"}"#;
        let reason = unwrap_native_output(false, missing, b"")
            .unwrap_err()
            .to_string();
        assert!(reason.contains("nothing matching missing.id"));
        let priority =
            br#"{"command":"pull","error":"native reason","exit_code":1,"output":"native output"}"#;
        assert!(unwrap_native_output(false, priority, b"stderr")
            .unwrap_err()
            .to_string()
            .contains("native reason"));
        let priority = br#"{"command":"pull","error":"","exit_code":1,"output":"native output"}"#;
        assert!(unwrap_native_output(false, priority, b"stderr")
            .unwrap_err()
            .to_string()
            .contains("native output"));
    }
    #[cfg(unix)]
    #[test]
    fn source_path_resolution_accepts_a_symlinked_workspace_alias() {
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real");
        let generation = real.join("generation");
        std::fs::create_dir_all(generation.join("payload/model/sources")).unwrap();
        let file = generation.join("payload/model/sources/a.md");
        std::fs::write(&file, b"source").unwrap();
        let alias = t.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let canonical = alias
            .join("generation/payload/model/sources/a.md")
            .canonicalize()
            .unwrap();
        let relative = relative_packaged_source(&alias.join("generation"), &canonical).unwrap();
        assert_eq!(relative, "model/sources/a.md");
    }
    #[test]
    fn typed_source_traversal_accepts_native_source_subjects() {
        let model = json!([
            "map",
            [[
                "known",
                [
                    "map",
                    [[
                        "source.quote",
                        [
                            "map",
                            [
                                ["file", ["text", "sources/quote.md"]],
                                ["v", ["text", "A source quotation"]]
                            ]
                        ]
                    ]]
                ]
            ]]
        ]);
        let mut found = Vec::new();
        collect_source_file_refs(&model, false, &mut found).unwrap();
        assert_eq!(found, vec!["sources/quote.md"]);
    }

    #[test]
    fn typed_source_traversal_only_collects_file_refs_under_sources() {
        let tree = serde_json::json!([
            "map",
            [
                [
                    "sources",
                    [
                        "map",
                        [["doc", ["map", [["file", ["text", "docs/source.md"]]]]]]
                    ]
                ],
                ["example", ["map", [["file", ["text", "not-a-source-ref"]]]]]
            ]
        ]);
        let mut refs = Vec::new();
        collect_source_file_refs(&tree, false, &mut refs).unwrap();
        assert_eq!(refs, vec!["docs/source.md"]);
    }
    #[test]
    fn strict_date_and_option_syntax_guard() {
        assert!(valid_date("2026-10-06"));
        assert!(!valid_date("2026-02-30"));
        assert!(valid_date("2024-02-29"));
        assert!(!valid_date("2026-1-06"));
        assert!(!valid_date("2026-10-06x"));
        assert!(native_search(
            Path::new("/missing"),
            Path::new("/tmp"),
            Path::new("/tmp/model"),
            Path::new("/tmp/state"),
            "--help"
        )
        .is_err());
    }
}

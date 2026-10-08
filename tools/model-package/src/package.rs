use crate::manifest::{
    Adapter, Descriptor, Engine, FileIdentity, PackageLock, Smoke, ENGINE_SHA256, ENGINE_SIZE,
    ENGINE_VERSION, LOCK_FORMAT, PACKAGE_FORMAT, TARGET,
};
use crate::Result;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, fmt};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Cursor, Read},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    process::Command,
};

const INVENTORY: &str = "engine-members.json";
const PLUGIN: &str = ".codex-plugin/plugin.json";

enum StrictJson {
    Unit,
    Seq,
    Map,
}
impl<'de> serde::Deserialize<'de> for StrictJson {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = StrictJson;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON without duplicate object keys")
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson::Unit)
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson::Unit)
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson::Unit)
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson::Unit)
            }
            fn visit_str<E: de::Error>(self, _: &str) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson::Unit)
            }
            fn visit_string<E: de::Error>(self, _: String) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson::Unit)
            }
            fn visit_none<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson::Unit)
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson::Unit)
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                while a.next_element::<StrictJson>()?.is_some() {}
                Ok(StrictJson::Seq)
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut keys = BTreeSet::new();
                while let Some(k) = a.next_key::<String>()? {
                    if !keys.insert(k.clone()) {
                        return Err(de::Error::custom(format!("duplicate JSON key: {k}")));
                    }
                    a.next_value::<StrictJson>()?;
                }
                Ok(StrictJson::Map)
            }
        }
        d.deserialize_any(V)
    }
}
fn reject_duplicate_keys(bytes: &[u8]) -> Result<()> {
    let _: StrictJson = serde_json::from_slice(bytes)?;
    Ok(())
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn read(path: &Path) -> Result<Vec<u8>> {
    check_cancelled()?;
    let mut file = File::open(path)?;
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 65536];
    loop {
        check_cancelled()?;
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..n]);
    }
    Ok(bytes)
}
fn check_cancelled() -> Result<()> {
    if crate::runtime::cancellation_exit_code().is_some() {
        Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "operation cancelled").into())
    } else {
        Ok(())
    }
}
fn hash_reader_with_cancel(
    mut reader: impl Read,
    check: &mut impl FnMut() -> Result<()>,
) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        check()?;
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
fn hash_file_with_cancel(path: &Path, check: &mut impl FnMut() -> Result<()>) -> Result<String> {
    hash_reader_with_cancel(File::open(path)?, check)
}
fn hash_bytes_with_cancel(bytes: &[u8], check: &mut impl FnMut() -> Result<()>) -> Result<String> {
    let mut hasher = Sha256::new();
    for chunk in bytes.chunks(65536) {
        check()?;
        hasher.update(chunk);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
fn rel(s: &str) -> Result<PathBuf> {
    let p = Path::new(s);
    if s.is_empty()
        || s.contains('\\')
        || s.chars().any(char::is_control)
        || p.is_absolute()
        || s.contains(':')
        || p.components().any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(format!("unsafe relative path: {s}").into());
    }
    Ok(p.to_path_buf())
}
fn safe_file(root: &Path, relpath: &str) -> Result<PathBuf> {
    let p = rel(relpath)?;
    let mut cur = root.to_path_buf();
    let parts: Vec<_> = p.components().collect();
    for (i, c) in parts.iter().enumerate() {
        check_cancelled()?;
        cur.push(c);
        let m = fs::symlink_metadata(&cur)?;
        let last = i + 1 == parts.len();
        if m.file_type().is_symlink()
            || (last && (!m.is_file() || m.nlink() != 1))
            || (!last && !m.is_dir())
        {
            return Err(format!("not a unique regular file: {}", cur.display()).into());
        }
    }
    Ok(cur)
}
fn identity(p: &Path) -> Result<FileIdentity> {
    identity_with_cancel(p, &mut check_cancelled)
}
fn identity_with_cancel(p: &Path, check: &mut impl FnMut() -> Result<()>) -> Result<FileIdentity> {
    check()?;
    let m = fs::metadata(p)?;
    Ok(FileIdentity {
        sha256: hash_file_with_cancel(p, check)?,
        size: m.len(),
        executable: m.mode() & 0o111 != 0,
    })
}
fn git(source: &Path, args: &[&str]) -> Result<String> {
    let o = Command::new("git")
        .arg("-C")
        .arg(source)
        .args(args)
        .output()?;
    if !o.status.success() {
        return Err(format!("git {:?} failed", args).into());
    }
    Ok(String::from_utf8(o.stdout)?.trim().to_owned())
}
fn committed(source: &Path, relpath: &str) -> Result<Vec<u8>> {
    let p = rel(relpath)?;
    let s = p.to_string_lossy();
    let tracked = Command::new("git")
        .arg("-C")
        .arg(source)
        .args(["ls-files", "--error-unmatch", s.as_ref()])
        .output()?;
    if !tracked.status.success() {
        return Err(format!("source file is not committed: {relpath}").into());
    }
    let dirty = Command::new("git")
        .arg("-C")
        .arg(source)
        .args(["diff", "--quiet", "HEAD", "--", s.as_ref()])
        .status()?;
    if !dirty.success() {
        return Err(format!("source file differs from HEAD: {relpath}").into());
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(source)
        .args(["show", &format!("HEAD:{relpath}")])
        .output()?;
    if !out.status.success() {
        return Err(format!("cannot read committed source blob: {relpath}").into());
    }
    let working = fs::read(source.join(&p))?;
    if working != out.stdout {
        return Err(format!("working source differs from committed HEAD bytes: {relpath}").into());
    }
    Ok(out.stdout)
}
fn walk(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = vec![];
    for e in fs::read_dir(root)? {
        check_cancelled()?;
        let e = e?;
        let p = e.path();
        let m = fs::symlink_metadata(&p)?;
        if m.file_type().is_symlink() || (m.is_file() && m.nlink() != 1) {
            return Err(format!("link or hardlink in model closure: {}", p.display()).into());
        }
        if m.is_dir() {
            out.extend(walk(&p)?)
        } else if m.is_file() {
            out.push(p)
        } else {
            return Err("special model member".into());
        }
    }
    out.sort();
    Ok(out)
}

/// Create an absent bundle directory and return its canonical lock digest.
pub fn build(
    source: &Path,
    descriptor: &Path,
    engine_archive: &Path,
    adapter: Option<&Path>,
    output: &Path,
) -> Result<String> {
    if output.exists() {
        return Err("output must be absent".into());
    }
    let descriptor = if descriptor.is_absolute() {
        descriptor.to_path_buf()
    } else {
        source.join(descriptor)
    };
    let desc_bytes = read(&descriptor)?;
    reject_duplicate_keys(&desc_bytes)?;
    let descriptor_rel = descriptor
        .strip_prefix(source)
        .map_err(|_| "descriptor must be a committed file inside the source repository")?
        .to_string_lossy()
        .into_owned();
    if committed(source, &descriptor_rel)? != desc_bytes {
        return Err("descriptor differs from committed source bytes".into());
    }
    let d: Descriptor = serde_json::from_slice(&desc_bytes)?;
    validate_descriptor(&d, adapter.is_some())?;
    let head = git(source, &["rev-parse", "HEAD"])?;
    let model_rel = rel(&d.model)?;
    let model_dir = model_rel.parent().ok_or("model must be nested")?;
    let mut payload: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let root_model = source.join(model_dir);
    if !root_model.is_dir() {
        return Err("model directory missing".into());
    }
    let closure = walk(&root_model)?;
    if closure.is_empty() {
        return Err("empty model closure".into());
    }
    let tracked: BTreeSet<String> = git(source, &["ls-files", "--", &model_dir.to_string_lossy()])?
        .lines()
        .map(str::to_owned)
        .collect();
    let found: BTreeSet<String> = closure
        .iter()
        .map(|p| {
            p.strip_prefix(source)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let all_model_paths: BTreeSet<String> = tracked.union(&found).cloned().collect();
    let transient: Vec<_> = all_model_paths
        .iter()
        .filter(|n| {
            let local = Path::new(n)
                .strip_prefix(model_dir)
                .unwrap_or(Path::new(n))
                .to_string_lossy();
            local == ".kpopper/project.lock"
                || local == ".kpopper/.history-local"
                || local.starts_with(".kpopper/.history-local/")
                || local.starts_with(".kpopper/migrations/")
        })
        .cloned()
        .collect();
    if !transient.is_empty() {
        return Err(format!(
            "transient or migration debris in model closure: {}",
            transient.join(", ")
        )
        .into());
    }
    let untracked: Vec<_> = found.difference(&tracked).cloned().collect();
    let missing: Vec<_> = tracked.difference(&found).cloned().collect();
    if tracked != found {
        return Err(format!("model directory differs from committed regular-file closure; unexpected/untracked: [{}]; missing: [{}]", untracked.join(", "), missing.join(", ")).into());
    }
    for p in closure {
        let r = p.strip_prefix(source)?.to_string_lossy().into_owned();
        let b = committed(source, &r)?;
        if payload.insert(r.clone(), b).is_some() {
            return Err(format!("duplicate package path: {r}").into());
        }
    }
    for path in std::iter::once(d.skill.as_str()).chain(d.include.iter().map(String::as_str)) {
        let b = committed(source, path)?;
        if payload.insert(path.to_owned(), b).is_some() {
            return Err(format!("duplicate package path: {path}").into());
        }
    }
    let engine = read(engine_archive)?;
    if engine.len() as u64 != ENGINE_SIZE
        || hash_bytes_with_cancel(&engine, &mut check_cancelled)? != ENGINE_SHA256
    {
        return Err("engine archive identity mismatch".into());
    }
    validate_engine_tar(&engine)?;
    if payload
        .insert("assets/kpopper.tar.gz".into(), engine)
        .is_some()
    {
        return Err("selected source collides with engine asset".into());
    }
    let members = engine_inventory(engine_archive)?;
    if payload.insert(INVENTORY.into(), members).is_some() {
        return Err("selected source collides with engine inventory".into());
    }
    if let Some(a) = adapter {
        let b = read(a)?;
        if payload.insert("bin/model-adapter".into(), b).is_some() {
            return Err("selected source collides with adapter".into());
        }
    }
    let exe = std::env::current_exe()?;
    if payload
        .insert("bin/kpop-model".into(), read(&exe)?)
        .is_some()
    {
        return Err("selected source collides with launcher".into());
    }
    let plugin = serde_json::json!({"name":d.id,"version":d.version,"description":format!("{} native knowledge model",d.id),"skills":"./skills/"});
    if payload
        .insert(PLUGIN.into(), serde_json::to_vec_pretty(&plugin)?)
        .is_some()
    {
        return Err("selected source collides with plugin manifest".into());
    }
    if payload
        .insert("model-package.json".into(), desc_bytes.clone())
        .is_some()
    {
        return Err("selected source collides with package descriptor".into());
    }
    if git(source, &["rev-parse", "HEAD"])? != head {
        return Err("source HEAD moved during capture".into());
    }
    for (name, bytes) in &payload {
        if name == "model-package.json"
            || name.starts_with("bin/")
            || name.starts_with("assets/")
            || name == INVENTORY
            || name == PLUGIN
        {
            continue;
        }
        if committed(source, name)? != *bytes {
            return Err(format!("source changed during capture: {name}").into());
        }
    }
    let mut files = BTreeMap::new();
    for (name, b) in &payload {
        if !safe_package_path(name) {
            return Err(format!("unsafe package path {name}").into());
        }
        files.insert(
            name.clone(),
            FileIdentity {
                sha256: hash(b),
                size: b.len() as u64,
                executable: name == "bin/kpop-model" || name == "bin/model-adapter",
            },
        );
    }
    let lock = PackageLock {
        format: LOCK_FORMAT.into(),
        descriptor_sha256: hash(&desc_bytes),
        source_commit: head,
        engine: Engine {
            version: ENGINE_VERSION.into(),
            sha256: ENGINE_SHA256.into(),
            size: ENGINE_SIZE,
        },
        files,
    };
    let lock_bytes = canonical_json(&lock)?;
    let digest = hash(&lock_bytes);
    let stage = output.with_extension(format!("stage-{}", std::process::id()));
    if stage.exists() {
        return Err("staging path exists".into());
    }
    fs::create_dir_all(&stage)?;
    let result = (|| -> Result<()> {
        for (name, b) in &payload {
            let p = stage.join(name);
            fs::create_dir_all(p.parent().unwrap())?;
            fs::write(&p, b)?;
            if name == "bin/kpop-model" || name == "bin/model-adapter" {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&p, fs::Permissions::from_mode(0o755))?;
            }
        }
        fs::write(stage.join("model-package.lock.json"), lock_bytes)?;
        verify_bundle(&stage)?;
        crate::runtime::validate_bundle_runtime(&stage)?;
        fs::rename(&stage, output)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    result?;
    Ok(digest)
}

pub fn verify_bundle(bundle: &Path) -> Result<(Descriptor, PackageLock, String)> {
    check_cancelled()?;
    let db = read(&bundle.join("model-package.json"))?;
    reject_duplicate_keys(&db)?;
    let d: Descriptor = serde_json::from_slice(&db)?;
    validate_descriptor(&d, bundle.join("bin/model-adapter").exists())?;
    let lb = read(&bundle.join("model-package.lock.json"))?;
    reject_duplicate_keys(&lb)?;
    let l: PackageLock = serde_json::from_slice(&lb)?;
    let canonical_lock = canonical_json(&l)?;
    if lb != canonical_lock {
        return Err("package lock is not in canonical serialized form".into());
    }
    for name in l.files.keys() {
        rel(name)?;
    }
    let digest = hash(&canonical_lock);
    if l.format != LOCK_FORMAT
        || l.descriptor_sha256 != hash(&db)
        || l.engine.sha256 != ENGINE_SHA256
        || l.engine.size != ENGINE_SIZE
        || l.engine.version != ENGINE_VERSION
    {
        return Err("lock identity mismatch".into());
    }
    let mut actual = BTreeSet::new();
    collect_files(bundle, bundle, &mut actual)?;
    let mut expected: BTreeSet<String> = l.files.keys().cloned().collect();
    expected.insert("model-package.lock.json".into());
    if actual != expected {
        return Err("bundle file closure differs from lock".into());
    }
    for (name, want) in &l.files {
        check_cancelled()?;
        let p = safe_file(bundle, name)?;
        let got = identity(&p)?;
        if &got != want {
            return Err(format!("bundle member mismatch: {name}").into());
        }
        if name == "assets/kpopper.tar.gz"
            && (got.sha256 != ENGINE_SHA256 || got.size != ENGINE_SIZE)
        {
            return Err("engine archive bytes do not match pinned identity".into());
        }
    }
    safe_file(bundle, &d.model)?;
    safe_file(bundle, &d.skill)?;
    let eng = safe_file(bundle, "assets/kpopper.tar.gz")?;
    if fs::metadata(&eng)?.len() != ENGINE_SIZE {
        return Err("engine archive bytes do not match pinned identity".into());
    }
    validate_engine_tar_file(&eng)?;
    if read(&bundle.join(INVENTORY))? != engine_inventory(&eng)? {
        return Err("engine member inventory differs from archive".into());
    }
    let plugin: serde_json::Value = serde_json::from_slice(&read(&bundle.join(PLUGIN))?)?;
    let expected_plugin = serde_json::json!({"name":d.id,"version":d.version,"description":format!("{} native knowledge model",d.id),"skills":"./skills/"});
    if plugin != expected_plugin {
        return Err("plugin manifest does not match descriptor".into());
    }
    Ok((d, l, digest))
}

fn validate_descriptor(d: &Descriptor, has_adapter: bool) -> Result<()> {
    if d.format != PACKAGE_FORMAT
        || d.target != TARGET
        || d.engine.version != ENGINE_VERSION
        || d.engine.sha256 != ENGINE_SHA256
        || d.engine.size != ENGINE_SIZE
    {
        return Err("unsupported descriptor identity".into());
    }
    for s in [&d.model, &d.skill].into_iter().chain(d.include.iter()) {
        rel(s)?;
    }
    if !d.model.ends_with("/GROUNDING.yaml") {
        return Err("model entry must be GROUNDING.yaml in a model directory".into());
    }
    if d.id.is_empty()
        || d.id.len() > 64
        || d.version.contains("..")
        || !d
            .id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("invalid package id".into());
    }
    if d.version.is_empty()
        || d.version.len() > 64
        || !d
            .version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'+')
    {
        return Err("invalid version".into());
    }
    match (&d.capability, &d.adapter, &d.smoke, has_adapter) {
        (crate::manifest::Capability::Read, None, Smoke::Read { id }, false) => {
            if id.is_empty() {
                return Err("empty smoke id".into());
            }
        }
        (
            crate::manifest::Capability::Application,
            Some(Adapter { protocol }),
            Smoke::Application { case, as_of, .. },
            true,
        ) => {
            if protocol != "kpop-case/v1" {
                return Err("unsupported adapter protocol".into());
            }
            rel(case)?;
            if as_of.len() != 10 {
                return Err("invalid smoke date".into());
            }
        }
        _ => return Err("descriptor capability, adapter and smoke do not agree".into()),
    }
    let reserved = [
        "model-package.json",
        "model-package.lock.json",
        INVENTORY,
        PLUGIN,
        "bin/kpop-model",
        "bin/model-adapter",
        "assets/kpopper.tar.gz",
    ];
    for p in std::iter::once(d.model.as_str())
        .chain(std::iter::once(d.skill.as_str()))
        .chain(d.include.iter().map(String::as_str))
    {
        if reserved.contains(&p) {
            return Err("descriptor collides with generated path".into());
        }
    }
    Ok(())
}
fn safe_package_path(s: &str) -> bool {
    rel(s).is_ok()
}
fn canonical_json<T: serde::Serialize>(v: &T) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(v)?)
}
fn collect_files(root: &Path, dir: &Path, out: &mut BTreeSet<String>) -> Result<()> {
    for e in fs::read_dir(dir)? {
        check_cancelled()?;
        let p = e?.path();
        let m = fs::symlink_metadata(&p)?;
        if m.file_type().is_symlink() || (m.is_file() && m.nlink() != 1) {
            return Err("link in bundle".into());
        }
        if m.is_dir() {
            collect_files(root, &p, out)?
        } else if m.is_file() {
            out.insert(p.strip_prefix(root)?.to_string_lossy().into_owned());
        } else {
            return Err("special bundle member".into());
        }
    }
    Ok(())
}
pub(crate) fn engine_inventory(path: &Path) -> Result<Vec<u8>> {
    engine_inventory_with_cancel(path, &mut check_cancelled)
}
fn engine_inventory_with_cancel(
    path: &Path,
    check: &mut impl FnMut() -> Result<()>,
) -> Result<Vec<u8>> {
    let check = RefCell::new(check);
    engine_inventory_with_hooks(path, &mut || (check.borrow_mut())(), &mut || {
        (check.borrow_mut())()
    })
}
fn engine_inventory_with_hooks(
    path: &Path,
    entry_check: &mut impl FnMut() -> Result<()>,
    chunk_check: &mut impl FnMut() -> Result<()>,
) -> Result<Vec<u8>> {
    entry_check()?;
    let f = File::open(path)?;
    let dec = flate2::read::GzDecoder::new(f);
    let mut ar = tar::Archive::new(dec);
    let mut v = vec![];
    for e in ar.entries()? {
        entry_check()?;
        let mut e = e?;
        let p = e.path()?.to_string_lossy().into_owned();
        let ty = e.header().entry_type();
        if !ty.is_file() && !ty.is_dir() {
            return Err("engine links or special members rejected".into());
        }
        let size = e.size();
        let mode = format!("0o{:o}", e.header().mode()?);
        let digest = if ty.is_file() {
            use std::io::Read;
            let mut hasher = Sha256::new();
            let mut buffer = [0u8; 65536];
            loop {
                let n = e.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buffer[..n]);
                chunk_check()?;
            }
            Some(format!("{:x}", hasher.finalize()))
        } else {
            None
        };
        v.push(serde_json::json!({"path":p,"size":size,"mode":mode,
            "type":if ty.is_file(){"file"}else{"dir"}, "sha256":digest}));
    }
    v.sort_by_key(|x| x["path"].as_str().unwrap_or("").to_owned());
    let archive_sha256 = hash_file_with_cancel(path, chunk_check)?;
    Ok(serde_json::to_vec_pretty(
        &serde_json::json!({"archive_sha256":archive_sha256,"members":v}),
    )?)
}
fn validate_engine_tar(bytes: &[u8]) -> Result<()> {
    validate_engine_tar_reader(
        flate2::read::GzDecoder::new(Cursor::new(bytes)),
        &mut check_cancelled,
    )
}
fn validate_engine_tar_file(path: &Path) -> Result<()> {
    validate_engine_tar_reader(
        flate2::read::GzDecoder::new(File::open(path)?),
        &mut check_cancelled,
    )
}
fn validate_engine_tar_reader(
    reader: impl Read,
    check: &mut impl FnMut() -> Result<()>,
) -> Result<()> {
    let mut ar = tar::Archive::new(reader);
    let mut paths = BTreeSet::new();
    let mut total = 0u64;
    let mut count = 0usize;
    for e in ar.entries()? {
        check()?;
        let mut e = e?;
        let p = e.path()?.to_string_lossy().into_owned();
        if !safe_package_path(&p) || !paths.insert(p.clone()) {
            return Err("unsafe or duplicate engine member".into());
        }
        let ty = e.header().entry_type();
        if !ty.is_file() && !ty.is_dir() {
            return Err("engine link or special member".into());
        }
        let mode = e.header().mode()?;
        if mode & !0o755 != 0 || mode & 0o022 != 0 {
            return Err("unsafe engine member mode".into());
        }
        let n = e.size();
        count += 1;
        total += n;
        if count > 2000 || total > 256 * 1024 * 1024 || n > 128 * 1024 * 1024 {
            return Err("engine archive exceeds observed bounds".into());
        }
        if ty.is_file() {
            let mut consumed = 0u64;
            let mut buffer = [0u8; 65536];
            loop {
                check()?;
                let read = e.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                consumed += read as u64;
            }
            if consumed != n {
                return Err("engine member length differs from archive header".into());
            }
        }
    }
    if !paths.contains("kpopper-0.15.1-darwin-arm64/bin/kpop") {
        return Err("engine archive root or executable missing".into());
    }
    Ok(())
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;

    fn cancelled() -> Result<()> {
        Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "operation cancelled").into())
    }

    #[test]
    fn identity_hash_interrupts_after_multiple_file_chunks() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        temp.as_file().set_len(8 * 1024 * 1024).unwrap();
        let mut checks = 0usize;
        let result = identity_with_cancel(temp.path(), &mut || {
            checks += 1;
            if checks == 5 {
                cancelled()
            } else {
                Ok(())
            }
        });
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("operation cancelled"));
        assert_eq!(checks, 5);
    }

    #[test]
    fn engine_inventory_interrupts_during_member_hashing() {
        let archive = PathBuf::from(
            std::env::var_os("KPOP_MODEL_ENGINE_ARCHIVE")
                .expect("set KPOP_MODEL_ENGINE_ARCHIVE to the pinned native release"),
        );
        let mut entry_checks = 0usize;
        let mut chunk_checks = 0usize;
        let result = engine_inventory_with_hooks(
            &archive,
            &mut || {
                entry_checks += 1;
                Ok(())
            },
            &mut || {
                chunk_checks += 1;
                if chunk_checks == 8 {
                    cancelled()
                } else {
                    Ok(())
                }
            },
        );
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("operation cancelled"));
        assert!(entry_checks > 0);
        assert_eq!(chunk_checks, 8);
    }
}

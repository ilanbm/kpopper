//! Bounded local Git capture. References are resolved once; no fetch, index,
//! checkout, configuration, pending ledger or source-record writes are performed.
use crate::{
    Result, history_authority as A,
    history_authority::Files,
    history_branch::{self as B, MAX_BYTES, MAX_FILES, Observation},
    history_contract::*,
    history_view::map_mut,
    identity::sha256,
    reasoning_snapshot::MAX_REQUEST_BYTES,
    require,
    value::{Integer, TypedValue as V},
};
use std::{collections::BTreeSet, path::Path, process::Command, time::Duration};

fn s(v: &str) -> V {
    V::Text(v.into())
}
pub(crate) fn git(root: &Path, args: &[&str], maximum: usize) -> Result<Vec<u8>> {
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
    crate::reasoning_runtime::run_command_bounded(
        &mut cmd,
        Vec::new(),
        Duration::from_secs(10),
        maximum,
    )
    .map_err(|e| {
        error(match e.0.as_str() {
            "runtime_timeout" => "branch_git_timeout",
            "output_limit" => "branch_capture_limit",
            _ => "branch_git_unavailable",
        })
    })
}
fn string(raw: Vec<u8>) -> Result<String> {
    String::from_utf8(raw).map_err(|_| error("invalid_branch_source"))
}
fn hex(value: &str) -> bool {
    [40, 64].contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
struct Reader<'a> {
    root: &'a Path,
    oid: String,
    algorithm: String,
    inventory: Map,
    files: Files,
    total: usize,
}
impl Reader<'_> {
    fn listing(&self, path: &str) -> Result<Map> {
        let raw = git(
            self.root,
            &[
                "--literal-pathspecs",
                "ls-tree",
                "-r",
                "-z",
                &self.oid,
                "--",
                path,
            ],
            MAX_REQUEST_BYTES,
        )?;
        let mut found = Map::new();
        for row in raw.split(|b| *b == 0).filter(|row| !row.is_empty()) {
            let split = row
                .iter()
                .position(|b| *b == b'\t')
                .ok_or_else(|| error("invalid_branch_path"))?;
            let info =
                std::str::from_utf8(&row[..split]).map_err(|_| error("invalid_branch_path"))?;
            let name =
                std::str::from_utf8(&row[split + 1..]).map_err(|_| error("invalid_branch_path"))?;
            B::portable_path(name)?;
            let fields: Vec<_> = info.split(' ').collect();
            require(fields.len() == 3, "invalid_branch_path")?;
            require(
                ["100644", "100755"].contains(&fields[0]) && fields[1] == "blob",
                "unsupported_branch_file",
            )?;
            require(!found.contains_key(name), "ambiguous_branch_path")?;
            found.insert(
                name.into(),
                V::Map(Map::from([
                    ("mode".into(), s(fields[0])),
                    ("git_oid".into(), s(fields[2])),
                ])),
            );
            require(found.len() <= MAX_FILES, "branch_capture_limit")?;
        }
        Ok(found)
    }
    fn include(&mut self, items: Map) -> Result<()> {
        self.inventory.extend(items);
        require(self.inventory.len() <= MAX_FILES, "branch_capture_limit")
    }
    fn read(&mut self, path: &str) -> Result<()> {
        if self.files.contains_key(path) {
            return Ok(());
        }
        if !self.inventory.contains_key(path) {
            self.include(self.listing(path)?)?;
        }
        let item = self
            .inventory
            .get(path)
            .ok_or_else(|| error("branch_evidence_unavailable"))?;
        require(self.files.len() < MAX_FILES, "branch_capture_limit")?;
        let blob = text(&map(item)?["git_oid"])?;
        require(hex(blob), "invalid_branch_blob")?;
        let size: usize = string(git(self.root, &["cat-file", "-s", blob], 64)?)?
            .trim()
            .parse()
            .map_err(|_| error("invalid_branch_blob"))?;
        require(
            size <= MAX_REQUEST_BYTES && self.total.saturating_add(size) <= MAX_BYTES,
            "branch_capture_limit",
        )?;
        let raw = git(self.root, &["cat-file", "blob", blob], size)?;
        require(
            raw.len() == size && B::blob_identity(&raw, &self.algorithm)? == blob,
            "branch_blob_mismatch",
        )?;
        self.total += size;
        self.files.insert(path.into(), raw);
        Ok(())
    }
}

fn pinned(
    repo: &Path,
    reference: &str,
    entry: &str,
) -> Result<(std::path::PathBuf, String, String)> {
    require(
        !reference.is_empty() && !reference.contains('\0'),
        "invalid_branch_ref",
    )?;
    B::portable_path(entry)?;
    let root = repo.canonicalize()?;
    let oid = string(git(
        &root,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
        128,
    )?)?
    .trim()
    .to_owned();
    require(hex(&oid), "invalid_branch_ref")?;
    let algorithm = string(git(&root, &["rev-parse", "--show-object-format"], 128)?)?
        .trim()
        .to_owned();
    require(
        ["sha1", "sha256"].contains(&algorithm.as_str()),
        "unsupported_git_object_format",
    )?;
    Ok((root, oid, algorithm))
}

pub fn capture(repo: &Path, reference: &str, entry: &str, as_of: Option<V>) -> Result<Observation> {
    let (root, oid, algorithm) = pinned(repo, reference, entry)?;
    let mut r = Reader {
        root: &root,
        oid,
        algorithm,
        inventory: Map::new(),
        files: Files::new(),
        total: 0,
    };
    let mut entry = entry.to_owned();
    let mut selected = r.listing(&entry)?;
    if !selected.contains_key(&entry)
        && ["GROUNDING.yaml", "PROVENANCE.yaml"].contains(&B::parts(&entry)?.1)
    {
        for name in ["GROUNDING.yaml", "PROVENANCE.yaml"] {
            let alternative = B::joined(&entry, name)?;
            let candidate = r.listing(&alternative)?;
            if candidate.contains_key(&alternative) {
                entry = alternative;
                selected = candidate;
                break;
            }
        }
    }
    require(selected.contains_key(&entry), "branch_record_unavailable")?;
    let roles = B::roles(&entry)?;
    r.include(selected)?;
    for path in [
        &roles.authority,
        &roles.objects,
        &roles.commits,
        &roles.cancellations,
    ] {
        r.include(r.listing(path)?)?;
    }
    require(
        r.inventory.contains_key(&roles.authority),
        "branch_history_not_active",
    )?;
    for path in r.inventory.keys().cloned().collect::<Vec<_>>() {
        r.read(&path)?;
    }
    let commit_prefix = format!("{}/", roles.commits);
    let object_prefix = format!("{}/", roles.objects);
    let mut commits = Files::new();
    let mut storage = Files::new();
    for (path, raw) in &r.files {
        if let Some(name) = path.strip_prefix(&commit_prefix) {
            let operation = name
                .strip_suffix(".yaml")
                .ok_or_else(|| error("invalid_history_bundle_path"))?;
            commits.insert(operation.into(), raw.clone());
        }
        if let Some(name) = path.strip_prefix(&object_prefix) {
            storage.insert(name.into(), raw.clone());
        }
    }
    let (_, object_paths) = A::objects_from_storage(&commits, &storage)?;
    let held: std::collections::BTreeSet<_> = object_paths.values().collect();
    r.files.retain(|path, _| {
        path.strip_prefix(&object_prefix)
            .is_none_or(|p| held.contains(&p.to_owned()))
    });
    let (captured, _) = B::core(&r.files, &entry)?;
    for path in B::required(&captured, &entry, &Files::new(), false)?.keys() {
        r.read(path)?;
    }
    let directory = B::hypothesis_directory(&entry)?;
    for (path, item) in r.listing(&directory)? {
        if B::hypothesis_file(&path, &directory).is_some() {
            r.include(Map::from([(path.clone(), item)]))?;
            r.read(&path)?;
        }
    }
    for path in B::required(&captured, &entry, &r.files, false)?.keys() {
        r.read(path)?;
    }
    let source = V::Map(Map::from([
        ("commit".into(), s(&r.oid)),
        ("entry".into(), s(&entry)),
        ("object_format".into(), s(&r.algorithm)),
        ("association".into(), s("local_git_capture")),
    ]));
    let (snapshot, _) =
        B::captured_snapshot(&captured, &r.files, &source, &as_of.unwrap_or(V::Null))?;
    let mut inventory = Map::new();
    for (path, raw) in &r.files {
        let mut item = r.inventory[path].clone();
        map_mut(&mut item)?.insert("sha256".into(), s(&sha256(raw)));
        inventory.insert(path.clone(), item);
    }
    let coverage = B::audit_coverage(&captured, &entry)?;
    let scoped = !crate::history_view::list(&map(&coverage)?["prior_branch_capsules"])?.is_empty();
    let mut manifest = V::Map(Map::from([
        (
            "version".into(),
            V::Integer(Integer::new(if scoped { "2" } else { "1" })?),
        ),
        (
            "kind".into(),
            s(if scoped { B::SCOPED_KIND } else { B::KIND }),
        ),
        ("source".into(), source),
        ("files".into(), V::Map(inventory)),
        ("snapshot".into(), snapshot.to_data()),
    ]));
    if scoped {
        map_mut(&mut manifest)?.insert("audit_coverage".into(), coverage);
    }
    let envelope = V::Map(Map::from([
        ("revision".into(), s(&manifest.digest()?)),
        ("manifest".into(), manifest),
    ]));
    B::validate(&envelope, &r.files)?;
    Ok(Observation {
        envelope,
        files: r.files,
    })
}

/// Freeze a node-history record at one Git commit without consulting its working tree.
pub fn capture_node(
    repo: &Path,
    reference: &str,
    entry: &str,
) -> Result<crate::history_node_branch_source::Observation> {
    use crate::history_node_publication as P;
    let (root, oid, algorithm) = pinned(repo, reference, entry)?;
    require(
        B::parts(entry)?.1 == "GROUNDING.yaml",
        "node_history_entry_unsupported",
    )?;
    let mut r = Reader {
        root: &root,
        oid,
        algorithm,
        inventory: Map::new(),
        files: Files::new(),
        total: 0,
    };
    for path in [
        entry.to_owned(),
        B::joined(entry, ".kpopper/history.yaml")?,
        B::joined(entry, ".kpopper/history")?,
        B::joined(entry, ".kpopper/history-commits")?,
    ] {
        let listing = r
            .listing(&path)?
            .into_iter()
            .filter(|(p, _)| !p.rsplit('/').next().unwrap_or_default().starts_with('.'))
            .collect();
        r.include(listing)?;
    }
    // Physical layers are not yet represented by portable node bundles. Never omit them.
    require(
        r.listing(&B::joined(entry, ".kpopper/hypotheses")?)?
            .keys()
            .all(|p| !p.ends_with(".yaml") && !p.ends_with(".yml")),
        "node_history_physical_hypotheses_unsupported",
    )?;
    for path in r.inventory.keys().cloned().collect::<Vec<_>>() {
        r.read(&path)?;
    }
    let commit_prefix = format!("{}/", B::joined(entry, ".kpopper/history-commits")?);
    let mut needed = BTreeSet::new();
    for (path, raw) in &r.files {
        if path.starts_with(&commit_prefix) {
            let manifest: serde_json::Value = serde_json::from_slice(raw)?;
            let Some(evidence) = manifest.get("evidence") else {
                continue;
            };
            let evidence = evidence
                .as_object()
                .ok_or_else(|| error("invalid_branch_source"))?;
            for relative in evidence.keys() {
                P::evidence_path(relative)?;
                needed.insert(B::joined(entry, relative)?);
            }
        }
    }
    for path in needed {
        r.read(&path)?;
    }
    let prefix = B::parts(entry)?.0;
    let files = r
        .files
        .into_iter()
        .map(|(p, raw)| {
            let relative = if prefix.is_empty() {
                p
            } else {
                p.strip_prefix(&format!("{prefix}/")).unwrap().into()
            };
            (relative, raw)
        })
        .collect();
    let bundle = P::Bundle::from_files(files)?;
    crate::history_node_branch_source::Observation::from_git(bundle, &r.oid, entry, &r.algorithm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    fn fixture() -> Observation {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/history-branch.json")).unwrap();
        let c = cases
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c.get("output").is_some() && c["files"].as_object().unwrap().len() < 10)
            .unwrap();
        Observation {
            envelope: V::from_tagged(&c["envelope"]).unwrap(),
            files: c["files"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(p, r)| (p.clone(), STANDARD.decode(r.as_str().unwrap()).unwrap()))
                .collect(),
        }
    }
    fn command(root: &Path, args: &[&str]) -> String {
        let result = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap().trim().to_owned()
    }
    fn write(root: &Path, path: &str, bytes: &[u8]) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    #[test]
    fn git_capture_pins_committed_bytes_ignores_dirty_checkout_and_orphans() {
        for algorithm in ["sha1", "sha256"] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let original = fixture();
            for (path, raw) in &original.files {
                write(root, &format!("records/{path}"), raw);
            }
            write(root, "unrelated.txt", b"not selected");
            let orphan = format!("records/.kpopper/history/orphan/{}.yaml", "f".repeat(64));
            write(root, &orphan, b"uncommitted staging body");
            command(
                root,
                &[
                    "init",
                    "-b",
                    "main",
                    &format!("--object-format={algorithm}"),
                ],
            );
            command(root, &["add", "."]);
            command(
                root,
                &[
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.test",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-m",
                    "History fixture",
                ],
            );
            let oid = command(root, &["rev-parse", "HEAD"]);
            write(root, "records/GROUNDING.yaml", b"dirty: true\n");
            let status = command(root, &["status", "--porcelain"]);
            let observed = capture(
                root,
                "main",
                "records/PROVENANCE.yaml",
                Some(s("2026-09-17")),
            )
            .unwrap();
            assert_eq!(command(root, &["status", "--porcelain"]), status);
            assert!(!observed.files.contains_key(&orphan));
            assert!(!observed.files.contains_key("unrelated.txt"));
            assert_eq!(
                observed.files["records/GROUNDING.yaml"],
                original.files["GROUNDING.yaml"]
            );
            let manifest = map(&map(&observed.envelope).unwrap()["manifest"]).unwrap();
            assert_eq!(map(&manifest["source"]).unwrap()["commit"], s(&oid));
            let wire = observed.to_bytes().unwrap();
            drop(dir);
            let restored = Observation::from_bytes(&wire).unwrap();
            assert_eq!(restored.envelope, observed.envelope);
            assert_eq!(restored.files, observed.files);
        }
    }
    #[test]
    fn typed_branch_wire_and_untrusted_git_references_refuse() {
        let original = fixture();
        let wire = original.to_bytes().unwrap();
        let restored = Observation::from_bytes(&wire).unwrap();
        assert_eq!(restored.files, original.files);
        let mut wire_with_junk = wire;
        wire_with_junk.extend_from_slice(b" null");
        assert!(Observation::from_bytes(&wire_with_junk).is_err());
        let dir = tempfile::tempdir().unwrap();
        command(dir.path(), &["init", "-b", "main"]);
        assert!(capture(dir.path(), "--help", "GROUNDING.yaml", None).is_err());
        assert!(capture(dir.path(), "HEAD", ".git/config", None).is_err());
        assert!(capture(dir.path(), "HEAD\0main", "GROUNDING.yaml", None).is_err());
    }
}

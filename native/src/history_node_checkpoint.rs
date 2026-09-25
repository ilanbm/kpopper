//! One optional accepted-view checkpoint for recovering a manual edit. It is
//! private derived state: only the committed manifest can authenticate its bytes.
use crate::{
    Result, history_contract::error, history_node_publication as P, history_transaction_fs as F,
};
use std::{fs, path::Path};

pub(crate) const PATH: &str = ".kpopper/.history-local/accepted-node-view.yaml";
const IGNORE: &str = ".kpopper/.history-local/.gitignore";
const LIMIT: usize = crate::history_yaml::MAX_DOCUMENT_BYTES;

/// Cache failure cannot turn a committed record into a stuck recovery journal.
pub(crate) fn remember(root: &Path, view: &[u8]) {
    let attempt = || -> Result<()> {
        crate::require(view.len() <= LIMIT, "node_checkpoint_limit")?;
        let path = F::target(root, PATH)?;
        let parent = path.parent().ok_or_else(|| error("invalid_path"))?;
        fs::create_dir_all(parent)?;
        fs::File::open(parent.parent().unwrap())?.sync_all()?;
        F::publish_immutable(root, IGNORE, b"*\n")?;
        F::replace(&path, Some(view))
    };
    let _ = attempt();
}

fn local(root: &Path, relative: &str) -> Option<Vec<u8>> {
    let path = F::target(root, relative).ok()?;
    if path.metadata().ok()?.len() > LIMIT as u64 {
        return None;
    }
    F::read(&path).ok().flatten()
}

/// Called under the write-route lock. A cache, index blob or HEAD blob must pass
/// the same manifest/closure check as a caller-supplied --baseline.
pub(crate) fn accepted(root: &Path) -> Result<Vec<u8>> {
    let valid = |raw: &[u8]| P::capture_edited_snapshot(root, raw).is_ok();
    for relative in [PATH, "GROUNDING.yaml"] {
        if let Some(raw) = local(root, relative) {
            if valid(&raw) {
                return Ok(raw);
            }
        }
    }
    if let Ok(prefix) = crate::history_branch_git::git(root, &["rev-parse", "--show-prefix"], 4096)
    {
        if let Ok(prefix) = std::str::from_utf8(&prefix) {
            let path = format!("{}GROUNDING.yaml", prefix.trim_end_matches('\n'));
            for revision in [format!(":{path}"), format!("HEAD:{path}")] {
                if let Ok(raw) = crate::history_branch_git::git(root, &["show", &revision], LIMIT) {
                    if valid(&raw) {
                        return Ok(raw);
                    }
                }
            }
        }
    }
    Err(error(
        "node_edit_baseline_required: no verified accepted view is available; supply --baseline FILE",
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{
        history_authoring::{n, obj, s},
        history_view::map_mut,
        value::TypedValue as V,
    };

    fn git(root: &Path, args: &[&str]) {
        let result = std::process::Command::new("git")
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
    }
    fn fixture() -> (tempfile::TempDir, Vec<u8>) {
        let root = tempfile::tempdir().unwrap();
        let runtime = crate::history_authoring::tests::runtime(&root.path().join("runtime"));
        let original = vec![root.path().join("GROUNDING.yaml")];
        let route = crate::project_modes::WriteRoute::capture(&original, root.path()).unwrap();
        crate::history_node_birth::create(
            &route,
            &original,
            &obj([
                ("kind", s("add")),
                ("id", s("p.a")),
                ("body", obj([("v", n("1"))])),
            ]),
            Some(&runtime),
            s("writer"),
        )
        .unwrap();
        let view = fs::read(&original[0]).unwrap();
        (root, view)
    }
    fn edit(root: &Path, original: &[u8]) -> Vec<u8> {
        let mut value = crate::history_yaml::decode_document(original).unwrap();
        let known = map_mut(map_mut(&mut value).unwrap().get_mut("known").unwrap()).unwrap();
        map_mut(known.get_mut("p.a").unwrap())
            .unwrap()
            .insert("v".into(), n("8"));
        let raw = crate::history_yaml::encode_document(&value).unwrap();
        fs::write(root.join("GROUNDING.yaml"), &raw).unwrap();
        raw
    }
    #[test]
    fn checkpoint_is_only_a_verified_preimage_and_explicit_baseline_never_falls_back() {
        let (root, canonical) = fixture();
        let edited = edit(root.path(), &canonical);
        assert_eq!(accepted(root.path()).unwrap(), canonical);
        fs::write(root.path().join("wrong.yaml"), &edited).unwrap();
        let original = vec![root.path().join("GROUNDING.yaml")];
        assert!(
            crate::public_node_edits::run(
                &original,
                root.path(),
                Some(Path::new("wrong.yaml")),
                None,
                "edit",
                None,
                &mut |_| Ok(())
            )
            .is_err()
        );
        assert_eq!(fs::read(&original[0]).unwrap(), edited);
        fs::write(root.path().join(PATH), &edited).unwrap();
        assert!(accepted(root.path()).is_err());
        fs::remove_file(root.path().join(PATH)).unwrap();
        assert!(accepted(root.path()).is_err());
        assert_eq!(fs::read(&original[0]).unwrap(), edited);
    }
    #[test]
    fn git_index_and_head_supply_exact_baselines_without_a_local_checkpoint() {
        for head in [false, true] {
            let (root, canonical) = fixture();
            git(root.path(), &["init", "-q", "-b", "main"]);
            git(root.path(), &["config", "user.name", "Fixture"]);
            git(
                root.path(),
                &["config", "user.email", "fixture@example.test"],
            );
            git(
                root.path(),
                &["add", "GROUNDING.yaml", ".kpopper", ".gitattributes"],
            );
            let ignored =
                crate::history_branch_git::git(root.path(), &["check-ignore", PATH], 4096).unwrap();
            assert!(!ignored.is_empty());
            if head {
                git(
                    root.path(),
                    &["-c", "commit.gpgsign=false", "commit", "-qm", "record"],
                );
            }
            fs::remove_file(root.path().join(PATH)).unwrap();
            let edited = edit(root.path(), &canonical);
            if head {
                git(root.path(), &["add", "GROUNDING.yaml"]);
            }
            assert_eq!(accepted(root.path()).unwrap(), canonical);
            assert_eq!(
                fs::read(root.path().join("GROUNDING.yaml")).unwrap(),
                edited
            );
            assert!(
                !root.path().join(PATH).exists(),
                "a read must not create the checkpoint"
            );
        }
    }
    #[test]
    fn an_unwritable_checkpoint_does_not_block_committed_writes() {
        let (root, _) = fixture();
        fs::remove_file(root.path().join(PATH)).unwrap();
        fs::create_dir(root.path().join(PATH)).unwrap();
        let runtime = crate::history_authoring::tests::runtime(&root.path().join("runtime"));
        let original = vec![root.path().join("GROUNDING.yaml")];
        let route = crate::project_modes::WriteRoute::capture(&original, root.path()).unwrap();
        crate::public_node_history::write_inner(
            &route,
            &original,
            &obj([
                ("kind", s("add")),
                ("id", s("p.b")),
                ("body", obj([("v", n("2"))])),
            ]),
            &mut |_| Ok(()),
            Some(&runtime),
            V::Null,
        )
        .unwrap();
        assert!(crate::history_node_capture::Capture::read(root.path()).is_ok());
        assert!(
            !root
                .path()
                .join(".kpopper/.history-node-publication.json")
                .exists()
        );
    }
}

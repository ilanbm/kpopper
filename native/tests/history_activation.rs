#![cfg(unix)]
use kpop_native::{
    Result,
    history_activation::{self as A, Deployment, DeploymentGuard, Options, Selection},
    history_transaction::{Layout, PreparedMutation},
    history_transaction_fs as FS, history_yaml,
};
use serde_json::{Value as J, json};
use std::{
    cell::Cell,
    fs,
    path::{Path, PathBuf},
};
struct Exclusion {
    held: Cell<bool>,
    calls: Cell<usize>,
}
struct Held<'a>(&'a Exclusion);
impl Drop for Held<'_> {
    fn drop(&mut self) {
        self.0.held.set(false);
    }
}
impl DeploymentGuard for Held<'_> {
    fn verify(&self) -> Result<()> {
        assert!(self.0.held.get());
        Ok(())
    }
}
impl Deployment for Exclusion {
    fn exclude<'a>(&'a self, _: &J, _: &J) -> Result<Box<dyn DeploymentGuard + 'a>> {
        assert!(!self.held.replace(true));
        self.calls.set(self.calls.get() + 1);
        Ok(Box::new(Held(self)))
    }
}
fn excluded() -> Exclusion {
    Exclusion {
        held: Cell::new(false),
        calls: Cell::new(0),
    }
}
fn selection(root: &Path) -> Selection {
    let corpus: J = serde_json::from_str(include_str!("fixtures/history-runtime.json")).unwrap();
    let mut declaration = corpus["cases"][0]["declaration"].clone();
    let shell = Path::new("/bin/sh").canonicalize().unwrap();
    let root = root.canonicalize().unwrap();
    fs::write(root.join("cli.py"), b"fixture").unwrap();
    declaration["nonce"] = json!("NONCE_PLACEHOLDER");
    declaration["resolved"]["executable"] = json!(shell);
    declaration["resolved"]["cli"] = json!(root.join("cli.py"));
    declaration["resolved"]["package_root"] = json!(root);
    let response = root.join("response.json");
    fs::write(&response, serde_json::to_vec(&declaration).unwrap()).unwrap();
    Selection {
        inventory: json!([{"id":"managed","argv":[shell,"-c","sed \"s/NONCE_PLACEHOLDER/$5/g\" \"$1\"","probe",response],"executable":shell,"package_root":root}]),
        expected_digests: json!({"managed":{"sources":declaration["sources"]["digest"],"native":declaration["native"]["digest"],"schemas":declaration["schemas"]["digest"]}}),
    }
}
fn options() -> Options {
    Options {
        operation: "activate-test".into(),
        recorded_at: "2026-09-19T12:00:00+00:00".into(),
        record_id: Some("record-test".into()),
    }
}
fn entry(root: &Path) -> PathBuf {
    let path = root.join("GROUNDING.yaml");
    fs::write(
        &path,
        b"meta:\n  purpose: Fixture\nm:\n  cost:\n    value: 12\n    place: Invoice\n",
    )
    .unwrap();
    path
}
fn mutation(root: &Path) -> (PathBuf, PreparedMutation, Exclusion, tempfile::TempDir) {
    let runtime = tempfile::tempdir().unwrap();
    let selection = selection(runtime.path());
    let entry = entry(root);
    let guard = excluded();
    let prepared = A::prepare_activation(&entry, &options(), &selection, &guard, None).unwrap();
    assert!(!guard.held.get());
    (entry, prepared, guard, runtime)
}
#[test]
fn activation_and_lossless_inverse_use_live_exclusion() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (entry, prepared, guard, _runtime) = mutation(&root);
    let before = fs::read(&entry).unwrap();
    let result = A::publish(&entry, &prepared, &guard, None).unwrap();
    assert_eq!(result.to_json().unwrap()["state"], "activated");
    let data = prepared.to_data().to_json().unwrap();
    let selection = Selection {
        inventory: data["baseline"]["deployment"]["inventory"].clone(),
        expected_digests: data["baseline"]["deployment"]["expected_digests"].clone(),
    };
    let inverse = A::prepare_deactivation(
        &entry,
        &prepared,
        "deactivate-test",
        &selection,
        &guard,
        None,
    )
    .unwrap();
    A::publish(&entry, &inverse, &guard, None).unwrap();
    assert_eq!(fs::read(&entry).unwrap(), before);
    let marker = history_yaml::decode_document(
        &fs::read(root.join(Layout::for_entry("GROUNDING.yaml").unwrap().authority)).unwrap(),
    )
    .unwrap()
    .to_json()
    .unwrap();
    assert_eq!(marker["authority"], "legacy");
    assert_eq!(marker["generation"], 2);
    assert!(guard.calls.get() >= 4);
    assert!(!guard.held.get());
}
#[test]
fn changed_source_blocks_before_journal_or_authority_publication() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let (entry, prepared, guard, _runtime) = mutation(&root);
    fs::write(
        &entry,
        b"meta: {}\nm: {cost: {value: 99, place: Changed}}\n",
    )
    .unwrap();
    assert!(A::publish(&entry, &prepared, &guard, None).is_err());
    let l = Layout::for_entry("GROUNDING.yaml").unwrap();
    assert!(!root.join(l.journal).exists());
    assert!(!root.join(l.authority).exists());
}
#[test]
fn partial_activation_recovers_after_or_cancels_with_retained_evidence() {
    for direction in [FS::Direction::After, FS::Direction::Before] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let (entry, prepared, guard, _runtime) = mutation(&root);
        let before = fs::read(&entry).unwrap();
        let l = Layout::for_entry("GROUNDING.yaml").unwrap();
        fs::create_dir_all(root.join(&l.journal).parent().unwrap()).unwrap();
        fs::write(root.join(&l.journal), prepared.to_bytes().unwrap()).unwrap();
        let item = prepared
            .files()
            .iter()
            .find(|i| i.role == "history_object")
            .unwrap();
        fs::create_dir_all(root.join(&item.path).parent().unwrap()).unwrap();
        fs::write(root.join(&item.path), item.after.as_ref().unwrap()).unwrap();
        A::recover(&entry, direction, &guard, None).unwrap();
        assert!(!root.join(l.journal).exists());
        let marker = history_yaml::decode_document(&fs::read(root.join(l.authority)).unwrap())
            .unwrap()
            .to_json()
            .unwrap();
        if direction == FS::Direction::Before {
            assert_eq!(fs::read(&entry).unwrap(), before);
            assert_eq!(marker["authority"], "legacy");
            assert_eq!(marker["generation"], 2);
            assert!(
                root.join(l.cancellations)
                    .join("activate-test.yaml")
                    .exists()
            );
        } else {
            assert_eq!(marker["authority"], "history");
        }
        assert!(root.join(&item.path).exists());
    }
}
#[test]
#[ignore = "requires explicit KPOP_TEST_MANAGED_LAUNCHERS selection"]
fn actual_managed_python_launcher_supports_native_activation() {
    let raw =
        fs::read(std::env::var_os("KPOP_TEST_MANAGED_LAUNCHERS").expect("selected deployment"))
            .unwrap();
    let value: J = serde_json::from_slice(&raw).unwrap();
    let selection = Selection {
        inventory: value["inventory"].clone(),
        expected_digests: value["expected"].clone(),
    };
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let entry = entry(&root);
    let guard = excluded();
    let prepared = A::prepare_activation(&entry, &options(), &selection, &guard, None).unwrap();
    A::publish(&entry, &prepared, &guard, None).unwrap();
}
fn git(root: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn worktrees(root: &Path) -> Vec<PathBuf> {
    let a = root.join("a");
    let b = root.join("b");
    fs::create_dir(&a).unwrap();
    git(&a, &["init", "-q"]);
    entry(&a);
    git(&a, &["add", "GROUNDING.yaml"]);
    git(&a, &["commit", "-qm", "fixture"]);
    git(
        &a,
        &["worktree", "add", "-qb", "other", b.to_str().unwrap()],
    );
    vec![a.join("GROUNDING.yaml"), b.join("GROUNDING.yaml")]
}
#[test]
fn group_transitions_hold_one_deployment_and_cover_all_worktrees() {
    use kpop_native::history_group_activation as G;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let entries = worktrees(&root);
    let runtime = tempfile::tempdir().unwrap();
    let selection = selection(runtime.path());
    let guard = excluded();
    assert!(A::prepare_activation(&entries[0], &options(), &selection, &guard, None).is_err());
    let group = G::prepare(
        &entries,
        "group-activate",
        "2026-09-19T12:00:00+00:00",
        &selection,
        &guard,
        None,
        None,
    )
    .unwrap();
    let before = entries
        .iter()
        .map(|p| fs::read(p).unwrap())
        .collect::<Vec<_>>();
    G::publish(&group, &guard, None).unwrap();
    G::publish(&group, &guard, None).unwrap(); // exact idempotent retry
    let inverse = G::prepare(
        &entries,
        "group-deactivate",
        "2026-09-19T12:01:00+00:00",
        &selection,
        &guard,
        Some(&group),
        None,
    )
    .unwrap();
    G::publish(&inverse, &guard, None).unwrap();
    for (entry, before) in entries.iter().zip(before) {
        assert_eq!(fs::read(entry).unwrap(), before);
    }
    assert!(!guard.held.get());
}
#[test]
fn group_unready_recovery_preserves_later_unguarded_edits() {
    use kpop_native::history_group_activation as G;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let entries = worktrees(&root);
    let runtime = tempfile::tempdir().unwrap();
    let selection = selection(runtime.path());
    let guard = excluded();
    let group = G::prepare(
        &entries,
        "group-cancel",
        "2026-09-19T12:00:00+00:00",
        &selection,
        &guard,
        None,
        None,
    )
    .unwrap();
    let journal = G::journal_for(&group).unwrap();
    fs::create_dir_all(journal.parent().unwrap()).unwrap();
    fs::write(&journal, group.to_bytes().unwrap()).unwrap();
    let changed = b"meta: {}\nm: {cost: {value: 54, place: User}}\n";
    fs::write(&entries[1], changed).unwrap();
    G::recover(&journal, FS::Direction::Before, &guard, None).unwrap();
    assert!(!journal.exists());
    assert_eq!(fs::read(&entries[1]).unwrap(), changed);
}
struct Tripwire {
    marker: PathBuf,
}
struct TripwireHeld<'a>(&'a Tripwire);
impl DeploymentGuard for TripwireHeld<'_> {
    fn verify(&self) -> Result<()> {
        if self.0.marker.exists() {
            Err(kpop_native::Error("injected_after_images".into()))
        } else {
            Ok(())
        }
    }
}
impl Deployment for Tripwire {
    fn exclude<'a>(&'a self, _: &J, _: &J) -> Result<Box<dyn DeploymentGuard + 'a>> {
        Ok(Box::new(TripwireHeld(self)))
    }
}
#[test]
fn started_group_compensation_fences_every_member_and_preserves_divergent_edit() {
    use kpop_native::history_group_activation as G;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let entries = worktrees(&root);
    let runtime = tempfile::tempdir().unwrap();
    let selection = selection(runtime.path());
    let guard = excluded();
    let group = G::prepare(
        &entries,
        "group-partial",
        "2026-09-19T12:00:00+00:00",
        &selection,
        &guard,
        None,
        None,
    )
    .unwrap();
    let tripwire = Tripwire {
        marker: entries[0]
            .parent()
            .unwrap()
            .join(Layout::for_entry("GROUNDING.yaml").unwrap().authority),
    };
    assert!(G::publish(&group, &tripwire, None).is_err());
    let journal = G::journal_for(&group).unwrap();
    assert!(journal.exists());
    assert!(journal.with_extension("ready").exists());
    // Model a crash between members: second mutable images have not switched.
    let second = &group.mutations()[entries[1].to_str().unwrap()];
    for item in second
        .files()
        .iter()
        .filter(|i| i.role == "record" || i.role == "history_authority")
    {
        let path = entries[1].parent().unwrap().join(&item.path);
        if let Some(raw) = &item.before {
            fs::write(path, raw).unwrap();
        } else {
            fs::remove_file(path).unwrap();
        }
    }
    let divergent = b"meta: {}\nm: {cost: {value: 71, place: Edit}}\n";
    fs::write(&entries[1], divergent).unwrap();
    assert!(G::recover(&journal, FS::Direction::Before, &guard, None).is_err());
    assert_eq!(fs::read(&entries[1]).unwrap(), divergent);
    assert!(journal.exists());
    let original = second
        .files()
        .iter()
        .find(|i| i.role == "record")
        .unwrap()
        .before
        .as_ref()
        .unwrap();
    fs::write(&entries[1], original).unwrap();
    G::recover(&journal, FS::Direction::Before, &guard, None).unwrap();
    assert!(!journal.exists());
    for entry in entries {
        let l = Layout::for_entry("GROUNDING.yaml").unwrap();
        let root = entry.parent().unwrap();
        let marker = history_yaml::decode_document(&fs::read(root.join(l.authority)).unwrap())
            .unwrap()
            .to_json()
            .unwrap();
        assert_eq!(marker["authority"], "legacy");
        assert_eq!(marker["generation"], 2);
        assert!(!root.join(l.journal).exists());
    }
}
#[test]
fn forged_group_completion_cannot_release_unchanged_members() {
    use kpop_native::history_group_activation as G;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let entries = worktrees(&root);
    let runtime = tempfile::tempdir().unwrap();
    let selection = selection(runtime.path());
    let guard = excluded();
    let group = G::prepare(
        &entries,
        "group-forged",
        "2026-09-19T12:00:00+00:00",
        &selection,
        &guard,
        None,
        None,
    )
    .unwrap();
    let journal = G::journal_for(&group).unwrap();
    fs::create_dir_all(journal.parent().unwrap()).unwrap();
    fs::write(&journal, group.to_bytes().unwrap()).unwrap();
    let data = group.to_data().to_json().unwrap();
    fs::write(journal.with_extension("complete"),serde_json::to_vec(&json!({"kind":"history-authority-group-complete/v1","digest":data["digest"],"direction":"after","phase":"applied"})).unwrap()).unwrap();
    assert!(G::recover(&journal, FS::Direction::After, &guard, None).is_err());
    assert!(journal.exists());
}
mod support;
#[test]
fn actual_pending_bundles_are_retained_and_ref_changes_stop_activation() {
    use kpop_native::{pending_state::Ledger, project_modes::Project};
    let corpus: J = serde_json::from_str(include_str!("fixtures/pending-state.json")).unwrap();
    let runtime = tempfile::tempdir().unwrap();
    let selection = selection(runtime.path());
    for case_name in ["one", "accepted", "two", "target_plain", "target_hyp"] {
        let case = corpus
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == case_name)
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        support::pending::fixture(&root, case);
        let entry = root.join("GROUNDING.yaml");
        let guard = excluded();
        let project = Project::open(&root).unwrap();
        let ledger = Ledger::capture(&project).unwrap().portable();
        let prepared = A::prepare_activation(&entry, &options(), &selection, &guard, None)
            .unwrap_or_else(|e| panic!("{case_name}: {e}"));
        if case_name == "one" {
            support::pending::git(
                &root,
                &["update-ref", "-d", kpop_native::pending_state::REF],
                None,
            );
            assert!(A::publish(&entry, &prepared, &guard, None).is_err());
            assert!(
                !root
                    .join(Layout::for_entry("GROUNDING.yaml").unwrap().authority)
                    .exists()
            );
        } else {
            A::publish(&entry, &prepared, &guard, None).unwrap_or_else(|e| {
                let actual = kpop_native::source_capture::capture_source(
                    std::slice::from_ref(&entry),
                    &root,
                    kpop_native::source_capture::ReadMode::Live,
                    None,
                )
                .unwrap();
                fs::write(
                    std::env::temp_dir().join("kpop-authority-live-actual.json"),
                    actual.snapshot().unwrap().to_json().unwrap(),
                )
                .unwrap();
                fs::copy(
                    root.join(".kpopper-history-migration/live.json"),
                    std::env::temp_dir().join("kpop-authority-live-expected.json"),
                )
                .unwrap();
                panic!("{case_name} publish: {e}")
            });
            assert_eq!(Ledger::capture(&project).unwrap().portable(), ledger);
            let inverse = A::prepare_deactivation(
                &entry,
                &prepared,
                "deactivate-pending",
                &selection,
                &guard,
                None,
            )
            .unwrap_or_else(|e| panic!("{case_name} inverse: {e}"));
            A::publish(&entry, &inverse, &guard, None)
                .unwrap_or_else(|e| panic!("{case_name} inverse publish: {e}"));
            assert_eq!(Ledger::capture(&project).unwrap().portable(), ledger);
        }
    }
}
fn portable_mutation(m: &PreparedMutation) -> kpop_native::value::TypedValue {
    use kpop_native::value::TypedValue as V;
    let mut data = m.to_data();
    let V::Map(d) = &mut data else { panic!("map") };
    d.remove("digest");
    let V::Map(b) = d.get_mut("baseline").unwrap() else {
        panic!("baseline")
    };
    for key in ["deployment", "project", "reads", "transaction_root"] {
        b.remove(key);
    }
    portable_data(data)
}
fn portable_data(mut data: kpop_native::value::TypedValue) -> kpop_native::value::TypedValue {
    use kpop_native::value::TypedValue as V;
    let V::Map(d) = &mut data else { panic!("map") };
    let V::Map(b) = &d["baseline"] else {
        panic!("baseline")
    };
    if b["direction"] == V::Text("deactivate".into()) {
        let V::Map(receipt) = d.get_mut("receipt").unwrap() else {
            panic!("receipt")
        };
        receipt.remove("digest");
        let V::Map(before) = receipt.get_mut("before").unwrap() else {
            panic!("before")
        };
        before.remove("activation_digest");
    }
    data
}
fn compare_oracle(m: &PreparedMutation, expected: &J, name: &str, phase: &str) {
    use kpop_native::value::TypedValue as V;
    let actual = portable_mutation(m);
    let expected = portable_data(V::from_tagged(expected).unwrap());
    if actual != expected {
        let dir = std::env::temp_dir().join("kpop-authority-differential");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{phase}-actual.json")),
            serde_json::to_vec_pretty(&actual.to_tagged().unwrap()).unwrap(),
        )
        .unwrap();
        fs::write(
            dir.join(format!("{phase}-expected.json")),
            serde_json::to_vec_pretty(&expected.to_tagged().unwrap()).unwrap(),
        )
        .unwrap();
        let V::Map(a) = actual else { panic!("map") };
        let V::Map(b) = expected else { panic!("map") };
        let diff = a.keys().filter(|k| a[*k] != b[*k]).collect::<Vec<_>>();
        panic!(
            "{name} {phase} differs: {diff:?}; diagnostics at {}",
            dir.display()
        );
    }
}
#[test]
fn final_python_authority_lifecycle_matches_all_portable_images_and_receipts() {
    let corpus: J = serde_json::from_str(include_str!("fixtures/history-activation.json")).unwrap();
    let runtime = tempfile::tempdir().unwrap();
    let selection = selection(runtime.path());
    let mut accepted = 0;
    for case in corpus["cases"].as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        for (path, raw) in case["files"].as_object().unwrap() {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, raw.as_str().unwrap()).unwrap();
        }
        let entry = root.join(case["entry"].as_str().unwrap());
        let guard = excluded();
        let options = Options {
            operation: "import-fixture".into(),
            recorded_at: "2026-09-19T12:00:00+00:00".into(),
            record_id: Some("record-import-fixture".into()),
        };
        let result = A::prepare_activation(&entry, &options, &selection, &guard, None);
        if case.get("activation").is_none() {
            assert!(result.is_err(), "{} accepted", case["name"]);
            continue;
        }
        let prepared = result.unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
        compare_oracle(
            &prepared,
            &case["activation"],
            case["name"].as_str().unwrap(),
            "activation",
        );
        A::publish(&entry, &prepared, &guard, None)
            .unwrap_or_else(|e| panic!("{} publish: {e}", case["name"]));
        let inverse = A::prepare_deactivation(
            &entry,
            &prepared,
            "deactivate-fixture",
            &selection,
            &guard,
            None,
        )
        .unwrap_or_else(|e| panic!("{} inverse: {e}", case["name"]));
        assert_eq!(
            value_at(
                &inverse.to_data(),
                &["receipt", "before", "activation_digest"]
            ),
            value_at(&prepared.to_data(), &["digest"])
        );
        compare_oracle(
            &inverse,
            &case["deactivation"],
            case["name"].as_str().unwrap(),
            "deactivation",
        );
        A::publish(&entry, &inverse, &guard, None)
            .unwrap_or_else(|e| panic!("{} inverse publish: {e}", case["name"]));
        assert_eq!(
            fs::read(&entry).unwrap(),
            case["files"][case["entry"].as_str().unwrap()]
                .as_str()
                .unwrap()
                .as_bytes()
        );
        accepted += 1;
    }
    assert_eq!(accepted, 24);
}

fn value_at<'a>(
    value: &'a kpop_native::value::TypedValue,
    path: &[&str],
) -> &'a kpop_native::value::TypedValue {
    if path.is_empty() {
        return value;
    }
    let kpop_native::value::TypedValue::Map(map) = value else {
        panic!("map")
    };
    value_at(&map[path[0]], &path[1..])
}
#[test]
fn deployment_locks_do_not_change_detached_target_observations() {
    use kpop_native::{
        reasoning_snapshot::Snapshot,
        source_capture::{ReadMode, capture_source},
    };
    let corpus: J = serde_json::from_str(include_str!("fixtures/pending-state.json")).unwrap();
    let case = corpus
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "target_hyp")
        .unwrap();
    // All detached temp roots sort before this live root. Sharing its lock-order
    // namespace would incorrectly turn an available target into unavailable.
    let temp = tempfile::Builder::new()
        .prefix("zzzz-kpop-live-")
        .tempdir()
        .unwrap();
    let root = temp.path().canonicalize().unwrap();
    support::pending::fixture(&root, case);
    let entry = root.join("GROUNDING.yaml");
    let expected = capture_source(std::slice::from_ref(&entry), &root, ReadMode::Live, None)
        .unwrap()
        .snapshot()
        .unwrap()
        .to_data();
    let runtime = tempfile::tempdir().unwrap();
    let selection = selection(runtime.path());
    let guard = excluded();
    let prepared = A::prepare_activation(&entry, &options(), &selection, &guard, None).unwrap();
    let raw = prepared
        .files()
        .iter()
        .find(|i| i.path == ".kpopper-history-migration/live.json")
        .unwrap()
        .after
        .as_ref()
        .unwrap();
    let observed = Snapshot::from_json(raw).unwrap().to_data();
    assert_eq!(
        value_at(&observed, &["context", "target"]),
        value_at(&expected, &["context", "target"])
    );
}
#[test]
fn group_worktree_hypotheses_are_captured_under_ordered_locks() {
    use kpop_native::history_group_activation as G;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let entries = worktrees(&root);
    for entry in &entries {
        let path = entry
            .parent()
            .unwrap()
            .join(".kpopper/hypotheses/idea.yaml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            b"meta:\n  claim: Alternate cost\nm:\n  cost:\n    value: 13\n    place: Estimate\n",
        )
        .unwrap();
    }
    let runtime = tempfile::tempdir().unwrap();
    let selection = selection(runtime.path());
    let guard = excluded();
    let group = G::prepare(
        &entries,
        "group-hypotheses",
        "2026-09-19T12:00:00+00:00",
        &selection,
        &guard,
        None,
        None,
    )
    .unwrap();
    G::publish(&group, &guard, None).unwrap();
}
#[test]
fn launcher_rewriting_a_terminal_image_keeps_recovery_journal() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let entry = entry(&root);
    let before = fs::read(&entry).unwrap();
    let runtime = tempfile::tempdir().unwrap();
    let mut selection = selection(runtime.path());
    let launcher_root = runtime.path().canonicalize().unwrap();
    let original = launcher_root.join("original.yaml");
    fs::write(&original, &before).unwrap();
    let l = Layout::for_entry("GROUNDING.yaml").unwrap();
    let shell = selection.inventory[0]["argv"][0].clone();
    selection.inventory[0]["argv"] = json!([
        shell,
        "-c",
        "if [ -f \"$2\" ]; then cp \"$3\" \"$4\"; fi; sed \"s/NONCE_PLACEHOLDER/$8/g\" \"$1\"",
        "probe",
        launcher_root.join("response.json"),
        root.join(&l.authority),
        original,
        entry
    ]);
    let guard = excluded();
    let prepared = A::prepare_activation(&entry, &options(), &selection, &guard, None).unwrap();
    assert!(A::publish(&entry, &prepared, &guard, None).is_err());
    assert!(root.join(l.journal).exists());
    assert_eq!(fs::read(&entry).unwrap(), before);
}
#[test]
fn group_launcher_deleting_an_immutable_image_keeps_all_guards() {
    use kpop_native::history_group_activation as G;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let entries = worktrees(&root);
    let runtime = tempfile::tempdir().unwrap();
    let launcher_root = runtime.path().canonicalize().unwrap();
    let mut selection = selection(&launcher_root);
    let l = Layout::for_entry("GROUNDING.yaml").unwrap();
    let first = entries[0].parent().unwrap();
    let shell = selection.inventory[0]["argv"][0].clone();
    selection.inventory[0]["argv"] = json!([
        shell,
        "-c",
        "if [ -f \"$2/.kpopper/history.yaml\" ]; then rm -f \"$2\"/.kpopper/history/*/*.yaml; fi; sed \"s/NONCE_PLACEHOLDER/$6/g\" \"$1\"",
        "probe",
        launcher_root.join("response.json"),
        first
    ]);
    let guard = excluded();
    let group = G::prepare(
        &entries,
        "group-probe-delete",
        "2026-09-19T12:00:00+00:00",
        &selection,
        &guard,
        None,
        None,
    )
    .unwrap();
    assert!(G::publish(&group, &guard, None).is_err());
    assert!(G::journal_for(&group).unwrap().exists());
    for entry in entries {
        assert!(entry.parent().unwrap().join(&l.journal).exists());
    }
}

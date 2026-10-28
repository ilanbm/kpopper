//! Black-box source-resolution contract for generated Simple bindings.
//!
//! These cases deliberately exercise the executable rather than private resolver
//! functions. The fixtures describe generated-owner source-root invariants.
#[path = "support/git_path.rs"]
mod git_path;
#[path = "support/simple_runtime.rs"]
mod simple_runtime;

use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    projects: PathBuf,
    home: PathBuf,
    resources: PathBuf,
}

impl Fixture {
    fn new(git: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(temp.path()).unwrap();
        let root = base.join("workspace");
        let projects = base.join("projects");
        let home = base.join("home");
        let resource_root = base.join("resources");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(&home).unwrap();
        let fixture = Self {
            _temp: temp,
            root,
            projects,
            home,
            resources: resource_root,
        };
        simple_runtime::resources(&fixture.resources);
        if git {
            fixture.git(&["init", "-q"]);
            fixture.git(&["config", "user.email", "test@example.invalid"]);
            fixture.git(&["config", "user.name", "Source Tests"]);
        }
        fixture
    }

    fn command(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(&self.root)
            .args(args)
            .env("KPOPPER_PROJECTS_DIR", &self.projects)
            .env("KPOPPER_PRIVATE_HOME", &self.home)
            .env("HOME", &self.home)
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("KPOPPER_NATIVE_RESOURCES", &self.resources)
            .env("KPOPPER_NATIVE_CACHE", self.home.join("cache"))
            .env_remove("KPOPPER_AGENT_SESSION")
            .env_remove("CODEX_THREAD_ID")
            .output()
            .unwrap()
    }

    fn git(&self, args: &[&str]) -> Output {
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn add_source(&self, id: &str, locator: &str) {
        let output = self.command(&["add", id, &format!("file={locator}")]);
        assert!(
            output.status.success(),
            "add {id}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn search(&self, query: &str) -> Output {
        self.command(&["--json", "search", "--limit", "10", query])
    }

    fn search_with_source_root(&self, source_root: &Path, query: &str) -> Output {
        let source_root = source_root.to_string_lossy().into_owned();
        self.command(&[
            "--json",
            "search",
            "--limit",
            "10",
            "--source-root",
            &source_root,
            query,
        ])
    }

    fn where_record(&self) -> PathBuf {
        let output = self.command(&["--json", "where"]);
        assert!(
            output.status.success(),
            "where: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
        PathBuf::from(envelope["output"].as_str().unwrap().trim())
    }
}

fn seed_files(fixture: &Fixture) {
    fs::create_dir_all(fixture.root.join("src")).unwrap();
    fs::write(fixture.root.join("README.md"), "needle from readme\n").unwrap();
    fs::write(fixture.root.join("src/foo.rs"), "needle from foo\n").unwrap();
    fixture.git(&["add", "README.md", "src/foo.rs"]);
    fixture.git(&["commit", "-qm", "initial sources"]);
}

#[test]
fn generated_git_binding_searches_readme_and_src_from_current_workspace() {
    let fixture = Fixture::new(true);
    seed_files(&fixture);
    fixture.add_source("s.readme", "README.md");
    fixture.add_source("s.foo", "src/foo.rs");

    let record = fixture.where_record();
    assert!(
        record.starts_with(&fixture.projects),
        "generated record escaped central root: {record:?}"
    );
    assert!(
        !record.starts_with(&fixture.root),
        "generated record remained in workspace: {record:?}"
    );

    let output = fixture.search("needle");
    assert!(
        output.status.success(),
        "search: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("needle from readme"),
        "README source was not searchable: {text}"
    );
    assert!(
        text.contains("needle from foo"),
        "src/foo.rs source was not searchable: {text}"
    );
    assert!(
        text.contains("README.md"),
        "search omitted README source path: {text}"
    );
    assert!(
        text.contains("foo.rs"),
        "search omitted src/foo.rs source path: {text}"
    );
}

#[test]
fn generated_non_git_binding_uses_workspace_sources() {
    let fixture = Fixture::new(false);
    fs::create_dir_all(fixture.root.join("src")).unwrap();
    fs::write(fixture.root.join("README.md"), "non-git needle\n").unwrap();
    fs::write(fixture.root.join("src/foo.rs"), "non-git foo needle\n").unwrap();
    fixture.add_source("s.readme", "README.md");
    fixture.add_source("s.foo", "src/foo.rs");

    let record = fixture.where_record();
    assert!(
        record.starts_with(&fixture.projects),
        "non-Git record was not central: {record:?}"
    );
    let output = fixture.search("non-git");
    assert!(
        output.status.success(),
        "search: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("non-git needle"));
}

#[test]
fn missing_and_revision_pinned_sources_have_distinct_check_advisories() {
    let fixture = Fixture::new(true);
    fs::write(fixture.root.join("README.md"), "pinned source\n").unwrap();
    fixture.git(&["add", "README.md"]);
    fixture.git(&["commit", "-qm", "pinned source"]);
    fixture.add_source("s.pinned", "HEAD:README.md");
    fixture.add_source("s.missing", "HEAD:no-longer-there.md");
    let output = fixture.command(&["check"]);
    let text = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(
        text.contains("s.missing"),
        "missing pin advisory absent: {text}"
    );
    assert!(
        !text.contains("s.pinned: file"),
        "available revision pin reported missing: {text}"
    );
}

#[test]
fn generated_source_resolution_refuses_duplicate_different_matches() {
    let fixture = Fixture::new(false);
    fs::write(fixture.root.join("README.md"), "workspace copy\n").unwrap();
    fixture.add_source("s.readme", "README.md");
    let record = fixture.where_record();
    let central_parent = record.parent().unwrap();
    fs::write(central_parent.join("README.md"), "central copy\n").unwrap();
    let output = fixture.search("copy");
    assert!(
        !output.status.success(),
        "ambiguous source unexpectedly searched: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let text = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    assert!(
        text.contains("ambiguous"),
        "ambiguity was not diagnosed: {text}"
    );
}

#[test]
fn generated_source_root_authorizes_reads_without_rebasing_relative_locator() {
    let fixture = Fixture::new(false);
    fs::write(fixture.root.join("README.md"), "owner README text\n").unwrap();
    fixture.add_source("s.readme", "README.md");
    let other = fixture.root.parent().unwrap().join("other");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("README.md"), "unrelated README text\n").unwrap();

    let output = fixture.search_with_source_root(&other, "README");
    assert!(
        output.status.success(),
        "search: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("owner README text"),
        "owner source was not used: {text}"
    );
    assert!(
        !text.contains("unrelated README text"),
        "source-root rebased locator: {text}"
    );
}

#[test]
fn generated_source_root_does_not_supply_missing_relative_locator() {
    let fixture = Fixture::new(false);
    fixture.add_source("s.readme", "README.md");
    let other = fixture.root.parent().unwrap().join("other");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("README.md"), "unrelated README text\n").unwrap();

    let output = fixture.search_with_source_root(&other, "unrelated");
    assert!(
        output.status.success(),
        "search: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
    assert!(
        !text.contains("unrelated readme text"),
        "source-root supplied missing locator: {text}"
    );
    assert!(
        text.contains("unavailable") || text.contains("unresolved"),
        "missing owner source was not diagnosed: {text}"
    );
}

#[test]
fn linked_worktree_reads_current_branch_source_without_cross_worktree_fallback() {
    let fixture = Fixture::new(true);
    fs::write(fixture.root.join("README.md"), "main branch text\n").unwrap();
    fixture.git(&["add", "README.md"]);
    fixture.git(&["commit", "-qm", "main source"]);
    fixture.add_source("s.readme", "README.md");
    let linked = fixture.root.parent().unwrap().join("linked");
    let linked_text = git_path::argument(&linked);
    let output = fixture.git(&["worktree", "add", "-q", "-b", "linked-source", &linked_text]);
    assert!(
        output.status.success(),
        "worktree add: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::write(linked.join("README.md"), "linked branch text\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&linked)
        .args(["--json", "search", "--limit", "10", "linked"])
        .env("KPOPPER_PROJECTS_DIR", &fixture.projects)
        .env("KPOPPER_PRIVATE_HOME", &fixture.home)
        .env("HOME", &fixture.home)
        .env("XDG_STATE_HOME", fixture.home.join("state"))
        .env("KPOPPER_NATIVE_RESOURCES", &fixture.resources)
        .env("KPOPPER_NATIVE_CACHE", fixture.home.join("cache"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "linked search: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("linked branch text"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("main branch text"));
}

#[test]
fn central_record_outside_identified_owner_does_not_guess_project_relative_sources() {
    let fixture = Fixture::new(false);
    fs::write(fixture.root.join("README.md"), "owner-only text\n").unwrap();
    fixture.add_source("s.readme", "README.md");
    let record = fixture.where_record();
    let unrelated = fixture.root.parent().unwrap().join("unrelated");
    fs::create_dir_all(&unrelated).unwrap();
    fs::write(unrelated.join("README.md"), "wrong workspace text\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&unrelated)
        .args([
            "--json",
            "search",
            "--record",
            record.to_str().unwrap(),
            "--source-root",
            unrelated.to_str().unwrap(),
            "wrong",
        ])
        .env("KPOPPER_PROJECTS_DIR", &fixture.projects)
        .env("KPOPPER_PRIVATE_HOME", &fixture.home)
        .env("HOME", &fixture.home)
        .env("XDG_STATE_HOME", fixture.home.join("state"))
        .env("KPOPPER_NATIVE_RESOURCES", &fixture.resources)
        .env("KPOPPER_NATIVE_CACHE", fixture.home.join("cache"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
    assert!(
        !stdout.contains("wrong workspace text"),
        "outside-owner search guessed a workspace: {stdout}"
    );
    let text = stdout + &String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    assert!(
        text.contains("unresolved") || text.contains("owner") || text.contains("workspace"),
        "missing owner diagnostic: {text}"
    );
}

#[test]
fn copied_generated_config_has_no_project_source_context_for_read() {
    let fixture = Fixture::new(false);
    fs::write(fixture.root.join("README.md"), "original owner text\n").unwrap();
    fixture.add_source("s.readme", "README.md");
    let record = fixture.where_record();
    let copy = fixture.root.parent().unwrap().join("copied");
    fs::create_dir_all(copy.join(".kpopper")).unwrap();
    fs::copy(
        fixture.root.join(".kpopper/project.json"),
        copy.join(".kpopper/project.json"),
    )
    .unwrap();
    fs::write(copy.join("README.md"), "copied owner text\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&copy)
        .args([
            "--json",
            "search",
            "--record",
            record.to_str().unwrap(),
            "--source-root",
            copy.to_str().unwrap(),
            "copied",
        ])
        .env("KPOPPER_PROJECTS_DIR", &fixture.projects)
        .env("KPOPPER_PRIVATE_HOME", &fixture.home)
        .env("HOME", &fixture.home)
        .env("XDG_STATE_HOME", fixture.home.join("state"))
        .env("KPOPPER_NATIVE_RESOURCES", &fixture.resources)
        .env("KPOPPER_NATIVE_CACHE", fixture.home.join("cache"))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
    assert!(
        !text.contains("copied owner text"),
        "copied config supplied source context: {text}"
    );
    let text = text + &String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    assert!(
        text.contains("unavailable") || text.contains("unresolved") || text.contains("owner"),
        "copied config missing-source diagnostic absent: {text}"
    );
}

#[test]
fn explicit_legacy_record_keeps_manual_external_source_semantics_and_no_central_write() {
    let fixture = Fixture::new(false);
    fs::write(fixture.root.join("GROUNDING.yaml"), "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  s.external: {file: /tmp/legacy-external.txt}\n").unwrap();
    let before = fs::read_dir(&fixture.projects).unwrap().count();
    let output = fixture.command(&["check"]);
    assert!(
        output.status.success(),
        "legacy check changed semantics: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_dir(&fixture.projects).unwrap().count(),
        before,
        "legacy explicit record created generated central state"
    );
}

#[test]
fn legacy_source_root_does_not_rebase_manual_relative_locator() {
    let fixture = Fixture::new(false);
    let external = fixture.root.join("external");
    fs::create_dir_all(&external).unwrap();
    fs::write(fixture.root.join("README.md"), "workspace source text\n").unwrap();
    fs::write(external.join("README.md"), "requested-root source text\n").unwrap();
    fs::write(
        fixture.root.join("GROUNDING.yaml"),
        "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  s.manual: {file: README.md}\n",
    )
    .unwrap();
    let external_arg = external.to_string_lossy().into_owned();
    let output = fixture.command(&[
        "--json",
        "search",
        "--source-root",
        &external_arg,
        "workspace",
    ]);
    assert!(
        output.status.success(),
        "legacy search changed semantics: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("workspace source text"),
        "legacy locator was rebased: {text}"
    );
    assert!(
        !text.contains("requested-root source text"),
        "requested root replaced legacy base: {text}"
    );
}

//! File locators are checked against this checkout; historical locators name a Git blob.
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

struct Fixture {
    root: tempfile::TempDir,
    private: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let f = Self {
            root: tempfile::tempdir().unwrap(),
            private: tempfile::tempdir().unwrap(),
        };
        f.git(&["init", "-q"]);
        fs::create_dir(f.root.path().join("sources")).unwrap();
        fs::write(f.root.path().join("sources/original.txt"), "original").unwrap();
        f.git(&["add", "."]);
        f.git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "-qm",
            "Original source",
        ]);
        f.git(&["tag", "source-final"]);
        f
    }
    fn git(&self, args: &[&str]) {
        let mut c = Command::new("git");
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_") {
                c.env_remove(name);
            }
        }
        let o = c.args(args).current_dir(self.root.path()).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
    fn check(&self, record: &str) -> Output {
        self.check_in(record, self.root.path())
    }
    fn check_in(&self, record: &str, cwd: &Path) -> Output {
        fs::write(self.root.path().join("GROUNDING.yaml"), record).unwrap();
        self.command(cwd).output().unwrap()
    }
    fn command(&self, cwd: &Path) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kpop"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_") {
                c.env_remove(name);
            }
        }
        c.args(["--frozen", "check"])
            .current_dir(cwd)
            .env("XDG_STATE_HOME", self.private.path())
            .env("XDG_CONFIG_HOME", self.private.path())
            .env("KPOPPER_NATIVE_CACHE", self.private.path().join("cache"))
            .env_remove("KPOPPER_ROOT")
            .env_remove("KPOPPER_NATIVE_RESOURCES");
        c
    }
}
fn text(output: Output) -> String {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
#[test]
fn missing_file_is_a_note_with_a_reread_or_revision_hint() {
    let f = Fixture::new();
    let out = text(f.check("sources:\n  s.gone: {file: sources/gone.txt}\nknown:\n  p.direct: {v: 1, from: sources/gone.rs}\n"));
    assert!(out.contains("NOTE s.gone: file sources/gone.txt"), "{out}");
    assert!(out.contains("NOTE p.direct: file sources/gone.rs"), "{out}");
    assert!(
        out.contains("re-read") && out.contains("revision:path"),
        "{out}"
    );
    assert!(out.contains("0 problems"), "{out}");
}

#[test]
fn a_bare_filename_is_checked_without_mistaking_entry_ids_for_files() {
    let f = Fixture::new();
    fs::write(f.root.path().join("package.json"), "{}").unwrap();
    let out = text(f.check("sources:\n  s.report: {name: report}\n  s.actual.md: {name: source id}\nknown:\n  p.gone: {v: 1, from: pyproject.toml}\n  p.present: {v: 1, from: package.json}\n  p.ref: {v: 1, from: s.report}\n  p.file_like_id: {v: 1, from: s.actual.md}\n"));
    assert!(out.contains("NOTE p.gone: file pyproject.toml"), "{out}");
    for id in ["p.present", "p.ref", "p.file_like_id"] {
        assert!(!out.contains(&format!("NOTE {id}: file")), "{out}");
    }
}

#[test]
fn relative_sources_follow_the_primary_record_even_when_named_from_another_directory() {
    let f = Fixture::new();
    let nested = f.root.path().join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("source.txt"), "present").unwrap();
    fs::write(
        nested.join("GROUNDING.yaml"),
        "sources:\n  s.present: {file: source.txt}\n  s.missing: {file: gone.txt}\n",
    )
    .unwrap();
    for mut command in [f.command(&nested), {
        let mut c = f.command(f.root.path());
        c.arg("nested/GROUNDING.yaml");
        c
    }] {
        let out = text(command.output().unwrap());
        assert!(!out.contains("NOTE s.present:"), "{out}");
        assert!(out.contains("NOTE s.missing: file gone.txt"), "{out}");
    }
}

#[test]
fn core_check_reports_the_same_advisory_without_a_false_computation_failure() {
    let f = Fixture::new();
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    let resources = f.private.path().join("resources");
    fs::create_dir_all(resources.join("reasoning")).unwrap();
    let archive = format!("{target}.kpopper-runtime");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(&archive),
        resources.join("reasoning").join(&archive),
    )
    .unwrap();
    fs::write(f.root.path().join("GROUNDING.yaml"), "meta: {reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}}\nsources:\n  s.missing: {file: sources/no.txt}\n").unwrap();
    let output = f
        .command(f.root.path())
        .env("KPOPPER_NATIVE_RESOURCES", resources)
        .output()
        .unwrap();
    let out = text(output);
    assert!(out.contains("NOTE s.missing: file sources/no.txt"), "{out}");
    assert!(out.contains("1 nodes, 0 problems"), "{out}");
}
#[test]
fn live_sources_symbolic_references_prose_urls_and_patterns_are_not_missing_files() {
    let f = Fixture::new();
    let out = text(f.check("sources:\n  s.file: {file: sources/original.txt}\n  s.web: {file: 'https://example.test/a'}\nknown:\n  p.reference: {v: 1, from: s.file}\n  p.unknown: {v: 1, from: s.other}\n  p.prose: {v: 1, from: 'a comparison of sources/ on another machine'}\n  p.pattern: {v: 1, from: 'sources/*.txt'}\n"));
    assert!(!out.contains("file "), "{out}");
}
#[test]
fn absolute_paths_are_checked_only_inside_this_repository() {
    let f = Fixture::new();
    let inside = f.root.path().join("inside missing.txt");
    let outside = f.private.path().join("outside missing.txt");
    let out = text(f.check(&format!(
        "sources:\n  s.inside: {{file: '{}' }}\n  s.outside: {{file: '{}' }}\n",
        inside.display(),
        outside.display()
    )));
    assert!(out.contains("NOTE s.inside:"), "{out}");
    assert!(!out.contains("s.outside:"), "{out}");
}
#[test]
fn pinned_sources_remain_openable_after_the_working_file_is_deleted() {
    let f = Fixture::new();
    fs::remove_file(f.root.path().join("sources/original.txt")).unwrap();
    let out = text(f.check("sources:\n  s.old: {file: 'source-final:sources/original.txt', at: 'first line', read: '2026-09-01'}\nknown:\n  p.old: {v: 1, from: 'source-final:sources/original.txt'}\n"));
    assert!(
        !out.contains("NOTE s.old:") && !out.contains("NOTE p.old:"),
        "{out}"
    );
    f.git(&["show", "source-final:sources/original.txt"]);
}
#[test]
fn a_pin_does_not_hide_an_unknown_revision_or_a_missing_blob() {
    let f = Fixture::new();
    let out = text(f.check("sources:\n  s.ref: {file: 'no-such-ref:sources/original.txt'}\n  s.blob: {file: 'source-final:sources/missing.txt'}\n  s.tree: {file: 'source-final:sources'}\n"));
    for id in ["s.ref", "s.blob", "s.tree"] {
        assert!(out.contains(&format!("NOTE {id}: pinned file")), "{out}");
    }
    assert!(out.contains("git show"), "{out}");
}
#[test]
fn file_paths_resolve_from_the_repository_when_called_in_a_subdirectory() {
    let f = Fixture::new();
    let out = text(f.check_in(
        "sources:\n  s.live: {file: sources/original.txt}\n  s.gone: {file: sources/gone.txt}\n",
        &f.root.path().join("sources"),
    ));
    assert!(!out.contains("NOTE s.live:"), "{out}");
    assert!(out.contains("NOTE s.gone:"), "{out}");
}
#[cfg(unix)]
#[test]
fn a_symlink_leading_outside_the_repository_is_not_a_missing_local_source() {
    let f = Fixture::new();
    std::os::unix::fs::symlink(f.private.path(), f.root.path().join("external")).unwrap();
    let out = text(f.check("sources:\n  s.external: {file: external/missing.txt}\n  s.traversal: {file: ../missing-outside.txt}\n"));
    assert!(
        !out.contains("NOTE s.external:") && !out.contains("NOTE s.traversal:"),
        "{out}"
    );
}

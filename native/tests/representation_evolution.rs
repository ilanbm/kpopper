//! A live stored-to-derived transition keeps identity and immutable evidence.
use std::{collections::BTreeMap, fs, path::{Path, PathBuf}, process::{Command, Output}};

fn images(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    for item in fs::read_dir(root).unwrap() {
        let path = item.unwrap().path();
        if path.is_dir() { out.extend(images(&path)); }
        else { out.insert(path.clone(), fs::read(path).unwrap()); }
    }
    out
}

struct Fixture {
    root: tempfile::TempDir,
    runtime: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let runtime = tempfile::tempdir().unwrap();
        let target = kpop_native::reasoning_runtime::target_name().unwrap();
        fs::create_dir(runtime.path().join("reasoning")).unwrap();
        fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")),
            runtime.path().join("reasoning").join(format!("{target}.zip"))).unwrap();
        let ordinary = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM").map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap())
                .join(".cache/kpopper/lean").join(&target).join(env!("KPOP_ORDINARY_SOURCE_SHA256")));
        let destination = runtime.path().join("ordinary").join(&target);
        fs::create_dir_all(&destination).unwrap();
        for name in ["build.json", if cfg!(windows) { "epistemic-core.exe" } else { "epistemic-core" }] {
            fs::copy(ordinary.join(name), destination.join(name)).unwrap();
        }
        Self { root: tempfile::tempdir().unwrap(), runtime }
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_kpop-native"))
            .args(["--workspace", self.root.path().to_str().unwrap()]).args(args)
            .current_dir(self.root.path()).env("HOME", self.root.path())
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("KPOPPER_NATIVE_CACHE", self.runtime.path().join("cache"))
            .env("KPOPPER_NATIVE_RESOURCES", self.runtime.path()).output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(output.status.success(), "{args:?}: {}{}", String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap()
    }
}

#[test]
fn scalar_reframe_is_guarded_and_numeric_updates_drive_the_original_boolean() {
    use sha2::Digest;
    let f = Fixture::new();
    let wrong_hash = "0".repeat(64);
    for extra in [vec![], vec!["--shareability", "private"]] {
        let mut args = vec!["add", "tank.at_seven", "v=true", "--expected-record-sha256", &wrong_hash];
        args.extend(extra);
        let result = f.run(&args);
        assert!(!result.status.success());
        assert!(!f.root.path().join("GROUNDING.yaml").exists());
    }
    f.ok(&["add", "tank.at_seven", "v=true", "--as-of", "2025-01-01"]);
    f.ok(&["add", "tank.level", "v=7", "--as-of", "2025-01-02"]);
    let record = f.root.path().join("GROUNDING.yaml");
    let before = fs::read(&record).unwrap();
    let history = images(&f.root.path().join(".kpopper/history"));
    assert!(!history.is_empty());
    for rule in ["rule={expr: 'tank.level == 8'}", "rule={expr: 'tank.level'}",
                 "rule={expr: 'missing.level == 7'}", "rule={expr: 'tank.at_seven'}",
                 "rule={expr: 'true'}"] {
        let result = f.run(&["add", "tank.at_seven", rule, "--reframe", "--why", "Same proposition"]);
        assert!(!result.status.success(), "must refuse {rule}");
        assert_eq!(fs::read(&record).unwrap(), before);
    }
    assert!(!f.run(&["add", "tank.at_seven", "rule={expr: 'tank.level == 7'}", "--reframe",
        "--why", "Same proposition", "--expected-record-sha256", &wrong_hash]).status.success());
    assert_eq!(fs::read(&record).unwrap(), before);
    let expected = format!("{:x}", sha2::Sha256::digest(&before));
    f.ok(&["add", "tank.at_seven", "rule={expr: 'tank.level == 7'}", "--reframe",
           "--why", "The same proposition is derived from the measured level", "--as-of", "2025-01-02",
           "--expected-record-sha256", &expected]);
    assert!(f.ok(&["pull", "tank.at_seven"]).to_lowercase().contains("true"));
    for (path, bytes) in history { assert_eq!(fs::read(path).unwrap(), bytes); }
    f.ok(&["set", "tank.level", "8", "--as-of", "2025-01-03"]);
    assert!(f.ok(&["pull", "tank.at_seven"]).to_lowercase().contains("false"));
    f.ok(&["check"]);
}

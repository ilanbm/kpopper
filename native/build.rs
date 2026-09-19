use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path};
fn hash(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}
fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
    for item in fs::read_dir(dir).expect("reasoning sources") {
        let path = item.unwrap().path();
        if path.is_dir() {
            walk(root, &path, out);
        } else if path.extension().is_some_and(|s| s == "lean")
            && !["Proof", "Audit"].iter().any(|prefix| {
                path.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with(prefix)
            })
        {
            println!("cargo:rerun-if-changed={}", path.display());
            out.insert(
                path.strip_prefix(root)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .replace('\\', "/"),
                hash(&fs::read(path).unwrap()),
            );
        }
    }
}
fn main() {
    let ordinary_source = "../scripts/session/lean/Main.lean";
    println!("cargo:rerun-if-changed={ordinary_source}");
    println!(
        "cargo:rustc-env=KPOP_ORDINARY_SOURCE_SHA256={}",
        hash(&fs::read(ordinary_source).unwrap())
    );
    let root = Path::new("../scripts/reasoning/lean");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut sources = BTreeMap::new();
    walk(root, root, &mut sources);
    assert!(!sources.is_empty());
    println!(
        "cargo:rustc-env=KPOP_REASONING_SOURCE_SHA256={}",
        hash(&serde_json::to_vec(&sources).unwrap())
    );
    let mut adapter = BTreeMap::new();
    let mut paths = fs::read_dir("src")
        .unwrap()
        .map(|p| p.unwrap().path())
        .filter(|p| p.extension().is_some_and(|s| s == "rs"))
        .collect::<Vec<_>>();
    paths.extend(["Cargo.lock", "Cargo.toml", "build.rs"].map(Into::into));
    paths.sort();
    for path in paths {
        println!("cargo:rerun-if-changed={}", path.display());
        adapter.insert(
            path.to_str().unwrap().to_owned(),
            hash(&fs::read(&path).unwrap()),
        );
    }
    println!(
        "cargo:rustc-env=KPOP_REASONING_ADAPTER_SHA256={}",
        hash(&serde_json::to_vec(&adapter).unwrap())
    );
}

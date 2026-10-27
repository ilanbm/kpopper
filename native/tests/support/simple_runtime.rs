use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn resources(root: &Path) {
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    fs::create_dir_all(root.join("reasoning")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")),
        root.join("reasoning").join(format!("{target}.zip")),
    )
    .unwrap();
    let program = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap())
                .join(".cache/kpopper/lean")
                .join(&target)
                .join(env!("KPOP_ORDINARY_SOURCE_SHA256"))
        });
    let destination = root.join("ordinary").join(&target);
    fs::create_dir_all(&destination).unwrap();
    for filename in [
        "build.json",
        if cfg!(windows) {
            "epistemic-core.exe"
        } else {
            "epistemic-core"
        },
    ] {
        fs::copy(program.join(filename), destination.join(filename)).unwrap();
    }
}

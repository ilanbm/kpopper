use kpop_native::public_remeasure::{self, Options};
use std::{fs, path::PathBuf};

#[test]
fn plan_and_run_use_only_the_allowlisted_recipe_and_leave_record_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join(".kpopper")).unwrap();
    fs::write(root.join("GROUNDING.yaml"), "known:\n  p.a:\n    v: 1\n    measure: echo\n").unwrap();
    let recipe = root.join("recipe");
    fs::write(&recipe, b"#!/bin/sh\nprintf '1\\n'\n").unwrap();
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&recipe, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(root.join(".kpopper/measure.yaml"), "echo: [./recipe]\n").unwrap();
    let before = fs::read(root.join("GROUNDING.yaml")).unwrap();
    let record = PathBuf::from(root.join("GROUNDING.yaml"));
    let plan = public_remeasure::run(&Options { run: false, record: Some(record.clone()) }, root, true).unwrap();
    assert!(plan.contains("nothing ran - add --run"));
    let measured = public_remeasure::run(&Options { run: true, record: Some(record) }, root, true).unwrap();
    assert!(measured.contains("p.a: 1 - as recorded (echo)"));
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
}

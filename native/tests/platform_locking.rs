use kpop_native::history_transaction_fs::DirectoryGuard;
#[cfg(windows)]
use std::path::PathBuf;
use std::{fs, path::Path};

fn code<T>(result: kpop_native::Result<T>) -> Option<String> {
    result.err().map(|error| error.0)
}

#[cfg(any(unix, windows))]
#[test]
fn directory_lock_reentrancy_order_and_writer_exclusion_are_portable() {
    use std::{sync::mpsc, time::Duration};

    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let z = temp.path().join("z");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&z).unwrap();

    let writer = DirectoryGuard::acquire(&a, true).unwrap();
    let nested_reader = DirectoryGuard::acquire(&a, false).unwrap();
    drop(nested_reader);
    assert_eq!(
        code(DirectoryGuard::acquire(&a, true)),
        None,
        "an exclusive lock reenters as the same held lock"
    );

    let (started_tx, started_rx) = mpsc::channel();
    let (entered_tx, entered_rx) = mpsc::channel();
    let contender_path = a.clone();
    let contender = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let _guard = DirectoryGuard::acquire(&contender_path, true).unwrap();
        entered_tx.send(()).unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(entered_rx.recv_timeout(Duration::from_millis(150)).is_err());
    drop(writer);
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    contender.join().unwrap();

    let reader = DirectoryGuard::acquire(&a, false).unwrap();
    assert_eq!(
        code(DirectoryGuard::acquire(&a, true)).as_deref(),
        Some("lock_upgrade_refused")
    );
    drop(reader);

    let higher = DirectoryGuard::acquire(&z, true).unwrap();
    assert_eq!(
        code(DirectoryGuard::acquire(&a, true)).as_deref(),
        Some("lock_order_refused")
    );
    drop(higher);
}

#[cfg(any(unix, windows))]
#[test]
fn shared_lock_does_not_modify_the_record_directory() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("GROUNDING.yaml"), b"known: {}\n").unwrap();
    let before = tree(root);
    let guard = DirectoryGuard::acquire(root, false).unwrap();
    assert_eq!(tree(root), before);
    drop(guard);
    assert_eq!(tree(root), before);
}

#[cfg(windows)]
#[test]
fn waiting_writer_refuses_a_replaced_directory() {
    use std::{sync::mpsc, time::Duration};

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("record");
    let displaced = temp.path().join("displaced");
    fs::create_dir(&root).unwrap();
    let writer = DirectoryGuard::acquire(&root, true).unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let contender_path = root.clone();
    let contender = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        result_tx
            .send(code(DirectoryGuard::acquire(&contender_path, true)))
            .unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(result_rx.recv_timeout(Duration::from_millis(150)).is_err());
    fs::rename(&root, &displaced).unwrap();
    fs::create_dir(&root).unwrap();
    drop(writer);
    assert_eq!(
        result_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .as_deref(),
        Some("directory_replaced")
    );
    contender.join().unwrap();
}

#[cfg(windows)]
#[test]
fn windows_directory_handle_serializes_path_aliases() {
    use std::process::Command;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("RecordRoot");
    fs::create_dir(&root).unwrap();

    let differently_cased = PathBuf::from(root.to_string_lossy().to_lowercase());
    assert_alias_waits(&root, &differently_cased);

    let extended = PathBuf::from(format!(r"\\?\{}", root.display()));
    assert_alias_waits(&root, &extended);

    let junction = temp.path().join("record-junction");
    let linked = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        linked.status.success(),
        "{}",
        String::from_utf8_lossy(&linked.stderr)
    );
    assert_alias_waits(&root, &junction);

    let drive = (b'R'..=b'Z')
        .rev()
        .map(|letter| format!("{}:", letter as char))
        .find(|candidate| !Path::new(&format!(r"{}\", candidate)).exists())
        .unwrap();
    let mounted = Command::new("subst")
        .arg(&drive)
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(
        mounted.status.success(),
        "{}",
        String::from_utf8_lossy(&mounted.stderr)
    );
    assert_alias_waits(&root, &PathBuf::from(format!(r"{}\RecordRoot", drive)));
    assert!(
        Command::new("subst")
            .arg(&drive)
            .arg("/D")
            .status()
            .unwrap()
            .success()
    );
}

#[cfg(windows)]
fn assert_alias_waits(root: &Path, alias: &Path) {
    use std::{process::Command, thread, time::Duration};

    let writer = DirectoryGuard::acquire(root, true).unwrap();
    let mut contender = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "windows_alias_child", "--nocapture"])
        .env("KPOP_WINDOWS_ALIAS_CHILD", alias)
        .spawn()
        .unwrap();
    thread::sleep(Duration::from_millis(200));
    assert!(contender.try_wait().unwrap().is_none());
    drop(writer);
    assert!(contender.wait().unwrap().success());
}

#[cfg(windows)]
#[test]
fn windows_alias_child() {
    let Some(root) = std::env::var_os("KPOP_WINDOWS_ALIAS_CHILD") else {
        return;
    };
    let _guard = DirectoryGuard::acquire(Path::new(&root), true).unwrap();
}

fn tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut result = Vec::new();
    for item in fs::read_dir(root).unwrap() {
        let item = item.unwrap();
        if item.file_type().unwrap().is_file() {
            result.push((
                item.file_name().to_string_lossy().into_owned(),
                fs::read(item.path()).unwrap(),
            ));
        }
    }
    result.sort();
    result
}

use std::{fs, path::Path};

pub struct WritableCleanup(pub std::path::PathBuf);
impl Drop for WritableCleanup {
    fn drop(&mut self) {
        fn thaw(path: &Path) {
            use std::os::unix::fs::PermissionsExt;
            let Ok(meta) = fs::symlink_metadata(path) else {
                return;
            };
            if meta.file_type().is_symlink() {
                return;
            }
            if meta.is_dir() {
                let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
                if let Ok(entries) = fs::read_dir(path) {
                    for entry in entries.flatten() {
                        thaw(&entry.path());
                    }
                }
            }
        }
        // Only this test's newly created temp tree is made removable.
        thaw(&self.0);
    }
}

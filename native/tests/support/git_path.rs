use std::path::Path;

pub fn argument(path: &Path) -> String {
    let path = path.to_str().unwrap();
    #[cfg(windows)]
    {
        if let Some(path) = path.strip_prefix(r"\\?\UNC\") {
            format!("//{}", path.replace('\\', "/"))
        } else {
            path.strip_prefix(r"\\?\").unwrap_or(path).replace('\\', "/")
        }
    }
    #[cfg(not(windows))]
    path.into()
}

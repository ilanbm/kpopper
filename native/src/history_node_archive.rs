//! Deterministic, lossless archive for copy-only history migration.
use crate::{
    json_ingress, require,
    value::{TypedValue, MAX_DEPTH},
    Error, Result,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Cursor, Read, Write},
    path::{Component, Path},
};
use zip::{write::SimpleFileOptions, CompressionMethod, DateTime, ZipArchive, ZipWriter};

const MAX_ARCHIVE_BYTES: usize = 100 * 1024 * 1024;
const MAX_JSON_BYTES: usize = 96 * 1024 * 1024;
const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const MAX_ENCODED_FILE_BYTES: usize = MAX_FILE_BYTES.div_ceil(3) * 4;
const MAX_FILES: usize = 64 * 1024;
const MAX_FILES_BYTES: usize = 64 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 4096;
const MAX_PATHS_BYTES: usize = 4 * 1024 * 1024;
const MAX_METADATA_STRING_BYTES: usize = 1024 * 1024;
const MAX_METADATA_STRINGS_BYTES: usize = 4 * 1024 * 1024;
const ARCHIVE_MEMBER: &str = "archive.json";
const FORMAT_VERSION: u64 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Archive {
    files: BTreeMap<String, Vec<u8>>,
    metadata: TypedValue,
}

impl Archive {
    pub fn new(files: BTreeMap<String, Vec<u8>>, metadata: TypedValue) -> Result<Self> {
        let archive = Self { files, metadata };
        archive.json_bytes()?;
        Ok(archive)
    }

    pub fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }

    pub fn metadata(&self) -> &TypedValue {
        &self.metadata
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let json = self.json_bytes()?;
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(DateTime::default())
            .unix_permissions(0o100644);
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(ARCHIVE_MEMBER, options)
            .map_err(|_| invalid_archive())?;
        writer.write_all(&json)?;
        let bytes = writer.finish().map_err(|_| invalid_archive())?.into_inner();
        require(bytes.len() <= MAX_ARCHIVE_BYTES, "history_archive_limit")?;
        Ok(bytes)
    }

    pub fn decode(raw: &[u8]) -> Result<Self> {
        require(
            !raw.is_empty() && raw.len() <= MAX_ARCHIVE_BYTES,
            "history_archive_limit",
        )?;
        let mut zip = ZipArchive::new(Cursor::new(raw)).map_err(|_| invalid_archive())?;
        require(zip.len() == 1, "invalid_history_archive")?;
        let mut member = zip.by_index(0).map_err(|_| invalid_archive())?;
        let mode = member.unix_mode();
        require(
            member.name() == ARCHIVE_MEMBER
                && !member.is_dir()
                && !member.is_symlink()
                && mode.is_none_or(|mode| mode & 0o170000 == 0 || mode & 0o170000 == 0o100000)
                && member.compression() == CompressionMethod::Deflated
                && member
                    .last_modified()
                    .is_some_and(|time| time == DateTime::default())
                && member.size() <= MAX_JSON_BYTES as u64,
            "invalid_history_archive",
        )?;
        let mut json = Vec::with_capacity(member.size() as usize);
        (&mut member)
            .take((MAX_JSON_BYTES + 1) as u64)
            .read_to_end(&mut json)?;
        require(json.len() <= MAX_JSON_BYTES, "history_archive_limit")?;
        drop(member);
        drop(zip);

        let value = json_ingress::parse_slice_bounded(
            &json,
            json_ingress::DuplicateKeys::Reject,
            MAX_DEPTH * 2 + 8,
        )
        .map_err(|_| invalid_archive())?;
        let archive = Self::from_json_value(value)?;
        require(archive.json_bytes()? == json, "invalid_history_archive")?;
        require(archive.encode()? == raw, "invalid_history_archive")?;
        Ok(archive)
    }

    pub fn reconstruct(&self) -> Result<tempfile::TempDir> {
        let root = tempfile::tempdir()?;
        for (relative, bytes) in &self.files {
            let path = safe_target(root.path(), relative)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
            file.write_all(bytes)?;
        }
        Ok(root)
    }

    fn json_bytes(&self) -> Result<Vec<u8>> {
        validate_files(&self.files)?;
        validate_metadata(&self.metadata)?;
        let encoded_files = self
            .files
            .iter()
            .map(|(path, bytes)| (path.clone(), Value::String(STANDARD.encode(bytes))))
            .collect::<Map<_, _>>();
        let value = json!({
            "files": encoded_files,
            "metadata": self.metadata.to_tagged()?,
            "version": FORMAT_VERSION,
        });
        let bytes = serde_json::to_vec(&value)?;
        require(bytes.len() <= MAX_JSON_BYTES, "history_archive_limit")?;
        Ok(bytes)
    }

    fn from_json_value(value: Value) -> Result<Self> {
        let Value::Object(mut object) = value else {
            return Err(invalid_archive());
        };
        require(
            object.len() == 3
                && object.contains_key("version")
                && object.contains_key("files")
                && object.contains_key("metadata"),
            "invalid_history_archive",
        )?;
        require(
            object.remove("version").and_then(|value| value.as_u64()) == Some(FORMAT_VERSION),
            "invalid_history_archive",
        )?;
        let encoded_files = object
            .remove("files")
            .and_then(|value| match value {
                Value::Object(files) => Some(files),
                _ => None,
            })
            .ok_or_else(invalid_archive)?;
        require(encoded_files.len() <= MAX_FILES, "history_archive_limit")?;
        let mut files = BTreeMap::new();
        for (path, value) in encoded_files {
            let encoded = value.as_str().ok_or_else(invalid_archive)?;
            require(
                encoded.len() <= MAX_ENCODED_FILE_BYTES,
                "history_archive_limit",
            )?;
            let bytes = STANDARD.decode(encoded).map_err(|_| invalid_archive())?;
            require(
                bytes.len() <= MAX_FILE_BYTES && STANDARD.encode(&bytes) == encoded,
                "invalid_history_archive",
            )?;
            files.insert(path, bytes);
        }
        let tagged_metadata = object.remove("metadata").ok_or_else(invalid_archive)?;
        let metadata = TypedValue::from_tagged(&tagged_metadata).map_err(|_| invalid_archive())?;
        require(
            metadata.to_tagged().map_err(|_| invalid_archive())? == tagged_metadata,
            "invalid_history_archive",
        )?;
        Self::new(files, metadata)
    }
}

fn validate_files(files: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    require(files.len() <= MAX_FILES, "history_archive_limit")?;
    let (mut total_bytes, mut total_path_bytes) = (0usize, 0usize);
    for (path, bytes) in files {
        validate_path(path)?;
        total_path_bytes = total_path_bytes.saturating_add(path.len());
        total_bytes = total_bytes.saturating_add(bytes.len());
        require(
            bytes.len() <= MAX_FILE_BYTES
                && total_bytes <= MAX_FILES_BYTES
                && total_path_bytes <= MAX_PATHS_BYTES,
            "history_archive_limit",
        )?;
    }
    for path in files.keys() {
        let mut ancestors = Path::new(path).ancestors();
        ancestors.next();
        for ancestor in ancestors {
            if ancestor.as_os_str().is_empty() {
                continue;
            }
            if let Some(ancestor) = ancestor.to_str() {
                require(
                    !files.contains_key(ancestor),
                    "invalid_history_archive_path",
                )?;
            }
        }
    }
    Ok(())
}

fn validate_path(path: &str) -> Result<()> {
    require(
        !path.is_empty()
            && path.len() <= MAX_PATH_BYTES
            && !path.starts_with('/')
            && !path.contains(['\\', '\0', ':', '<', '>', '"', '|', '?', '*'])
            && !path.chars().any(char::is_control),
        "invalid_history_archive_path",
    )?;
    for segment in path.split('/') {
        require(
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && !segment.eq_ignore_ascii_case(".git")
                && segment.len() <= 255
                && !segment.ends_with(' ')
                && !segment.ends_with('.'),
            "invalid_history_archive_path",
        )?;
        let stem = segment
            .split('.')
            .next()
            .unwrap_or(segment)
            .to_ascii_uppercase();
        require(
            !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                && !matches!(
                    stem.as_str(),
                    "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6" | "COM7" | "COM8" | "COM9"
                )
                && !matches!(
                    stem.as_str(),
                    "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9"
                ),
            "invalid_history_archive_path",
        )?;
    }
    require(
        Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_))),
        "invalid_history_archive_path",
    )
}

fn validate_metadata(metadata: &TypedValue) -> Result<()> {
    metadata.validate()?;
    let mut pending = vec![metadata];
    let mut strings_bytes = 0usize;
    while let Some(value) = pending.pop() {
        match value {
            TypedValue::Text(text) => add_metadata_string(text, &mut strings_bytes)?,
            TypedValue::Integer(integer) => {
                add_metadata_string(integer.as_str(), &mut strings_bytes)?;
            }
            TypedValue::Date(date) => add_metadata_string(date.as_str(), &mut strings_bytes)?,
            TypedValue::DateTime(date_time) => {
                add_metadata_string(date_time.as_str(), &mut strings_bytes)?;
            }
            TypedValue::List(values) => pending.extend(values),
            TypedValue::Map(values) => {
                for (key, value) in values {
                    add_metadata_string(key, &mut strings_bytes)?;
                    pending.push(value);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn add_metadata_string(value: &str, total: &mut usize) -> Result<()> {
    require(
        value.len() <= MAX_METADATA_STRING_BYTES,
        "history_archive_limit",
    )?;
    *total = total.saturating_add(value.len());
    require(
        *total <= MAX_METADATA_STRINGS_BYTES,
        "history_archive_limit",
    )
}

fn safe_target(root: &Path, relative: &str) -> Result<std::path::PathBuf> {
    validate_path(relative)?;
    let mut target = root.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(part) = component else {
            return Err(Error("invalid_history_archive_path".into()));
        };
        target.push(part);
    }
    Ok(target)
}

fn invalid_archive() -> Error {
    Error("invalid_history_archive".into())
}

//! Template-bound evidence for physical hypotheses retained after history import.
use crate::{
    Result, history_authority as A, history_capture::Layout, history_contract::*,
    history_view::list, require, value::TypedValue as V,
};

pub const MAPPING_KEY: &str = "history_hypothesis_import";

pub(crate) fn directory(entry: &str) -> Result<String> {
    Layout::for_entry(entry)?;
    Ok(if entry == "GROUNDING.yaml" {
        ".kpopper/hypotheses"
    } else {
        "PROVENANCE.d"
    }
    .into())
}

pub fn validate_mapping(value: &V, entry: &str) -> Result<()> {
    crate::history_yaml::validate_value(value, crate::reasoning_snapshot::MAX_REQUEST_BYTES)?;
    let m = schema(value, &["version", "physical"], &[])?;
    require(
        is_int(&m["version"], "1"),
        "invalid_hypothesis_import_mapping",
    )?;
    let directory = directory(entry)?;
    let mut previous: Option<&str> = None;
    for item in list(&m["physical"])? {
        let item = schema(item, &["name", "path", "sha256"], &[])?;
        let name = text(&item["name"])?;
        hypothesis_name(&item["name"])?;
        let path = text(&item["path"])?;
        A::relative_path(path)?;
        require(
            path == format!("{directory}/{name}.yaml") || path == format!("{directory}/{name}.yml"),
            "invalid_hypothesis_import_path",
        )?;
        require(
            text(&item["sha256"])?.len() == 64
                && crate::history_paths::object_id(text(&item["sha256"])?),
            "invalid_identifier",
        )?;
        require(
            previous.is_none_or(|p| p < name),
            "duplicate_hypothesis_import",
        )?;
        previous = Some(name);
    }
    Ok(())
}

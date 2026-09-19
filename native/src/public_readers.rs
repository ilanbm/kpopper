//! Source capture and public read command routing.
use crate::{
    Result,
    history_contract::*,
    public_core_readers as C, public_workspace as W,
    reasoning_context::CapturedAssessment,
    reasoning_runtime::{OperationalBounds, Runtime},
    source_capture::{self, ReadMode},
    source_inventory::{Inventory, absolute},
};
use serde_json::{Value as J, json};
use std::path::{Path, PathBuf};
#[derive(Clone, Debug, Default, clap::Args)]
pub struct Options {
    pub subjects: Vec<String>,
    #[arg(long,value_parser=["core/v1"])]
    pub profile: Option<String>,
    #[arg(long, allow_negative_numbers = true)]
    pub budget: Option<i64>,
    #[arg(long)]
    pub chars: Option<i64>,
    #[arg(long,value_parser=["claude","codex"],hide=true)]
    pub host: Option<String>,
    #[arg(long)]
    pub history: bool,
}
fn files(options: &Options, cwd: &Path, command: &str) -> Result<(Vec<PathBuf>, Vec<String>)> {
    let mut paths = vec![];
    let mut seeds = vec![];
    for s in &options.subjects {
        let filename = if ["pull", "affects"].contains(&command) {
            s.clone()
        } else {
            s.to_lowercase()
        };
        if filename.ends_with(".yaml") || filename.ends_with(".yml") {
            let expanded = if let Some(s) = s.strip_prefix("~/") {
                PathBuf::from(
                    std::env::var_os("HOME").ok_or_else(|| error("home_directory_unavailable"))?,
                )
                .join(s)
            } else {
                cwd.join(s)
            };
            let found = crate::source_inventory::glob(&expanded)?;
            if found.is_empty() {
                paths.push(absolute(&expanded)?);
            } else {
                paths.extend(found);
            }
        } else {
            seeds.push(s.clone());
        }
    }
    if paths.is_empty() {
        paths = W::records(cwd)?;
    }
    Ok((paths, seeds))
}
fn mapping(location: &W::Location, inventory: &mut Inventory) -> Result<Option<J>> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
        .ok_or_else(|| error("home_directory_unavailable"))?;
    let path = state
        .join("kpopper/first-use/projects")
        .join(&location.key)
        .join("mapping.json");
    if !inventory.exists(&path)? {
        return Ok(None);
    }
    let value: J = serde_json::from_slice(&inventory.read(&path)?).map_err(|_| {
        error(&format!(
            "Invalid first-use state; keep it for inspection: {}",
            path.display()
        ))
    })?;
    crate::require(
        value.is_object() && (value["schema"] == 1 || value["schema"] == true),
        &format!(
            "Invalid first-use state; keep it for inspection: {}",
            path.display()
        ),
    )?;
    crate::require(
        matches!(value["mode"].as_str(), Some("map" | "deep"))
            && matches!(
                value["mapping"].as_str(),
                Some("ready" | "running" | "complete" | "failed")
            ),
        "Invalid mapping task state",
    )?;
    crate::require(
        value["request"].as_str().is_some_and(|s| {
            s.len() == 32
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        }),
        "Invalid mapping request ID",
    )?;
    crate::require(
        value["owner"].as_str().is_some_and(|s| {
            !s.is_empty()
                && s.len() <= 200
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        }),
        "Invalid mapping owner",
    )?;
    Ok(Some(J::Object(
        ["request", "mapping", "mode", "report", "check", "error"]
            .iter()
            .filter_map(|k| value.get(*k).map(|v| ((*k).into(), v.clone())))
            .collect(),
    )))
}
fn private_draft_count(paths: &[PathBuf], cwd: &Path, inventory: &mut Inventory) -> Result<usize> {
    let project = crate::project_modes::project_for(paths, cwd)?;
    let key = crate::identity::sha256(
        project
            .common
            .as_ref()
            .unwrap_or(&project.root)
            .to_string_lossy()
            .as_bytes(),
    );
    let home = std::env::var_os("KPOPPER_PRIVATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share/kpopper/private"))
        })
        .ok_or_else(|| error("home_directory_unavailable"))?;
    let paths = inventory.glob(&home.join(key).join("*.json"))?;
    let mut count = 0;
    for path in paths {
        if inventory.file(&path)? {
            count += 1;
        }
    }
    Ok(count)
}
fn brief(paths: &[PathBuf], cwd: &Path, inventory: &mut Inventory) -> Result<Option<Vec<u8>>> {
    let first = paths.first().ok_or_else(|| error("record_required"))?;
    let candidates = if first.file_name().is_some_and(|n| n == "GROUNDING.yaml") {
        vec![first.parent().unwrap().join(".kpopper/view.yaml")]
    } else {
        paths
            .iter()
            .cloned()
            .chain([cwd.join("PROVENANCE.yaml")])
            .map(|p| {
                let name = p.to_string_lossy();
                let stem = name
                    .strip_suffix(".yaml")
                    .or_else(|| name.strip_suffix(".yml"))
                    .unwrap_or(&name);
                PathBuf::from(format!("{stem}.view.yaml"))
            })
            .collect()
    };
    for path in candidates {
        if inventory.exists(&path)? {
            return inventory.read(&path).map(Some);
        }
    }
    Ok(None)
}
fn replaced(
    paths: &[PathBuf],
    inventory: &mut Inventory,
) -> Result<(Option<crate::value::TypedValue>, String)> {
    let first = paths.first().ok_or_else(|| error("record_required"))?;
    let path = if first
        .file_name()
        .is_some_and(|name| name == "GROUNDING.yaml")
    {
        first.parent().unwrap().join(".kpopper/replaced.yaml")
    } else {
        let name = first.to_string_lossy();
        let stem = name
            .strip_suffix(".yaml")
            .or_else(|| name.strip_suffix(".yml"))
            .unwrap_or(&name);
        PathBuf::from(format!("{stem}.replaced.yaml"))
    };
    let relative = path
        .strip_prefix(first.parent().unwrap())
        .unwrap_or(&path)
        .to_string_lossy()
        .to_string();
    if !inventory.exists(&path)? || !inventory.file(&path)? {
        return Ok((None, relative));
    }
    let value = crate::history_yaml::decode_document(&inventory.read(&path)?)?;
    Ok((Some(value), relative))
}
fn prefix_order(source: &crate::history_yaml::SourceValue) -> Vec<String> {
    let Some(crate::history_yaml::SourceValue::Map(prefixes)) =
        source.get("meta").and_then(|meta| meta.get("prefixes"))
    else {
        return vec![];
    };
    prefixes.iter().map(|(key, _)| key.clone()).collect()
}
pub fn run(
    command: &str,
    options: &Options,
    cwd: &Path,
    mode: ReadMode,
    as_json: bool,
    runtime: Option<&Runtime>,
) -> Result<C::Output> {
    let cwd = cwd.canonicalize()?;
    let location = W::locate(&cwd, mode)?;
    let (paths, seeds) = files(options, &cwd, command)?;
    let mut inventory = Inventory::default();
    let mut data =
        json!({"workspace":location.workspace,"record":location.record,"status":location.status});
    if command == "open" {
        crate::require(
            seeds.is_empty(),
            "Record filenames must have a .yaml or .yml extension.",
        )?;
        if !options.subjects.is_empty() {
            data["record"] = json!(paths[0]);
            data["status"] = json!(if paths[0].is_file() {
                "found"
            } else {
                "unavailable"
            });
        }
        match mapping(&location, &mut inventory) {
            Ok(Some(v)) => data["mapping"] = v,
            Err(e) => data["warning"] = json!(e.0),
            _ => {}
        }
        if data["status"] == "unavailable" || data["status"] == "missing" {
            let unavailable = data["status"] == "unavailable";
            let mut message = if unavailable {
                if options.subjects.is_empty() {
                    location.reason.clone()
                } else {
                    format!(
                        "The requested record is unavailable: {}",
                        paths[0].display()
                    )
                }
            } else {
                "No knowledge record yet. Continue your work and keep useful findings as they arise, or run `kpop map` for an initial map (`--deep` for a deeper investigation).".into()
            };
            if !unavailable && let Some(state) = data["mapping"]["mapping"].as_str() {
                message.push_str(&format!("\nMapping: {state}"));
            }
            data[if unavailable { "error" } else { "message" }] = json!(message);
            inventory.verify()?;
            crate::require(
                W::locate(&cwd, mode)? == location,
                "workspace changed while opening it; retry",
            )?;
            return Ok(C::Output {
                text: if as_json {
                    serde_json::to_string_pretty(&data)? + "\n"
                } else {
                    message + "\n"
                },
                code: i32::from(unavailable),
            });
        }
    }
    let capture = source_capture::capture_source_with_runtime(&paths, &cwd, mode, None, runtime)?;
    let capabilities =
        crate::reasoning_fields::capabilities(&capture.document(), options.profile.as_deref())?;
    if !string_is(&map(&capabilities)?["profile"], "core/v1") {
        let context = capture.ordinary_context();
        let conflicts = map(&map(&context)?["conflicts"])?;
        let mut knowledge = capture.reader_lines()?;
        if mode == ReadMode::Live {
            let drafts = private_draft_count(&paths, &cwd, &mut inventory)?;
            if drafts > 0 {
                knowledge.push(format!(
                    "{drafts} private drafts retained; inspect `kpop knowledge status`"
                ));
            }
        }
        let projection = crate::public_ordinary_readers::Projection::new(
            &capture.document(),
            map(capture.hypotheses())?,
            conflicts,
            knowledge,
            runtime,
        )?;
        let page = brief(&paths, &cwd, &mut inventory)?
            .map(|bytes| crate::history_yaml::decode_document(&bytes))
            .transpose()?;
        let prefix_order = prefix_order(capture.source());
        let output = match command {
            "open" => {
                projection.opening(options.budget.unwrap_or(25), page.as_ref(), &prefix_order)?
            }
            "check" => {
                let (text, code) = projection.check(page.as_ref())?;
                capture.verify()?;
                inventory.verify()?;
                crate::require(
                    W::locate(&cwd, mode)? == location,
                    "workspace changed while reading it; retry",
                )?;
                return Ok(C::Output { text, code });
            }
            "pull" => {
                if options.history {
                    let (history, path) = replaced(&paths, &mut inventory)?;
                    projection.history(history.as_ref(), &path, &seeds)?
                } else {
                    projection.pull(&seeds, options.budget.unwrap_or(40))?
                }
            }
            "affects" => projection.affects(&seeds)?,
            _ => return Err(error("unknown ordinary reader command")),
        };
        capture.verify()?;
        inventory.verify()?;
        crate::require(
            W::locate(&cwd, mode)? == location,
            "workspace changed while reading it; retry",
        )?;
        return Ok(C::Output {
            text: output,
            code: 0,
        });
    }
    let unsupported = [
        ("--chars", options.chars.is_some()),
        ("--budget", options.budget.is_some()),
        ("--host", options.host.is_some()),
    ]
    .into_iter()
    .filter(|(_, v)| *v)
    .map(|(k, _)| k)
    .collect::<Vec<_>>();
    crate::require(
        unsupported.is_empty(),
        &format!(
            "core_profile_option_unsupported: {}",
            unsupported.join(", ")
        ),
    )?;
    crate::require(
        !options.history,
        "core_profile_option_unsupported: --history; core pull already includes captured history",
    )?;
    let context = CapturedAssessment::from_snapshot(
        capture.snapshot().clone(),
        None,
        "focused-review/v1",
        runtime,
        OperationalBounds::default(),
        None,
    )?;
    let output = match command {
        "open" => {
            let (data, output) = C::opening(
                &context,
                data,
                paths[0]
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("record"),
            )?;
            C::Output {
                text: if as_json {
                    serde_json::to_string_pretty(&data)? + "\n"
                } else {
                    output + "\n"
                },
                code: 0,
            }
        }
        "pull" => C::pull(&context, &seeds)?,
        "affects" => C::affects(&context, &seeds)?,
        "check" => {
            let page = brief(&paths, &cwd, &mut inventory)
                .and_then(|b| crate::core_page::project(&context, b.as_deref(), true));
            C::check(&context, page)?
        }
        _ => return Err(error("unknown read command")),
    };
    capture.verify()?;
    inventory.verify()?;
    crate::require(
        W::locate(&cwd, mode)? == location,
        "workspace changed while reading it; retry",
    )?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn brief_absence_bytes_and_private_file_type_are_revalidated() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let paths = vec![root.join("GROUNDING.yaml")];
        let mut absent = Inventory::default();
        assert!(brief(&paths, &root, &mut absent).unwrap().is_none());
        std::fs::create_dir(root.join(".kpopper")).unwrap();
        std::fs::write(root.join(".kpopper/view.yaml"), "title: first\n").unwrap();
        assert!(absent.verify().is_err());
        let mut captured = Inventory::default();
        assert_eq!(
            brief(&paths, &root, &mut captured).unwrap().unwrap(),
            b"title: first\n"
        );
        std::fs::write(root.join(".kpopper/view.yaml"), "title: changed\n").unwrap();
        assert!(captured.verify().is_err());
        let path = root.join("draft.json");
        std::fs::write(&path, b"secret").unwrap();
        let mut draft = Inventory::default();
        assert!(draft.file(&path).unwrap());
        assert!(draft.files.is_empty());
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(draft.verify().is_err());
    }
}

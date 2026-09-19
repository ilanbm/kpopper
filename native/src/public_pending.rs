//! Read-only public inspection of the local contribution ledger and cached
//! publication observations. Status never contacts or mutates a remote.
use crate::{
    Result,
    history_contract::{map, string_is},
    history_view::map_mut,
    pending_state::Observation,
    project_modes::Project,
    value::TypedValue as V,
};
use clap::Subcommand;
use std::{collections::BTreeMap, path::Path};

fn s(value: &str) -> V {
    V::Text(value.into())
}
fn object(items: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(
        items
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    )
}

#[derive(Clone, Debug, clap::Args)]
pub struct Options {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    /// Inspect local capture and cached publication state without remote reads.
    Status(StatusOptions),
}

#[derive(Clone, Debug, Default, clap::Args)]
pub struct StatusOptions {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

pub fn status(workspace: &Path, _options: &StatusOptions) -> Result<V> {
    let project = Project::open(workspace)?;
    if !project.is_git() {
        return Ok(object([
            ("mode", s("simple")),
            ("states", V::Map(BTreeMap::new())),
            ("verified", V::Bool(false)),
            ("publication", V::Null),
            ("reason", s("Simple uses the shared project record")),
        ]));
    }
    let config = project.config()?;
    let observation = Observation::capture(&project, &config)?;
    let publication = map(&config)?
        .get("publication")
        .filter(|value| **value != V::Null)
        .cloned();
    let authority = object([
        ("configured", V::Bool(publication.is_some())),
        (
            "standing_permission",
            V::Bool(
                publication
                    .as_ref()
                    .and_then(|value| map(value).ok())
                    .and_then(|fields| fields.get("standing_permission"))
                    == Some(&V::Bool(true)),
            ),
        ),
        ("scope", publication.unwrap_or(V::Null)),
        (
            "actions",
            V::List(
                [
                    "push_managed_branch",
                    "create_pull_request",
                    "update_pull_request",
                ]
                .into_iter()
                .map(s)
                .collect(),
            ),
        ),
    ]);
    let mut result = observation.publication;
    let fields = map_mut(&mut result)?;
    fields.insert("authority".into(), authority);
    fields.insert(
        "mode".into(),
        if string_is(&map(&config)?["mode"], "simple") {
            s("simple")
        } else {
            s("advanced")
        },
    );
    Ok(result)
}

pub fn run(options: &Options, workspace: &Path) -> Result<V> {
    match &options.command {
        Command::Status(status_options) => status(workspace, status_options),
    }
}

/// CLI boundary matching `pending_cli`: status is structured by default while
/// failures use stderr unless the global JSON switch was supplied.
pub fn dispatch(options: &Options, workspace: &Path, as_json: bool) -> CommandOutput {
    match run(options, workspace) {
        Ok(value) => match value
            .to_json()
            .and_then(|value| serde_json::to_string_pretty(&value).map_err(Into::into))
        {
            Ok(stdout) => CommandOutput {
                stdout: stdout + "\n",
                stderr: String::new(),
                code: 0,
            },
            Err(error) => refusal(error, as_json),
        },
        Err(error) => refusal(error, as_json),
    }
}

fn refusal(error: crate::Error, as_json: bool) -> CommandOutput {
    if as_json {
        CommandOutput {
            stdout: serde_json::json!({"error":error.to_string()}).to_string() + "\n",
            stderr: String::new(),
            code: 2,
        }
    } else {
        CommandOutput {
            stdout: String::new(),
            stderr: format!("kpop pending: {error}\n"),
            code: 2,
        }
    }
}

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
    /// Reconcile and attempt one authorized publication cycle.
    Publish(PublishOptions),
    #[command(name = "_publish", hide = true)]
    AutomaticPublish,
    /// Retain an exact local publication scope and permission.
    Configure(ConfigureOptions),
    /// Pause local publication attempts.
    Pause(ControlOptions),
    /// Resume local publication attempts and optionally selected revisions.
    Resume(ControlOptions),
    /// Withdraw selected immutable revisions locally.
    Withdraw(ControlOptions),
    /// Reject selected immutable revisions locally.
    Reject(ControlOptions),
    /// Supersede selected revisions with another captured revision.
    Supersede(SupersedeOptions),
    /// Clear local publication backoff state.
    Retry(ControlOptions),
}

#[derive(Clone, Debug, Default, clap::Args)]
pub struct StatusOptions {
    /// Also check the pending revisions against the publication remote.
    #[arg(long)]
    pub verify: bool,
}

#[derive(Clone, Debug, Default, clap::Args)]
pub struct PublishOptions {
    /// Permit this one cycle to push and open or update the pull request without standing permission.
    #[arg(long)]
    pub authorize: bool,
    /// Attempt now even while a failure backoff is in effect.
    #[arg(long)]
    pub retry: bool,
}

#[derive(Clone, Debug, Default, clap::Args)]
pub struct ConfigureOptions {
    /// Git remote to publish through; required the first time, then kept.
    #[arg(long)]
    pub remote: Option<String>,
    /// Branch the pull request targets; required the first time, then kept.
    #[arg(long)]
    pub target: Option<String>,
    /// Managed branch that carries the pending contributions; defaults to pending_grounding.
    #[arg(long)]
    pub branch: Option<String>,
    /// Grant standing permission to publish to this scope without --authorize.
    #[arg(long, conflicts_with = "revoke")]
    pub grant: bool,
    /// Revoke standing publication permission.
    #[arg(long, conflicts_with = "grant")]
    pub revoke: bool,
}

#[derive(Clone, Debug, Default, clap::Args)]
pub struct ControlOptions {
    /// Captured immutable revisions the decision applies to; withdraw and reject need at least one.
    pub revisions: Vec<String>,
    /// Why, kept with the decision; withdraw and reject need one.
    #[arg(long, default_value = "")]
    pub reason: String,
}

#[derive(Clone, Debug, clap::Args)]
pub struct SupersedeOptions {
    /// Captured immutable revisions to supersede; at least one.
    pub revisions: Vec<String>,
    /// Why, kept with the decision; required.
    #[arg(long, default_value = "")]
    pub reason: String,
    /// A different captured revision that replaces them.
    #[arg(long)]
    pub replacement: String,
}

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
    if _options.verify {
        fields.insert(
            "verification".into(),
            crate::pending_publication::verify(project)?,
        );
    }
    Ok(result)
}

pub fn status_with_provider<P: crate::publication_provider::Provider>(
    workspace: &Path,
    options: &StatusOptions,
    provider: &mut P,
    clock: fn() -> f64,
) -> Result<V> {
    let project = Project::open(workspace)?;
    let mut result = status(workspace, &StatusOptions { verify: false })?;
    if options.verify && project.is_git() {
        map_mut(&mut result)?.insert(
            "verification".into(),
            crate::pending_publication::Publisher::with_clock(project, provider, clock).verify()?,
        );
    }
    Ok(result)
}

pub fn publish_with_provider<P: crate::publication_provider::Provider>(
    workspace: &Path,
    options: &PublishOptions,
    provider: &mut P,
    clock: fn() -> f64,
) -> Result<V> {
    crate::pending_publication::Publisher::with_clock(Project::open(workspace)?, provider, clock)
        .run(options.authorize, options.retry)
}

pub fn run(options: &Options, workspace: &Path) -> Result<V> {
    match &options.command {
        Command::Status(status_options) => status(workspace, status_options),
        Command::Publish(options) => crate::pending_publication::publish(
            Project::open(workspace)?,
            options.authorize,
            options.retry,
        ),
        Command::AutomaticPublish => crate::pending_publication::publish_automatic(Project::open(workspace)?),
        Command::Configure(options) => {
            let project = Project::open(workspace)?;
            crate::pending_control::configure(
                &project,
                &crate::pending_control::Configure {
                    remote: options.remote.as_deref(),
                    target: options.target.as_deref(),
                    branch: options.branch.as_deref(),
                    grant: options.grant,
                    revoke: options.revoke,
                },
            )
        }
        Command::Pause(options) => {
            let project = Project::open(workspace)?;
            crate::pending_control::decision(&project, "pause", &options.revisions, &options.reason)
        }
        Command::Resume(options) => {
            let project = Project::open(workspace)?;
            crate::pending_control::decision(
                &project,
                "resume",
                &options.revisions,
                &options.reason,
            )
        }
        Command::Withdraw(options) => {
            let project = Project::open(workspace)?;
            crate::pending_control::action(
                &project,
                "withdraw",
                &options.revisions,
                &options.reason,
                None,
            )
        }
        Command::Reject(options) => {
            let project = Project::open(workspace)?;
            crate::pending_control::action(
                &project,
                "reject",
                &options.revisions,
                &options.reason,
                None,
            )
        }
        Command::Supersede(options) => {
            let project = Project::open(workspace)?;
            crate::pending_control::action(
                &project,
                "supersede",
                &options.revisions,
                &options.reason,
                Some(&options.replacement),
            )
        }
        Command::Retry(options) => {
            let project = Project::open(workspace)?;
            crate::pending_control::action(
                &project,
                "retry",
                &options.revisions,
                &options.reason,
                None,
            )
        }
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

//! Project-local onboarding for the shared findings Board. Read-only discovery
//! is separate from an explicit, destination-bound publication grant.
use crate::{
    Error, Result, onboarding, pending_control, project_modes::Project,
    publication_provider::GitHubProvider, require, value::TypedValue,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, clap::Args)]
pub struct Options {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Clone, Debug, clap::Subcommand)]
pub enum Command {
    /// Inspect local Board configuration without contacting a remote.
    Status,
    /// Export this runtime's bundled onboarding illustration for the host to display.
    Illustration,
    /// Inspect repository access and its target branch without granting permission.
    Inspect {
        #[arg(long)]
        remote: Option<String>,
        #[arg(long)]
        target: Option<String>,
    },
    /// Connect the exact destination approved by the project owner.
    Connect {
        #[arg(long)]
        remote: String,
        #[arg(long)]
        repository: String,
        #[arg(long)]
        target: String,
        #[arg(long)]
        generation: u64,
        #[arg(long)]
        grant: bool,
    },
    /// Keep findings local and revoke any standing publication permission.
    Local,
    /// Remember that the onboarding offer was actually displayed.
    #[command(hide = true)]
    Shown {
        /// Acknowledge the Simple/Advanced offer rather than the Board offer.
        #[arg(long)]
        mode: bool,
    },
}

fn git(project: &Project, args: &[&str]) -> Result<String> {
    let (ok, raw) = pending_control::git(&project.root, args)?;
    require(ok, "Board repository discovery is unavailable")?;
    String::from_utf8(raw).map_err(|_| Error("nonportable repository destination".into()))
}
fn repository(project: &Project, remote: &str) -> Result<String> {
    require(
        git(project, &["remote"])?.lines().any(|s| s == remote),
        "unknown Board remote",
    )?;
    let raw = git(project, &["remote", "get-url", "--push", "--all", remote])?;
    let urls = raw.lines().collect::<Vec<_>>();
    require(urls.len() == 1, "Board needs exactly one push destination")?;
    Ok(urls[0].into())
}
fn preference(project: &Project) -> Result<Value> {
    let value = onboarding::read(&project.state.join("board.json"))?.unwrap_or(json!({}));
    require(
        value.get("choice").is_none_or(|v| v == "local"),
        "invalid Board choice",
    )?;
    Ok(value)
}
fn illustration() -> Option<String> {
    if let Ok(exe) = std::env::current_exe() {
        for dir in exe.ancestors().skip(1).take(6) {
            let image = dir.join("assets/diagrams/two-working-modes.png");
            if image.is_file() {
                return Some(image.to_string_lossy().into_owned());
            }
        }
    }
    None
}
fn export_illustration() -> Result<Value> {
    use std::io::Write;
    let bytes = include_bytes!("../shared/board.png");
    let digest = crate::identity::sha256(bytes);
    let dir = onboarding::state_dir()?.join("illustrations");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{digest}.png"));
    if std::fs::read(&path).ok().as_deref() != Some(bytes.as_slice()) {
        let mut file = tempfile::NamedTempFile::new_in(&dir)?;
        file.write_all(bytes)?;
        file.persist(&path)
            .map_err(|e| Error(e.error.to_string()))?;
    }
    Ok(json!({"illustration":path.canonicalize()?,"sha256":digest,"source":"bundled"}))
}

pub fn status(workspace: &Path) -> Result<Value> {
    let project = Project::open(workspace)?;
    let config = project.config()?.to_json()?;
    let advanced = project.is_git() && config["mode"] == "advanced";
    let saved = preference(&project)?;
    let mode_selected = project.config_path.is_file() || !advanced;
    let guidance = onboarding::guidance()?;
    let mode_offer = advanced && !mode_selected && saved["mode_shown"] != true && guidance;
    let cached = pending_control::load_state(&project.state.join("publication.json"))?.to_json()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| Error("clock unavailable".into()))?
        .as_secs_f64();
    let retry_state = if cached["paused"] == true {
        "paused"
    } else if cached["receipts"]
        .as_array()
        .and_then(|r| r.last())
        .is_some_and(|r| r["event"] == "attention")
    {
        "needs_attention"
    } else if cached["failures"].as_u64().unwrap_or(0) > 0 {
        if now < cached["retry_at"].as_f64().unwrap_or(0.0) {
            "waiting_to_retry"
        } else {
            "retry_due"
        }
    } else {
        "ready"
    };
    let publication = &config["publication"];
    let granted = publication["standing_permission"] == true;
    let chosen = if granted {
        json!("shared")
    } else if saved["choice"] == "local" {
        json!("local")
    } else {
        Value::Null
    };
    let mut remotes = Vec::new();
    if advanced {
        for remote in git(&project, &["remote"])?.lines() {
            remotes.push(match repository(&project, remote) {
                Ok(url) if GitHubProvider::new(&url).identity().is_ok() => json!({"remote":remote,"repository":url}),
                Ok(_) => json!({"remote":remote,"unavailable":"Select a supported GitHub destination without embedded credentials."}),
                Err(e) => json!({"remote":remote,"unavailable":e.0}),
            });
        }
    }
    Ok(
        json!({"name":"kpopper Board","mode":config["mode"],"is_git":project.is_git(),"generation":config["generation"],
        "choice":chosen,"offer":advanced && mode_selected && publication.is_null() && chosen.is_null() && saved["shown"] != true && guidance,
        "mode_offer":mode_offer,"mode_selected":mode_selected,"advanced_experimental":true,
        "record":project.record(None)?,"simple_record_suggestion":simple_record_suggestion(&project),
        "standing_permission":granted,"publication":publication,"remotes":remotes,
        "paused":cached["paused"],"pr":cached["pr"],"failures":cached["failures"],"retry_at":cached["retry_at"],
        "retry_state":retry_state,
        "illustration":illustration(),"illustration_command":["board","illustration"],"state_path":project.state.join("board.json"),
        "reading_scope":"Pending findings are read across local worktrees without merging; remote publication is a separate review proposal."}),
    )
}

fn simple_record_suggestion(project: &Project) -> Option<PathBuf> {
    let common = project.common.as_ref()?;
    if common.file_name()? != ".git" {
        return None;
    }
    let primary = common.parent()?;
    let name = primary.file_name()?.to_str()?;
    // A sibling of the primary checkout is stable across ephemeral worktrees.
    Some(
        primary
            .parent()?
            .join(format!("{name}-knowledge"))
            .join("GROUNDING.yaml"),
    )
}

fn inspect(project: &Project, remote: Option<&str>, target: Option<&str>) -> Result<Value> {
    let config = project.config()?.to_json()?;
    require(
        project.is_git() && config["mode"] == "advanced",
        "Board publication needs Advanced mode; preserve the existing project mode",
    )?;
    let remotes = git(project, &["remote"])?;
    let names = remotes.lines().collect::<Vec<_>>();
    let remote = remote.or_else(|| config["publication"]["remote"].as_str())
        .or_else(|| (names.len() == 1).then(|| names[0]))
        .ok_or_else(|| Error("select a Board remote before offering publication; the destination is ambiguous or missing".into()))?;
    let url = repository(project, remote)?;
    let target = target.or_else(|| config["publication"]["target"].as_str());
    let destination = GitHubProvider::new(&url).inspect_destination(target)?;
    require(
        repository(project, remote)? == url && project.config()?.to_json()? == config,
        "Board destination or project policy changed; inspect it again",
    )?;
    Ok(
        json!({"remote":remote,"destination":destination,"generation":config["generation"],
        "illustration":illustration(),"permission_changed":false,"publication_verified":false}),
    )
}

fn remember(project: &Project, local: bool) -> Result<()> {
    require(project.is_git(), "Board choices belong to a Git project")?;
    require(
        !local || project.config_path.is_file(),
        "choose Simple or Advanced explicitly before setting Board publication; board local does not select a mode",
    )?;
    let policy = project.lock()?;
    let previous = preference(project)?;
    let mut config = policy.config().to_json()?;
    if local
        && (previous["choice"] != "local" || config["publication"]["standing_permission"] == true)
    {
        if config["publication"].is_object() {
            config["publication"]["standing_permission"] = json!(false);
        }
        config["generation"] = json!(
            config["generation"]
                .as_u64()
                .and_then(|n| n.checked_add(1))
                .ok_or_else(|| Error("invalid project generation".into()))?
        );
        policy.verify()?;
        pending_control::save(&project.config_path, &TypedValue::from_json(&config)?)?;
    } else {
        policy.verify()?;
    }
    let mut next = previous.clone();
    next["shown"] = json!(true);
    if local || previous["choice"] == "local" {
        next["choice"] = json!("local");
    }
    onboarding::write(&project.state.join("board.json"), &next)
}

pub fn run(options: &Options, workspace: &Path) -> Result<Value> {
    let project = Project::open(workspace)?;
    match &options.command {
        None | Some(Command::Status) => status(workspace),
        Some(Command::Illustration) => export_illustration(),
        Some(Command::Inspect { remote, target }) => {
            inspect(&project, remote.as_deref(), target.as_deref())
        }
        Some(Command::Shown { mode }) => {
            if *mode {
                let policy = project.lock()?;
                let mut saved = preference(&project)?;
                saved["mode_shown"] = json!(true);
                policy.verify()?;
                onboarding::write(&project.state.join("board.json"), &saved)?;
            } else {
                remember(&project, false)?;
            }
            status(workspace)
        }
        Some(Command::Local) => {
            remember(&project, true)?;
            status(workspace)
        }
        Some(Command::Connect {
            remote,
            repository: expected,
            target,
            generation,
            grant,
        }) => {
            require(
                *grant,
                "Board connect requires --grant after the owner approves this repository and target",
            )?;
            let config = project.config()?.to_json()?;
            require(
                config["mode"] == "advanced"
                    && repository(&project, remote)? == *expected
                    && config["generation"] == *generation,
                "Board destination or project policy changed; inspect it again before connecting",
            )?;
            let inspected = inspect(&project, Some(remote), Some(target))?;
            require(
                inspected["destination"]["can_push"] == true,
                "Board write access could not be verified; no publication permission was saved",
            )?;
            let configured = pending_control::configure_expected(
                &project,
                &pending_control::Configure {
                    remote: Some(remote),
                    target: Some(target),
                    branch: None,
                    grant: true,
                    revoke: false,
                },
                Some((expected, *generation)),
            )?
            .to_json()?;
            let mut result = status(workspace)?;
            require(
                result["publication"] == configured["publication"]
                    && result["standing_permission"] == true,
                "Board configuration changed before readback; inspect its current state",
            )?;
            result["connection_verified"] = json!(true);
            result["destination"] = inspected["destination"].clone();
            // Connection evidence does not establish a completed publication;
            // the asynchronous child may still be running when this returns.
            result["publication_verified"] = json!(false);
            result["publication_attempt"] =
                crate::pending_publication::trigger_after_capture(&project).to_json()?;
            Ok(result)
        }
    }
}

pub fn opening(state: &Value) -> Result<Option<String>> {
    if state["mode_offer"] == true {
        return Ok(Some(format!(
            "KPOPPER_MODE_OFFER {}\nAt the first suitable moment in meaningful user work, offer Simple (recommended) or Advanced (experimental), using the existing README illustration and the user's language. Simple uses one shared record outside all Git checkouts; show the suggested path and let the user choose another. Advanced keeps GROUNDING.yaml with each branch and supports kpopper Board, but merging can cause record conflicts and repeated CI. Explain local consolidation/resolution without promising automatic resolution of disputed claims. Read the start guide for the exact safe setup and migration flow. A Git default is not a saved user choice. After actually showing the mode offer run `board shown --mode`; silence changes no mode or permissions. Keep doing the user's work and never offer on a greeting, during urgent work or in background tasks.",
            serde_json::to_string(state)?
        )));
    }
    if state["offer"] != true {
        if state["standing_permission"] == true && state["retry_state"] != "ready" {
            return Ok(Some(format!(
                "KPOPPER_BOARD_STATUS {}\nFindings remain durable locally. Transient failures retry on later active sessions after the bounded delay; pending retry requests an earlier attempt. Respect a deliberate pause. Mention this status only when it affects the requested work or needs a decision, not as a repeated setup offer.",
                serde_json::to_string(
                    &json!({"state":state["retry_state"],"retry_at":state["retry_at"],"failures":state["failures"],"pr":state["pr"]})
                )?
            )));
        }
        return Ok(None);
    }
    Ok(Some(format!(
        "KPOPPER_BOARD_OFFER {}\nAdvanced is experimental. At the first suitable moment in meaningful user work, offer a shared kpopper Board or Keep Board local. Read `kpop board inspect` for the exact repository, target and access before offering remote publication; select a remote explicitly when ambiguous. Use the existing README illustration supplied above, with enlargement, and the user's language. Explain branch-specific knowledge, shared proposed findings and automatic updates to one knowledge PR. Show the exact destination. Choosing shared authorizes `board connect --grant` for that destination; no second branch/PR approval. Choosing local runs `board local`; this keeps Advanced and does not select Simple. After actually showing this card run `board shown`; silence grants nothing. Never offer on a greeting, in urgent work, or in background tasks. The start guide has the full wording and protocol.",
        serde_json::to_string(&state)?
    )))
}

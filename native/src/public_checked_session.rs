//! Public core session service. Follow-up reads reuse retained findings.
use crate::{
    Result,
    checked_session::{ContextDirection, ContextOptions},
    checked_session_store::CheckedSessionStore,
    history_contract::*,
    project_modes::{self, Project},
    public_workspace as W,
    reasoning_context::CapturedAssessment,
    reasoning_runtime::OperationalBounds,
    source_capture::{self, ReadMode},
    source_inventory::Inventory,
    tokenizer::Encoding,
    value::TypedValue as V,
};
use serde_json::Value as J;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, clap::ValueEnum)]
pub enum Operation {
    Open,
    Read,
    Context,
    Serve,
}

#[derive(Clone, Debug, clap::Args)]
pub struct Options {
    #[arg(value_enum)]
    pub operation: Operation,
    #[arg(long)]
    pub input: Option<PathBuf>,
    #[arg(long)]
    pub no_settings: bool,
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long)]
    pub state: Option<PathBuf>,
    #[arg(long)]
    pub profile: Option<PathBuf>,
    #[arg(long, value_parser = ["checked-reader/v1", "core/v1"])]
    pub assessment_profile: Option<String>,
    #[arg(long, value_enum, default_value = "o200k_base")]
    pub encoding: Encoding,
    #[arg(long)]
    pub tokens: Option<usize>,
    #[arg(long = "ref")]
    pub reference: Option<String>,
    #[arg(long)]
    pub revision: Option<String>,
    #[arg(long)]
    pub offset: Option<usize>,
    #[arg(long = "id")]
    pub ids: Vec<String>,
    #[arg(long, value_enum)]
    pub direction: Option<ContextDirection>,
    #[arg(long, default_value_t = 1)]
    pub depth: usize,
    #[arg(long, default_value_t = 16)]
    pub max_nodes: usize,
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| error("home_directory_unavailable"))
}
fn path(cwd: &Path, input: &Path) -> Result<PathBuf> {
    let expanded = if let Ok(tail) = input.strip_prefix("~") {
        home()?.join(tail)
    } else {
        cwd.join(input)
    };
    project_modes::resolved(&expanded)
}
fn json_file(inventory: &mut Inventory, path: &Path) -> Result<J> {
    Ok(serde_json::from_slice(&inventory.read(path)?)?)
}
fn settings_file(inventory: &mut Inventory, path: &Path) -> Result<J> {
    if !inventory.exists(path)? {
        return Ok(J::Object(Default::default()));
    }
    let value = json_file(inventory, path)?;
    crate::require(
        value.is_object() && value["schema"] == 1 && value["enabled"].is_boolean(),
        "invalid checked-session settings",
    )?;
    if value["enabled"] == true {
        crate::require(
            value["python"]
                .as_str()
                .is_some_and(|s| Path::new(s).is_absolute()),
            "session settings need an absolute Python executable",
        )?;
        crate::require(
            value["tokens"]
                .as_u64()
                .is_some_and(|n| (64..=65_536).contains(&n)),
            "session token budget must be 64..65536",
        )?;
    }
    Ok(value)
}
fn settings(inventory: &mut Inventory, directory: &Path, cwd: &Path) -> Result<J> {
    if std::env::var("KPOPPER_SESSION_DISABLE").as_deref() == Ok("1") {
        return Ok(serde_json::json!({"enabled":false}));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or(home()?.join(".config"))
        .join("kpopper");
    let local =
        if let Some(p) = std::env::var_os("KPOPPER_SESSION_CONFIG").filter(|p| !p.is_empty()) {
            path(cwd, Path::new(&p))?
        } else if let Some(common) = Project::open(directory)?.common {
            common.join("kpopper-session.json")
        } else {
            base.join("projects").join(format!(
                "{}.json",
                crate::identity::sha256(directory.to_string_lossy().as_bytes())
            ))
        };
    let value = settings_file(inventory, &local)?;
    if value.as_object().is_some_and(|m| !m.is_empty()) {
        Ok(value)
    } else {
        settings_file(inventory, &base.join("session.json"))
    }
}

pub struct Service {
    cwd: PathBuf,
    input: PathBuf,
    mode: ReadMode,
    store: CheckedSessionStore,
    inputs: Inventory,
}
impl Service {
    pub fn new(options: &Options, cwd: &Path, mode: ReadMode) -> Result<Self> {
        let cwd = project_modes::resolved(cwd)?;
        let input = match &options.input {
            Some(p) => path(&cwd, p)?,
            None => W::records(&cwd)?
                .into_iter()
                .next()
                .ok_or_else(|| error("record_required"))?,
        };
        let mut inputs = Inventory::default();
        let settings_directory = if options.input.is_some() {
            input.parent().unwrap_or(&cwd)
        } else {
            &cwd
        };
        let config = if options.no_settings {
            J::Object(Default::default())
        } else {
            settings(&mut inputs, settings_directory, &cwd)?
        };
        let name = options
            .project
            .clone()
            .or_else(|| config["project"].as_str().map(str::to_owned))
            .unwrap_or_else(|| {
                let name = input
                    .parent()
                    .and_then(Path::file_name)
                    .unwrap_or_default()
                    .to_string_lossy();
                let clean: String = name
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || "._-".contains(c) {
                            c
                        } else {
                            '-'
                        }
                    })
                    .collect::<String>()
                    .trim_matches(['-', '.', '_'])
                    .chars()
                    .take(70)
                    .collect();
                if clean.is_empty() {
                    "project".into()
                } else {
                    clean
                }
            });
        let requested_state = options
            .state
            .clone()
            .or_else(|| config["state"].as_str().map(PathBuf::from));
        let state = if let Some(state) = requested_state {
            path(&cwd, &state)?
        } else {
            let base = std::env::var_os("XDG_STATE_HOME")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .unwrap_or(home()?.join(".local/state"));
            crate::require(base.is_absolute(), "XDG_STATE_HOME must be absolute")?;
            base.join("kpopper").join(crate::identity::sha256(
                format!("{name}\0{}", input.display()).as_bytes(),
            ))
        };
        let profile = options
            .profile
            .clone()
            .or_else(|| config["profile"].as_str().map(PathBuf::from));
        let profile = match profile {
            Some(p) => Some(path(&cwd, &p)?),
            None => {
                let candidate = input.parent().unwrap_or(&cwd).join(
                    if input.file_name().is_some_and(|n| n == "PROVENANCE.yaml") {
                        "PROVENANCE.session.json"
                    } else {
                        ".kpopper/session.json"
                    },
                );
                inputs.file(&candidate)?.then_some(candidate)
            }
        };
        let navigation = profile
            .as_ref()
            .map(|p| json_file(&mut inputs, p).and_then(|v| V::from_json(&v)))
            .transpose()?;
        // The legacy checked-reader route has its own assessment contract.
        crate::require(
            options.assessment_profile.as_deref() != Some("checked-reader/v1"),
            "unsupported_capability: native checked-reader/v1 sessions are not yet available",
        )?;
        let store = CheckedSessionStore::open(&state, &name, &input, navigation, options.encoding)?;
        inputs.verify()?;
        Ok(Self {
            cwd,
            input,
            mode,
            store,
            inputs,
        })
    }

    pub fn opening(&self, tokens: usize) -> Result<String> {
        crate::require((64..=65_536).contains(&tokens), "tokens must be 64..65536")?;
        let runtime = W::core_runtime()?;
        let capture = source_capture::capture_source_with_runtime(
            std::slice::from_ref(&self.input),
            &self.cwd,
            self.mode,
            None,
            runtime.as_ref(),
        )?;
        let capabilities = crate::reasoning_fields::capabilities(&capture.document(), None)?;
        crate::require(
            string_is(&map(&capabilities)?["profile"], "core/v1"),
            "unsupported_capability: native sessions currently require a core/v1 record",
        )?;
        let context = CapturedAssessment::from_snapshot(
            capture.snapshot().clone(),
            None,
            "focused-review/v1",
            runtime.as_ref(),
            OperationalBounds::default(),
            None,
        )?;
        let revision = self.store.save(&context, capture.snapshot())?;
        let session = self.store.load(&revision, capture.snapshot())?;
        let result = session
            .opening(tokens, |s| self.store.encoding().count(s))?
            .text;
        capture.verify()?;
        self.inputs.verify()?;
        Ok(result)
    }

    pub fn reading(
        &self,
        reference: &str,
        revision: &str,
        tokens: usize,
        offset: Option<usize>,
    ) -> Result<String> {
        crate::require((64..=65_536).contains(&tokens), "tokens must be 64..65536")?;
        // Source recapture validates freshness; it never recomputes findings.
        let capture = source_capture::capture_source(
            std::slice::from_ref(&self.input),
            &self.cwd,
            self.mode,
            None,
        )?;
        let session = self.store.load(revision, capture.snapshot())?;
        let result = session.read(reference, revision, tokens, offset, |s| {
            self.store.encoding().count(s)
        })?;
        capture.verify()?;
        self.inputs.verify()?;
        Ok(result)
    }
    pub fn contextualizing(
        &self,
        ids: &[String],
        revision: &str,
        options: &ContextOptions,
    ) -> Result<String> {
        let capture = source_capture::capture_source(
            std::slice::from_ref(&self.input),
            &self.cwd,
            self.mode,
            None,
        )?;
        let session = self.store.load(revision, capture.snapshot())?;
        let result =
            session.contextualize(ids, revision, options, |s| self.store.encoding().count(s))?;
        capture.verify()?;
        self.inputs.verify()?;
        Ok(result)
    }
}

pub fn run(options: &Options, cwd: &Path, mode: ReadMode) -> Result<String> {
    let service = Service::new(options, cwd, mode)?;
    match options.operation {
        Operation::Open => service.opening(options.tokens.unwrap_or(700)),
        Operation::Read => service.reading(
            options
                .reference
                .as_deref()
                .ok_or_else(|| error("read requires --ref and --revision from open"))?,
            options
                .revision
                .as_deref()
                .ok_or_else(|| error("read requires --ref and --revision from open"))?,
            options.tokens.unwrap_or(1600),
            options.offset,
        ),
        Operation::Context => service.contextualizing(
            &options.ids,
            options.revision.as_deref().ok_or_else(|| {
                error("context requires --revision and --direction support|impact")
            })?,
            &ContextOptions {
                direction: options.direction.ok_or_else(|| {
                    error("context requires --revision and --direction support|impact")
                })?,
                tokens: options.tokens.unwrap_or(2000),
                depth: options.depth,
                max_nodes: options.max_nodes,
            },
        ),
        Operation::Serve => {
            let tools = crate::session_mcp::declared_tools()
                .into_iter()
                .filter(|t| {
                    ["kpopper_open", "kpopper_read", "kpopper_context"].contains(&t.name.as_str())
                })
                .collect::<Vec<_>>();
            crate::session_mcp::server(
                &mut std::io::stdin().lock(),
                &mut std::io::stdout().lock(),
                &tools,
                |name, args| {
                    let token_count = |default| -> Result<usize> {
                        let n = match args.get("tokens") {
                            Some(n) => n
                                .as_u64()
                                .ok_or_else(|| error("tokens must be 64..65536"))?,
                            None => default,
                        };
                        crate::require((64..=65_536).contains(&n), "tokens must be 64..65536")?;
                        Ok(n as usize)
                    };
                    (|| match name {
                        "kpopper_open" => service.opening(token_count(700)?),
                        "kpopper_read" => {
                            let offset = match args.get("offset") {
                                None | Some(J::Null) => None,
                                Some(n) => Some(
                                    usize::try_from(
                                        n.as_u64().ok_or_else(|| error("invalid offset"))?,
                                    )
                                    .map_err(|_| error("invalid offset"))?,
                                ),
                            };
                            service.reading(
                                args["ref"].as_str().ok_or_else(|| error("missing ref"))?,
                                args["revision"]
                                    .as_str()
                                    .ok_or_else(|| error("missing revision"))?,
                                token_count(1600)?,
                                offset,
                            )
                        }
                        "kpopper_context" => {
                            let number = |key: &str, default| -> Result<usize> {
                                args.get(key).map_or(Ok(default), |v| {
                                    v.as_u64()
                                        .and_then(|n| usize::try_from(n).ok())
                                        .ok_or_else(|| error(&format!("invalid {key}")))
                                })
                            };
                            let ids = args["ids"]
                                .as_array()
                                .ok_or_else(|| error("missing IDs"))?
                                .iter()
                                .map(|v| {
                                    v.as_str()
                                        .map(str::to_owned)
                                        .ok_or_else(|| error("invalid ID"))
                                })
                                .collect::<Result<Vec<_>>>()?;
                            let direction = match args["direction"].as_str() {
                                Some("support") => ContextDirection::Support,
                                Some("impact") => ContextDirection::Impact,
                                _ => {
                                    return Err(error(
                                        "context direction must be support or impact",
                                    ));
                                }
                            };
                            service.contextualizing(
                                &ids,
                                args["revision"]
                                    .as_str()
                                    .ok_or_else(|| error("missing revision"))?,
                                &ContextOptions {
                                    direction,
                                    tokens: token_count(2000)?,
                                    depth: number("depth", 1)?,
                                    max_nodes: number("max_nodes", 16)?,
                                },
                            )
                        }
                        _ => Err(error("unknown tool")),
                    })()
                    .map_err(|e| e.0)
                },
            )
            .map_err(|e| error(&format!("MCP transport: {e:?}")))?;
            Ok(String::new())
        }
    }
}

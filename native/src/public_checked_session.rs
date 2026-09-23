//! Public core session service. Follow-up reads reuse retained findings.
use crate::{
    Result,
    checked_session::{CheckedSession, ContextDirection, ContextOptions},
    checked_session_store::{CheckedSessionStore, ProposalRequest},
    history_contract::*,
    project_modes, public_workspace as W,
    reasoning_context::CapturedAssessment,
    reasoning_runtime::OperationalBounds,
    session_embeddings::E5Index,
    session_search::{SearchMode, SearchRequest},
    source_capture::{self, ReadMode},
    source_inventory::Inventory,
    tokenizer::Encoding,
    value::TypedValue as V,
};
use serde_json::Value as J;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, clap::ValueEnum)]
pub enum Operation {
    Setup,
    Status,
    Enable,
    Disable,
    Open,
    HookOpen,
    Read,
    Context,
    Search,
    Propose,
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
    #[arg(long = "global")]
    pub global_scope: bool,
    #[arg(long)]
    pub rebuild: bool,
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
    #[arg(long, default_value = "")]
    pub query: String,
    #[arg(long, default_value_t = 8)]
    pub limit: usize,
    #[arg(long)]
    pub branch: Option<String>,
    #[arg(long, value_enum, default_value = "hybrid")]
    pub search_mode: SearchMode,
    #[arg(long)]
    pub cursor: Option<String>,
    /// Optional directory containing the exact pinned local E5 assets.
    #[arg(long)]
    pub embedding_dir: Option<PathBuf>,
    #[arg(long, value_parser = ["observed", "inferred", "assumed", "question"])]
    pub kind: Option<String>,
    #[arg(long)]
    pub text: Option<String>,
    #[arg(long)]
    pub basis: Vec<String>,
    #[arg(long, default_value = "")]
    pub revisit: String,
}

/// Read graph context without first opening a transport session.
#[derive(Clone, Debug, clap::Args)]
pub struct ContextCommand {
    /// Known record IDs or node:ID handles.
    #[arg(value_name = "ID", required = true, num_args = 1..=8)]
    pub ids: Vec<String>,
    #[arg(long)]
    pub input: Option<PathBuf>,
    #[arg(long)]
    pub no_settings: bool,
    #[arg(long)]
    pub project: Option<String>,
    /// Writable private cache for checked reads and proposals.
    #[arg(long)]
    pub state: Option<PathBuf>,
    #[arg(long)]
    pub profile: Option<PathBuf>,
    #[arg(long, value_parser = ["checked-reader/v1", "core/v1"])]
    pub assessment_profile: Option<String>,
    #[arg(long, value_enum, default_value = "o200k_base")]
    pub encoding: Encoding,
    #[arg(long, default_value_t = 2000)]
    pub tokens: usize,
    /// Require this exact record revision; stale revisions refuse.
    #[arg(long)]
    pub revision: Option<String>,
    /// Support follows premises and sources; impact follows dependents.
    #[arg(long, value_enum, default_value = "support")]
    pub direction: ContextDirection,
    #[arg(long, default_value_t = 1)]
    pub depth: usize,
    #[arg(long, default_value_t = 16)]
    pub max_nodes: usize,
}

impl ContextCommand {
    fn session_options(&self) -> Options {
        Options {
            operation: Operation::Context,
            input: self.input.clone(),
            no_settings: self.no_settings,
            global_scope: false,
            rebuild: false,
            project: self.project.clone(),
            state: self.state.clone(),
            profile: self.profile.clone(),
            assessment_profile: self.assessment_profile.clone(),
            encoding: self.encoding,
            tokens: Some(self.tokens),
            reference: None,
            revision: self.revision.clone(),
            offset: None,
            ids: self.ids.clone(),
            direction: Some(self.direction),
            depth: self.depth,
            max_nodes: self.max_nodes,
            query: String::new(),
            limit: 8,
            branch: None,
            search_mode: SearchMode::Hybrid,
            cursor: None,
            embedding_dir: None,
            kind: None,
            text: None,
            basis: vec![],
            revisit: String::new(),
        }
    }
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
    Ok(crate::json_ingress::parse_slice(
        &inventory.read(path)?,
        crate::json_ingress::DuplicateKeys::LastWins,
    )?)
}
fn settings(inventory: &mut Inventory, directory: &Path, cwd: &Path) -> Result<J> {
    crate::session_settings::current(inventory, directory, cwd)
}

pub struct Service {
    cwd: PathBuf,
    input: PathBuf,
    mode: ReadMode,
    store: CheckedSessionStore,
    inputs: Inventory,
    project: String,
    navigation: Option<V>,
    navigation_source: Option<Vec<u8>>,
    profile_path: Option<PathBuf>,
    requested_profile: Option<String>,
    state: PathBuf,
    semantic: Option<E5Index>,
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
        // JSON ingress above remains authoritative. Retain source order only
        // for ordinary read-directory presentation, using captured bytes.
        let navigation_source = profile.as_ref().map(|path| inputs.files[path].clone());
        let profile_path = profile.clone();
        let store =
            CheckedSessionStore::open(&state, &name, &input, navigation.clone(), options.encoding)?;
        let semantic =
            options
                .embedding_dir
                .as_ref()
                .map(|directory| match path(&cwd, directory) {
                    Ok(directory) => E5Index::new(directory),
                    Err(_) => E5Index::unavailable("embedding directory could not be resolved"),
                });
        inputs.verify()?;
        Ok(Self {
            cwd,
            input,
            mode,
            store,
            inputs,
            project: name,
            navigation,
            navigation_source,
            profile_path,
            requested_profile: options.assessment_profile.clone(),
            state,
            semantic,
        })
    }

    fn shell_quote(value: &Path) -> String {
        let text = value.to_string_lossy();
        format!("'{}'", text.replace('\'', "'\\''"))
    }

    pub fn hook_open(&self, budget: usize) -> Result<String> {
        let executable = std::env::current_exe()?.canonicalize()?;
        let mut command = vec![
            Self::shell_quote(&executable),
            "session".into(),
            "read".into(),
            "--no-settings".into(),
            "--input".into(),
            Self::shell_quote(&self.input),
            "--project".into(),
            Self::shell_quote(Path::new(&self.project)),
            "--state".into(),
            Self::shell_quote(&self.state),
        ];
        let ordinary = self.ordinary()?;
        if !ordinary {
            command.extend(["--assessment-profile".into(), "core/v1".into()]);
        }
        if let Some(profile) = &self.profile_path {
            command.extend(["--profile".into(), Self::shell_quote(profile)]);
        }
        let hint = format!(
            "Read via MCP kpopper_read, or (POSIX shell): {} --ref REF --revision REV_FROM_ABOVE\n",
            command.join(" ")
        );
        let hint_tokens = self.store.encoding().count(&hint);
        let mut available = budget.saturating_sub(hint_tokens + 1);
        for _ in 0..3 {
            if available < 64 {
                break;
            }
            let result = self.opening(available)? + &hint;
            if self.store.encoding().count(&result) <= budget {
                return Ok(result);
            }
            let overflow = self.store.encoding().count(&result) - budget;
            available = available.saturating_sub(overflow);
        }
        Err(error(
            "hook budget cannot carry the complete view and read route",
        ))
    }

    fn ordinary(&self) -> Result<bool> {
        if let Some(profile) = &self.requested_profile {
            return Ok(profile == "checked-reader/v1");
        }
        let capture = source_capture::capture_source(
            std::slice::from_ref(&self.input),
            &self.cwd,
            self.mode,
            None,
        )?;
        let capabilities =
            crate::reasoning_fields::capabilities(capture.ordinary_document(), None)?;
        let ordinary = !string_is(&map(&capabilities)?["profile"], "core/v1");
        capture.verify()?;
        Ok(ordinary)
    }

    fn ordinary_session(
        &self,
    ) -> Result<(
        source_capture::CapturedSource,
        crate::reasoning_runtime::Runtime,
        crate::ordinary_checked_session::OrdinarySession,
    )> {
        let runtime = W::runtime()?
            .ok_or_else(|| error("checked session core is not ready; run kpop session setup"))?;
        let capture = source_capture::capture_source_with_runtime(
            std::slice::from_ref(&self.input),
            &self.cwd,
            self.mode,
            None,
            Some(&runtime),
        )?;
        let capabilities =
            crate::reasoning_fields::capabilities(capture.ordinary_document(), None)?;
        crate::require(
            string_is(&map(&capabilities)?["profile"], "ordinary-reader/v1"),
            "checked-reader/v1 requires an ordinary record",
        )?;
        let navigation_source = self
            .navigation_source
            .as_deref()
            .map(crate::history_yaml::decode_ordinary_source_value)
            .transpose()?;
        let session = crate::ordinary_checked_session::OrdinarySession::capture(
            &capture,
            &runtime,
            &self.project,
            self.navigation.as_ref(),
        )?
        .with_navigation_order(navigation_source.as_ref())
        .with_proposals(self.store.proposals()?)?;
        capture.verify()?;
        Ok((capture, runtime, session))
    }

    pub fn opening(&self, tokens: usize) -> Result<String> {
        crate::require((64..=65_536).contains(&tokens), "tokens must be 64..65536")?;
        if self.ordinary()? {
            let (capture, runtime, session) = self.ordinary_session()?;
            let opening = session.opening(tokens, |s| self.store.encoding().count(s))?;
            let program = runtime
                .ordinary_program()
                .ok_or_else(|| error("ordinary expression program is not configured"))?;
            session.guard_opening(&opening, program)?;
            capture.verify()?;
            self.inputs.verify()?;
            return Ok(opening.text);
        }
        let (capture, session) = self.capture_core_session()?;
        let result = session
            .with_proposals(self.store.proposals()?)?
            .opening(tokens, |s| self.store.encoding().count(s))?
            .text;
        capture.verify()?;
        self.inputs.verify()?;
        Ok(result)
    }

    fn capture_core_session(&self) -> Result<(source_capture::CapturedSource, CheckedSession)> {
        let runtime = W::core_runtime()?;
        let capture = source_capture::capture_source_with_runtime(
            std::slice::from_ref(&self.input),
            &self.cwd,
            self.mode,
            None,
            runtime.as_ref(),
        )?;
        let capabilities =
            crate::reasoning_fields::capabilities(capture.ordinary_document(), None)?;
        crate::require(
            string_is(&map(&capabilities)?["profile"], "core/v1"),
            "unsupported_capability: native sessions currently require a core/v1 record",
        )?;
        let context = CapturedAssessment::from_snapshot(
            capture.snapshot()?.clone(),
            None,
            "focused-review/v1",
            runtime.as_ref(),
            OperationalBounds::default(),
            None,
        )?;
        let revision = self.store.save(&context, capture.snapshot()?)?;
        let session = self.store.load(&revision, capture.snapshot()?)?;
        Ok((capture, session))
    }

    pub fn reading(
        &self,
        reference: &str,
        revision: &str,
        tokens: usize,
        offset: Option<usize>,
    ) -> Result<String> {
        crate::require((64..=65_536).contains(&tokens), "tokens must be 64..65536")?;
        if self.ordinary()? {
            let (capture, _, session) = self.ordinary_session()?;
            let result = session.read(reference, revision, tokens, offset, |s| {
                self.store.encoding().count(s)
            })?;
            capture.verify()?;
            self.inputs.verify()?;
            return Ok(result);
        }
        // Source recapture validates freshness; it never recomputes findings.
        let capture = source_capture::capture_source(
            std::slice::from_ref(&self.input),
            &self.cwd,
            self.mode,
            None,
        )?;
        let session = self.store.load(revision, capture.snapshot()?)?;
        let base = reference.split('#').next().unwrap_or(reference);
        let session = if reference.starts_with('/')
            || base.starts_with("links:")
            || base.starts_with("conditions:")
            || base.starts_with("proposal:")
            || base.starts_with("source:")
            || base.starts_with("edges:")
            || ["pending", "native", "alerts"].contains(&base)
        {
            session.with_proposals(self.store.proposals()?)?
        } else {
            session
        };
        let result = session.read(reference, revision, tokens, offset, |s| {
            self.store.encoding().count(s)
        })?;
        capture.verify()?;
        self.inputs.verify()?;
        Ok(result)
    }
    pub fn searching(&self, revision: &str, request: &SearchRequest) -> Result<String> {
        if self.ordinary()? {
            let (capture, _, session) = self.ordinary_session()?;
            let result = session.search_with_semantic(
                revision,
                request,
                |s| self.store.encoding().count(s),
                self.semantic
                    .as_ref()
                    .map(|provider| provider as &dyn crate::session_search::SemanticProvider),
            )?;
            capture.verify()?;
            self.inputs.verify()?;
            return Ok(result);
        }
        let capture = source_capture::capture_source(
            std::slice::from_ref(&self.input),
            &self.cwd,
            self.mode,
            None,
        )?;
        let session = self.store.load(revision, capture.snapshot()?)?;
        let result = session.search_with_semantic(
            revision,
            request,
            |s| self.store.encoding().count(s),
            self.semantic
                .as_ref()
                .map(|provider| provider as &dyn crate::session_search::SemanticProvider),
        )?;
        capture.verify()?;
        self.inputs.verify()?;
        Ok(result)
    }

    pub fn proposing(&self, revision: &str, request: &ProposalRequest) -> Result<String> {
        if self.ordinary()? {
            let (capture, _, session) = self.ordinary_session()?;
            session.expect(revision)?;
            capture.verify()?;
            self.inputs.verify()?;
            let result = self
                .store
                .propose_revision(revision, request, |reference| {
                    session.validate_reference(reference)
                })?;
            capture.verify()?;
            self.inputs.verify()?;
            return Ok(serde_json::to_string(&result)?);
        }
        let capture = source_capture::capture_source(
            std::slice::from_ref(&self.input),
            &self.cwd,
            self.mode,
            None,
        )?;
        let result = self.store.propose(revision, capture.snapshot()?, request)?;
        capture.verify()?;
        self.inputs.verify()?;
        Ok(serde_json::to_string(&result)?)
    }

    pub fn verify_claims_boundary(
        &self,
        judgment: &str,
        revision: &str,
        assertions: &[J],
    ) -> Result<String> {
        crate::require(
            !assertions.is_empty(),
            "no assertions supplied; nothing was checked",
        )?;
        if self.ordinary()? {
            let (capture, runtime, session) = self.ordinary_session()?;
            let program = runtime
                .ordinary_program()
                .ok_or_else(|| error("ordinary expression program is not configured"))?;
            let result = session.verify_claims(judgment, revision, assertions, program)?;
            capture.verify()?;
            self.inputs.verify()?;
            return Ok(result);
        }
        let capture = source_capture::capture_source(
            std::slice::from_ref(&self.input),
            &self.cwd,
            self.mode,
            None,
        )?;
        self.store.load(revision, capture.snapshot()?)?;
        capture.verify()?;
        self.inputs.verify()?;
        Err(error(
            "unsupported_capability: kpopper_verify_claims belongs to checked-reader/v1; read the bound core finding instead",
        ))
    }

    pub fn contextualizing(
        &self,
        ids: &[String],
        revision: &str,
        options: &ContextOptions,
    ) -> Result<String> {
        if self.ordinary()? {
            let (capture, _, session) = self.ordinary_session()?;
            let result = session
                .contextualize(ids, revision, options, |s| self.store.encoding().count(s))?;
            capture.verify()?;
            self.inputs.verify()?;
            return Ok(result);
        }
        let capture = source_capture::capture_source(
            std::slice::from_ref(&self.input),
            &self.cwd,
            self.mode,
            None,
        )?;
        let session = self.store.load(revision, capture.snapshot()?)?;
        let result =
            session.contextualize(ids, revision, options, |s| self.store.encoding().count(s))?;
        capture.verify()?;
        self.inputs.verify()?;
        Ok(result)
    }

    pub fn current_context(&self, ids: &[String], options: &ContextOptions) -> Result<String> {
        let result = if self.ordinary()? {
            let (capture, _, session) = self.ordinary_session()?;
            let text = session.contextualize(ids, session.revision(), options, |s| {
                self.store.encoding().count(s)
            })?;
            capture.verify()?;
            text
        } else {
            let (capture, session) = self.capture_core_session()?;
            let text = session.contextualize(ids, session.revision(), options, |s| {
                self.store.encoding().count(s)
            })?;
            capture.verify()?;
            text
        };
        self.inputs.verify()?;
        Ok(result)
    }
}

pub fn run_context(options: &ContextCommand, cwd: &Path, mode: ReadMode) -> Result<String> {
    let service = Service::new(&options.session_options(), cwd, mode)?;
    let context = ContextOptions {
        direction: options.direction,
        tokens: options.tokens,
        depth: options.depth,
        max_nodes: options.max_nodes,
    };
    match options.revision.as_deref() {
        Some(revision) => service.contextualizing(&options.ids, revision, &context),
        None => service.current_context(&options.ids, &context),
    }
}

pub fn run(options: &Options, cwd: &Path, mode: ReadMode) -> Result<String> {
    let service = Service::new(options, cwd, mode)?;
    match options.operation {
        Operation::Setup | Operation::Status | Operation::Enable | Operation::Disable => Err(
            error("session management uses the explicit admin dispatcher"),
        ),
        Operation::Open => service.opening(options.tokens.unwrap_or(700)),
        Operation::HookOpen => service.hook_open(options.tokens.unwrap_or(1000)),
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
        Operation::Search => service.searching(
            options
                .revision
                .as_deref()
                .ok_or_else(|| error("search requires --revision from open"))?,
            &SearchRequest {
                query: options.query.clone(),
                ids: Some(options.ids.clone()),
                tokens: options.tokens.unwrap_or(1000),
                limit: options.limit,
                branch: options.branch.clone(),
                mode: options.search_mode,
                cursor: options.cursor.clone(),
            },
        ),
        Operation::Propose => service.proposing(
            options
                .revision
                .as_deref()
                .ok_or_else(|| error("propose requires --revision, --kind and --text"))?,
            &ProposalRequest {
                kind: options
                    .kind
                    .clone()
                    .ok_or_else(|| error("propose requires --revision, --kind and --text"))?,
                text: options
                    .text
                    .clone()
                    .ok_or_else(|| error("propose requires --revision, --kind and --text"))?,
                basis: options.basis.clone(),
                revisit: options.revisit.clone(),
            },
        ),
        Operation::Serve => {
            let tools = crate::session_mcp::declared_tools()
                .into_iter()
                .filter(|t| {
                    [
                        "kpopper_open",
                        "kpopper_read",
                        "kpopper_context",
                        "kpopper_search",
                        "kpopper_propose",
                        "kpopper_verify_claims",
                    ]
                    .contains(&t.name.as_str())
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
                        "kpopper_verify_claims" => service.verify_claims_boundary(
                            args["judgment"]
                                .as_str()
                                .ok_or_else(|| error("missing judgment"))?,
                            args["revision"]
                                .as_str()
                                .ok_or_else(|| error("missing revision"))?,
                            args["assertions"]
                                .as_array()
                                .ok_or_else(|| error("invalid assertions"))?,
                        ),
                        "kpopper_propose" => {
                            let required = |name: &str| {
                                args[name]
                                    .as_str()
                                    .map(str::to_owned)
                                    .ok_or_else(|| error(&format!("missing {name}")))
                            };
                            let basis = args["basis"]
                                .as_array()
                                .ok_or_else(|| error("invalid basis"))?
                                .iter()
                                .map(|v| {
                                    v.as_str()
                                        .map(str::to_owned)
                                        .ok_or_else(|| error("invalid basis"))
                                })
                                .collect::<Result<Vec<_>>>()?;
                            service.proposing(
                                &required("revision")?,
                                &ProposalRequest {
                                    kind: required("kind")?,
                                    text: required("text")?,
                                    basis,
                                    revisit: args
                                        .get("revisit")
                                        .and_then(J::as_str)
                                        .unwrap_or("")
                                        .into(),
                                },
                            )
                        }
                        "kpopper_search" => {
                            let ids = match args.get("ids") {
                                None | Some(J::Null) => None,
                                Some(v) => Some(
                                    v.as_array()
                                        .ok_or_else(|| error("invalid IDs"))?
                                        .iter()
                                        .map(|v| {
                                            v.as_str()
                                                .map(str::to_owned)
                                                .ok_or_else(|| error("invalid ID"))
                                        })
                                        .collect::<Result<Vec<_>>>()?,
                                ),
                            };
                            let limit = args.get("limit").map_or(Ok(8), |v| {
                                v.as_u64()
                                    .and_then(|n| usize::try_from(n).ok())
                                    .ok_or_else(|| error("limit must be1..32"))
                            })?;
                            let mode =
                                match args.get("mode").and_then(J::as_str).unwrap_or("hybrid") {
                                    "lexical" => SearchMode::Lexical,
                                    "semantic" => SearchMode::Semantic,
                                    "hybrid" => SearchMode::Hybrid,
                                    _ => return Err(error("unknown search mode")),
                                };
                            service.searching(
                                args["revision"]
                                    .as_str()
                                    .ok_or_else(|| error("missing revision"))?,
                                &SearchRequest {
                                    query: args["query"]
                                        .as_str()
                                        .ok_or_else(|| error("missing query"))?
                                        .into(),
                                    ids,
                                    tokens: token_count(1000)?,
                                    limit,
                                    branch: args
                                        .get("branch")
                                        .and_then(J::as_str)
                                        .map(str::to_owned),
                                    mode,
                                    cursor: args
                                        .get("cursor")
                                        .and_then(J::as_str)
                                        .map(str::to_owned),
                                },
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

#[cfg(test)]
mod tests {
    use super::Service;
    use std::path::Path;

    #[test]
    fn hook_route_uses_posix_safe_single_quotes() {
        assert_eq!(
            Service::shell_quote(Path::new("/tmp/project with 'quote'")),
            "'/tmp/project with '\\''quote'\\'''"
        );
    }
}

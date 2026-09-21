//! Writer preparation owns its live source observation and policy lock together.
use crate::{
    Result,
    history_authority::Files,
    history_contract::*,
    project_modes::WriteRoute,
    reasoning_authoring::World,
    reasoning_authoring_preparation::{self as A, PendingOverlay},
    reasoning_runtime::Runtime,
    require,
    source_capture::{self as S, CapturedSource, ReadMode},
    value::TypedValue as V,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub struct AuthoringSource<'a> {
    route: WriteRoute,
    source: CapturedSource,
    document: V,
    world: Option<World<'a>>,
}
impl<'a> AuthoringSource<'a> {
    /// Computational time is separate from the action's recording date. The
    /// caller must verify this holder again at its publication boundary.
    pub fn capture(
        paths: &[PathBuf],
        cwd: &Path,
        action: &V,
        as_of: Option<V>,
        runtime: Option<&'a Runtime>,
    ) -> Result<Self> {
        let route = WriteRoute::capture(paths, cwd)?;
        let source =
            S::capture_source_with_runtime(route.paths(), cwd, ReadMode::Live, as_of, runtime)?;
        let required = route.pending_required()?;
        let observed = source.pending_observation();
        require(!required || observed.is_some(), "pending_capture_required")?;
        let bundles = observed
            .map(|o| {
                o.ledger
                    .bundles
                    .iter()
                    .map(|(key, b)| (key.clone(), b.value.clone()))
                    .collect::<Map>()
            })
            .unwrap_or_default();
        let files = observed
            .map(|o| {
                o.ledger
                    .bundles
                    .iter()
                    .map(|(key, b)| (key.clone(), b.files.clone()))
                    .collect::<BTreeMap<String, Files>>()
            })
            .unwrap_or_default();
        let pending = if required {
            Some(PendingOverlay {
                bundles: &bundles,
                files: &files,
                decisions: &map(&observed.unwrap().publication)?["decisions"],
                resume: &[],
            })
        } else {
            None
        };
        let (document, world) = A::prepare(source.snapshot()?, action, pending.as_ref(), runtime)?;
        let prepared = Self {
            route,
            source,
            document,
            world,
        };
        prepared.verify()?;
        Ok(prepared)
    }
    pub fn paths(&self) -> &[PathBuf] {
        self.route.paths()
    }
    pub fn config(&self) -> &V {
        self.route.config()
    }
    pub fn source(&self) -> &CapturedSource {
        &self.source
    }
    pub fn document(&self) -> &V {
        &self.document
    }
    pub fn world(&mut self) -> Option<&mut World<'a>> {
        self.world.as_mut()
    }
    pub fn verify(&self) -> Result<()> {
        self.route.verify()?;
        self.source.verify()?;
        self.route.verify()
    }
}

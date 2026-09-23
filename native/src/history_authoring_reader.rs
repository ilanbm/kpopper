//! Profile dispatch for immutable authoring. Ordinary reads never create a core world.
use crate::{
    Error, Result,
    history_authoring_audit::ReplayAudit,
    history_contract::{Map, map, string_is, text},
    ordinary_reader::Reader,
    reasoning_authoring::World,
    reasoning_fields as F,
    reasoning_runtime::{OperationalBounds, Runtime},
    value::TypedValue as V,
};
pub(crate) enum AuthoringReader<'a> {
    Core(Box<World<'a>>),
    Ordinary(Reader<'a>),
}
impl<'a> AuthoringReader<'a> {
    /// Ordinary receipts retain authored evidence without requiring inferred
    /// judgment fields. Explicit acts can target a record with no judgments.
    pub fn document_evidence(
        doc: &V,
        runtime: Option<&'a Runtime>,
        audit: Option<&ReplayAudit>,
        versions: &Map,
    ) -> Result<V> {
        if string_is(&map(&F::capabilities(doc, None)?)?["profile"], "core/v1") {
            Self::new(doc, runtime)?.evidence(doc, audit, versions)
        } else {
            Ok(V::Map(Map::from([
                (
                    "kind".into(),
                    V::Text("authored-computational-projection/v1".into()),
                ),
                ("document".into(), doc.clone()),
            ])))
        }
    }
    pub fn new(doc: &V, runtime: Option<&'a Runtime>) -> Result<Self> {
        if string_is(&map(&F::capabilities(doc, None)?)?["profile"], "core/v1") {
            Ok(Self::Core(Box::new(World::new(
                doc,
                None,
                runtime,
                OperationalBounds::default(),
            )?)))
        } else {
            Ok(Self::Ordinary(Reader::new(doc, runtime)?))
        }
    }
    pub fn batch_admission(
        final_doc: &V,
        prior: &V,
        subject: &str,
        runtime: Option<&'a Runtime>,
    ) -> Result<Self> {
        if crate::reasoning_authoring::selected(final_doc, None)? {
            Ok(Self::Core(Box::new(World::batch_admission(
                final_doc,
                prior,
                subject,
                runtime,
                OperationalBounds::default(),
            )?)))
        } else {
            Ok(Self::Ordinary(Reader::new(prior, runtime)?))
        }
    }
    pub fn for_action(&mut self, a: &V) -> Result<()> {
        if let Self::Ordinary(w) = self {
            w.for_action(a)?;
        }
        Ok(())
    }
    pub fn attach_temporal(&self, evidence: &mut V, doc: &V, versions: &Map) -> Result<()> {
        if let Self::Core(w) = self {
            crate::history_authoring::attach_temporal_replay(evidence, doc, w, versions)?;
        }
        Ok(())
    }
    pub fn core(&self) -> bool {
        matches!(self, Self::Core(_))
    }
    pub fn fields(&self) -> &Map {
        match self {
            Self::Core(w) => w.fields(),
            Self::Ordinary(w) => w.fields(),
        }
    }
    pub fn raw(&self) -> &Map {
        match self {
            Self::Core(w) => w.raw(),
            Self::Ordinary(w) => w.raw(),
        }
    }
    pub fn normalize(&mut self, a: &V) -> Result<(V, Vec<String>)> {
        match self {
            Self::Core(w) => w.normalize(a, None),
            Self::Ordinary(w) => {
                w.for_action(a)?;
                w.normalize(a)
            }
        }
    }
    pub fn validate(&mut self, a: &V) -> Result<Vec<String>> {
        match self {
            Self::Core(w) => w.validate(a),
            Self::Ordinary(w) => w.validate(a),
        }
    }
    /// The note naming the entries nearest an add. A history write reads its base
    /// alone, so no hypothesis layer beside it is consulted.
    pub fn nearest_existing(&self, a: &V) -> Result<String> {
        use crate::public_identity::ordinary_sameness as S;
        let notice = match self {
            Self::Core(w) => {
                let ids = w.raw().keys().cloned().collect();
                S::nearest_existing(
                    &S::Inputs {
                        document: w.document(),
                        hypotheses: &Map::new(),
                        deps: text(&w.fields()["deps"])?,
                        ids: &ids,
                        raw: w.raw(),
                    },
                    a,
                    &S::Sources::default(),
                )?
            }
            Self::Ordinary(w) => S::nearest_existing_from_sources(w, a, &S::Sources::default())?,
        };
        Ok(notice.text)
    }
    pub fn candidate_document(&self, a: &V) -> Result<V> {
        match self {
            Self::Core(w) => Ok(w.candidate(a)?.document().clone()),
            Self::Ordinary(w) => w.candidate(a),
        }
    }
    pub fn history(&mut self, id: &str) -> Result<V> {
        match self {
            Self::Core(w) => w.history(id),
            Self::Ordinary(_) => Err(Error("ordinary_history_has_no_core_assessment".into())),
        }
    }
    pub fn evidence(&mut self, doc: &V, audit: Option<&ReplayAudit>, versions: &Map) -> Result<V> {
        match self {
            Self::Core(w) => {
                crate::history_authoring::evidence_with_versions(doc, w, audit, versions)
            }
            Self::Ordinary(_) => Ok(V::Map(Map::from([
                (
                    "kind".into(),
                    V::Text("authored-computational-projection/v1".into()),
                ),
                ("document".into(), doc.clone()),
            ]))),
        }
    }
}

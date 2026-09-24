//! Lazy original-version bindings in the readable current document.
//! No history files or publication authority are created by this pure module.
use crate::{
    Error, Result,
    history_contract::{map, text},
    history_node_codec as C, require,
    value::TypedValue as V,
};
use serde_json::Value;
use std::collections::BTreeMap;

const FORMAT: &str = "node-current/v1";
/// A current node's authored body remains at document[collection][subject].
/// Other node-local context (pins, provenance, acts) is retained once in the binding.
#[derive(Clone, Debug)]
pub struct Original {
    subject: String,
    collection: String,
    context: BTreeMap<String, V>,
    header: Value,
}
fn payload(collection: &str, body: Option<V>, context: &BTreeMap<String, V>) -> V {
    V::Map(BTreeMap::from([
        ("collection".into(), V::Text(collection.into())),
        ("body".into(), body.unwrap_or(V::Null)),
        ("context".into(), V::Map(context.clone())),
    ]))
}
impl Original {
    pub fn create(
        subject: &str,
        collection: &str,
        operation: &str,
        body: V,
        context: BTreeMap<String, V>,
    ) -> Result<Self> {
        require(!collection.is_empty(), "node_current_collection")?;
        let event = C::Event::create(
            subject,
            operation,
            vec![],
            None,
            Some(payload(collection, Some(body), &context)),
        )?;
        let (header, _) = event.initial_binding()?;
        Ok(Self {
            subject: subject.into(),
            collection: collection.into(),
            context,
            header,
        })
    }
    pub fn subject(&self) -> &str {
        &self.subject
    }
    pub fn collection(&self) -> &str {
        &self.collection
    }
    /// Metadata uses TypedValue tags only in this explicitly versioned envelope.
    pub fn encode(&self) -> Result<V> {
        Ok(V::Map(BTreeMap::from([
            ("format".into(), V::Text(FORMAT.into())),
            ("subject".into(), V::Text(self.subject.clone())),
            ("collection".into(), V::Text(self.collection.clone())),
            (
                "context".into(),
                V::from_json(&V::Map(self.context.clone()).to_tagged()?)?,
            ),
            ("event".into(), V::from_json(&self.header)?),
        ])))
    }
    pub fn decode(binding: &V) -> Result<Self> {
        let m = crate::history_contract::schema(
            binding,
            &["format", "subject", "collection", "context", "event"],
            &[],
        )?;
        require(text(&m["format"])? == FORMAT, "node_current_format")?;
        let context = V::from_tagged(&m["context"].to_json()?)?;
        let subject = text(&m["subject"])?;
        C::subject_path(subject)?;
        let collection = text(&m["collection"])?;
        require(!collection.is_empty(), "node_current_collection")?;
        Ok(Self {
            subject: subject.into(),
            collection: collection.into(),
            context: map(&context)?.clone(),
            header: m["event"].to_json()?,
        })
    }
    pub fn restore(&self, body: V) -> Result<C::Event> {
        C::Event::restore_initial(
            &self.header,
            Some(payload(&self.collection, Some(body), &self.context)),
            &self.subject,
        )
    }
    /// Verify the current body before yielding the original immutable frame for first-change retention.
    pub fn from_document(&self, document: &V) -> Result<C::Event> {
        let body = map(document)?
            .get(&self.collection)
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get(&self.subject))
            .ok_or_else(|| Error("node_current_missing_body".into()))?;
        self.restore(body.clone())
    }
    pub fn state(&self, body: V) -> V {
        payload(&self.collection, Some(body), &self.context)
    }
}

//! Experimental node-local history frames. This codec grants no publication authority.
//! Public history routing remains on the existing format until the storage gates pass.
use crate::{Error, Result, identity::sha256, require, value::TypedValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const FORMAT: &str = "node-history/v1";
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_EVENTS: usize = 40_000;

type State = Option<TypedValue>;

/// Absent is different from a present typed null. Mapping edits preserve that distinction.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", deny_unknown_fields)]
enum Delta {
    Absent,
    Replace(Value),
    Map(BTreeMap<String, Delta>),
    /// Only the root history object's `saw` field has set semantics in v1.
    Saw {
        add: Vec<String>,
        remove: Vec<String>,
    },
}
fn sorted(values: &[String]) -> bool {
    values.windows(2).all(|v| v[0] < v[1])
}
fn texts(value: &TypedValue) -> Option<Vec<String>> {
    let TypedValue::List(values) = value else {
        return None;
    };
    let values = values
        .iter()
        .map(|v| match v {
            TypedValue::Text(s) => Some(s.clone()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    (sorted(&values) && values.iter().all(|v| crate::history_paths::object_id(v))).then_some(values)
}
fn diff(before: Option<&TypedValue>, after: Option<&TypedValue>, path: &[String]) -> Result<Delta> {
    let Some(after) = after else {
        return Ok(Delta::Absent);
    };
    let replace = Delta::Replace(after.to_tagged()?);
    let Some(before) = before else {
        return Ok(replace);
    };
    let candidate = if let (TypedValue::Map(old), TypedValue::Map(new)) = (before, after) {
        let mut changes = BTreeMap::new();
        for key in old.keys().chain(new.keys()).collect::<BTreeSet<_>>() {
            if old.get(key) != new.get(key) {
                let mut next = path.to_vec();
                next.push(key.clone());
                changes.insert(key.clone(), diff(old.get(key), new.get(key), &next)?);
            }
        }
        Delta::Map(changes)
    } else if path == ["saw"] {
        match (texts(before), texts(after)) {
            (Some(old), Some(new)) => {
                let old: BTreeSet<_> = old.into_iter().collect();
                let new: BTreeSet<_> = new.into_iter().collect();
                Delta::Saw {
                    add: new.difference(&old).cloned().collect(),
                    remove: old.difference(&new).cloned().collect(),
                }
            }
            _ => return Ok(replace),
        }
    } else {
        return Ok(replace);
    };
    Ok(
        if serde_json::to_vec(&candidate)?.len() < serde_json::to_vec(&replace)?.len() {
            candidate
        } else {
            replace
        },
    )
}
fn apply(
    delta: &Delta,
    before: Option<&TypedValue>,
    path: &mut Vec<String>,
    depth: usize,
) -> Result<State> {
    require(depth <= crate::value::MAX_DEPTH, "node_history_depth")?;
    Ok(match delta {
        Delta::Absent => None,
        Delta::Replace(value) => Some(TypedValue::from_tagged(value)?),
        Delta::Map(changes) => {
            let Some(TypedValue::Map(before)) = before else {
                return Err(Error("node_history_patch_base".into()));
            };
            let mut after = before.clone();
            for (key, delta) in changes {
                path.push(key.clone());
                let value = apply(delta, before.get(key), path, depth + 1)?;
                path.pop();
                match value {
                    Some(value) => {
                        after.insert(key.clone(), value);
                    }
                    None => {
                        require(after.remove(key).is_some(), "node_history_patch_delete")?;
                    }
                }
            }
            Some(TypedValue::Map(after))
        }
        Delta::Saw { add, remove } => {
            require(
                path == &["saw"]
                    && sorted(add)
                    && sorted(remove)
                    && add
                        .iter()
                        .chain(remove)
                        .all(|v| crate::history_paths::object_id(v)),
                "node_history_patch_set",
            )?;
            let mut values: BTreeSet<_> = before
                .and_then(texts)
                .ok_or_else(|| Error("node_history_patch_set".into()))?
                .into_iter()
                .collect();
            require(
                add.iter().all(|v| !remove.contains(v)),
                "node_history_patch_set",
            )?;
            for value in remove {
                require(values.remove(value), "node_history_patch_set")?;
            }
            for value in add {
                require(values.insert(value.clone()), "node_history_patch_set")?;
            }
            Some(TypedValue::List(
                values.into_iter().map(TypedValue::Text).collect(),
            ))
        }
    })
}
fn state_digest(state: Option<&TypedValue>) -> Result<String> {
    let mut bytes = b"node-history-state/v1\0".to_vec();
    match state {
        None => bytes.push(0),
        Some(value) => {
            bytes.push(1);
            bytes.extend(value.canonical_bytes()?);
        }
    }
    Ok(sha256(&bytes))
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub fn subject_path(subject: &str) -> Result<String> {
    crate::history_paths::subject(subject)?;
    let mut bytes = b"node-history-subject/v1\0".to_vec();
    bytes.extend(subject.as_bytes());
    Ok(format!("{}.jsonl", sha256(&bytes)))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Base {
    event: String,
    result: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Relation {
    Initial,
    Change,
    Merge,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    format: String,
    subject: String,
    operation: String,
    relation: Relation,
    parents: Vec<String>,
    base: Option<Base>,
    delta: Delta,
    result: String,
    id: String,
}
/// A reconstructed version. Private identity fields prevent inventing a verified encoding base.
#[derive(Clone, Debug)]
pub struct Version {
    id: String,
    subject: String,
    result: String,
    frame_sha256: String,
    state: State,
}
impl Version {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn frame_sha256(&self) -> &str {
        &self.frame_sha256
    }
    pub fn state(&self) -> Option<&TypedValue> {
        self.state.as_ref()
    }
}
impl Event {
    pub fn create(
        subject: &str,
        operation: &str,
        parents: Vec<String>,
        base: Option<&Version>,
        state: State,
    ) -> Result<Self> {
        require(parents.len() <= 1, "node_history_explicit_merge_required")?;
        Self::build(subject, operation, parents, base, state, false)
    }
    /// The caller supplies the reconciled state; the authority layer must validate its acts.
    pub fn create_merge(
        subject: &str,
        operation: &str,
        parents: Vec<String>,
        base: &Version,
        state: State,
    ) -> Result<Self> {
        require(parents.len() >= 2, "node_history_merge_parents")?;
        Self::build(subject, operation, parents, Some(base), state, true)
    }
    fn build(
        subject: &str,
        operation: &str,
        mut parents: Vec<String>,
        base: Option<&Version>,
        state: State,
        merge: bool,
    ) -> Result<Self> {
        subject_path(subject)?;
        crate::history_contract::token(&TypedValue::Text(operation.into()))?;
        if let Some(state) = &state {
            state.validate()?;
        }
        parents.sort();
        require(
            sorted(&parents) && parents.iter().all(|v| hash(v)),
            "node_history_parents",
        )?;
        require(base.is_some() == !parents.is_empty(), "node_history_base")?;
        if let Some(base) = base {
            require(
                base.subject == subject && parents.contains(&base.id),
                "node_history_base",
            )?;
        }
        let mut event = Self {
            format: FORMAT.into(),
            subject: subject.into(),
            operation: operation.into(),
            relation: if merge {
                Relation::Merge
            } else if parents.is_empty() {
                Relation::Initial
            } else {
                Relation::Change
            },
            parents,
            base: base.map(|v| Base {
                event: v.id.clone(),
                result: v.result.clone(),
            }),
            delta: diff(base.and_then(|v| v.state.as_ref()), state.as_ref(), &[])?,
            result: state_digest(state.as_ref())?,
            id: String::new(),
        };
        event.id = event.identity()?;
        event.encode()?;
        Ok(event)
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Original versions can remain in the current record without a history file. The
    /// current body is supplied separately, so metadata does not duplicate it.
    pub fn initial_binding(&self) -> Result<(Value, State)> {
        require(
            self.relation == Relation::Initial,
            "node_history_not_initial",
        )?;
        let version = self.reconstruct(None)?;
        let mut binding = serde_json::to_value(self)?;
        binding.as_object_mut().unwrap().remove("delta");
        Ok((binding, version.state))
    }
    pub fn restore_initial(binding: &Value, state: State, subject: &str) -> Result<Self> {
        let mut binding = binding.clone();
        let fields = binding
            .as_object_mut()
            .ok_or_else(|| Error("node_history_current_binding".into()))?;
        require(
            !fields.contains_key("delta"),
            "node_history_current_binding",
        )?;
        fields.insert(
            "delta".into(),
            serde_json::to_value(diff(None, state.as_ref(), &[])?)?,
        );
        let event: Self = serde_json::from_value(binding)?;
        require(
            event.relation == Relation::Initial && event.subject == subject,
            "node_history_current_binding",
        )?;
        Self::decode(&event.encode()?, subject)?;
        event.reconstruct(None)?;
        Ok(event)
    }
    pub fn parents(&self) -> &[String] {
        &self.parents
    }
    fn identity(&self) -> Result<String> {
        let mut value = serde_json::to_value(self)?;
        value.as_object_mut().unwrap().remove("id");
        let mut bytes = b"node-history-event/v1\0".to_vec();
        bytes.extend(serde_json::to_vec(&value)?);
        Ok(sha256(&bytes))
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        require(bytes.len() <= MAX_FRAME_BYTES, "node_history_frame_limit")?;
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8], subject: &str) -> Result<Self> {
        require(
            bytes.len() <= MAX_FRAME_BYTES && bytes.last() == Some(&b'\n'),
            "node_history_frame",
        )?;
        let event: Self = serde_json::from_slice(&bytes[..bytes.len() - 1])?;
        require(
            event.format == FORMAT && event.subject == subject,
            "node_history_format",
        )?;
        subject_path(subject)?;
        crate::history_contract::token(&TypedValue::Text(event.operation.clone()))?;
        require(
            hash(&event.result) && hash(&event.id) && event.id == event.identity()?,
            "node_history_identity",
        )?;
        require(
            sorted(&event.parents) && event.parents.iter().all(|v| hash(v)),
            "node_history_parents",
        )?;
        require(
            event.base.is_some() == !event.parents.is_empty(),
            "node_history_base",
        )?;
        require(
            match event.relation {
                Relation::Initial => event.parents.is_empty(),
                Relation::Change => event.parents.len() == 1,
                Relation::Merge => event.parents.len() >= 2,
            },
            "node_history_relation",
        )?;
        if let Some(base) = &event.base {
            require(
                event.parents.contains(&base.event) && hash(&base.result),
                "node_history_base",
            )?;
        }
        require(event.encode()? == bytes, "node_history_noncanonical")?;
        Ok(event)
    }
    pub fn reconstruct(&self, base: Option<&Version>) -> Result<Version> {
        // Deserialize is public for transport; it must not bypass the bounded checked decoder.
        Self::decode(&self.encode()?, &self.subject)?;
        self.reconstruct_checked(base)
    }
    fn reconstruct_checked(&self, base: Option<&Version>) -> Result<Version> {
        match (&self.base, base) {
            (None, None) => (),
            (Some(expected), Some(actual)) => require(
                expected.event == actual.id
                    && expected.result == actual.result
                    && self.subject == actual.subject,
                "node_history_base",
            )?,
            _ => return Err(Error("node_history_base".into())),
        }
        let state = apply(
            &self.delta,
            base.and_then(|v| v.state.as_ref()),
            &mut vec![],
            0,
        )?;
        require(
            state_digest(state.as_ref())? == self.result,
            "node_history_result",
        )?;
        Ok(Version {
            id: self.id.clone(),
            subject: self.subject.clone(),
            result: self.result.clone(),
            frame_sha256: sha256(&self.encode()?),
            state,
        })
    }
}

/// Fully validates a detached node stream, including every parent. No current head is selected.
/// Transaction membership and cross-node pins are deliberately left to the authority layer.
pub fn decode_stream(bytes: &[u8], subject: &str) -> Result<BTreeMap<String, Version>> {
    require(bytes.len() <= MAX_STREAM_BYTES, "node_history_stream_limit")?;
    let mut events = BTreeMap::new();
    for frame in bytes.split_inclusive(|v| *v == b'\n') {
        let event = Event::decode(frame, subject)?;
        require(
            !events.contains_key(event.id()) && events.len() < MAX_EVENTS,
            "node_history_duplicate_or_limit",
        )?;
        events.insert(event.id.clone(), event);
    }
    let mut pending = BTreeMap::new();
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut ready = VecDeque::new();
    for (id, event) in &events {
        pending.insert(id.clone(), event.parents.len());
        if event.parents.is_empty() {
            ready.push_back(id.clone());
        }
        for parent in &event.parents {
            require(events.contains_key(parent), "node_history_missing_parent")?;
            children.entry(parent.clone()).or_default().push(id.clone());
        }
    }
    let mut versions = BTreeMap::new();
    let mut reconstructed_bytes = 0usize;
    while let Some(id) = ready.pop_front() {
        let event = &events[&id];
        let base = event
            .base
            .as_ref()
            .and_then(|base| versions.get(&base.event));
        let version = event.reconstruct_checked(base)?;
        reconstructed_bytes += version
            .state
            .as_ref()
            .map(|v| v.canonical_bytes())
            .transpose()?
            .map_or(0, |v| v.len());
        require(
            reconstructed_bytes <= MAX_STREAM_BYTES,
            "node_history_reconstruction_limit",
        )?;
        versions.insert(id.clone(), version);
        for child in children.get(&id).into_iter().flatten() {
            let count = pending.get_mut(child).unwrap();
            *count -= 1;
            if *count == 0 {
                ready.push_back(child.clone());
            }
        }
    }
    require(versions.len() == events.len(), "node_history_cycle")?;
    Ok(versions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn invalid_set_edits_fail_after_rehashing() {
        let original = Event::create(
            "p.a",
            "initial",
            vec![],
            None,
            Some(TypedValue::from_json(&json!({"saw":["a".repeat(64)]})).unwrap()),
        )
        .unwrap();
        let version = original.reconstruct(None).unwrap();
        for (add, remove) in [
            (vec!["a".repeat(64)], vec![]),
            (vec![], vec!["b".repeat(64)]),
            (vec!["b".repeat(64), "b".repeat(64)], vec![]),
            (vec!["b".repeat(64)], vec!["b".repeat(64)]),
            (vec!["invalid-id".into()], vec![]),
        ] {
            let mut event = Event::create(
                "p.a",
                "bad",
                vec![version.id().into()],
                Some(&version),
                version.state.clone(),
            )
            .unwrap();
            event.delta = Delta::Map(BTreeMap::from([("saw".into(), Delta::Saw { add, remove })]));
            event.id = event.identity().unwrap();
            let result = event.reconstruct(Some(&version));
            assert_eq!(result.unwrap_err().0, "node_history_patch_set");
        }
    }
    #[test]
    fn transport_deserialization_cannot_bypass_envelope_validation() {
        let mut event =
            Event::create("p.a", "initial", vec![], None, Some(TypedValue::Null)).unwrap();
        event.format = "unknown/v2".into();
        event.id = event.identity().unwrap();
        let event: Event = serde_json::from_slice(&event.encode().unwrap()).unwrap();
        assert_eq!(
            event.reconstruct(None).unwrap_err().0,
            "node_history_format"
        );
    }
    #[test]
    fn decoded_state_amplification_has_an_aggregate_limit() {
        let initial = Event::create(
            "p.a",
            "initial",
            vec![],
            None,
            Some(TypedValue::Map(BTreeMap::from([(
                "large".into(),
                TypedValue::Text("x".repeat(600_000)),
            )]))),
        )
        .unwrap();
        let mut version = initial.reconstruct(None).unwrap();
        let mut raw = initial.encode().unwrap();
        for i in 0..112 {
            let event = Event::create(
                "p.a",
                &format!("review-{i}"),
                vec![version.id().into()],
                Some(&version),
                version.state.clone(),
            )
            .unwrap();
            raw.extend(event.encode().unwrap());
            version = event.reconstruct(Some(&version)).unwrap();
        }
        assert!(raw.len() < 1_000_000);
        assert_eq!(
            decode_stream(&raw, "p.a").unwrap_err().0,
            "node_history_reconstruction_limit"
        );
    }
}

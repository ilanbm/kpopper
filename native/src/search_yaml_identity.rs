//! Recover only serialization identity from captured YAML bytes. Projected values
//! must equal the captured body; this neither rereads files nor changes semantics.
use crate::{
    Error, Result,
    history_emit::{OrdinaryIdentity, encode_ordinary_display},
    history_yaml as Y,
    ordinary_source::{Key as OrdinaryKey, Source as O},
    ordinary_value::{Scalar, Value as V},
    require,
    value::TypedValue as CV,
};
use libyaml_safer::{Event, EventData as E, Parser, ScalarStyle};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone)]
enum Node {
    Pending,
    Scalar(Scalar),
    Merge,
    List(Vec<usize>),
    Map(Vec<(usize, usize)>),
}
struct Graph<'a> {
    parser: Parser<&'a [u8]>,
    nodes: Vec<Node>,
    anchors: BTreeMap<String, usize>,
}
impl Graph<'_> {
    fn next(&mut self) -> Result<Event> {
        self.parser
            .parse()
            .map_err(|e| Error(format!("invalid search YAML: {e}")))
    }
    fn node(&mut self, event: Event, depth: usize) -> Result<usize> {
        require(
            depth <= crate::value::MAX_DEPTH + 1 && self.nodes.len() < crate::value::MAX_VALUES * 2,
            "history_limit",
        )?;
        if let E::Alias { anchor } = &event.data {
            return self
                .anchors
                .get(anchor)
                .copied()
                .ok_or_else(|| Error("invalid search YAML alias".into()));
        }
        let id = self.nodes.len();
        self.nodes.push(Node::Pending);
        let anchor = match &event.data {
            E::Scalar { anchor, .. }
            | E::MappingStart { anchor, .. }
            | E::SequenceStart { anchor, .. } => anchor.clone(),
            _ => None,
        };
        if let Some(anchor) = anchor {
            require(
                self.anchors.insert(anchor, id).is_none(),
                "invalid search YAML anchor",
            )?;
        }
        let node = match event.data {
            E::Scalar {
                value, style, tag, ..
            } => {
                if tag.as_deref() == Some("tag:yaml.org,2002:merge")
                    || (tag.is_none() && style == ScalarStyle::Plain && value == "<<")
                {
                    Node::Merge
                } else if tag.as_deref() == Some("tag:yaml.org,2002:value")
                    || (tag.is_none() && style == ScalarStyle::Plain && value == "=")
                {
                    Node::Scalar(Scalar::Finite(CV::Text(value)))
                } else {
                    Node::Scalar(Y::ordinary_atom(&value, style, tag.as_deref())?)
                }
            }
            E::SequenceStart { .. } => {
                let mut out = vec![];
                loop {
                    let event = self.next()?;
                    if matches!(event.data, E::SequenceEnd) {
                        break;
                    }
                    out.push(self.node(event, depth + 1)?);
                }
                Node::List(out)
            }
            E::MappingStart { .. } => {
                let mut out = vec![];
                loop {
                    let event = self.next()?;
                    if matches!(event.data, E::MappingEnd) {
                        break;
                    }
                    let key = self.node(event, depth + 1)?;
                    let event = self.next()?;
                    out.push((key, self.node(event, depth + 1)?));
                }
                Node::Map(out)
            }
            _ => return Err(Error("invalid search YAML event".into())),
        };
        self.nodes[id] = node;
        Ok(id)
    }
    fn pairs(
        &self,
        id: usize,
        active: &mut BTreeSet<usize>,
        budget: &mut usize,
    ) -> Result<Vec<(usize, usize)>> {
        require(active.insert(id), "cyclic search YAML alias")?;
        let Node::Map(pairs) = &self.nodes[id] else {
            return Err(Error("invalid search YAML mapping".into()));
        };
        let mut merged = vec![];
        let mut regular = vec![];
        for &(key, value) in pairs {
            *budget += 1;
            require(*budget <= crate::value::MAX_VALUES * 4, "history_limit")?;
            if matches!(self.nodes[key], Node::Merge) {
                match &self.nodes[value] {
                    Node::Map(_) => merged.extend(self.pairs(value, active, budget)?),
                    Node::List(items) => {
                        for item in items.iter().rev() {
                            merged.extend(self.pairs(*item, active, budget)?);
                        }
                    }
                    _ => return Err(Error("invalid search YAML merge".into())),
                }
            } else {
                regular.push((key, value));
            }
        }
        active.remove(&id);
        merged.extend(regular);
        Ok(merged)
    }
    fn key(&self, id: usize) -> Result<OrdinaryKey> {
        let Node::Scalar(value) = &self.nodes[id] else {
            return Err(Error("invalid search YAML key".into()));
        };
        Y::ordinary_atom_key(value.clone(), None)
    }
    fn project(
        &self,
        id: usize,
        active: &mut BTreeSet<usize>,
        budget: &mut usize,
        depth: usize,
    ) -> Result<(O, OrdinaryIdentity)> {
        *budget += 1;
        require(
            *budget <= crate::value::MAX_VALUES * 2 && depth <= crate::value::MAX_DEPTH + 1,
            "history_limit",
        )?;
        require(active.insert(id), "cyclic search YAML alias")?;
        let mut identity = OrdinaryIdentity {
            id: Some(id),
            children: vec![],
        };
        let result = match &self.nodes[id] {
            Node::Scalar(v) => {
                if !matches!(v, Scalar::Finite(CV::Date(_) | CV::DateTime(_))) {
                    identity.id = None;
                }
                O::Scalar(v.clone())
            }
            Node::List(ids) => {
                let mut values = vec![];
                for id in ids {
                    let (value, child) = self.project(*id, active, budget, depth + 1)?;
                    values.push(value);
                    identity.children.push(child);
                }
                O::List(values)
            }
            Node::Map(_) => {
                let pairs = self.pairs(id, &mut BTreeSet::new(), &mut 0)?;
                let mut fields: Vec<(OrdinaryKey, usize, usize)> = vec![];
                for (key, value) in pairs {
                    let parsed = self.key(key)?;
                    if let Some(old) = fields.iter_mut().find(|(k, _, _)| k.python_eq(&parsed)) {
                        old.2 = value;
                    } else {
                        fields.push((parsed, key, value));
                    }
                }
                let mut values = vec![];
                for (key, key_id, value_id) in fields {
                    let (_, key_identity) = self.project(key_id, active, budget, depth + 1)?;
                    let (value, value_identity) =
                        self.project(value_id, active, budget, depth + 1)?;
                    values.push((key, value));
                    identity.children.extend([key_identity, value_identity]);
                }
                O::Map(values)
            }
            _ => return Err(Error("invalid search YAML node".into())),
        };
        active.remove(&id);
        Ok((result, identity))
    }
}
pub(super) struct Document<'a> {
    graph: Graph<'a>,
    entries: BTreeMap<String, usize>,
}
impl<'a> Document<'a> {
    pub(super) fn new(raw: &'a [u8]) -> Result<Self> {
        require(raw.len() <= Y::MAX_DOCUMENT_BYTES, "history_limit")?;
        let mut parser = Parser::new();
        parser.set_input(raw);
        let mut graph = Graph {
            parser,
            nodes: vec![],
            anchors: BTreeMap::new(),
        };
        require(
            matches!(graph.next()?.data, E::StreamStart { .. }),
            "invalid search YAML stream",
        )?;
        let start = graph.next()?;
        if matches!(start.data, E::StreamEnd) {
            return Ok(Self {
                graph,
                entries: BTreeMap::new(),
            });
        }
        require(
            matches!(start.data, E::DocumentStart { .. }),
            "invalid search YAML document",
        )?;
        let event = graph.next()?;
        let root = graph.node(event, 0)?;
        require(
            matches!(graph.next()?.data, E::DocumentEnd { .. })
                && matches!(graph.next()?.data, E::StreamEnd),
            "invalid search YAML documents",
        )?;
        let mut entries = BTreeMap::new();
        if matches!(graph.nodes[root], Node::Map(_)) {
            for (key, collection) in graph.pairs(root, &mut BTreeSet::new(), &mut 0)? {
                let Node::Scalar(Scalar::Finite(CV::Text(name))) = &graph.nodes[key] else {
                    continue;
                };
                if ["meta", "schema", "record", "also", "hypothesis"].contains(&name.as_str())
                    || !matches!(graph.nodes[collection], Node::Map(_))
                {
                    continue;
                }
                for (key, entry) in graph.pairs(collection, &mut BTreeSet::new(), &mut 0)? {
                    if let Node::Scalar(Scalar::Finite(CV::Text(id))) = &graph.nodes[key] {
                        entries.insert(id.clone(), entry);
                    }
                }
            }
        }
        Ok(Self { graph, entries })
    }
    pub(super) fn dump_finite(&self, id: &str, expected: &CV) -> Result<Option<String>> {
        self.dump(id, &V::from_typed(expected))
    }
    pub(super) fn dump(&self, id: &str, expected: &V) -> Result<Option<String>> {
        let Some(selected) = self.entries.get(id).copied() else {
            return Ok(None);
        };
        let (value, identity) = self
            .graph
            .project(selected, &mut BTreeSet::new(), &mut 0, 0)?;
        require(
            super::search_corpus::full_matches(&value, expected),
            "search source body differs from captured value",
        )?;
        let bytes = encode_ordinary_display(&value, 80, Some(&identity))?;
        Ok(Some(
            String::from_utf8(bytes).map_err(|_| Error("invalid search YAML output".into()))?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_identity_rejects_cycles_unknown_anchors_and_value_mismatches() {
        for raw in [
            "known:\n  p.a: &a {child: *a}\n",
            "known:\n  p.a: *missing\n",
        ] {
            assert!(
                Document::new(raw.as_bytes())
                    .and_then(|d| d.dump("p.a", &V::Null))
                    .is_err()
            );
        }
        let doc = Document::new(b"known:\n  p.a: {v: 1}\n").unwrap();
        assert_eq!(
            doc.dump("p.a", &V::Null).unwrap_err().0,
            "search source body differs from captured value"
        );
    }
    #[test]
    fn source_identity_bounds_expanded_aliases_before_emission() {
        let mut source = "known:\n  p.a0: &a0 [one, two, three, four]\n".to_owned();
        for i in 1..15 {
            source += &format!(
                "  p.a{i}: &a{i} [*a{}, *a{}, *a{}, *a{}]\n",
                i - 1,
                i - 1,
                i - 1,
                i - 1
            );
        }
        let doc = Document::new(source.as_bytes()).unwrap();
        assert_eq!(doc.dump("p.a14", &V::Null).unwrap_err().0, "history_limit");
    }
}

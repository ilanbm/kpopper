//! YAML source spans protect expression literals and historical computed values.
use crate::{
    Result,
    history_authoring::{obj, s},
    history_contract::*,
    history_identity_rewrite::{expression, tokens},
    require,
};
use libyaml_safer::{Event, EventData as E, Parser, ScalarStyle};

pub(crate) struct Node {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}
pub(crate) enum Kind {
    Scalar(String, ScalarStyle),
    List(Vec<Node>),
    Map(Vec<(Node, Node)>),
}
impl Node {
    fn scalar(&self) -> Option<&str> {
        if let Kind::Scalar(v, _) = &self.kind {
            Some(v)
        } else {
            None
        }
    }
}
fn next(parser: &mut Parser<&[u8]>) -> Result<Event> {
    parser.parse().map_err(|_| error("invalid_identity_yaml"))
}
fn node(
    parser: &mut Parser<&[u8]>,
    event: Event,
    depth: usize,
    visits: &mut usize,
) -> Result<Node> {
    *visits += 1;
    require(depth <= 128 && *visits <= 200_000, "history_limit")?;
    let start = event.start_mark.index as usize;
    let mut end = event.end_mark.index as usize;
    let kind = match event.data {
        E::Scalar { value, style, .. } => Kind::Scalar(value, style),
        E::SequenceStart { .. } => {
            let mut values = vec![];
            loop {
                let event = next(parser)?;
                if matches!(event.data, E::SequenceEnd) {
                    end = event.end_mark.index as usize;
                    break;
                }
                values.push(node(parser, event, depth + 1, visits)?);
            }
            Kind::List(values)
        }
        E::MappingStart { .. } => {
            let mut pairs = vec![];
            loop {
                let event = next(parser)?;
                if matches!(event.data, E::MappingEnd) {
                    end = event.end_mark.index as usize;
                    break;
                }
                let key = node(parser, event, depth + 1, visits)?;
                let event = next(parser)?;
                let value = node(parser, event, depth + 1, visits)?;
                pairs.push((key, value));
            }
            Kind::Map(pairs)
        }
        _ => return Err(error("invalid_identity_yaml")),
    };
    Ok(Node { start, end, kind })
}
pub(crate) fn parse(text: &str) -> Result<Node> {
    require(text.len() <= 16 * 1024 * 1024, "history_limit")?;
    let mut parser = Parser::new();
    parser.set_input(text.as_bytes());
    require(
        matches!(next(&mut parser)?.data, E::StreamStart { .. }),
        "invalid_identity_yaml",
    )?;
    require(
        matches!(next(&mut parser)?.data, E::DocumentStart { .. }),
        "invalid_identity_yaml",
    )?;
    let event = next(&mut parser)?;
    let value = node(&mut parser, event, 0, &mut 0)?;
    require(
        matches!(next(&mut parser)?.data, E::DocumentEnd { .. })
            && matches!(next(&mut parser)?.data, E::StreamEnd),
        "invalid_identity_yaml",
    )?;
    Ok(value)
}
struct Visitor<'a> {
    text: &'a str,
    retired: &'a str,
    survivor: &'a str,
    predicate: &'a str,
    snapshot: &'a str,
    spans: Vec<(usize, usize, String)>,
}
impl Visitor<'_> {
    fn original(&self, node: &Node) -> Result<&str> {
        self.text
            .get(node.start..node.end)
            .ok_or_else(|| error("invalid_identity_yaml"))
    }
    fn protect(&mut self, node: &Node) -> Result<()> {
        self.spans
            .push((node.start, node.end, self.original(node)?.into()));
        Ok(())
    }
    fn visit(&mut self, node: &Node, executable: bool, snapshot: bool) -> Result<()> {
        match &node.kind {
            Kind::Map(pairs)
                if executable && pairs.len() == 1 && pairs[0].0.scalar() == Some("expr") =>
            {
                let value = &pairs[0].1;
                let original = value.scalar().map(|v| obj([("expr", s(v))]));
                if let Some(original) = original.filter(|v| {
                    crate::reasoning_language::legacy_references(v)
                        .iter()
                        .any(|id| id == self.retired)
                }) {
                    let changed = expression(&original, self.retired, self.survivor)?;
                    let mut replacement = serde_json::to_string(text(&map(&changed)?["expr"])?)?;
                    if matches!(
                        value.kind,
                        Kind::Scalar(_, ScalarStyle::Literal | ScalarStyle::Folded)
                    ) && self.original(value)?.ends_with('\n')
                    {
                        replacement.push('\n');
                    }
                    self.spans.push((value.start, value.end, replacement));
                } else {
                    self.protect(value)?;
                }
            }
            Kind::Map(pairs)
                if executable
                    && pairs.len() == 1
                    && pairs[0]
                        .0
                        .scalar()
                        .is_some_and(|v| ["text", "num", "bool"].contains(&v)) =>
            {
                self.protect(&pairs[0].1)?
            }
            Kind::Map(pairs) => {
                for (key, value) in pairs {
                    if snapshot
                        && key.scalar() == Some("computed")
                        && let Kind::Map(parts) = &value.kind
                    {
                        for (part, payload) in parts {
                            if part.scalar() == Some("value") {
                                self.protect(payload)?;
                            } else if part.scalar() == Some("rule") {
                                self.visit(payload, true, false)?;
                            }
                        }
                    } else {
                        self.visit(
                            value,
                            executable
                                || key
                                    .scalar()
                                    .is_some_and(|v| v == "rule" || v == self.predicate),
                            snapshot || key.scalar() == Some(self.snapshot),
                        )?;
                    }
                }
            }
            Kind::List(values) => {
                for value in values {
                    self.visit(value, executable, snapshot)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
pub(crate) fn rewrite(
    text: &str,
    retired: &str,
    survivor: &str,
    predicate: &str,
    snapshot: &str,
) -> Result<String> {
    let mut visitor = Visitor {
        text,
        retired,
        survivor,
        predicate,
        snapshot,
        spans: vec![],
    };
    match parse(text) {
        Ok(node) => visitor.visit(&node, false, false)?,
        Err(error) if error.0 == "history_limit" => return Err(error),
        Err(_) => return Ok(tokens(text, retired, survivor)),
    }
    visitor.spans.sort();
    let mut start = 0;
    let mut output = String::new();
    for (left, right, replacement) in visitor.spans {
        require(
            start <= left && left <= right && right <= text.len(),
            "invalid_identity_yaml",
        )?;
        output.push_str(&tokens(&text[start..left], retired, survivor));
        output.push_str(&replacement);
        start = right;
    }
    output.push_str(&tokens(&text[start..], retired, survivor));
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn yaml_spans_match_token_and_literal_boundaries() {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/history-identity-text.json"))
                .unwrap();
        for (index, case) in data.as_array().unwrap().iter().enumerate() {
            let actual = rewrite(
                case["raw"].as_str().unwrap(),
                case["old"].as_str().unwrap(),
                case["new"].as_str().unwrap(),
                "wrong_if",
                "seen",
            );
            if let Some(expected) = case.get("output") {
                assert_eq!(actual.unwrap(), expected.as_str().unwrap(), "case {index}");
            } else {
                assert!(actual.is_err(), "case {index}");
            }
        }
    }
}

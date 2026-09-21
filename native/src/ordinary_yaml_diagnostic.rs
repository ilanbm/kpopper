//! PyYAML-compatible diagnostics for malformed ordinary hypothesis bytes.
//!
//! This is presentation only. Strict history decoding keeps its compact stable
//! error codes, and callers retain ownership of the already captured bytes.
use libyaml_safer::{EventData, Mark, Parser};

const LIMIT: usize = 120;

fn line(source: &str, wanted: usize) -> Vec<char> {
    let mut lines = vec![String::new()];
    let mut cr = false;
    for ch in source.chars() {
        if ch == '\r' {
            lines.push(String::new());
            cr = true;
        } else if ch == '\n' {
            if cr {
                cr = false;
            } else {
                lines.push(String::new());
            }
        } else if matches!(ch, '\u{85}' | '\u{2028}' | '\u{2029}') {
            lines.push(String::new());
            cr = false;
        } else {
            lines.last_mut().unwrap().push(ch);
            cr = false;
        }
    }
    lines
        .get(wanted)
        .map_or_else(Vec::new, |line| line.chars().collect())
}

fn mark(source: &str, mark: Mark) -> String {
    let content = line(source, mark.line as usize);
    let pointer = (mark.column as usize).min(content.len());
    let mut start = pointer;
    let mut head = "";
    while start > 0 {
        start -= 1;
        if pointer - start >= 37 {
            head = " ... ";
            start = (start + 5).min(pointer);
            break;
        }
    }
    let mut end = pointer;
    let mut tail = "";
    while end < content.len() {
        end += 1;
        if end - pointer >= 37 {
            tail = " ... ";
            end = end.saturating_sub(5);
            break;
        }
    }
    let snippet = content[start..end].iter().collect::<String>();
    format!(
        "  in \"<unicode string>\", line {}, column {}:\n    {head}{snippet}{tail}\n{}^",
        mark.line + 1,
        mark.column + 1,
        " ".repeat(4 + pointer - start + head.chars().count())
    )
}

fn normalized(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(LIMIT)
        .collect()
}

fn token_at(source: &str, mark: Mark) -> &'static str {
    let content = line(source, mark.line as usize);
    match content.get(mark.column as usize) {
        None => "'<stream end>'",
        Some('{') => "'{'",
        Some('[') => "'['",
        Some('}') => "'}'",
        Some(']') => "']'",
        _ => "'<scalar>'",
    }
}

fn python_problem(context: Option<&str>, problem: &str, source: &str, mark: Mark) -> String {
    match (context, problem) {
        (Some("while parsing a flow mapping"), "did not find expected ',' or '}'") => {
            format!("expected ',' or '}}', but got {}", token_at(source, mark))
        }
        (Some("while parsing a flow sequence"), "did not find expected ',' or ']'") => {
            format!("expected ',' or ']', but got {}", token_at(source, mark))
        }
        (Some("while parsing a block mapping"), "did not find expected key") => {
            "expected <block end>, but found '<block mapping start>'".into()
        }
        _ => problem.into(),
    }
}

fn parser_error(raw: &[u8]) -> Option<String> {
    let source = std::str::from_utf8(raw).ok()?;
    let mut parser = Parser::new();
    parser.set_input(raw);
    for _ in 0..800_000 {
        match parser.parse() {
            Ok(event) if matches!(event.data, EventData::StreamEnd) => return None,
            Ok(_) => {}
            Err(error) => {
                let problem_mark = error.problem_mark()?;
                let mut parts = Vec::new();
                if let Some(context) = error.context() {
                    parts.push(context.to_owned());
                }
                if let Some(context_mark) = error.context_mark()
                    && (context_mark.line != problem_mark.line
                        || context_mark.column != problem_mark.column)
                {
                    parts.push(mark(source, context_mark));
                }
                parts.push(python_problem(
                    error.context(),
                    error.problem(),
                    source,
                    problem_mark,
                ));
                parts.push(mark(source, problem_mark));
                return Some(normalized(&parts.join("\n")));
            }
        }
    }
    None
}

/// Format a malformed hypothesis exactly as the ordinary Python loader does.
/// A structural failure that libyaml can parse is preserved from the caller.
pub(crate) fn hypothesis_error(raw: &[u8], failure: Option<&crate::Error>) -> Option<String> {
    parser_error(raw).or_else(|| failure.map(|error| normalized(&error.0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn malformed_hypothesis_diagnostics_match_python() {
        let corpus: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/ordinary-yaml-diagnostics.json"
        ))
        .unwrap();
        for case in corpus["cases"].as_array().unwrap() {
            let actual = hypothesis_error(case["source"].as_str().unwrap().as_bytes(), None);
            assert_eq!(
                actual.as_deref(),
                case["expected"].as_str(),
                "{}",
                case["name"]
            );
        }
    }

    #[test]
    fn structural_failure_is_preserved_and_normalized_without_reparse_claims() {
        let error = crate::Error("hypothesis:  must be\n a mapping - claim".into());
        assert_eq!(
            hypothesis_error(b"hypothesis: []\n", Some(&error)).as_deref(),
            Some("hypothesis: must be a mapping - claim")
        );
        assert_eq!(hypothesis_error(b"known: {}\n", None), None);
    }
}

//! PyYAML-compatible diagnostics for malformed ordinary record and hypothesis bytes.
//!
//! This is presentation only. Strict history decoding keeps its compact stable
//! error codes, and callers retain ownership of the already captured bytes.
use libyaml_safer::{EventData, Mark, Parser, ScalarStyle, Scanner, TokenData};
use std::{collections::BTreeMap, path::Path};

const LIMIT: usize = 120;
/// Follows the name of a record file whose bytes do not parse, before the parser's account.
const NOT_YAML: &str = ": the record is not valid YAML.\n";

/// A place in the source as PyYAML counts it: lines by their breaks, columns in
/// characters. The byte offset orders two places.
#[derive(Clone, Copy)]
struct Point {
    index: usize,
    line: usize,
    column: usize,
}
/// A libyaml mark as a place in `source`. libyaml counts its offsets from after a byte
/// order mark at the start; a place counts from the first byte, as PyYAML's reader does.
fn point(source: &str, mark: Mark) -> Point {
    let bom = if source.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    Point {
        index: mark.index as usize + bom,
        line: mark.line as usize,
        column: mark.column as usize,
    }
}

/// What PyYAML's MarkedYAMLError carries: what the loader was reading and where it
/// began, and what it found there.
struct Diagnostic {
    context: Option<String>,
    context_mark: Option<Point>,
    problem: String,
    problem_mark: Point,
}
impl Diagnostic {
    /// The error as PyYAML prints it, naming the source as the loader was given it.
    fn render(&self, source: &str, name: &str) -> String {
        let mut parts = Vec::new();
        if let Some(context) = &self.context {
            parts.push(context.clone());
        }
        if let Some(at) = self.context_mark
            && (at.line, at.column) != (self.problem_mark.line, self.problem_mark.column)
        {
            parts.push(mark(source, name, at));
        }
        parts.push(self.problem.clone());
        parts.push(mark(source, name, self.problem_mark));
        parts.join("\n")
    }
}

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

/// How many characters of its line come before `at`: PyYAML's buffer pointer, which
/// counts a byte order mark where its column does not.
fn pointer(source: &str, at: Point) -> usize {
    let before = source.get(..at.index).unwrap_or(source);
    let start = before
        .char_indices()
        .rfind(|(_, ch)| matches!(ch, '\r' | '\n' | '\u{85}' | '\u{2028}' | '\u{2029}'))
        .map_or(0, |(at, ch)| at + ch.len_utf8());
    before[start..].chars().count()
}

fn mark(source: &str, name: &str, mark: Point) -> String {
    let content = line(source, mark.line);
    let pointer = pointer(source, mark).min(content.len());
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
        "  in \"{name}\", line {}, column {}:\n    {head}{snippet}{tail}\n{}^",
        mark.line + 1,
        mark.column + 1,
        " ".repeat(4 + pointer - start + head.chars().count())
    )
}

/// The place of a byte offset, counting breaks as `line` splits them.
fn point_at(source: &str, index: usize) -> Point {
    let (mut line, mut column, mut cr) = (0, 0, false);
    for ch in source[..index].chars() {
        match ch {
            '\n' if cr => {}
            '\r' | '\n' | '\u{85}' | '\u{2028}' | '\u{2029}' => {
                line += 1;
                column = 0;
            }
            '\u{feff}' => {}
            _ => column += 1,
        }
        cr = ch == '\r';
    }
    Point {
        index,
        line,
        column,
    }
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

/// PyYAML's name for the token that starts at `mark`.
fn token_at(source: &str, mark: Point) -> &'static str {
    let content = line(source, mark.line);
    let pointer = pointer(source, mark);
    let at = |offset: usize| content.get(pointer + offset).copied();
    match at(0) {
        None => "'<stream end>'",
        Some('{') => "'{'",
        Some('[') => "'['",
        Some('}') => "'}'",
        Some(']') => "']'",
        Some(',') => "','",
        Some(':') => "':'",
        Some('?') => "'?'",
        Some('-') if at(1).is_none_or(|c| c == ' ' || c == '\t') => "'-'",
        Some('&') => "'<anchor>'",
        Some('*') => "'<alias>'",
        Some('!') => "'<tag>'",
        _ => "'<scalar>'",
    }
}

/// A character as Python's repr shows it.
fn repr(ch: char) -> String {
    match ch {
        '\t' => "'\\t'".into(),
        '\n' => "'\\n'".into(),
        '\r' => "'\\r'".into(),
        '\'' => "\"'\"".into(),
        '\\' => "'\\\\'".into(),
        ch if ch.is_control() && (ch as u32) < 0x100 => format!("'\\x{:02x}'", ch as u32),
        ch if ch.is_control() => format!("'\\u{:04x}'", ch as u32),
        ch => format!("'{ch}'"),
    }
}

/// The character at `mark`, as PyYAML's reader peeks it: NUL past the end.
fn char_at(source: &str, mark: Point) -> char {
    source
        .get(mark.index..)
        .and_then(|rest| rest.chars().next())
        .unwrap_or('\0')
}

/// Whether a plain key starts at `mark`: before any comment, the rest of its line holds a
/// ':' that reads as the value indicator. PyYAML puts a key token there.
fn simple_key(source: &str, mark: Point) -> bool {
    let content = line(source, mark.line);
    let rest = content.get(pointer(source, mark)..).unwrap_or_default();
    let blank = |at: usize| rest.get(at).is_none_or(|c| *c == ' ' || *c == '\t');
    for (at, ch) in rest.iter().enumerate() {
        match ch {
            '#' if at > 0 && blank(at - 1) => return false,
            ':' if blank(at + 1) => return true,
            _ => {}
        }
    }
    false
}

/// The place one character on from `mark`, on the same line.
fn after(source: &str, mark: Point) -> Point {
    let width = char_at(source, mark).len_utf8();
    Point {
        index: mark.index + width,
        line: mark.line,
        column: mark.column + 1,
    }
}

/// libyaml's context in PyYAML's words: the escapes libyaml reads in a quoted scalar are
/// the double-quoted scalar's.
fn python_context(context: &str) -> &str {
    match context {
        "while parsing a quoted scalar" => "while scanning a double-quoted scalar",
        context => context,
    }
}

/// libyaml's problem in PyYAML's words, where PyYAML words the same failure differently.
fn python_problem(context: Option<&str>, problem: &str, source: &str, mark: Point) -> String {
    let token = || token_at(source, mark);
    let found = || repr(char_at(source, mark));
    match (context, problem) {
        // libyaml marks the backslash, PyYAML the character after it.
        (Some("while parsing a quoted scalar"), "found unknown escape character") => format!(
            "found unknown escape character {}",
            repr(char_at(source, after(source, mark)))
        ),
        (Some("while parsing a quoted scalar"), "did not find expected hexdecimal number") => {
            let code = source.get(..mark.index).and_then(|s| s.chars().next_back());
            let length = match code {
                Some('x') => 2,
                Some('u') => 4,
                _ => 8,
            };
            let digits = source.get(mark.index..).unwrap_or_default().chars();
            let found = digits.chain(std::iter::repeat('\0'));
            format!(
                "expected escape sequence of {length} hexadecimal numbers, but found {}",
                repr(
                    found
                        .take(length)
                        .find(|c| !c.is_ascii_hexdigit())
                        .unwrap_or('\0')
                )
            )
        }
        (Some("while parsing a block collection"), "did not find expected '-' indicator") => {
            format!(
                "expected <block end>, but found {}",
                if simple_key(source, mark) {
                    "'?'"
                } else {
                    token()
                }
            )
        }
        (None, "did not find expected <document start>") => format!(
            "expected '<document start>', but found {}",
            match token() {
                "'-'" => "'<block sequence start>'",
                _ if simple_key(source, mark) => "'<block mapping start>'",
                token => token,
            }
        ),
        (Some("while parsing a flow mapping"), "did not find expected ',' or '}'") => {
            format!("expected ',' or '}}', but got {}", token())
        }
        (Some("while parsing a flow sequence"), "did not find expected ',' or ']'") => {
            format!("expected ',' or ']', but got {}", token())
        }
        // A key further in than the mapping's own opens a mapping there.
        (Some("while parsing a block mapping"), "did not find expected key") => format!(
            "expected <block end>, but found {}",
            match token() {
                "'<scalar>'" => "'<block mapping start>'",
                token => token,
            }
        ),
        (
            Some("while parsing a flow node" | "while parsing a block node"),
            "did not find expected node content",
        ) => {
            format!("expected the node content, but found {}", token())
        }
        (
            Some("while scanning for the next token"),
            "found character that cannot start any token",
        ) => {
            format!("found character {} that cannot start any token", found())
        }
        (
            Some("while scanning an anchor" | "while scanning an alias"),
            "did not find expected alphabetic or numeric character",
        ) => format!(
            "expected alphabetic or numeric character, but found {}",
            found()
        ),
        (None, "mapping values are not allowed in this context") => {
            "mapping values are not allowed here".into()
        }
        (None, "mapping keys are not allowed in this context") => {
            "mapping keys are not allowed here".into()
        }
        (None, "block sequence entries are not allowed in this context") => {
            "sequence entries are not allowed here".into()
        }
        _ => problem.into(),
    }
}

/// Whether PyYAML's safe loader constructs a node with this tag, or reads it as a merge or
/// value key.
fn constructed(tag: &str) -> bool {
    tag == "!"
        || tag.strip_prefix("tag:yaml.org,2002:").is_some_and(|name| {
            [
                "null",
                "bool",
                "int",
                "float",
                "binary",
                "timestamp",
                "omap",
                "pairs",
                "set",
                "str",
                "seq",
                "map",
                "merge",
                "value",
            ]
            .contains(&name)
        })
}

/// The first error PyYAML raises while it loads these bytes, found with libyaml's parser:
/// a syntax error, an alias to no anchor, an anchor given twice or a second document -
/// and only once the whole stream composes, a tag the safe loader has no constructor for.
fn parse_error(raw: &[u8], source: &str) -> Option<Diagnostic> {
    let mut parser = Parser::new();
    parser.set_input(raw);
    let mut anchors = BTreeMap::new();
    let (mut documents, mut root, mut unconstructed) = (0, None, None);
    // libyaml ends a stream that lacks a final line break on a line of its own;
    // PyYAML's end is where the bytes end.
    let placed = |mark: Mark| match point(source, mark) {
        at if at.index >= source.len() => point_at(source, source.len()),
        at => at,
    };
    for _ in 0..800_000 {
        let event = match parser.parse() {
            Ok(event) => event,
            Err(error) => {
                let marked = placed(error.problem_mark()?);
                return Some(Diagnostic {
                    context: error.context().map(|c| python_context(c).to_owned()),
                    context_mark: error.context_mark().map(placed),
                    problem: python_problem(error.context(), error.problem(), source, marked),
                    problem_mark: if error.problem() == "found unknown escape character" {
                        after(source, marked)
                    } else {
                        marked
                    },
                });
            }
        };
        let at = point(source, event.start_mark);
        if let EventData::Scalar { tag: Some(tag), .. }
        | EventData::SequenceStart { tag: Some(tag), .. }
        | EventData::MappingStart { tag: Some(tag), .. } = &event.data
            && !constructed(tag)
        {
            unconstructed.get_or_insert(Diagnostic {
                context: None,
                context_mark: None,
                problem: format!("could not determine a constructor for the tag '{tag}'"),
                problem_mark: at,
            });
        }
        let anchor = match &event.data {
            EventData::StreamEnd => return unconstructed,
            EventData::DocumentStart { .. } => {
                documents += 1;
                if documents > 1 {
                    return Some(Diagnostic {
                        context: Some("expected a single document in the stream".into()),
                        context_mark: root,
                        problem: "but found another document".into(),
                        problem_mark: at,
                    });
                }
                continue;
            }
            EventData::Alias { anchor } => {
                root.get_or_insert(at);
                if !anchors.contains_key(anchor) {
                    return Some(Diagnostic {
                        context: None,
                        context_mark: None,
                        problem: format!("found undefined alias '{anchor}'"),
                        problem_mark: at,
                    });
                }
                continue;
            }
            EventData::Scalar { anchor, .. }
            | EventData::SequenceStart { anchor, .. }
            | EventData::MappingStart { anchor, .. } => {
                root.get_or_insert(at);
                anchor
            }
            _ => continue,
        };
        if let Some(anchor) = anchor {
            if let Some(first) = anchors.get(anchor) {
                return Some(Diagnostic {
                    context: Some(format!(
                        "found duplicate anchor '{anchor}'; first occurrence"
                    )),
                    context_mark: Some(*first),
                    problem: "second occurrence".into(),
                    problem_mark: at,
                });
            }
            anchors.insert(anchor.clone(), at);
        }
    }
    None
}

/// Where a run of bytes between tokens holds a tab outside a comment.
fn gap_tab(gap: &[u8]) -> Option<usize> {
    let mut comment = false;
    for (at, ch) in std::str::from_utf8(gap).ok()?.char_indices() {
        match ch {
            '#' => comment = true,
            '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}' => comment = false,
            '\t' if !comment => return Some(at),
            _ => {}
        }
    }
    None
}

/// The first tab the Python loader refuses where libyaml reads on: PyYAML takes a tab as
/// neither indentation nor a separator, so one between tokens outside a comment, or in
/// a token other than a quoted scalar, stops it. The ordinary decoder refuses the same
/// tabs.
fn refused_tab(raw: &[u8], source: &str) -> Option<Diagnostic> {
    if !raw.contains(&b'\t') {
        return None;
    }
    let found = |index| {
        let at = point_at(source, index);
        Diagnostic {
            context: Some("while scanning for the next token".into()),
            context_mark: Some(at),
            problem: "found character '\\t' that cannot start any token".into(),
            problem_mark: at,
        }
    };
    // A directive is scanned to the end of its line, so a tab after its name or its
    // value stops that scan.
    let in_directive = |start, index, problem: &str| Diagnostic {
        context: Some("while scanning a directive".into()),
        context_mark: Some(point_at(source, start)),
        problem: problem.into(),
        problem_mark: point_at(source, index),
    };
    let mut scanner = Scanner::new();
    scanner.set_input(raw);
    let (mut end, mut directive) = (0, None);
    for _ in 0..800_000 {
        let token = match Scanner::scan(&mut scanner) {
            Ok(token) => token,
            // A tab before the token the scanner could not read comes first. The scanner
            // can fail while it looks ahead past tokens it has not returned yet; a quoted
            // scalar among them may hold tabs, so such a stretch names none.
            Err(error) => {
                let stop = point(source, error.context_mark().or(error.problem_mark())?).index;
                let gap = raw.get(end..stop.max(end))?;
                if gap.contains(&b'"') || gap.contains(&b'\'') {
                    return None;
                }
                return Some(found(end + gap_tab(gap)?));
            }
        };
        let (start, stop) = (
            point(source, token.start_mark).index,
            point(source, token.end_mark).index,
        );
        let text = raw.get(start..stop)?;
        if start > end
            && let Some(at) = gap_tab(raw.get(end..start)?)
        {
            let gap = &raw[end..end + at];
            return Some(match directive {
                // Right after its value the scan wants the value's end; past blanks, a
                // comment or the line's end.
                Some((directive, value_end)) if gap.is_empty() => {
                    in_directive(directive, end + at, value_end)
                }
                Some((directive, _)) if !gap.contains(&b'\n') && !gap.contains(&b'\r') => {
                    in_directive(
                        directive,
                        end + at,
                        "expected a comment or a line break, but found '\\t'",
                    )
                }
                _ => found(end + at),
            });
        }
        directive = None;
        match token.data {
            TokenData::StreamEnd => return None,
            TokenData::Scalar {
                style: ScalarStyle::SingleQuoted | ScalarStyle::DoubleQuoted,
                ..
            } => {}
            TokenData::Scalar {
                style: ScalarStyle::Literal | ScalarStyle::Folded,
                ..
            } => {
                // The header line: the indicator, its chomping and indentation marks,
                // then blanks and a comment.
                let header = text
                    .split(|c| *c == b'\n' || *c == b'\r')
                    .next()
                    .unwrap_or_default();
                if let Some(at) = gap_tab(header) {
                    let marks = header[1..]
                        .iter()
                        .take_while(|c| b"+-123456789".contains(c))
                        .count();
                    return Some(Diagnostic {
                        context: Some("while scanning a block scalar".into()),
                        context_mark: Some(point_at(source, start)),
                        problem: if at == 1 + marks {
                            "expected chomping or indentation indicators, but found '\\t'"
                        } else {
                            "expected a comment or a line break, but found '\\t'"
                        }
                        .into(),
                        problem_mark: point_at(source, start + at),
                    });
                }
            }
            TokenData::VersionDirective { .. } | TokenData::TagDirective { .. } => {
                let value_end = match token.data {
                    TokenData::VersionDirective { .. } => {
                        "expected a digit or ' ', but found '\\t'"
                    }
                    _ => "expected ' ', but found '\\t'",
                };
                if let Some(at) = text.iter().position(|c| *c == b'\t') {
                    let name = text[1..]
                        .iter()
                        .take_while(|c| c.is_ascii_alphanumeric() || b"-_".contains(c))
                        .count();
                    return Some(if at == 1 + name {
                        in_directive(
                            start,
                            start + at,
                            "expected alphabetic or numeric character, but found '\\t'",
                        )
                    } else {
                        found(start + at)
                    });
                }
                directive = Some((start, value_end));
            }
            _ => {
                if let Some(at) = text.iter().position(|c| *c == b'\t') {
                    return Some(found(start + at));
                }
            }
        }
        end = end.max(stop);
    }
    None
}

/// The first error PyYAML raises while it composes these bytes, whichever comes first.
fn diagnose(raw: &[u8], source: &str) -> Option<Diagnostic> {
    let parsed = parse_error(raw, source);
    match refused_tab(raw, source) {
        Some(tab)
            if parsed
                .as_ref()
                .is_none_or(|error| tab.problem_mark.index <= error.problem_mark.index) =>
        {
            Some(tab)
        }
        _ => parsed,
    }
}

/// Format a malformed hypothesis exactly as the ordinary Python loader does.
/// A structural failure that libyaml can parse is preserved from the caller.
pub(crate) fn hypothesis_error(raw: &[u8], failure: Option<&crate::Error>) -> Option<String> {
    std::str::from_utf8(raw)
        .ok()
        .and_then(|source| {
            Some(normalized(
                &diagnose(raw, source)?.render(source, "<unicode string>"),
            ))
        })
        .or_else(|| failure.map(|error| normalized(&error.0)))
}

/// A record file that does not parse as YAML, named as the person reading the message
/// would type it, with PyYAML's account of where and why. Any other failure, and one the
/// parser cannot place (a date that is no date, bytes that are not UTF-8), keeps its code.
pub(crate) fn record_error(path: &Path, raw: &[u8], failure: crate::Error) -> crate::Error {
    if failure.0 != "invalid_history_yaml" {
        return failure;
    }
    let Ok(source) = std::str::from_utf8(raw) else {
        return failure;
    };
    let Some(diagnostic) = diagnose(raw, source) else {
        return failure;
    };
    // The directory is resolved as the working directory is, so a path given through a
    // link to it still reads relative; the file keeps its own name.
    let path = path
        .parent()
        .and_then(|directory| directory.canonicalize().ok())
        .zip(path.file_name())
        .map_or_else(
            || path.to_path_buf(),
            |(directory, file)| directory.join(file),
        );
    let name = std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok())
        .unwrap_or(&path)
        .display()
        .to_string();
    crate::Error(format!(
        "{name}{NOT_YAML}{}",
        diagnostic.render(source, &name)
    ))
}

/// Whether a failure is `record_error`'s account rather than a code.
pub(crate) fn explains_record(failure: &crate::Error) -> bool {
    failure.0.contains(NOT_YAML)
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
    fn malformed_records_are_told_as_pyyaml_tells_them() {
        let corpus: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/ordinary-yaml-record-diagnostics.json"
        ))
        .unwrap();
        let mut differ = Vec::new();
        for case in corpus["cases"].as_array().unwrap() {
            let source = case["source"].as_str().unwrap();
            let expected = case["expected"].as_str().unwrap();
            // The account is given only where the ordinary decoder refuses the bytes.
            assert_eq!(
                crate::history_yaml::decode_full_ordinary_source_value(source.as_bytes())
                    .err()
                    .map(|error| error.0),
                Some("invalid_history_yaml".to_owned()),
                "{}",
                case["name"]
            );
            let told = diagnose(source.as_bytes(), source)
                .map(|diagnostic| diagnostic.render(source, "<unicode string>"));
            if told.as_deref() != Some(expected)
                || hypothesis_error(source.as_bytes(), None) != Some(normalized(expected))
            {
                differ.push(format!(
                    "{}:\n{}\n-- PyYAML:\n{expected}",
                    case["name"],
                    told.unwrap_or_default()
                ));
            }
        }
        assert!(differ.is_empty(), "{}", differ.join("\n\n"));
    }

    #[test]
    fn a_record_that_does_not_parse_is_named_with_the_account() {
        let refused = || crate::Error("invalid_history_yaml".into());
        let raw = b"known:\n  a.b: {v: 1\n  c.d: [unclosed\n";
        let here = std::env::current_dir().unwrap().join("GROUNDING.yaml");
        let told = record_error(&here, raw, refused());
        assert_eq!(
            told.0,
            "GROUNDING.yaml: the record is not valid YAML.\nwhile parsing a flow mapping\n  in \"GROUNDING.yaml\", line 2, column 8:\n      a.b: {v: 1\n           ^\nexpected ',' or '}', but got ':'\n  in \"GROUNDING.yaml\", line 3, column 6:\n      c.d: [unclosed\n         ^"
        );
        assert!(explains_record(&told));
        let elsewhere = Path::new("/nowhere/near/notes.yaml");
        assert!(
            record_error(elsewhere, raw, refused())
                .0
                .starts_with("/nowhere/near/notes.yaml: the record is not valid YAML.\n")
        );
        // A value the parser reads, and a refusal that is not the parser's, keep their codes.
        let date = record_error(&here, b"known:\n  p.a: {v: 2026-02-30}\n", refused());
        assert_eq!(date.0, "invalid_history_yaml");
        assert!(!explains_record(&date));
        assert_eq!(
            record_error(&here, raw, crate::Error("history_limit".into())).0,
            "history_limit"
        );
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

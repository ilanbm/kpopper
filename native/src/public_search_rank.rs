//! Bounded local evidence ranking and exact corpus-revision reads.
use crate::{Error, Result, identity::sha256, require, value::TypedValue};
use serde_json::{Value, json};
use std::collections::BTreeSet;

const SCOPE: &str = "Matches locate evidence; recorded claims, hypotheses and captured reports retain their own status. Source truth is not verified.";

pub(crate) fn encode(value: &Value) -> Result<String> {
    crate::ordinary_assessment_report::legacy_json(&TypedValue::from_json(value)?)
}

fn excerpt(content: &str, terms: &[String]) -> Result<Value> {
    // Python re.IGNORECASE includes these four non-ASCII equivalents for ASCII
    // letters, unlike full casefold (which can expand one scalar into several).
    let alternatives = terms
        .iter()
        .map(|term| {
            term.chars()
                .map(|ch| match ch {
                    'i' | 'I' | 'ı' | 'İ' => "[iIıİ]".into(),
                    's' | 'S' | 'ſ' => "[sSſ]".into(),
                    'k' | 'K' | 'K' => "[kKK]".into(),
                    _ => regex::escape(&ch.to_string()),
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let anchor = if alternatives.is_empty() {
        0
    } else {
        let re = regex::RegexBuilder::new(&alternatives.join("|"))
            .case_insensitive(true)
            .build()
            .map_err(|e| Error(e.to_string()))?;
        re.find(content)
            .map_or(0, |m| content[..m.start()].chars().count())
    };
    let chars = content.chars().collect::<Vec<_>>();
    let mut start = anchor.saturating_sub(80);
    if let Some(index) = chars[start..anchor].iter().rposition(|c| *c == '\n') {
        start += index + 1;
    }
    let end = (start + 320).min(chars.len());
    Ok(
        json!({"excerpt":chars[start..end].iter().collect::<String>(),"offset":start,
        "line":chars[..anchor].iter().filter(|c| **c == '\n').count()+1,
        "excerpt_truncated":start>0 || end<chars.len(),"total_characters":chars.len()}),
    )
}

fn sql_error(error: rusqlite::Error) -> Error {
    Error(error.to_string())
}

pub(crate) fn search(mut corpus: Value, query: &str, limit: i64, chars: i64) -> Result<Value> {
    require(
        !query.trim().is_empty() && query.chars().count() <= 2000,
        "query must contain 1..2000 characters",
    )?;
    require(
        (1..=50).contains(&limit) && (500..=100000).contains(&chars),
        "limit must be 1..50; chars must be 500..100000",
    )?;
    let mut seen = BTreeSet::new();
    let terms = crate::session_search::terms(query)
        .into_iter()
        .filter(|t| seen.insert(t.clone()))
        .take(32)
        .collect::<Vec<_>>();
    let rows = corpus
        .as_object_mut()
        .ok_or_else(|| Error("invalid search corpus".into()))?
        .remove("rows")
        .and_then(|rows| rows.as_array().cloned())
        .ok_or_else(|| Error("invalid search corpus rows".into()))?;
    let mut hits = vec![];
    if !terms.is_empty() {
        let connection = rusqlite::Connection::open_in_memory().map_err(sql_error)?;
        connection.execute("CREATE VIRTUAL TABLE evidence USING fts5(identity, name, content, tokenize='unicode61')", []).map_err(sql_error)?;
        {
            let mut insert = connection
                .prepare("INSERT INTO evidence(rowid, identity, name, content) VALUES (?, ?, ?, ?)")
                .map_err(sql_error)?;
            for (i, row) in rows.iter().enumerate() {
                let mut text = row["content"]
                    .as_str()
                    .ok_or_else(|| Error("invalid search content".into()))?
                    .to_owned();
                if let Some(value) = row["value_text"].as_str() {
                    text.push('\n');
                    text.push_str(value);
                } else if !row["calculation"]["value"].is_null() {
                    text.push('\n');
                    text.push_str(&crate::public_ordinary_readers::display(
                        &TypedValue::from_json(&row["calculation"]["value"])?,
                    ));
                }
                insert
                    .execute(rusqlite::params![
                        i + 1,
                        row["id"].as_str(),
                        row["name"].as_str(),
                        text
                    ])
                    .map_err(sql_error)?;
            }
        }
        let expression = terms
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let mut statement = connection.prepare("SELECT rowid FROM evidence WHERE evidence MATCH ? ORDER BY bm25(evidence, 8, 4, 1), rowid").map_err(sql_error)?;
        hits = statement
            .query_map([expression], |r| r.get::<_, usize>(0))
            .map_err(sql_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sql_error)?;
    }
    let result = corpus.as_object_mut().unwrap();
    let unindexed = result
        .get("unindexed")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    result.extend([
        ("query".into(), json!(query)),
        ("scope".into(), json!(SCOPE)),
        ("results".into(), json!([])),
        ("omitted".into(), json!(hits.len())),
        ("unindexed_count".into(), json!(unindexed)),
        ("unindexed_omitted".into(), json!(0)),
    ]);
    for i in hits.iter().take(limit as usize) {
        let mut row = rows[*i - 1].as_object().unwrap().clone();
        let content = row.remove("content").unwrap();
        let content = content.as_str().unwrap();
        row.extend(excerpt(content, &terms)?.as_object().unwrap().clone());
        row.insert("sha256".into(), json!(sha256(content.as_bytes())));
        result
            .get_mut("results")
            .unwrap()
            .as_array_mut()
            .unwrap()
            .push(Value::Object(row));
    }
    result.insert(
        "omitted".into(),
        json!(hits.len().saturating_sub(limit as usize)),
    );
    for (field, count) in [("unindexed", "unindexed_omitted"), ("results", "omitted")] {
        while encode(&corpus)?.chars().count() > chars as usize
            && corpus[field].as_array().is_some_and(|v| !v.is_empty())
        {
            corpus[field].as_array_mut().unwrap().pop();
            corpus[count] = json!(corpus[count].as_u64().unwrap() + 1);
        }
    }
    require(
        encode(&corpus)?.chars().count() <= chars as usize,
        "chars budget cannot carry search metadata; increase --chars",
    )?;
    Ok(corpus)
}

pub(crate) fn read(
    corpus: &Value,
    reference: &str,
    revision: &str,
    offset: i64,
    length: i64,
) -> Result<Value> {
    require(
        corpus["revision"] == revision,
        "record, capture state or source changed; search again",
    )?;
    let row = corpus["rows"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["ref"] == reference))
        .ok_or_else(|| Error("unknown search reference".into()))?;
    let text = row["content"]
        .as_str()
        .ok_or_else(|| Error("invalid search content".into()))?;
    let chars = text.chars().collect::<Vec<_>>();
    require(
        offset >= 0 && offset as usize <= chars.len() && (1..=100000).contains(&length),
        "offset must address this text; length must be 1..100000",
    )?;
    let start = offset as usize;
    let end = (start + length as usize).min(chars.len());
    let mut result = json!({"ref":reference,"revision":revision,"scope":row["scope"],"status":row["status"],
        "content":chars[start..end].iter().collect::<String>(),"offset":start,
        "next_offset":if end<chars.len(){json!(end)}else{Value::Null},"complete":start==0&&end==chars.len(),
        "total_characters":chars.len(),"sha256":sha256(text.as_bytes())});
    for (data, keys) in [
        (
            corpus,
            &["snapshot_id", "findings_revision", "corpus_revision"][..],
        ),
        (
            row,
            &[
                "calculation",
                "assessment",
                "publication",
                "conflict",
                "findings",
                "value_text",
            ][..],
        ),
    ] {
        for key in keys {
            if let Some(value) = data.get(*key) {
                result[*key] = value.clone();
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranking_budget_excerpt_and_reads_match_complete_python_outputs() {
        let fixture: Value = serde_json::from_slice(include_bytes!(
            "../tests/fixtures/local-search-ranking.json"
        ))
        .unwrap();
        for case in fixture["search"].as_array().unwrap() {
            let a = &case["args"];
            let result = search(
                fixture["corpus"].clone(),
                a["query"].as_str().unwrap(),
                a["limit"].as_i64().unwrap(),
                a["chars"].as_i64().unwrap(),
            );
            if let Some(error) = case["error"].as_str() {
                assert_eq!(result.unwrap_err().0, error, "{a}");
            } else {
                assert_eq!(result.unwrap(), case["value"], "{a}");
            }
        }
        for case in fixture["read"].as_array().unwrap() {
            let a = &case["args"];
            let result = read(
                &fixture["corpus"],
                a["ref"].as_str().unwrap(),
                a["revision"].as_str().unwrap(),
                a["offset"].as_i64().unwrap(),
                a["length"].as_i64().unwrap(),
            );
            if let Some(error) = case["error"].as_str() {
                assert_eq!(result.unwrap_err().0, error, "{a}");
            } else {
                assert_eq!(result.unwrap(), case["value"], "{a}");
            }
        }
    }
}

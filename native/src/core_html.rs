//! Static HTML renderer for the source-free `page-secondary/v1` envelope.
//! This module deliberately performs presentation validation only.

use serde_json::Value;

const DIMENSIONS: &[&str] = &[
    "acceptance",
    "computation",
    "basis",
    "falsifier",
    "contention",
    "integrity",
    "coverage",
    "assurance",
    "support",
];

fn err(message: &str) -> Result<String, String> {
    Err(message.to_owned())
}

fn text<'a>(value: &'a Value, name: &str) -> Result<&'a str, String> {
    value.as_str().ok_or_else(|| format!("{name} must be text"))
}

fn esc(value: &str, attribute: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\'' if attribute => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn safe_href(raw: &str) -> bool {
    let s = raw.trim();
    if s.is_empty() {
        return true;
    }
    if s.starts_with("//") || s.chars().any(|c| (c as u32) < 32) {
        return false;
    }
    let Some((scheme, tail)) = s.split_once(':').filter(|(scheme, _)| {
        scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    }) else {
        return true;
    };
    let scheme = scheme.to_ascii_lowercase();
    if let Some(rest) = tail.strip_prefix("//") {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if (matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https")
            && (authority.is_empty() || authority.chars().any(char::is_whitespace)))
            || authority.contains(['@', '\\', '<', '>', '"', '\''])
        {
            return false;
        }
    }
    matches!(scheme.as_str(), "http" | "https" | "mailto" | "file")
}

fn push(out: &mut String, part: &str, max: usize) -> Result<(), String> {
    if part.len() > max.saturating_sub(out.len()) {
        return Err(format!(
            "html output limit exceeded ({max} bytes; at least {} needed)",
            out.len().saturating_add(part.len())
        ));
    }
    out.push_str(part);
    Ok(())
}

fn status_rows(node: &Value, out: &mut String, max: usize) -> Result<(), String> {
    let status = node.get("status").ok_or("node status is required")?;
    let object = status.as_object().ok_or("node status must be an object")?;
    for dimension in DIMENSIONS {
        let value = object
            .get(*dimension)
            .ok_or_else(|| format!("missing status dimension {dimension}"))?;
        let mut rendered = if *dimension == "computation" {
            value
                .get("status")
                .and_then(Value::as_str)
                .ok_or("computation status must be text")?
        } else if *dimension == "falsifier" || *dimension == "support" {
            value
                .get("status")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{dimension} status must be text"))?
        } else {
            text(value, dimension)?
        }
        .to_owned();
        let mut suffix = String::new();
        if *dimension == "support"
            && let Some(states) = value.get("states").and_then(Value::as_array)
        {
            let states = states
                .iter()
                .map(|s| text(s, "support state"))
                .collect::<Result<Vec<_>, _>>()?;
            if !states.is_empty() {
                suffix = format!(" [{}]", states.join(", "));
            }
        }
        rendered.push_str(&suffix);
        push(
            out,
            &format!(
                "<li data-state-dimension=\"{}\"><span>{}</span> {}</li>",
                esc(dimension, true),
                esc(dimension, false),
                esc(&rendered, false)
            ),
            max,
        )?;
    }
    Ok(())
}

fn validate_and_index(values: &Value) -> Result<std::collections::BTreeMap<String, Value>, String> {
    let nodes = values
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or("page values nodes must be an array")?;
    let mut index = std::collections::BTreeMap::new();
    for node in nodes {
        let object = node.as_object().ok_or("node must be an object")?;
        let id = text(object.get("id").ok_or("node id is required")?, "node id")?;
        if id.is_empty() || index.insert(id.to_owned(), node.clone()).is_some() {
            return Err("node ids must be unique and non-empty".into());
        }
        for field in ["kind", "label", "status_text"] {
            text(
                object
                    .get(field)
                    .ok_or_else(|| format!("node {field} is required"))?,
                field,
            )?;
        }
        for field in ["value", "rule", "href"] {
            let value = object
                .get(field)
                .ok_or_else(|| format!("node {field} is required"))?;
            if !value.is_null() {
                let value = text(value, field)?;
                if field == "href" && !safe_href(value) {
                    return Err("unsafe supplied href".into());
                }
            }
        }
        for field in ["dependencies", "flags"] {
            let values = object
                .get(field)
                .and_then(Value::as_array)
                .ok_or_else(|| format!("node {field} must be an array"))?;
            for value in values {
                text(value, field)?;
            }
        }
        status_rows(node, &mut String::new(), 1024 * 1024)?;
    }
    Ok(index)
}

fn card(node: &Value, out: &mut String, max: usize) -> Result<(), String> {
    let id = text(&node["id"], "node id")?;
    let kind = text(&node["kind"], "node kind")?;
    push(
        out,
        &format!(
            "<article class=\"core-node {}\" data-id=\"{}\"><h3>",
            esc(kind, true),
            esc(id, true)
        ),
        max,
    )?;
    let label = text(&node["label"], "node label")?;
    if let Some(href) = node
        .get("href")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        push(
            out,
            &format!("<a href=\"{}\">{}</a>", esc(href, true), esc(label, false)),
            max,
        )?;
    } else {
        push(out, &esc(label, false), max)?;
    }
    push(out, "</h3>", max)?;
    match node.get("value") {
        Some(Value::String(value)) => push(
            out,
            &format!(
                "<div class=\"core-value\" data-value-kind=\"typed\">{}</div>",
                esc(value, false)
            ),
            max,
        )?,
        _ => match node.get("rule").and_then(Value::as_str) {
            Some(rule) if !rule.is_empty() => push(
                out,
                &format!("<div class=\"core-rule\">= {}</div>", esc(rule, false)),
                max,
            )?,
            _ => push(
                out,
                "<div class=\"core-value unavailable\">unavailable</div>",
                max,
            )?,
        },
    }
    push(out, "<ul class=\"core-state\">", max)?;
    status_rows(node, out, max)?;
    push(out, "</ul>", max)?;
    if let Some(deps) = node.get("dependencies").and_then(Value::as_array) {
        push(out, "<div class=\"core-dependencies\">", max)?;
        for dep in deps {
            push(
                out,
                &format!(
                    "<code class=\"core-dependency\">{}</code>",
                    esc(text(dep, "dependency")?, false)
                ),
                max,
            )?;
        }
        push(out, "</div>", max)?;
    }
    push(out, "</article>", max)
}

/// Render one validated `page-secondary/v1` envelope as standalone static HTML.
pub fn render(page: &Value, max_bytes: usize) -> Result<String, String> {
    if max_bytes == 0 {
        return err(&format!("html output limit exceeded ({max_bytes} bytes)"));
    }
    let object = page.as_object().ok_or("page envelope must be an object")?;
    if object.get("schema_version").and_then(Value::as_u64) != Some(1)
        || object.get("assessment_profile").and_then(Value::as_str) != Some("page-secondary/v1")
        || object
            .get("page_projection_version")
            .and_then(Value::as_u64)
            != Some(1)
    {
        return err("unsupported page-secondary/v1 envelope");
    }
    let snapshot = text(
        object.get("snapshot_id").ok_or("snapshot_id is required")?,
        "snapshot_id",
    )?;
    let findings = text(
        object
            .get("findings_revision")
            .ok_or("findings_revision is required")?,
        "findings_revision",
    )?;
    let page_revision = text(
        object
            .get("page_assessment_revision")
            .ok_or("page_assessment_revision is required")?,
        "page_assessment_revision",
    )?;
    let inputs = object
        .get("page_inputs")
        .and_then(Value::as_object)
        .ok_or("page_inputs is required")?;
    text(
        inputs
            .get("revision")
            .ok_or("page_inputs.revision is required")?,
        "page_inputs.revision",
    )?;
    if !object.get("brief_identity").is_some_and(Value::is_object) {
        return err("brief_identity must be an object");
    }
    let values = inputs
        .get("values")
        .ok_or("page_inputs.values is required")?;
    let title = text(
        values.get("title").ok_or("page title is required")?,
        "title",
    )?;
    let language = text(
        values.get("language").ok_or("language is required")?,
        "language",
    )?;
    let direction = text(
        values.get("direction").ok_or("direction is required")?,
        "direction",
    )?;
    if direction != "ltr" && direction != "rtl" {
        return err("direction must be ltr or rtl");
    }
    let now_label = match language {
        "he" => "עכשיו",
        "ar" => "الآن",
        _ => "Now",
    };
    let record_label = match language {
        "he" => "הרשומה",
        "ar" => "السجل",
        _ => "Record",
    };
    let index = validate_and_index(values)?;
    let arrangements = values
        .get("arrangements")
        .and_then(Value::as_array)
        .ok_or("arrangements must be an array")?;
    let mut out = String::new();
    push(&mut out, "<!doctype html><html lang=\"", max_bytes)?;
    push(&mut out, &esc(language, true), max_bytes)?;
    push(&mut out, "\" dir=\"", max_bytes)?;
    push(&mut out, &esc(direction, true), max_bytes)?;
    push(
        &mut out,
        "\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta name=\"kpopper-assessment-profile\" content=\"core/v1\"><meta name=\"kpopper-snapshot-id\" content=\"",
        max_bytes,
    )?;
    push(&mut out, &esc(snapshot, true), max_bytes)?;
    push(
        &mut out,
        "\"><meta name=\"kpopper-findings-revision\" content=\"",
        max_bytes,
    )?;
    push(&mut out, &esc(findings, true), max_bytes)?;
    push(
        &mut out,
        "\"><meta name=\"kpopper-page-assessment-revision\" content=\"",
        max_bytes,
    )?;
    push(&mut out, &esc(page_revision, true), max_bytes)?;
    push(&mut out, "\"><title>", max_bytes)?;
    push(
        &mut out,
        &esc(&title.chars().take(60).collect::<String>(), false),
        max_bytes,
    )?;
    push(
        &mut out,
        "</title><style>body{font-family:system-ui,sans-serif;margin:0;background:#f7f7f5;color:#20201e}.core-wrap{max-width:960px;margin:auto;padding:32px}.core-meta{font-family:monospace;font-size:12px;overflow-wrap:anywhere}.core-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(260px,1fr));gap:12px}.core-node{background:white;border:1px solid #ddd;border-radius:10px;padding:14px}.core-node h3{margin:0 0 10px}.core-value,.core-rule{font-size:1.1rem;overflow-wrap:anywhere}.unavailable{color:#777}.core-state{padding-left:20px;font-size:13px}.core-state span{font-weight:600}.core-dependency{display:inline-block;margin:2px;padding:2px 5px;background:#eee;border-radius:4px}.core-arrangement{margin:28px 0}.core-section{margin:18px 0}@media(prefers-color-scheme:dark){body{background:#181817;color:#eee}.core-node{background:#242422;border-color:#555}.core-dependency{background:#383835}a{color:#8fc7ff}}@media(prefers-reduced-motion:reduce){*{animation:none!important;transition:none!important;scroll-behavior:auto!important}}</style></head><body data-profile=\"core/v1\" data-snapshot-id=\"",
        max_bytes,
    )?;
    push(&mut out, &esc(snapshot, true), max_bytes)?;
    push(&mut out, "\" data-findings-revision=\"", max_bytes)?;
    push(&mut out, &esc(findings, true), max_bytes)?;
    push(&mut out, "\" data-page-assessment-revision=\"", max_bytes)?;
    push(&mut out, &esc(page_revision, true), max_bytes)?;
    push(&mut out, "\"><main class=\"core-wrap\"><h1>", max_bytes)?;
    push(&mut out, &esc(title, false), max_bytes)?;
    push(&mut out, "</h1><p class=\"core-meta\">snapshot ", max_bytes)?;
    push(&mut out, &esc(snapshot, false), max_bytes)?;
    push(&mut out, " · findings ", max_bytes)?;
    push(&mut out, &esc(findings, false), max_bytes)?;
    push(&mut out, " · page ", max_bytes)?;
    push(&mut out, &esc(page_revision, false), max_bytes)?;
    push(&mut out, "</p>", max_bytes)?;
    for arrangement in arrangements {
        let key = text(
            arrangement
                .get("key")
                .ok_or("arrangement key is required")?,
            "arrangement key",
        )?;
        let arrangement_title = text(
            arrangement
                .get("title")
                .ok_or("arrangement title is required")?,
            "arrangement title",
        )?;
        push(
            &mut out,
            &format!(
                "<section class=\"core-arrangement\" data-page-tab=\"{}\"><h2>{}</h2>",
                esc(key, true),
                esc(
                    if arrangement_title.is_empty() {
                        now_label
                    } else {
                        arrangement_title
                    },
                    false
                )
            ),
            max_bytes,
        )?;
        if let Some(occasion) = arrangement.get("occasion").and_then(Value::as_str)
            && !occasion.is_empty()
        {
            push(
                &mut out,
                &format!("<p>{}</p>", esc(occasion, false)),
                max_bytes,
            )?;
        }
        for section in arrangement
            .get("sections")
            .and_then(Value::as_array)
            .ok_or("arrangement sections must be an array")?
        {
            let section_title = text(
                section.get("title").ok_or("section title is required")?,
                "section title",
            )?;
            push(
                &mut out,
                &format!(
                    "<section class=\"core-section\"><h3>{}</h3>",
                    esc(section_title, false)
                ),
                max_bytes,
            )?;
            if let Some(why) = section.get("why").and_then(Value::as_str)
                && !why.is_empty()
            {
                push(&mut out, &format!("<p>{}</p>", esc(why, false)), max_bytes)?;
            }
            push(&mut out, "<div class=\"core-grid\">", max_bytes)?;
            for id in section
                .get("ids")
                .and_then(Value::as_array)
                .ok_or("section ids must be an array")?
            {
                let id = text(id, "section id")?;
                card(
                    index.get(id).ok_or("arrangement references unknown node")?,
                    &mut out,
                    max_bytes,
                )?;
            }
            push(&mut out, "</div></section>", max_bytes)?;
        }
        push(&mut out, "</section>", max_bytes)?;
    }
    if let Some(spill) = values
        .get("coverage")
        .and_then(|v| v.get("spill"))
        .and_then(Value::as_array)
        && !spill.is_empty()
    {
        push(
            &mut out,
            "<section class=\"core-arrangement\" data-page-spill=\"true\"><h2>Flagged outside the arrangement</h2><div class=\"core-grid\">",
            max_bytes,
        )?;
        for id in spill {
            let id = text(id, "spill id")?;
            card(
                index.get(id).ok_or("spill references unknown node")?,
                &mut out,
                max_bytes,
            )?;
        }
        push(&mut out, "</div></section>", max_bytes)?;
    }
    push(
        &mut out,
        &format!(
            "<section class=\"core-record\" id=\"tree\"><h2>{}</h2><div class=\"core-grid\">",
            esc(record_label, false)
        ),
        max_bytes,
    )?;
    for node in index.values() {
        card(node, &mut out, max_bytes)?;
    }
    push(&mut out, "</div></section>", max_bytes)?;
    let json = serde_json::to_string(page)
        .map_err(|e| e.to_string())?
        .replace('<', "\\u003c");
    push(
        &mut out,
        "<script type=\"application/json\" id=\"kpopper-page-assessment\">",
        max_bytes,
    )?;
    push(&mut out, &json, max_bytes)?;
    push(&mut out, "</script></main></body></html>", max_bytes)?;
    Ok(out)
}

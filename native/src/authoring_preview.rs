//! Human-readable output of the actual writer's uncommitted candidate.
use crate::{
    Error, Result,
    history_contract::{field, map, text},
    history_transaction::PreparedMutation,
    value::TypedValue as V,
};

pub(crate) fn render(
    mutation: &PreparedMutation,
    action: &V,
    profile: &str,
    diagnostics: &[String],
) -> Result<String> {
    let action = map(action)?;
    let id = text(field(action, "id")?)?;
    let kind = text(field(action, "kind")?)?;
    let hypothesis = action.get("hypothesis").and_then(|value| text(value).ok());
    let destination = hypothesis
        .map(|name| format!(" in hypothesis {name}"))
        .unwrap_or_default();
    let mut output = format!(
        "dry run ({profile}): candidate prepared for {kind} {id}{destination}; nothing recorded\n"
    );
    output.push_str(
        "This checks write admission, not source accuracy or the sufficiency of the conclusion.\n",
    );
    for diagnostic in diagnostics {
        output.push_str(&format!("NOTE {diagnostic}\n"));
    }
    let data = mutation.to_data();
    let receipt = map(field(map(&data)?, "receipt")?)?;
    let after = map(field(receipt, "after")?)?;
    let documents = if let Some(document) = after.get("document") {
        // Core receipts retain the prepared semantic world, including a named
        // proposal. The projected base file still holds its accepted old value.
        vec![document.clone()]
    } else {
        let mut files = mutation.files().iter().collect::<Vec<_>>();
        if hypothesis.is_some() {
            files.sort_by_key(|file| file.role != "hypothesis");
        }
        files
            .into_iter()
            .filter(|file| ["record", "record_member", "hypothesis"].contains(&file.role.as_str()))
            .filter_map(|file| file.after.as_ref())
            .map(|raw| crate::history_yaml::decode_document(raw))
            .collect::<Result<Vec<_>>>()?
    };
    for document in documents {
        let entries = crate::reasoning_snapshot::entries(&document)?;
        if let Some((_, body)) = entries.get(id) {
            let mut body = body.clone();
            let fields = crate::reasoning_fields::snapshot_fields(&document)?;
            if let V::Map(body) = &mut body {
                // The writer supplies the full snapshot; keep the candidate readable.
                if fields
                    .get("snapshot")
                    .and_then(|v| text(v).ok())
                    .is_some_and(|snapshot| body.remove(snapshot).is_some())
                {
                    output.push_str(
                        "Review snapshot prepared from the current premises (omitted below).\n",
                    );
                }
            }
            let candidate = V::Map(std::collections::BTreeMap::from([(id.to_owned(), body)]));
            let yaml = crate::history_emit::encode_document(&candidate)?;
            output.push_str(
                std::str::from_utf8(&yaml).map_err(|_| Error("invalid preview text".into()))?,
            );
            break;
        }
    }
    output.push_str("Remove --dry-run to write; the writer will recheck the current record.\n");
    Ok(output)
}

pub(crate) fn explain(error: Error) -> Error {
    let mut message = error.0;
    if message.contains("wrong_if")
        || message.contains("predicate")
        || message.contains("condition")
        || message.contains("expression")
    {
        message.push_str(
            "\nUse wrong_if={expr: \"...\"} with actual recorded premises declared in rests_on.",
        );
        if message.starts_with("core/v1:") {
            message
                .push_str(" This core/v1 record supports comparisons and and/or/not inside expr.");
        } else if message.starts_with("ordinary:") {
            message.push_str(" This ordinary record supports one comparison with arithmetic operands; compound Boolean conditions require core/v1.");
        }
        message.push_str(" If the condition truly cannot be observed, explain that with blocked_on. A syntax error is not missing evidence; do not weaken the condition or invent a threshold.");
    }
    if !message.contains("nothing recorded") {
        message.push_str("\ndry run refused; nothing recorded");
    }
    Error(message)
}

pub(crate) fn in_profile(profile: &str, error: Error) -> Error {
    Error(format!("{profile}: {error}"))
}

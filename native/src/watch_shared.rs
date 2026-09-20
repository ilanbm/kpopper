//! Retained external observations and verified shared-record publication.
use crate::{
    Result,
    history_authoring::{n, obj, s},
    history_contract::{Map, map, text},
    history_transaction::{FileImage, PreparedMutation},
    history_view::map_mut,
    require,
    value::TypedValue as V,
    watch_store::{
        Watch, atomic, digest, digest_value, error, json_files, load, lock, save, truth, typed,
    },
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value as J, json};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};
pub fn load_report(path: &str) -> Result<J> {
    let mut raw = vec![];
    if path == "-" {
        std::io::stdin().take(65537).read_to_end(&mut raw)?;
    } else {
        std::fs::File::open(path)?
            .take(65537)
            .read_to_end(&mut raw)?;
    }
    require(raw.len() <= 65536, "shared report exceeds 64 KiB")?;
    crate::json_ingress::parse_slice_bounded(&raw, crate::json_ingress::DuplicateKeys::Reject, 128)
        .map_err(|e| {
            let message = e.to_string();
            if has_nonfinite_token(&raw) {
                return error("shared observations need a finite scalar value");
            }
            if let Some(key) = message.strip_prefix("Duplicate JSON key: ") {
                let key = key
                    .rsplit_once(" at line ")
                    .map(|(key, _)| key)
                    .unwrap_or(key);
                error(format!("duplicate report field: {key}"))
            } else {
                error(message)
            }
        })
}
fn has_nonfinite_token(raw: &[u8]) -> bool {
    let mut quoted = false;
    let mut escaped = false;
    let mut index = 0;
    while index < raw.len() {
        let byte = raw[index];
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
            index += 1;
            continue;
        }
        if byte == b'"' {
            quoted = true;
            index += 1;
            continue;
        }
        for token in [b"NaN".as_slice(), b"Infinity".as_slice()] {
            if raw[index..].starts_with(token)
                && (index == 0 || !raw[index - 1].is_ascii_alphanumeric())
                && (index + token.len() == raw.len()
                    || !raw[index + token.len()].is_ascii_alphanumeric())
            {
                return true;
            }
        }
        index += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::load_report;
    use std::fs;

    #[test]
    fn nonfinite_json_reports_the_shared_scalar_contract() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        fs::write(temp.path(), br#"{"id":"e","value":NaN}"#).unwrap();
        assert_eq!(
            load_report(temp.path().to_str().unwrap()).unwrap_err().0,
            "shared observations need a finite scalar value"
        );
    }

    #[test]
    fn quoted_nonfinite_text_and_duplicate_json_keep_parser_errors() {
        let quoted = tempfile::NamedTempFile::new().unwrap();
        fs::write(quoted.path(), br#"{"id":"e","value":"NaN"}"#).unwrap();
        assert!(load_report(quoted.path().to_str().unwrap()).is_ok());
        let duplicate = tempfile::NamedTempFile::new().unwrap();
        fs::write(duplicate.path(), br#"{"id":"e","id":"e2","value":1}"#).unwrap();
        assert!(
            !load_report(duplicate.path().to_str().unwrap())
                .unwrap_err()
                .0
                .contains("finite scalar")
        );
    }
}
fn layout(watch: &Watch, enabled: bool) -> Result<(PathBuf, PathBuf)> {
    let config = watch.config()?.unwrap_or(J::Null);
    require(
        (!enabled || truth(&config["enabled"])) && truth(&config["shared_record"]),
        "configure a shared destination with watch setup before sharing facts",
    )?;
    let record = PathBuf::from(
        config["shared_record"]
            .as_str()
            .ok_or_else(|| error("invalid shared record path"))?,
    );
    let root = watch
        .project_state
        .join("shared-inbox")
        .join(digest(&json!(record))?);
    Ok((record, root))
}
fn fields(value: &J, required: &[&str], optional: &[&str]) -> bool {
    value.as_object().is_some_and(|m| {
        required.iter().all(|k| m.contains_key(*k))
            && m.keys()
                .all(|k| required.contains(&k.as_str()) || optional.contains(&k.as_str()))
    })
}
fn nonblank(value: &str) -> bool {
    !value
        .trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
        .is_empty()
}
fn date(value: &J) -> Result<()> {
    let Some(value) = value.as_str() else {
        return Err(error("date must be YYYY-MM-DD"));
    };
    if let Ok(day) = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return require(day.to_string() == value, "date must be YYYY-MM-DD");
    }
    if regex::Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$")
        .unwrap()
        .is_match(value)
    {
        let year = value[..4].parse::<u32>().unwrap();
        let month = value[5..7].parse::<u32>().unwrap();
        let day = value[8..].parse::<u32>().unwrap();
        let reason = if year == 0 {
            format!("year must be in 1..9999, not {year}")
        } else if !(1..=12).contains(&month) {
            format!("month must be in 1..12, not {month}")
        } else {
            let maximum = match month {
                4 | 6 | 9 | 11 => 30,
                2 => {
                    if year % 400 == 0 || year % 4 == 0 && year % 100 != 0 {
                        29
                    } else {
                        28
                    }
                }
                _ => 31,
            };
            format!("day {day} must be in range 1..{maximum} for month {month} in year {year}")
        };
        return Err(error(reason));
    }
    if chrono::NaiveDate::parse_from_str(value, "%Y%m%d").is_ok()
        || chrono::NaiveDate::parse_from_str(value, "%G-W%V-%u").is_ok()
    {
        return Err(error("date must be YYYY-MM-DD"));
    }
    Err(error(format!(
        "Invalid isoformat string: '{}'",
        value.replace('\\', "\\\\").replace('\'', "\\'")
    )))
}
pub fn validate(report: &J) -> Result<()> {
    require(
        fields(
            report,
            &[
                "id",
                "name",
                "value",
                "date",
                "scope",
                "source",
                "source_quote",
            ],
            &["event_id"],
        ),
        "shared report needs id, name, value, date, scope, source and source_quote",
    )?;
    require(
        report["id"].as_str().is_some_and(|v| {
            regex::Regex::new(r"^[A-Za-z][A-Za-z0-9_.-]{1,159}$")
                .unwrap()
                .is_match(v)
        }),
        "invalid shared fact ID",
    )?;
    for key in ["name", "source_quote"] {
        require(
            report[key].as_str().is_some_and(|v| {
                !v.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
                    .is_empty()
            }),
            &format!("{key} must be nonempty text"),
        )?;
    }
    require(
        matches!(report["value"], J::String(_) | J::Bool(_) | J::Number(_)),
        "shared observations need a finite scalar value",
    )?;
    date(&report["date"])?;
    require(
        fields(&report["scope"], &["kind", "environment"], &[])
            && report["scope"]["kind"] == "external"
            && report["scope"]["environment"]
                .as_str()
                .is_some_and(nonblank),
        "scope must explicitly name kind=external and its environment; branch/code claims cannot be shared",
    )?;
    let source = &report["source"];
    require(
        source.as_object().is_some_and(|m| {
            m.keys()
                .all(|k| ["url", "file", "at"].contains(&k.as_str()))
                && truth(&source["at"])
                && (m.contains_key("url") ^ m.contains_key("file"))
                && m.values().all(|v| v.as_str().is_some_and(nonblank))
        }),
        "source needs one URL or absolute file path, and an exact at location",
    )?;
    if let Some(file) = source["file"].as_str() {
        require(
            Path::new(file).is_absolute(),
            "shared source file paths must be absolute",
        )?;
    }
    if let Some(url) = source["url"].as_str() {
        require(
            url.starts_with("https://") || url.starts_with("http://"),
            "source URL must be HTTP(S)",
        )?;
    }
    require(
        crate::watch_store::bytes(report)?.len() <= 65536,
        "shared report exceeds 64 KiB",
    )?;
    if let Some(id) = report.get("event_id") {
        require(id.as_str().is_some_and(event_id), "invalid event_id")?;
    }
    Ok(())
}
fn event_id(id: &str) -> bool {
    regex::Regex::new(r"^[A-Za-z0-9_-]{1,120}$")
        .unwrap()
        .is_match(id)
}
fn document(path: &Path) -> Result<(V, Vec<u8>)> {
    let raw = fs::read(path)?;
    let captured = crate::source_capture::capture_source(
        &[path.to_owned()],
        path.parent().unwrap(),
        crate::source_capture::ReadMode::Live,
        None,
    )?;
    Ok((captured.strict_document()?, raw))
}
pub fn capture(watch: &Watch, report: J) -> Result<J> {
    capture_with(watch, report, &|watch| watch.launch())
}
pub fn capture_with(watch: &Watch, report: J, launch: &dyn Fn(&Watch) -> Result<()>) -> Result<J> {
    validate(&report)?;
    let (record, root) = layout(watch, true)?;
    require(
        record.is_file(),
        "configured shared record is unavailable; it was not recreated",
    )?;
    let id = report["event_id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
    let path = root.join("events").join(format!("{id}.json"));
    {
        let _guard = lock(&root.join("capture.lock"), true)?;
        if let Some(existing) = load(&path)?.filter(truth) {
            require(
                crate::source_clock::python_equal(&typed(&existing["report"])?, &typed(&report)?),
                "event_id was already used for a different report",
            )?;
        } else {
            let target = {
                let _record = crate::history_transaction_fs::DirectoryGuard::acquire(
                    record.parent().unwrap(),
                    true,
                )?;
                let (doc, _) = document(&record)?;
                crate::watch_compare::entries(&doc)?
                    .get(report["id"].as_str().unwrap())
                    .map(|v| v.1.clone())
                    .unwrap_or(V::Null)
            };
            save(&root.join("sources").join(format!("{id}.json")), &report)?;
            save(
                &path,
                &json!({"id":id,"report":report,"report_hash":digest(&report)?,"record":record,"state":"captured","captured_at":watch.clock.nanos(),"target_before":digest_value(&target)?,"origin":watch.tree,"captured_from":watch.cwd}),
            )?;
        }
    }
    watch.request_with(launch)?;
    Ok(json!({"state":"captured","event_id":id,"record":record,"receipt":path}))
}
fn authority(record: &Path) -> Result<V> {
    let name = record.file_name().unwrap().to_str().unwrap();
    let marker = record
        .parent()
        .unwrap()
        .join(crate::history_transaction::Layout::for_entry(name)?.authority);
    if marker.is_file() {
        let value = crate::history_yaml::decode_document(&fs::read(marker)?)?;
        crate::history_authority::validate_authority(&value)?;
        require(
            map(&value)?["authority"] == s("legacy"),
            "history_direct_writer_unsupported: use the history writer",
        )?;
        Ok(value)
    } else {
        crate::history_authority::authority(
            &format!(
                "legacy-{}",
                &crate::identity::sha256(record.canonicalize()?.to_string_lossy().as_bytes())[..32]
            ),
            "legacy",
            &n("0"),
            Map::new(),
        )
    }
}
fn mutation(journal: &J) -> Result<PreparedMutation> {
    let encoded = journal["prepared"]
        .as_str()
        .ok_or_else(|| error("shared prepared operation is invalid"))?;
    let raw = STANDARD
        .decode(encoded)
        .map_err(|_| error("shared prepared operation is invalid"))?;
    PreparedMutation::from_bytes(&raw).map_err(|_| error("shared prepared operation is invalid"))
}
fn verify(
    watch: &Watch,
    record: &Path,
    root: &Path,
    event: &J,
    mutation: &PreparedMutation,
) -> Result<()> {
    let data = mutation.to_data();
    let data = map(&data)?;
    let baseline = map(&data["baseline"])?;
    let source = root
        .join("sources")
        .join(format!("{}.json", event["id"].as_str().unwrap()));
    let expected = obj([
        ("kind", s("watch-shared/v1")),
        ("record", s(record.to_str().unwrap())),
        ("state_dir", s(root.to_str().unwrap())),
        ("event_id", typed(&event["id"])?),
        ("report_hash", typed(&event["report_hash"])?),
        ("watch_config", typed(&watch.config()?.unwrap_or(J::Null))?),
        (
            "record_members",
            baseline.get("record_members").cloned().unwrap_or(V::Null),
        ),
    ]);
    require(
        data["baseline"] == expected
            && digest(&event["report"])? == event["report_hash"]
            && load(&source)? == Some(event["report"].clone())
            && data["authority"] == authority(record)?,
        "shared source, routing or operation identity changed",
    )?;
    let config = watch.config()?.unwrap_or(J::Null);
    require(
        truth(&config["enabled"]) && config["shared_record"] == json!(record),
        "watch was paused or its destination changed before commit",
    )?;
    require(
        mutation.files().len() == 1 && mutation.files()[0].role == "record",
        "shared operation changed unexpected files",
    )?;
    let item = &mutation.files()[0];
    let before = crate::history_yaml::decode_document(
        item.before
            .as_deref()
            .ok_or_else(|| error("missing shared before image"))?,
    )?;
    let after = crate::history_yaml::decode_document(
        item.after
            .as_deref()
            .ok_or_else(|| error("missing shared after image"))?,
    )?;
    let receipt = map(&data["receipt"])?;
    require(
        before == map(&receipt["before"])?["document"]
            && after == map(&receipt["after"])?["document"],
        "shared prepared semantic evidence changed",
    )?;
    let before_entries = crate::watch_compare::entries(&before)?;
    let after_entries = crate::watch_compare::entries(&after)?;
    let report = &event["report"];
    let id = report["id"].as_str().unwrap();
    let source_id = format!(
        "s.shared_{}",
        event["id"].as_str().unwrap().replace('-', "_")
    );
    let source_body = typed(
        &json!({"name":format!("{} — source report",report["name"].as_str().unwrap()),"file":source,"read":report["date"],"origin":report["source"]}),
    )?;
    let mut target = before_entries
        .get(id)
        .map(|(_, v)| v.clone())
        .filter(|v| matches!(v, V::Map(_)))
        .unwrap_or(typed(
            &json!({"name":report["name"],"scope":report["scope"]}),
        )?);
    let tm = map_mut(&mut target)?;
    for (key, value) in [
        ("v", typed(&report["value"])?),
        ("of", typed(&report["date"])?),
        ("from", s(&source_id)),
        ("at", typed(&report["source"]["at"])?),
    ] {
        tm.insert(key.into(), value);
    }
    require(
        after_entries.get(id).map(|(_, v)| v) == Some(&target)
            && map(&target)?.get("scope") == Some(&typed(&report["scope"])?)
            && after_entries.get(&source_id).map(|(_, v)| v) == Some(&source_body),
        "shared prepared source or observation differs from report",
    )?;
    for id in before_entries.keys().chain(after_entries.keys()) {
        if id != report["id"].as_str().unwrap() && id != &source_id {
            require(
                before_entries.get(id).map(|(_, v)| v) == after_entries.get(id).map(|(_, v)| v),
                "shared prepared operation changes an unrelated entry",
            )?;
        }
    }
    Ok(())
}
fn publish(
    watch: &Watch,
    record: &Path,
    root: &Path,
    event: &J,
    journal: &J,
    recovery: bool,
) -> Result<()> {
    let mutation = mutation(journal)?;
    let event_path = root
        .join("journals")
        .join(format!("{}.json", event["id"].as_str().unwrap()));
    let digest = map(&mutation.to_data())?["digest"].clone();
    let mut verify_mutation = |data: &V| {
        require(
            map(data)?["digest"] == digest,
            "shared recovery journal belongs to another operation",
        )?;
        verify(watch, record, root, event, &mutation)
    };
    let mut committed = |data: &V| {
        let mut completed = journal.clone();
        completed["phase"] = json!("committed");
        completed["mutation_digest"] =
            crate::ordinary_reader::json_value(&map(data)?["digest"], 0)?;
        save(&event_path, &completed)
    };
    let _config = lock(&watch.config_path.with_extension("lock"), true)?;
    let _record =
        crate::history_transaction_fs::DirectoryGuard::acquire(record.parent().unwrap(), true)?;
    let layout = crate::history_transaction::Layout::for_entry(
        record.file_name().unwrap().to_str().unwrap(),
    )?;
    if recovery {
        crate::history_transaction_fs::recover_legacy(
            record.parent().unwrap(),
            &layout.journal,
            crate::history_transaction_fs::Direction::After,
            &mut verify_mutation,
            Some(&mut committed),
            None,
        )?;
    } else {
        crate::history_transaction_fs::publish_legacy(
            record.parent().unwrap(),
            &layout.journal,
            &mutation,
            &mut verify_mutation,
            Some(&mut committed),
        )?;
    }
    Ok(())
}
fn resume_shared(watch: &Watch, record: &Path, root: &Path, event: &J, journal: &J) -> Result<J> {
    let m = mutation(journal)?;
    let data = m.to_data();
    let d = map(&data)?;
    let baseline = map(&d["baseline"])?;
    require(
        baseline["event_id"] == typed(&event["id"])?
            && baseline["report_hash"] == typed(&event["report_hash"])?,
        "shared prepared event identity changed",
    )?;
    let primary = record.parent().unwrap().join(
        crate::history_transaction::Layout::for_entry(
            record.file_name().unwrap().to_str().unwrap(),
        )?
        .journal,
    );
    if primary.exists() {
        publish(watch, record, root, event, journal, true)?;
    } else if journal["phase"] != "committed" || typed(&journal["mutation_digest"])? != d["digest"]
    {
        publish(watch, record, root, event, journal, false)?;
    }
    Ok(
        json!({"state":"applied","recovered":true,"target_after":journal["target_after"],"findings":crate::ordinary_reader::json_value(&map(&map(&d["receipt"])?["after"])?["findings"],0)?}),
    )
}
fn apply(watch: &Watch, record: &Path, root: &Path, event: &J) -> Result<J> {
    let report = &event["report"];
    validate(report)?;
    let eid = event["id"].as_str().unwrap();
    let id = report["id"].as_str().unwrap();
    let source_id = format!("s.shared_{}", eid.replace('-', "_"));
    let journal_path = root.join("journals").join(format!("{eid}.json"));
    let source_path = root.join("sources").join(format!("{eid}.json"));
    require(
        digest(report)? == event["report_hash"] && load(&source_path)? == Some(report.clone()),
        "retained shared source failed its integrity check",
    )?;
    if let Some(journal) = load(&journal_path)?.filter(|j| j["version"] == 2) {
        return resume_shared(watch, record, root, event, &journal);
    }
    let empty_collections = regex::Regex::new(r"(?m)^(sources|known):[ \t]*\{\}[ \t]*$").unwrap();
    for _ in 0..3 {
        let (doc, before, target) = {
            let _guard = crate::history_transaction_fs::DirectoryGuard::acquire(
                record.parent().unwrap(),
                true,
            )?;
            let captured = crate::source_capture::capture_source(
                &[record.to_owned()],
                record.parent().unwrap(),
                crate::source_capture::ReadMode::Live,
                None,
            )?;
            let doc = captured.strict_document()?;
            require(
                captured.members().len() == 1
                    && !map(&doc)?.contains_key("record")
                    && !map(&doc)?.contains_key("also")
                    && map(captured.hypotheses())?.is_empty(),
                "shared write needs a single canonical record; existing pointers/hypotheses need explicit review",
            )?;
            let target = crate::watch_compare::entries(&doc)?
                .get(id)
                .map(|(_, body)| body.clone())
                .unwrap_or(V::Null);
            if let Some(journal) = load(&journal_path)?
                && digest_value(&target)? == journal["target_after"]
            {
                let entries = crate::watch_compare::entries(&doc)?;
                if entries.get(&source_id).is_some_and(|(_, source)| {
                    map(source)
                        .is_ok_and(|m| m.get("file") == Some(&s(source_path.to_str().unwrap())))
                }) && map(&target).is_ok_and(|m| m.get("from") == Some(&s(&source_id)))
                {
                    return Ok(
                        json!({"state":"applied","recovered":true,"target_after":digest_value(&target)?}),
                    );
                }
            }
            require(
                digest_value(&target)? == event["target_before"],
                "shared target changed after capture; retained for review",
            )?;
            require(
                target == V::Null
                    || map(&target).is_ok_and(|m| {
                        m.contains_key("v")
                            && m.get("scope") == typed(&report["scope"]).ok().as_ref()
                    }),
                "existing target has another scope or is not a shared scalar observation",
            )?;
            (doc, fs::read(record)?, target)
        };
        let temp = tempfile::tempdir()?;
        let shadow = temp.path().join(record.file_name().unwrap());
        let before_text = std::str::from_utf8(&before).map_err(|e| error(e.to_string()))?;
        let prepared_text = empty_collections
            .replace_all(before_text, "$1:")
            .into_owned();
        fs::write(&shadow, prepared_text)?;
        let source_body = typed(
            &json!({"name":format!("{} — source report",report["name"].as_str().unwrap()),"file":source_path,"read":report["date"],"origin":report["source"]}),
        )?;
        let empty = crate::watch_compare::entries(&doc)?.is_empty();
        if empty {
            let mut initial = map(&doc)?
                .iter()
                .filter(|(_, v)| **v != V::Map(Map::new()))
                .map(|(k, v)| {
                    (
                        crate::history_yaml::OrdinaryKey::text_key(k.clone()),
                        crate::history_yaml::OrdinaryValue::from_typed(v),
                    )
                })
                .collect::<Vec<_>>();
            initial.push((
                crate::history_yaml::OrdinaryKey::text_key("sources"),
                crate::history_yaml::OrdinaryValue::Map(vec![(
                    crate::history_yaml::OrdinaryKey::text_key(source_id.clone()),
                    crate::history_yaml::OrdinaryValue::Map(
                        ["name", "file", "read", "origin"]
                            .iter()
                            .map(|key| {
                                (
                                    crate::history_yaml::OrdinaryKey::text_key(*key),
                                    crate::history_yaml::OrdinaryValue::from_typed(
                                        &map(&source_body).unwrap()[*key],
                                    ),
                                )
                            })
                            .collect(),
                    ),
                )]),
            ));
            let mut raw = crate::public_ordinary_readers::python_safe_dump_unicode(
                &crate::history_yaml::OrdinaryValue::Map(initial),
            )?;
            raw.extend_from_slice(b"known:\n");
            fs::write(&shadow, raw)?;
        } else {
            write_shadow(
                &shadow,
                &typed(
                    &json!({"kind":"add","id":source_id,"into":"sources","as_of":report["date"],"body":crate::ordinary_reader::json_value(&source_body,0)?}),
                )?,
            )?;
        }
        let action = if target == V::Null {
            json!({"kind":"add","id":id,"into":"known","as_of":report["date"],"body":{"name":report["name"],"v":report["value"],"of":report["date"],"scope":report["scope"],"from":source_id,"at":report["source"]["at"]}})
        } else {
            json!({"kind":"set","id":id,"value":report["value"],"as_of":report["date"],"source":source_id,"at":report["source"]["at"]})
        };
        write_shadow(&shadow, &typed(&action)?)?;
        let (after_doc, after) = document(&shadow)?;
        let updated = crate::watch_compare::entries(&after_doc)?
            .get(id)
            .ok_or_else(|| error("shared update did not preserve the value and source"))?
            .1
            .clone();
        require(
            map(&updated)?.get("v") == Some(&typed(&report["value"])?)
                && map(&updated)?.get("from") == Some(&s(&source_id)),
            "shared update did not preserve the value and source",
        )?;
        let before_fail = if empty {
            vec![]
        } else {
            crate::watch_compare::check(&doc, None, None)?.0
        };
        let after_fail = crate::watch_compare::check(&after_doc, None, None)?.0;
        let findings = after_fail
            .into_iter()
            .filter(|line| !before_fail.contains(line))
            .collect::<Vec<_>>();
        let cap = crate::reasoning_fields::capabilities(&after_doc, None)?;
        let receipt = crate::history_transaction::semantic_receipt(
            text(&map(&cap)?["profile"])?,
            &cap,
            &obj([("document", doc.clone())]),
            &obj([
                ("document", after_doc),
                ("findings", V::List(findings.iter().map(|v| s(v)).collect())),
            ]),
        )?;
        let baseline = obj([
            ("kind", s("watch-shared/v1")),
            ("record", s(record.to_str().unwrap())),
            ("state_dir", s(root.to_str().unwrap())),
            ("event_id", s(eid)),
            ("report_hash", typed(&event["report_hash"])?),
            ("watch_config", typed(&watch.config()?.unwrap_or(J::Null))?),
            (
                "record_members",
                V::Map(Map::from([(
                    record.file_name().unwrap().to_string_lossy().into_owned(),
                    s(&crate::identity::sha256(&before)),
                )])),
            ),
        ]);
        let m = PreparedMutation::prepare(
            &format!("watch-{eid}"),
            &authority(record)?,
            &baseline,
            vec![FileImage {
                path: record.file_name().unwrap().to_string_lossy().into_owned(),
                role: "record".into(),
                before: Some(before.clone()),
                after: Some(after),
            }],
            &receipt,
            record.file_name().unwrap().to_str().unwrap(),
            None,
        )?;
        let journal = json!({"version":2,"phase":"prepared","target_after":digest_value(&updated)?,"prepared":STANDARD.encode(m.to_bytes()?)});
        save(&journal_path, &journal)?;
        {
            let _config = lock(&watch.config_path.with_extension("lock"), true)?;
            let config = watch.config()?.unwrap_or(J::Null);
            require(
                truth(&config["enabled"]) && config["shared_record"] == json!(record),
                "watch was paused or its destination changed before commit",
            )?;
            let _record = crate::history_transaction_fs::DirectoryGuard::acquire(
                record.parent().unwrap(),
                true,
            )?;
            if fs::read(record)? != before {
                continue;
            }
            atomic(&root.join("backups").join(format!("{eid}.yaml")), &before)?;
        }
        publish(watch, record, root, event, &journal, false)?;
        return Ok(
            json!({"state":"applied","target_after":digest_value(&updated)?,"findings":findings}),
        );
    }
    Err(error(
        "shared record kept changing; report retained for review",
    ))
}
fn write_shadow(shadow: &Path, action: &V) -> Result<()> {
    let route =
        crate::project_modes::WriteRoute::capture(&[shadow.to_owned()], shadow.parent().unwrap())?;
    let body = map(action)?
        .get("body")
        .and_then(|v| map(v).ok())
        .map(|body| {
            crate::history_yaml::SourceValue::Map(
                [
                    "name", "v", "of", "scope", "from", "at", "file", "read", "origin",
                ]
                .iter()
                .filter_map(|key| {
                    body.get(*key).map(|value| {
                        (
                            (*key).to_owned(),
                            crate::history_yaml::SourceValue::from_typed(value),
                        )
                    })
                })
                .collect(),
            )
        });
    crate::legacy_authoring::write(action, &route, body.as_ref())?;
    Ok(())
}
pub fn process(watch: &Watch) -> Result<bool> {
    let (record, root) = layout(watch, true)?;
    let Some(_guard) = lock(&root.join("processor.lock"), false)? else {
        return Ok(false);
    };
    let mut events = json_files(&root.join("events"))?
        .iter()
        .map(|p| load(p)?.ok_or_else(|| error("missing shared event")))
        .collect::<Result<Vec<_>>>()?;
    events.sort_by_key(|v| v["captured_at"].as_u64().unwrap_or(0));
    let mut applied = false;
    for mut event in events {
        if !matches!(
            event["state"].as_str(),
            Some("captured" | "recovery_required")
        ) {
            continue;
        }
        let outcome = (|| -> Result<J> {
            require(
                event["record"] == json!(record),
                "shared destination changed",
            )?;
            apply(watch, &record, &root, &event)
        })();
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(e) => {
                let primary = record.parent().unwrap().join(
                    crate::history_transaction::Layout::for_entry(
                        record.file_name().unwrap().to_str().unwrap(),
                    )?
                    .journal,
                );
                let recoverable = fs::read(primary)
                    .ok()
                    .and_then(|raw| PreparedMutation::from_bytes(&raw).ok())
                    .is_some_and(|m| {
                        map(&m.to_data())
                            .ok()
                            .and_then(|d| map(&d["baseline"]).ok())
                            .is_some_and(|b| {
                                b.get("kind") == Some(&s("watch-shared/v1"))
                                    && b.get("event_id") == typed(&event["id"]).ok().as_ref()
                            })
                    });
                json!({"state":if recoverable{"recovery_required"}else{"needs_review"},"reason":e.to_string().chars().take(1200).collect::<String>()})
            }
        };
        applied |= outcome["state"] == "applied";
        event
            .as_object_mut()
            .unwrap()
            .extend(outcome.as_object().unwrap().clone());
        save(
            &root
                .join("events")
                .join(format!("{}.json", event["id"].as_str().unwrap())),
            &event,
        )?;
    }
    Ok(applied)
}
pub fn read(watch: &Watch) -> Result<J> {
    let (record, root) = layout(watch, false)?;
    let (doc, _) = document(&record)?;
    let entries = V::Map(
        crate::watch_compare::entries(&doc)?
            .into_iter()
            .map(|(id, (_, v))| (id, v))
            .collect(),
    );
    let reports = json_files(&root.join("events"))?
        .iter()
        .map(|p| load(p))
        .collect::<Result<Vec<_>>>()?;
    Ok(
        json!({"record":record,"entries":crate::ordinary_reader::json_value(&entries,0)?,"reports":reports}),
    )
}
pub fn receipts(watch: &Watch) -> Result<Vec<J>> {
    let (_, root) = layout(watch, false)?;
    let mut result = vec![];
    for path in json_files(&root.join("events"))? {
        if let Some(event) = load(&path)?.filter(truth) {
            result.push(json!({"id":event["id"],"state":event["state"],"origin":event["origin"],"reason":event["reason"],"resolution":event["resolution"],"target":event["report"]["id"]}));
        }
    }
    Ok(result)
}
pub fn resolve(watch: &Watch, id: &str, evidence: &str) -> Result<J> {
    require(
        nonblank(evidence) && evidence.chars().count() <= 4000,
        "record the evidence for resolving this retained report",
    )?;
    require(event_id(id), "invalid event_id")?;
    let (_, root) = layout(watch, false)?;
    {
        let _guard = lock(&root.join("processor.lock"), true)?;
        let path = root.join("events").join(format!("{id}.json"));
        let mut event = load(&path)?
            .filter(|e| e["state"] == "needs_review")
            .ok_or_else(|| error("only a retained review report can be resolved"))?;
        event["state"] = json!("resolved");
        event["resolution"] = json!(evidence);
        save(&path, &event)?;
    }
    watch.request()?;
    Ok(json!({"state":"resolved","event_id":id}))
}

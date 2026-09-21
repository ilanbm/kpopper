//! Pure union and comparison of independently captured active history records.
//! Transported history files use the Snapshot's checksummed hex byte envelopes.
use crate::history_authoring::{empty, n, obj, s, strings};
use crate::{
    Result, history_adapter, history_authority as Authority, history_bundle,
    history_capture::{self as H, Capture},
    history_contract::*,
    history_hypotheses, history_reduce,
    history_view::{self as View, list, map_mut, truth},
    history_yaml,
    reasoning_context::CapturedAssessment,
    reasoning_operations,
    reasoning_runtime::{OperationalBounds, Runtime},
    reasoning_scenario as Scenario,
    reasoning_snapshot::{CaptureOptions, Snapshot, digest, entries},
    require,
    source_clock::python_equal,
    value::TypedValue as V,
};
use std::collections::{BTreeMap, BTreeSet};
const SIDES: [&str; 3] = ["ancestor", "main", "working"];

#[cfg(test)]
#[path = "../tests/support/history_watch.rs"]
mod tests;
fn get<'a>(m: &'a Map, key: &str) -> &'a V {
    m.get(key).unwrap_or(&V::Null)
}
fn hyps(record: &V) -> Result<&[V]> {
    map(record)?
        .get("hypotheses")
        .map(list)
        .transpose()
        .map(|a| a.unwrap_or(&[]))
}
fn transport_files(value: &V) -> Result<Authority::Files> {
    let files = map(value)?;
    require(files.len() <= 2 * MAX_OBJECTS + 2, "history_limit")?;
    let mut output = Authority::Files::new();
    let mut total = 0;
    for (path, value) in files {
        let value = schema(value, &["encoding", "data", "sha256"], &[])?;
        require(string_is(&value["encoding"], "hex"), "invalid_history")?;
        let hex = text(&value["data"])?;
        total += hex.len();
        require(
            hex.len() <= 32 * 1024 * 1024 && total <= 128 * 1024 * 1024,
            "history_limit",
        )?;
        require(
            hex.len().is_multiple_of(2)
                && hex
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid_history",
        )?;
        let raw = hex
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        require(
            value["sha256"] == s(&crate::identity::sha256(&raw)),
            "history_bundle_checksum",
        )?;
        output.insert(path.clone(), raw);
    }
    history_bundle::files_valid(&output)?;
    Ok(output)
}
fn validated(record: &V) -> Result<(Capture, Snapshot)> {
    let record_map = map(record)?;
    let evidence = field(record_map, "history")?;
    let files = transport_files(field(map(evidence)?, "files")?)?;
    let (captured, adapted) = history_bundle::validate_observation_capture(evidence, &files)
        .map_err(|e| observation_error(e, &files))?;
    let raw = get(record_map, "snapshot");
    require(
        matches!(raw, V::Map(_)),
        "history snapshot evidence is missing",
    )?;
    let observed = Snapshot::from_snapshot(raw)?;
    let data = map(observed.data())?;
    require(
        digest(&data["document"])? == digest(adapted.document())?
            && digest(get(record_map, "doc"))? == digest(&data["document"])?,
        "history snapshot document mismatch",
    )?;
    require(
        digest(get(map(&data["context"])?, "history"))? == digest(adapted.projection())?,
        "history snapshot closure mismatch",
    )?;
    let mut hypotheses = Map::new();
    for item in hyps(record)? {
        let item = map(item)?;
        let mut body = obj([
            ("document", field(item, "doc")?.clone()),
            ("head", field(item, "head")?.clone()),
            ("error", V::Null),
        ]);
        if let Some(kind) = item.get("kind").filter(|v| truth(v)) {
            map_mut(&mut body)?.insert("kind".into(), kind.clone());
        }
        hypotheses.insert(text(field(item, "name")?)?.into(), body);
    }
    require(
        hypotheses.len() == hyps(record)?.len(),
        "history snapshot contains duplicate hypothesis names",
    )?;
    require(
        digest(&V::Map(hypotheses))? == digest(&data["hypotheses"])?,
        "history snapshot hypotheses mismatch",
    )?;
    Ok((captured, observed))
}
// Keep the watch finding's retained diagnostic precise without changing the
// storage layer's stable error codes. This only describes an already refused
// observation; it cannot make an incomplete closure admissible.
fn observation_error(original: crate::Error, files: &Authority::Files) -> crate::Error {
    if original.0 != "incomplete_commit" {
        return original;
    }
    let commits = files
        .iter()
        .filter_map(|(path, raw)| {
            path.strip_prefix("commits/")
                .and_then(|p| p.strip_suffix(".yaml"))
                .map(|op| (op, raw))
        })
        .collect::<BTreeMap<_, _>>();
    for raw in commits.values() {
        if let Ok(commit) = history_yaml::decode_document(raw)
            && Authority::validate_commit(&commit).is_ok()
            && let Ok(parents) = map(&commit).and_then(|m| map(&m["parents"]))
        {
            for parent in parents.keys() {
                if !commits.contains_key(parent.as_str()) {
                    return error(&format!("incomplete_commit: {parent}"));
                }
            }
        }
    }
    original
}
fn physical(record: &V) -> Result<Map> {
    let mut result = Map::new();
    for item in hyps(record)? {
        let item = map(item)?;
        if string_is(get(item, "kind"), history_hypotheses::KIND) {
            continue;
        }
        let mut body = obj([
            ("doc", field(item, "doc")?.clone()),
            ("head", field(item, "head")?.clone()),
        ]);
        if let Some(kind) = item.get("kind").filter(|v| truth(v)) {
            map_mut(&mut body)?.insert("kind".into(), kind.clone());
        }
        result.insert(text(field(item, "name")?)?.into(), body);
    }
    Ok(result)
}
fn merge_physical(records: &Map) -> Result<(V, BTreeSet<String>)> {
    let ancestor = physical(field(records, "ancestor")?)?;
    let main = physical(field(records, "main")?)?;
    let working = physical(field(records, "working")?)?;
    let mut merged = main.clone();
    let mut changed = BTreeSet::new();
    for name in ancestor
        .keys()
        .chain(working.keys())
        .collect::<BTreeSet<_>>()
    {
        if python_equal(get(&ancestor, name), get(&working, name)) {
            continue;
        }
        for item in [ancestor.get(name), working.get(name)]
            .into_iter()
            .flatten()
        {
            changed.extend(entries(field(map(item)?, "doc")?)?.into_keys());
        }
        require(
            python_equal(get(&main, name), get(&ancestor, name))
                || python_equal(get(&main, name), get(&working, name)),
            &format!("history collision: physical hypothesis {name}"),
        )?;
        if let Some(item) = working.get(name) {
            merged.insert(name.clone(), item.clone());
        } else {
            merged.remove(name);
        }
    }
    Ok((V::Map(merged), changed))
}
fn snap(capture: &Capture, hypotheses: V, context: V, as_of: V) -> Result<Snapshot> {
    history_adapter::from_store_capture(capture)?.snapshot(CaptureOptions {
        context: Some(context),
        hypotheses: Some(hypotheses),
        as_of: Some(as_of),
        ..Default::default()
    })
}
pub fn merged_snapshot(records: &V) -> Result<Snapshot> {
    let records = map(records)?;
    let mut captures = BTreeMap::new();
    let mut observations = BTreeMap::new();
    for side in SIDES {
        let (capture, snapshot) = validated(field(records, side)?)?;
        captures.insert(side, capture);
        observations.insert(side, snapshot);
    }
    let as_of = map(observations["main"].data())?["as_of"].clone();
    require(
        observations
            .values()
            .all(|o| map(o.data()).is_ok_and(|d| d["as_of"] == as_of)),
        "history inputs have incompatible temporal bases",
    )?;
    let ancestor = &captures["ancestor"];
    let main = &captures["main"];
    let working = &captures["working"];
    for side in ["main", "working"] {
        let candidate = &captures[side];
        require(
            python_equal(&candidate.marker, &ancestor.marker),
            &format!("history collision: {side} authority"),
        )?;
        require(
            python_equal(
                &map(&candidate.state)?["rules"],
                &map(&ancestor.state)?["rules"],
            ),
            &format!("history collision: {side} rules"),
        )?;
        for (operation, raw) in &ancestor.commits {
            require(
                candidate.commits.get(operation) == Some(raw),
                &format!("history incomplete ancestor closure: {operation}"),
            )?;
        }
        for (key, raw) in &ancestor.object_bytes {
            require(
                candidate.object_bytes.get(key) == Some(raw),
                "history incomplete ancestor object closure",
            )?;
        }
    }
    let mut commits = main.commits.clone();
    for (operation, raw) in &working.commits {
        require(
            commits.get(operation).is_none_or(|old| old == raw),
            &format!("history collision: commit {operation}"),
        )?;
        commits.insert(operation.clone(), raw.clone());
    }
    let mut raw = main.object_bytes.clone();
    for (key, bytes) in &working.object_bytes {
        require(
            raw.get(key).is_none_or(|old| old == bytes),
            &format!("history collision: object {}", key.1),
        )?;
        raw.insert(key.clone(), bytes.clone());
    }
    require(
        commits
            .values()
            .chain(raw.values())
            .map(Vec::len)
            .sum::<usize>()
            <= 64 * 1024 * 1024,
        "history_limit: union closure exceeds captured byte limit",
    )?;
    let objects = Authority::committed_objects(&main.marker, &commits, &raw)?;
    let selected_raw = raw
        .iter()
        .filter(|((_, id), _)| objects.contains_key(id))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let state =
        history_reduce::reduce_bytes(&selected_raw, Some(map(&map(&main.state)?["rules"])?), None)?;
    let mut virtual_capture = main.clone();
    virtual_capture.baseline = H::baseline(&main.marker, &commits, &state)?;
    virtual_capture.entry_bytes = vec![];
    virtual_capture.document = empty();
    virtual_capture.commits = commits;
    virtual_capture.objects = objects;
    virtual_capture.object_bytes = raw;
    virtual_capture.state = state;
    virtual_capture.entry_bytes = View::render(
        &virtual_capture,
        &virtual_capture.objects,
        &virtual_capture.object_bytes,
        &virtual_capture.commits,
    )?;
    virtual_capture.document = history_yaml::decode_document(&virtual_capture.entry_bytes)?;
    let mut inputs = Map::new();
    for side in SIDES {
        inputs.insert(
            side.into(),
            obj([
                (
                    "identity",
                    s(&digest(field(
                        map(field(map(&records[side])?, "history")?)?,
                        "sha256",
                    )?)?),
                ),
                ("baseline", captures[side].baseline.clone()),
                ("snapshot_id", s(observations[side].snapshot_id())),
            ]),
        );
    }
    snap(
        &virtual_capture,
        merge_physical(records)?.0,
        obj([
            ("read_mode", s("supplied")),
            (
                "operation",
                obj([
                    ("version", n("1")),
                    ("phase", s("prospective")),
                    ("kind", s("history_union")),
                    ("inputs", V::Map(inputs)),
                ]),
            ),
        ]),
        as_of,
    )
}
fn entry_map(document: &V) -> Result<Map> {
    Ok(entries(document)?
        .into_iter()
        .map(|(name, (collection, body))| (name, V::List(vec![s(&collection), body])))
        .collect())
}
fn named_items(record: &V) -> Result<Map> {
    hyps(record)?
        .iter()
        .map(|item| Ok((text(field(map(item)?, "name")?)?.into(), item.clone())))
        .collect()
}
fn changed(records: &Map) -> Result<V> {
    let before = entry_map(field(map(field(records, "ancestor")?)?, "doc")?)?;
    let after = entry_map(field(map(field(records, "working")?)?, "doc")?)?;
    let mut changed = merge_physical(records)?.1;
    for name in before.keys().chain(after.keys()) {
        if !python_equal(get(&before, name), get(&after, name)) {
            changed.insert(name.clone());
        }
    }
    let before = named_items(&records["ancestor"])?;
    let after = named_items(&records["working"])?;
    for name in before.keys().chain(after.keys()) {
        if !python_equal(get(&before, name), get(&after, name)) {
            for item in [before.get(name), after.get(name)].into_iter().flatten() {
                changed.extend(entries(field(map(item)?, "doc")?)?.into_keys());
            }
        }
    }
    Ok(strings(changed))
}
fn project(
    snapshot: &Snapshot,
    runtime: Option<&Runtime>,
    bounds: &OperationalBounds,
) -> Result<V> {
    reasoning_operations::findings(&CapturedAssessment::from_snapshot(
        snapshot.clone(),
        None,
        "focused-review/v1",
        runtime,
        bounds.clone(),
        None,
    )?)
}
fn finding(kind: &str, id: &str, reason: &str) -> Result<V> {
    let mut value = obj([("kind", s(kind)), ("id", s(id)), ("reason", s(reason))]);
    let hash = digest(&value)?;
    map_mut(&mut value)?.insert("fingerprint".into(), s(&hash));
    Ok(value)
}
fn main_snapshot(records: &Map) -> Result<Snapshot> {
    let record = field(records, "main")?;
    let (capture, observed) = validated(record)?;
    snap(
        &capture,
        V::Map(physical(record)?),
        obj([
            ("read_mode", s("supplied")),
            (
                "operation",
                obj([
                    ("version", n("1")),
                    ("phase", s("observed")),
                    ("kind", s("history_watch_main")),
                    (
                        "identity",
                        s(&digest(field(
                            map(field(map(record)?, "history")?)?,
                            "sha256",
                        )?)?),
                    ),
                    ("baseline", capture.baseline.clone()),
                ]),
            ),
        ]),
        map(observed.data())?["as_of"].clone(),
    )
}
fn shared_snapshot(record: &V) -> Result<Snapshot> {
    let r = map(record)?;
    if r.get("history").is_some_and(truth) {
        return Ok(validated(record)?.1);
    }
    let observed = if let Some(snapshot) = r.get("snapshot").filter(|v| matches!(v, V::Map(_))) {
        Snapshot::from_snapshot(snapshot)?
    } else if let Some(snapshot) = r
        .get("core")
        .map(map)
        .transpose()?
        .and_then(|m| m.get("snapshot"))
        .filter(|v| **v != V::Null)
    {
        Snapshot::from_json(text(snapshot)?.as_bytes())?
    } else {
        return Err(error("shared facts need a captured core/v1 interpretation"));
    };
    let data = map(observed.data())?;
    require(
        digest(&data["document"])? == digest(get(r, "doc"))?,
        "shared snapshot document mismatch",
    )?;
    if let Some(capture) = map(&data["context"])?.get("watch_capture") {
        require(
            python_equal(capture, &obj([("hash", get(r, "hash").clone())])),
            "shared snapshot capture mismatch",
        )?;
    }
    Ok(observed)
}
fn lines(value: &V) -> Result<BTreeSet<String>> {
    list(value)?
        .iter()
        .map(|v| text(v).map(str::to_owned))
        .collect()
}
fn scenario_findings(
    snapshot: &Snapshot,
    records: &Map,
    runtime: Option<&Runtime>,
    bounds: &OperationalBounds,
) -> Result<(Vec<V>, Vec<V>)> {
    let current = map(&map(snapshot.data())?["hypotheses"])?;
    let shared = records.get("shared").filter(|v| truth(v));
    if current.is_empty() && shared.is_none() {
        return Ok((vec![], vec![]));
    }
    let old = named_items(&records["ancestor"])?;
    let mut selection = Map::new();
    for (name, hypothesis) in current {
        let before = old
            .get(name)
            .map(|v| field(map(v)?, "doc"))
            .transpose()?
            .map(entry_map)
            .transpose()?
            .unwrap_or_default();
        let after = entry_map(field(map(hypothesis)?, "document")?)?;
        selection.insert(
            name.clone(),
            strings(
                after
                    .iter()
                    .filter(|(id, body)| !python_equal(get(&before, id), body))
                    .map(|(id, _)| id.clone()),
            ),
        );
    }
    let observed_shared = shared.map(shared_snapshot).transpose()?;
    let candidate = Scenario::build(
        snapshot,
        &V::Map(selection.clone()),
        observed_shared.as_ref(),
    )?;
    let assessed = Scenario::assess(&candidate, runtime, bounds.clone())?;
    let baseline = if shared.is_some() || selection.values().any(truth) {
        let empty_selection = V::Map(
            selection
                .keys()
                .map(|name| (name.clone(), V::List(vec![])))
                .collect(),
        );
        Scenario::assess(
            &Scenario::build(snapshot, &empty_selection, None)?,
            runtime,
            bounds.clone(),
        )?
    } else {
        assessed.clone()
    };
    let a = map(&assessed)?;
    let mut findings = vec![];
    let mut add = |kind: &str, id: &str, reason: &str, computation: Option<&V>| -> Result<()> {
        let mut value = finding(kind, id, reason)?;
        let m = map_mut(&mut value)?;
        m.remove("fingerprint");
        m.extend([
            ("perspective".into(), s("scenario")),
            ("source_snapshot_id".into(), a["source_snapshot_id"].clone()),
            ("scenario_id".into(), a["scenario_id"].clone()),
        ]);
        if let Some(computation) = computation {
            m.insert("computation".into(), computation.clone());
        }
        let hash = digest(&value)?;
        map_mut(&mut value)?.insert("fingerprint".into(), s(&hash));
        findings.push(value);
        Ok(())
    };
    for id in list(&a["collisions"])? {
        let id = text(id)?;
        add(
            "collision",
            id,
            &format!("{id}: captured hypothetical/shared bodies disagree"),
            None,
        )?;
    }
    for (kind, key) in [
        ("falsified", "falsified"),
        ("uncheckable", "holes"),
        ("uncheckable", "moved"),
    ] {
        let current = lines(field(map(&a["findings"])?, key)?)?;
        let prior = lines(field(map(&map(&baseline)?["findings"])?, key)?)?;
        for line in current.difference(&prior) {
            add(kind, line.split(':').next().unwrap(), line, None)?;
        }
    }
    for head in list(&a["heads"])? {
        let head = map(head)?;
        let name = text(field(head, "name")?)?;
        if head["truth"] == V::Bool(true) {
            add(
                "falsified",
                name,
                &format!("{name}: hypothetical wrong_if holds under core/v1"),
                Some(&head["computation"]),
            )?;
        } else if head["truth"] == V::Null {
            add(
                "uncheckable",
                name,
                &format!("{name}: hypothetical condition is unavailable"),
                Some(&head["computation"]),
            )?;
        }
    }
    Ok((findings, vec![assessed]))
}
fn compare_inner(records: &V, runtime: Option<&Runtime>, bounds: &OperationalBounds) -> Result<V> {
    let snapshot = merged_snapshot(records)?;
    let records = map(records)?;
    let baseline_snapshot = main_snapshot(records)?;
    let projected = project(&snapshot, runtime, bounds)?;
    let baseline = project(&baseline_snapshot, runtime, bounds)?;
    let (scenario_findings, scenarios) = scenario_findings(&snapshot, records, runtime, bounds)?;
    let subjects = |snapshot: &Snapshot| -> Result<Map> {
        Ok(map(field(
            map(field(map(&map(snapshot.data())?["context"])?, "history")?)?,
            "subjects",
        )?)?
        .clone())
    };
    let current = subjects(&snapshot)?;
    let prior = subjects(&baseline_snapshot)?;
    let mut findings = vec![];
    for (subject, state) in current {
        let old = prior
            .get(&subject)
            .map(map)
            .transpose()?
            .and_then(|m| m.get("acceptance"));
        if string_is(field(map(&state)?, "acceptance")?, "contested")
            && !old.is_some_and(|v| string_is(v, "contested"))
        {
            findings.push(finding(
                "collision",
                &subject,
                &format!("{subject}: contested immutable history heads"),
            )?);
        }
    }
    findings.extend(scenario_findings);
    for (kind, key) in [
        ("falsified", "falsified"),
        ("uncheckable", "holes"),
        ("uncheckable", "moved"),
    ] {
        let current = lines(field(map(&projected)?, key)?)?;
        let prior = lines(field(map(&baseline)?, key)?)?;
        for line in current.difference(&prior) {
            findings.push(finding(kind, line.split(':').next().unwrap(), line)?);
        }
    }
    Ok(obj([
        (
            "state",
            s(if findings.is_empty() {
                "clear"
            } else {
                "attention"
            }),
        ),
        ("findings", V::List(findings)),
        ("changed", changed(records)?),
        ("versions", get(records, "versions").clone()),
        ("identity", get(records, "identity").clone()),
        ("scenarios", V::List(scenarios)),
    ]))
}
pub fn compare(records: &V, runtime: Option<&Runtime>, bounds: OperationalBounds) -> Result<V> {
    let m = map(records)?;
    match compare_inner(records, runtime, &bounds) {
        Ok(value) => Ok(value),
        Err(e) => Ok(obj([
            ("state", s("attention")),
            (
                "findings",
                V::List(vec![finding(
                    "uncheckable",
                    "record",
                    if e.0 == "stale_snapshot" {
                        "stale_snapshot: snapshot digest does not match"
                    } else {
                        &e.0
                    },
                )?]),
            ),
            ("changed", V::List(vec![])),
            ("versions", get(m, "versions").clone()),
            ("identity", get(m, "identity").clone()),
        ])),
    }
}

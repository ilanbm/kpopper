use super::*;
use crate::legacy_authoring::legacy_replaced as kept;
use crate::public_identity::ordinary_identity as I;
use crate::{
    history_transaction::{self as T, FileImage, PreparedMutation},
    history_transaction_fs as F,
    ordinary_write_report::Ancillary,
};
#[path = "ordinary_consolidation_archive.rs"]
mod archive;
fn indent(s: &str) -> usize {
    s.chars().take_while(|c| *c == ' ').count()
}
fn blank(s: &str) -> bool {
    s.trim().is_empty() || s.trim_start().starts_with('#')
}
fn collections(lines: &[String]) -> Vec<(String, usize, usize)> {
    let re = regex::Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*):(?: |$)").unwrap();
    let found: Vec<(String, usize)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| re.captures(l).map(|c| (c[1].into(), i)))
        .collect::<Vec<_>>();
    found
        .iter()
        .enumerate()
        .map(|(i, (n, s))| {
            (
                n.clone(),
                *s,
                found.get(i + 1).map(|(_, s)| *s).unwrap_or(lines.len()),
            )
        })
        .collect()
}
fn members(lines: &[String], start: usize, end: usize) -> (Option<usize>, Vec<(String, usize)>) {
    let re = regex::Regex::new(r"^( +)([A-Za-z_][A-Za-z0-9_.]*):(?: |$)").unwrap();
    let mut ind = None;
    let mut out = vec![];
    for (i, line) in lines.iter().enumerate().take(end).skip(start + 1) {
        if blank(line) {
            continue;
        }
        let n = indent(line);
        let expected = *ind.get_or_insert(n);
        if let Some(c) = re.captures(line)
            && c[1].len() == expected
        {
            out.push((c[2].into(), i));
        }
    }
    (ind, out)
}
fn block_end(lines: &[String], i: usize, ind: usize, end: usize) -> usize {
    let mut j = i + 1;
    while j < end {
        if lines[j].trim().is_empty() {
            let mut k = j;
            while k < end && lines[k].trim().is_empty() {
                k += 1;
            }
            if k < end && indent(&lines[k]) > ind {
                j = k;
                continue;
            }
            break;
        }
        if indent(&lines[j]) > ind {
            j += 1;
        } else {
            break;
        }
    }
    j
}
fn locate(lines: &[String], id: &str) -> Option<(String, usize, usize, usize)> {
    for (n, s, e) in collections(lines) {
        let (ind, ms) = members(lines, s, e);
        for (k, i) in ms {
            if k == id {
                return Some((n, ind.unwrap(), i, block_end(lines, i, ind.unwrap(), e)));
            }
        }
    }
    None
}
fn inline(s: &str) -> &str {
    s.split_once(':').map(|(_, v)| v.trim()).unwrap_or("")
}
fn common(a: &str, b: &str) -> usize {
    a.split('.')
        .zip(b.split('.'))
        .take_while(|(a, b)| a == b)
        .count()
}
fn place(ms: &[(String, usize)], id: &str) -> (usize, bool) {
    let best = ms.iter().map(|(m, _)| common(m, id)).max().unwrap_or(0);
    let sib = ms
        .iter()
        .enumerate()
        .filter(|(_, (m, _))| best == 0 || common(m, id) == best)
        .collect::<Vec<_>>();
    sib.iter()
        .find(|(_, (m, _))| m.as_str() > id)
        .map(|(i, _)| (*i, true))
        .unwrap_or_else(|| (sib.last().unwrap().0, false))
}
fn ensure(lines: &mut Vec<String>, collection: &str) {
    if collections(lines).iter().any(|(n, _, _)| n == collection) {
        return;
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.push(format!("{collection}:"));
    lines.push(String::new());
}
fn shifted(block: &[String], ind: usize) -> Vec<String> {
    let old = indent(&block[0]);
    block
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                l.clone()
            } else {
                format!(
                    "{}{}",
                    " ".repeat((indent(l) as isize + ind as isize - old as isize).max(0) as usize),
                    l.trim_start_matches(' ')
                )
            }
        })
        .collect()
}
fn insert_block(lines: &mut Vec<String>, col: &str, id: &str, block: &[String]) -> Result<String> {
    ensure(lines, col);
    let (_, s, e) = collections(lines)
        .into_iter()
        .find(|(n, _, _)| n == col)
        .unwrap();
    let (ind, ms) = members(lines, s, e);
    let block = shifted(block, ind.unwrap_or(2));
    if ms.is_empty() {
        lines.splice(s + 1..s + 1, block);
        return Ok(format!("{id} into {col}, its first entry"));
    }
    let (ai, before) = place(&ms, id);
    let (anchor, start) = &ms[ai];
    let pos = if before {
        *start
    } else {
        block_end(lines, *start, ind.unwrap(), e)
    };
    let out = format!(
        "{id} into {col}, {} {anchor}",
        if before { "before" } else { "after" }
    );
    lines.splice(pos..pos, block);
    Ok(out)
}
fn insert_body(lines: &mut Vec<String>, col: &str, id: &str, body: &Source) -> Result<String> {
    ensure(lines, col);
    let (_, s, e) = collections(lines)
        .into_iter()
        .find(|(n, _, _)| n == col)
        .unwrap();
    let (ind, ms) = members(lines, s, e);
    if ms.is_empty() {
        let block = crate::legacy_authoring::entry_lines_ordered(
            id,
            body,
            ind.unwrap_or(2),
            ind.unwrap_or(2) + 2,
            false,
        )?;
        lines.splice(s + 1..s + 1, block);
        return Ok(format!("{id} into {col}, its first entry"));
    }
    let (ai, before) = place(&ms, id);
    let (anchor, start) = &ms[ai];
    let ind = ind.unwrap();
    let end = block_end(lines, *start, ind, e);
    let find = members(lines, *start, end).0.unwrap_or(ind + 2);
    let mut block = crate::legacy_authoring::entry_lines_ordered(
        id,
        body,
        ind,
        find,
        inline(&lines[*start]).starts_with('{'),
    )?;
    let mut pos = if before { *start } else { end };
    if before {
        if pos > 0 && pos - 1 > s && lines[pos - 1].trim().is_empty() {
            block.push(String::new());
        }
    } else if pos < e && lines[pos].trim().is_empty() {
        pos += 1;
        block.push(String::new());
    }
    let out = format!(
        "{id} into {col}, {} {anchor}",
        if before { "before" } else { "after" }
    );
    lines.splice(pos..pos, block);
    Ok(out)
}
fn file_for(
    files: &[PathBuf],
    texts: &BTreeMap<PathBuf, Vec<String>>,
    id: &str,
    col: &str,
) -> PathBuf {
    let head = id.split('.').next().unwrap();
    let (mut opened, mut own, mut held) = (None, None, None);
    for f in files {
        let lines = &texts[f];
        if locate(lines, id).is_some() {
            return f.clone();
        }
        let cols = collections(lines);
        let members = cols
            .iter()
            .filter(|(n, _, _)| n != "meta")
            .map(|(n, s, e)| {
                (
                    n.clone(),
                    members(lines, *s, *e)
                        .1
                        .into_iter()
                        .map(|(m, _)| m)
                        .filter(|m| m.contains('.'))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let theirs = members.values().flatten().collect::<Vec<_>>();
        if held.is_none() && !theirs.is_empty() {
            held = Some(f.clone());
        }
        if cols.iter().any(|(n, _, _)| n == col) {
            if members
                .get(col)
                .into_iter()
                .flatten()
                .any(|m| m.split('.').next() == Some(head))
            {
                return f.clone();
            }
            opened.get_or_insert_with(|| f.clone());
        }
        if own.is_none()
            && !theirs.is_empty()
            && theirs.iter().all(|m| m.split('.').next() == Some(head))
        {
            own = Some(f.clone());
        }
    }
    own.or(opened).or(held).unwrap_or_else(|| files[0].clone())
}
fn ordered(source: &O) -> Result<Source> {
    Ok(match source {
        O::Scalar(v) => Source::Scalar(v.clone()),
        O::List(a) => Source::List(a.iter().map(ordered).collect::<Result<_>>()?),
        O::Map(m) => Source::Map(
            m.iter()
                .map(|(k, v)| {
                    Ok((
                        k.text()
                            .ok_or_else(|| {
                                error(
                                    "invalid_history_value: snapshot mappings require string keys",
                                )
                            })?
                            .into(),
                        ordered(v)?,
                    ))
                })
                .collect::<Result<_>>()?,
        ),
    })
}
fn source_body<'a>(source: &'a O, id: &str) -> Option<&'a O> {
    if let O::Map(cols) = source {
        cols.iter().find_map(|(_, v)| v.get(id))
    } else {
        None
    }
}
fn block(h: &Hypothesis, id: &str, c: &Union<'_>) -> Result<(String, Vec<String>)> {
    let lines = h.text.split('\n').map(str::to_owned).collect::<Vec<_>>();
    if let Some((col, _, s, e)) = locate(&lines, id) {
        let mut block = lines[s..e].to_vec();
        while block.last().is_some_and(|l| l.trim().is_empty()) {
            block.pop();
        }
        return Ok((col, block));
    }
    let col = crate::reasoning_fields::collections(&h.doc)?
        .into_iter()
        .find(|(_, m)| m.contains_key(id))
        .map(|(col, _)| col)
        .unwrap_or(G::collection_for_document(
            &c.base_doc,
            &c.view.as_ref().unwrap().base.reader.fields,
            id,
            &h.raw[id],
            None,
        )?);
    Ok((
        col,
        crate::legacy_authoring::entry_lines_ordered(
            id,
            &ordered(
                source_body(&h.source, id).ok_or_else(|| error("hypothesis_source_changed"))?,
            )?,
            2,
            4,
            false,
        )?,
    ))
}
fn renewed(
    c: &Union<'_>,
    id: &str,
    hi: usize,
    page: Option<&V>,
    stamp: &str,
    why: &str,
) -> Result<Option<Source>> {
    let Some(old) = c.base.base.judgments.get(id) else {
        return Ok(None);
    };
    let h = &c.hyps[hi];
    let mut body =
        ordered(source_body(&h.source, id).ok_or_else(|| error("hypothesis_source_changed"))?)?;
    let Source::Map(fields) = &mut body else {
        return Ok(None);
    };
    let snapshot = text(&c.base.base.reader.fields["snapshot"]).unwrap_or("");
    let snapshot = if snapshot.is_empty() {
        "seen"
    } else {
        snapshot
    };
    let seen = fields
        .iter()
        .find(|(k, _)| k == snapshot)
        .map(|(_, v)| v.clone());
    let arrangement = G::arrangement(&c.base.base.reader, old);
    let mut extra = Map::new();
    let ended = if arrangement {
        let stood = page
            .and_then(|p| map(p).ok())
            .and_then(|p| p.get(id))
            .map(|v| get(v, "stood"))
            .filter(|v| **v != V::Null);
        extra.insert("born".into(), s(stamp));
        format!(
            "born {}{}; {why} on {stamp}",
            if truth(get(old, "born")) {
                py(get(old, "born"))
            } else {
                "undated".into()
            },
            stood
                .map(|n| format!(
                    ", stood {} session{}",
                    py(n),
                    if py(n) == "1" { "" } else { "s" }
                ))
                .unwrap_or_default()
        )
    } else {
        format!("{why} on {stamp}")
    };
    let base = kept::judgment_renewal(old, &V::Map(Map::new()), &ended)?;
    extra.insert("replaced".into(), get(&base, "replaced").clone());
    fields.retain(|(k, _)| !extra.contains_key(k) && k != snapshot);
    if arrangement {
        fields.push(("born".into(), Source::from_typed(&extra["born"])));
    }
    fields.push(("replaced".into(), Source::from_typed(&extra["replaced"])));
    if let Some(seen) = seen {
        fields.push((snapshot.into(), seen));
    }
    Ok(Some(body))
}
fn relative(root: &Path, path: &Path) -> Result<String> {
    let p = crate::source_inventory::absolute(path)?;
    let root = crate::source_inventory::absolute(root)?;
    let value = p
        .strip_prefix(root)
        .map_err(|_| error("invalid_path"))?
        .to_string_lossy()
        .replace('\\', "/");
    crate::history_authority::relative_path(&value)?;
    Ok(value)
}
fn notice_path(route: &WriteRoute, path: &Path) -> String {
    let here = route.paths()[0].parent().unwrap();
    let root = if route.project().is_git() && path.starts_with(&route.project().root) {
        route.project().root.as_path()
    } else {
        here
    };
    relative(root, path).unwrap_or_else(|_| path.to_string_lossy().into())
}
fn parsed(files: &[PathBuf], texts: &BTreeMap<PathBuf, Vec<String>>) -> Result<O> {
    let mut out = O::Map(vec![]);
    let mut origins = BTreeMap::new();
    for f in files {
        let source =
            crate::history_yaml::decode_ordinary_source_value(texts[f].join("\n").as_bytes())?;
        crate::source_document::merge(&mut out, &source, f, &mut origins)?;
    }
    Ok(out)
}
fn count_flagged(doc: &V, hyps: &Map, runtime: Option<&Runtime>) -> Result<String> {
    let mut reader = projection(doc, hyps, runtime)?.base.reader;
    reader.ids.insert("graph.flagged".into());
    let layers = reader.hypotheses.clone();
    let conflicts = reader.knowledge_conflicts.clone();
    let reader = reader.with_layers(layers, conflicts)?;
    Ok(py(&reader.value("graph.flagged")?))
}

pub(super) struct WriteContext<'a> {
    pub capture: &'a CapturedSource,
    pub route: &'a WriteRoute,
    pub side: &'a Ancillary,
    pub inventory: &'a Inventory,
    pub stamp: &'a str,
    pub runtime: Option<&'a Runtime>,
    pub page_capture: Option<&'a crate::ordinary_page_capture::PageCapture>,
}
pub(super) fn fold(
    c: &Union<'_>,
    context: &WriteContext<'_>,
    drops: &Map,
    page: Option<&V>,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<String> {
    let WriteContext {
        capture,
        route,
        side,
        inventory,
        stamp,
        ..
    } = *context;
    let entry = &route.paths()[0];
    let layout = T::Layout::for_entry(
        entry
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| error("invalid_path"))?,
    )?;
    let replaced_path = entry.parent().unwrap().join(&layout.replaced);
    let files = capture.members();
    let mut texts = files
        .iter()
        .map(|p| {
            let raw = capture
                .files()
                .get(p)
                .ok_or_else(|| error("snapshot_changed"))?;
            Ok((
                p.clone(),
                std::str::from_utf8(raw)
                    .map_err(|_| error("invalid_utf8"))?
                    .split('\n')
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut writes = c
        .arrived
        .iter()
        .map(|(id, h)| (id.clone(), *h, false))
        .chain(c.updates.iter().map(|u| (u.id.clone(), u.hyp, true)))
        .collect::<Vec<_>>();
    let world = &c.view.as_ref().unwrap().base;
    writes.sort_by(|a, b| {
        (world.judgments.contains_key(&a.0), &a.0).cmp(&(world.judgments.contains_key(&b.0), &b.0))
    });
    let (mut out, mut added, mut replaced, mut side_after) = (vec![], 0usize, 0usize, None);
    let mut trailed = vec![];
    for (id, hi, replace) in &writes {
        let h = &c.hyps[*hi];
        let (col, block) = block(h, id, c)?;
        if *replace {
            let target = files
                .iter()
                .find(|p| locate(&texts[*p], id).is_some())
                .ok_or_else(|| error("consolidation_target_missing"))?;
            let why = &c.updates.iter().find(|u| u.id == *id).unwrap().why;
            let renewed = renewed(c, id, *hi, page, stamp, why)?;
            let lines = texts.get_mut(target).unwrap();
            let (_, ind, start, end) = locate(lines, id).unwrap();
            if let Some(body) = &renewed {
                let find = members(lines, start, end).0.unwrap_or(ind + 2);
                let mut identities = I::FieldIdentity::new();
                if let Source::Map(fields) = body {
                    for (key, value) in fields {
                        if key != "born"
                            && key != "replaced"
                            && matches!(value, Source::Map(_) | Source::List(_))
                        {
                            identities
                                .insert(key.clone(), I::field_identity(&h.text, id, key, value)?);
                        }
                    }
                }
                let rendered = I::entry_lines(
                    id,
                    body,
                    ind,
                    find,
                    inline(&lines[start]).starts_with('{'),
                    &identities,
                )?;
                lines.splice(start..end, rendered);
                trailed.push((id.clone(), *hi, body.typed(), target.clone()));
            } else {
                lines.splice(start..end, shifted(&block, ind));
            }
            replaced += 1;
            out.push(format!(
                "replace {id} with what {} holds, where it stands{}",
                h.name,
                if renewed.as_ref().is_some_and(|b| b.get("born").is_some()) {
                    " - born renewed, and what it replaced kept"
                } else if renewed.is_some() {
                    " - what it replaced kept"
                } else {
                    ""
                }
            ));
            if renewed.is_some() {
                let old = &c.base.base.judgments[id];
                let gone = ["verdict", "because", "request"]
                    .into_iter()
                    .filter(|k| map(old).is_ok_and(|m| m.contains_key(*k)))
                    .collect::<Vec<_>>();
                out.push(format!(
                    "  kept: the replaced {}, in {}",
                    if gone.is_empty() {
                        "body".into()
                    } else {
                        gone.join(", ")
                    },
                    relative(entry.parent().unwrap(), &replaced_path)?
                ));
                let dep = text(&world.reader.fields["deps"])?;
                let now = strings(get(&h.raw[id], dep));
                let gone = strings(get(old, dep))
                    .into_iter()
                    .filter(|d| !now.contains(d))
                    .collect::<Vec<_>>();
                if !gone.is_empty() {
                    out.push(format!("  no longer rests on {}", gone.join("; ")));
                }
            }
        } else {
            let target = file_for(files, &texts, id, &col);
            let where_ = insert_block(texts.get_mut(&target).unwrap(), &col, id, &block)?;
            out.push(
                "carry ".to_owned()
                    + &where_.replacen(
                        &format!("{id} into"),
                        &format!("{id} from {} into", h.name),
                        1,
                    ),
            );
            added += 1;
        }
    }
    if !writes.is_empty() {
        for f in files {
            if I::bump_updated(texts.get_mut(f).unwrap(), stamp)? {
                break;
            }
        }
    }
    let after_source = parsed(files, &texts)
        .map_err(|e| error(&format!("the fold broke the record and was undone: {e}")))?;
    let after_doc = after_source.projected();
    let all = map(capture.hypotheses())?;
    let after = projection(&after_doc, all, c.runtime)
        .map_err(|e| error(&format!("the fold broke the record and was undone: {e}")))?;
    for (id, hi, _) in &writes {
        require(
            after.base.reader.ids.contains(id),
            &format!(
                "the fold broke the record and was undone: {id} is not in the record after the fold"
            ),
        )?;
        let want = claim(&c.hyps[*hi].raw[id]);
        let got = claim(after.base.reader.raw.get(id).unwrap_or(&V::Null));
        require(
            same_claim(&want, &got),
            &format!(
                "the fold broke the record and was undone: {id} reads back as {:?}",
                short(&got, 110)
            ),
        )?;
    }
    let before = checks(&c.base, Some(&side.brief.projected()))?.0;
    let worse = checks(&after, Some(&side.brief.projected()))?
        .0
        .into_iter()
        .filter(|l| !before.contains(l))
        .collect::<Vec<_>>();
    require(
        worse.is_empty(),
        &format!(
            "the fold broke the record and was undone: check fails on what was folded: {}",
            worse.iter().take(3).cloned().collect::<Vec<_>>().join("; ")
        ),
    )?;
    for (id, hi, new, target) in &trailed {
        let before = side_after
            .as_deref()
            .or_else(|| capture.files().get(&replaced_path).map(Vec::as_slice))
            .or_else(|| inventory.files.get(&replaced_path).map(Vec::as_slice));
        let old = kept::old_body(&capture.files()[target], id)?;
        let dep = text(&world.reader.fields["deps"])?;
        let gone = kept::dropped_dependencies(
            &old.typed(),
            &c.hyps[*hi].raw[id],
            dep,
            Some(&V::Map(drops.clone())),
        )?
        .into_iter()
        .filter_map(|(id, why)| why.map(|why| (id, why)))
        .collect::<Map>();
        let why = &c.updates.iter().find(|u| u.id == *id).unwrap().why;
        let finite_before = archive::decision_input(before)?;
        let (encoded, version, _) = kept::keep_replaced(
            finite_before.as_deref(),
            id,
            &old,
            new,
            why,
            stamp,
            (!gone.is_empty()).then_some(&V::Map(gone)),
        )?;
        side_after = Some(archive::preserve(
            before,
            &capture.files()[target],
            id,
            &encoded,
            version,
        )?);
    }
    let changed = files
        .iter()
        .filter(|p| texts[*p].join("\n").as_bytes() != capture.files()[*p])
        .cloned()
        .collect::<Vec<_>>();
    let mut images = changed
        .iter()
        .map(|p| FileImage {
            path: p.to_string_lossy().into(),
            role: if p == entry {
                "record"
            } else {
                "record_member"
            }
            .into(),
            before: Some(capture.files()[p].clone()),
            after: Some(texts[p].join("\n").into_bytes()),
        })
        .collect::<Vec<_>>();
    if let Some(bytes) = side_after {
        images.push(FileImage {
            path: replaced_path.to_string_lossy().into(),
            role: "replaced".into(),
            before: inventory.files.get(&replaced_path).cloned(),
            after: Some(bytes),
        });
    }
    for h in &c.hyps {
        images.push(FileImage {
            path: h.path()?.to_string_lossy().into(),
            role: "hypothesis".into(),
            before: Some(capture.files()[h.path()?].clone()),
            after: None,
        });
    }
    let names = c
        .hyps
        .iter()
        .map(|h| h.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    if writes.is_empty() {
        out.push(format!(
            "nothing to write: the base already holds everything {names} propose{}",
            if c.hyps.len() == 1 { "s" } else { "" }
        ));
    } else {
        let nj = writes
            .iter()
            .filter(|(id, _, _)| world.judgments.contains_key(id))
            .count();
        let ne = writes.len() - nj;
        out.push(format!(
            "folded {names}: {ne} entr{} and {nj} judgment{} - {added} added, {replaced} replaced",
            if ne == 1 { "y" } else { "ies" },
            if nj == 1 { "" } else { "s" }
        ));
    }
    let mut shown = changed
        .iter()
        .map(|p| notice_path(route, p))
        .collect::<Vec<_>>();
    if !trailed.is_empty() {
        shown.push(notice_path(route, &replaced_path));
    }
    let mut commit = shown.clone();
    for h in &c.hyps {
        let p = notice_path(route, h.path()?);
        shown.push(format!("{p} (deleted)"));
        commit.push(p);
    }
    out.push(format!(
        "files to commit: {}",
        if shown.is_empty() {
            "none".into()
        } else {
            shown.join(", ")
        }
    ));
    if route.project().is_git() && entry.starts_with(&route.project().root) && !commit.is_empty() {
        out.push(format!("next: git add {} && git commit", commit.join(" ")));
    }
    let n = count_flagged(&after_doc, all, c.runtime)?;
    out.extend([
        String::new(),
        format!(
            "the record needs a person on {n} judgment{} - check says the rest",
            if n == "1" { "" } else { "s" }
        ),
    ]);
    publish(context, images, &after_doc, probe)?;
    Ok(out.join("\n") + "\n")
}
fn publish(
    context: &WriteContext<'_>,
    mut files: Vec<FileImage>,
    after: &V,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<()> {
    let WriteContext {
        capture,
        route,
        side,
        inventory,
        page_capture,
        ..
    } = *context;
    let entry = &route.paths()[0];
    let local = T::Layout::for_entry(
        entry
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| error("invalid_path"))?,
    )?;
    capture.snapshot().map_err(|e| {
        if e.0 == "invalid_yaml_key" {
            error("invalid_history_value: snapshot mappings require string keys")
        } else {
            e
        }
    })?;
    let mut root = entry.parent().unwrap().to_path_buf();
    for p in capture
        .members()
        .iter()
        .map(PathBuf::as_path)
        .chain(files.iter().map(|f| Path::new(&f.path)))
    {
        while !p.starts_with(&root) {
            require(root.pop(), "invalid_path")?;
        }
    }
    for item in &mut files {
        item.path = relative(&root, Path::new(&item.path))?;
    }
    let hypdir = entry.parent().unwrap().join(&local.hypotheses);
    let record_members = capture
        .members()
        .iter()
        .map(|p| {
            Ok((
                relative(&root, p)?,
                s(&crate::identity::sha256(&capture.files()[p])),
            ))
        })
        .collect::<Result<Map>>()?;
    let hypothesis_members = capture
        .files()
        .iter()
        .filter(|(p, _)| {
            p.parent() == Some(hypdir.as_path())
                && p.extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s == "yaml" || s == "yml")
        })
        .map(|(p, b)| Ok((relative(&root, p)?, s(&crate::identity::sha256(b)))))
        .collect::<Result<Map>>()?;
    let input_files = side
        .inputs
        .iter()
        .map(|(p, d)| Ok((relative(&root, p)?, d.as_deref().map(s).unwrap_or(V::Null))))
        .collect::<Result<Map>>()?;
    let routing = crate::source_capture::routing_observation(route.paths(), &route.project().root)?;
    if let Some(page) = page_capture {
        require(page.routing == routing, "snapshot_changed")?;
    }
    let baseline = obj([
        ("kind", s("direct/v1")),
        ("routing", routing.clone()),
        (
            "transaction_root",
            s(root.to_str().ok_or_else(|| error("invalid_path"))?),
        ),
        ("record_members", V::Map(record_members)),
        ("hypothesis_members", V::Map(hypothesis_members)),
        ("hypotheses_directory", s(&relative(&root, &hypdir)?)),
        ("input_files", V::Map(input_files)),
        ("policy", route.config().clone()),
    ]);
    let marker = if let Some(raw) = F::read(&entry.parent().unwrap().join(&local.authority))? {
        let marker = crate::history_yaml::decode_document(&raw)?;
        crate::history_authority::validate_authority(&marker)?;
        require(
            get(&marker, "authority") == &s("legacy"),
            "project_route_changed",
        )?;
        marker
    } else {
        crate::history_authority::authority(
            &format!(
                "legacy-{}",
                &crate::identity::sha256(entry.to_string_lossy().as_bytes())[..32]
            ),
            "legacy",
            &crate::history_authoring::n("0"),
            Map::new(),
        )?
    };
    let capabilities = crate::reasoning_fields::capabilities(capture.ordinary_document(), None)?;
    let receipt = T::semantic_receipt(
        text(get(&capabilities, "profile"))?,
        &capabilities,
        &obj([("document", capture.ordinary_document().clone())]),
        &obj([("document", after.clone())]),
    )?;
    let mutation = PreparedMutation::prepare(
        &crate::public_history::fresh_id("direct")?,
        &marker,
        &baseline,
        files,
        &receipt,
        &relative(&root, entry)?,
        None,
    )?;
    let journal = relative(&root, &entry.parent().unwrap().join(&local.journal))?;
    let probe = std::cell::RefCell::new(probe);
    probe.borrow_mut()("prepared")?;
    let mut verify = |_: &V| {
        route.verify()?;
        capture.verify()?;
        inventory.verify()?;
        if let Some(page) = page_capture {
            page.verify()?;
        }
        require(
            crate::source_capture::routing_observation(route.paths(), &route.project().root)?
                == routing,
            "snapshot_changed",
        )?;
        require(
            !crate::public_consolidation::active_history(entry)?,
            "project_route_changed",
        )?;
        probe.borrow_mut()("validated")
    };
    let mut committed = |_: &V| probe.borrow_mut()("published");
    F::publish_legacy(
        &root,
        &journal,
        &mutation,
        &mut verify,
        Some(&mut committed),
    )?;
    crate::session_activity::published(&root, mutation.files(), None);
    Ok(())
}

pub(super) fn refute(
    context: &WriteContext<'_>,
    h: &Hypothesis,
    why: &str,
    source: Option<&str>,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<String> {
    let WriteContext {
        capture,
        route,
        side,
        stamp,
        runtime,
        ..
    } = *context;
    let base = captured_projection(capture, runtime)?;
    let world = &base.base;
    let src = if let Some(src) = source {
        require(
            truth(get(world.reader.raw.get(src).unwrap_or(&V::Null), "asked")),
            &format!("refused - {src} is not a session source carrying what it was asked"),
        )?;
        src.into()
    } else {
        world.reader.raw.iter().filter(|(id,b)|world.reader.ids.contains(*id)&&!crate::reasoning_fields::BUILTINS.contains(&id.as_str())&&truth(get(b,"asked"))).map(|(id,b)|(G::read_on(b,&world.reader).unwrap_or("0001-01-01".into()),id.clone())).max().map(|(_,id)|id).ok_or_else(||error("refused - a finding is from a session, and this record holds no session source: add s.<date>_<slug> asked=\"...\" name=\"...\" first, or name one with --as"))?
    };
    super::privacy(
        capture,
        route,
        std::slice::from_ref(h),
        &obj([("kind", s("refute")), ("why", s(why)), ("source", s(&src))]),
        std::slice::from_ref(&src),
    )?;
    let stem = format!(
        "hyp.{}",
        h.name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            })
            .collect::<String>()
    );
    let mut id = stem.clone();
    let mut n = 1;
    while world.reader.ids.contains(&id) {
        n += 1;
        id = format!("{stem}_{n}");
    }
    let reason = why
        .split(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut fields = vec![
        ("v".into(), Source::Scalar(s("refuted"))),
        (
            "name".into(),
            Source::Scalar(s(&if truth(get(&h.head, "claim")) {
                py(get(&h.head, "claim"))
            } else {
                format!("hypothesis {}", h.name)
            })),
        ),
        ("from".into(), Source::Scalar(s(&src))),
        ("at".into(), Source::Scalar(s(&reason))),
        ("of".into(), Source::Scalar(s(stamp))),
    ];
    let refutes = h
        .raw
        .iter()
        .filter(|(_, b)| shaped(b, &world.reader.fields))
        .map(|(id, _)| Source::Scalar(s(id)))
        .collect::<Vec<_>>();
    if !refutes.is_empty() {
        fields.push(("refutes".into(), Source::List(refutes)));
    }
    let body = Source::Map(fields);
    let action = obj([
        ("kind", s("add")),
        ("id", s(&id)),
        ("body", body.typed()),
        ("as_of", s(stamp)),
        ("why", V::Null),
        ("into", V::Null),
        ("hypothesis", V::Null),
    ]);
    let mut reader = Reader::new(capture.ordinary_document(), runtime)?.with_layers(
        world.reader.hypotheses.clone(),
        world.reader.knowledge_conflicts.clone(),
    )?;
    let (action, notes) = reader.normalize(&action)?;
    let refusal = reader.validate(&action)?;
    require(
        refusal.is_empty(),
        &format!("refused - {}", refusal.join("\n          ")),
    )?;
    let sources = crate::public_identity::ordinary_sameness::Sources {
        base: Some(capture.source()),
        hypotheses: BTreeMap::from([(h.name.clone(), &h.source)]),
    };
    let notice = crate::public_identity::ordinary_sameness::nearest_existing_from_sources(
        &reader, &action, &sources,
    )?;
    require(
        notice.refusals.is_empty(),
        &format!("refused - {}", notice.refusals.join("\n          ")),
    )?;
    let col = G::collection_for(&reader, &id, &body.typed(), None)?;
    let mut texts = capture
        .members()
        .iter()
        .map(|p| {
            Ok((
                p.clone(),
                std::str::from_utf8(&capture.files()[p])
                    .map_err(|_| error("invalid_utf8"))?
                    .split('\n')
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let target = file_for(capture.members(), &texts, &id, &col);
    let where_ = insert_body(texts.get_mut(&target).unwrap(), &col, &id, &body)?;
    for f in capture.members() {
        if I::bump_updated(texts.get_mut(f).unwrap(), stamp)? {
            break;
        }
    }
    let after = parsed(capture.members(), &texts)?.projected();
    let after_view = projection(&after, map(capture.hypotheses())?, runtime)?;
    require(
        after_view
            .base
            .reader
            .raw
            .get(&id)
            .is_some_and(|v| same_claim(&claim(v), &s("refuted"))),
        "the write broke the record and was undone",
    )?;
    let changed = capture
        .members()
        .iter()
        .filter(|p| texts[*p].join("\n").as_bytes() != capture.files()[*p])
        .cloned()
        .collect::<Vec<_>>();
    let entry = &route.paths()[0];
    let mut images = changed
        .iter()
        .map(|p| FileImage {
            path: p.to_string_lossy().into(),
            role: if p == entry {
                "record"
            } else {
                "record_member"
            }
            .into(),
            before: Some(capture.files()[p].clone()),
            after: Some(texts[p].join("\n").into_bytes()),
        })
        .collect::<Vec<_>>();
    images.push(FileImage {
        path: h.path()?.to_string_lossy().into(),
        role: "hypothesis".into(),
        before: Some(capture.files()[h.path()?].clone()),
        after: None,
    });
    let mut out = notice.text.lines().map(str::to_owned).collect::<Vec<_>>();
    out.extend(notes);
    out.push(format!("add {where_}"));
    out.extend(crate::ordinary_write_report::render(
        "add",
        &id,
        &after_view.base,
        None,
        side,
    )?);
    out.retain(|line| !line.trim().is_empty());
    out.push(format!("refuted {}: {id} holds its claim as a negative finding, from {src}; {} deleted, and nothing else of it enters",h.name,notice_path(route,h.path()?)));
    out.push(format!(
        "files to commit: {}",
        changed
            .iter()
            .map(|p| notice_path(route, p))
            .chain(std::iter::once(format!(
                "{} (deleted)",
                notice_path(route, h.path()?)
            )))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    publish(context, images, &after, probe)?;
    Ok(out.join("\n") + "\n")
}
/// Only an explicit layout contribution needs the optional Hub's captured facts.
/// Its semantic computation is shared with Hub; this adapter only selects the boundary.
pub(super) fn page(
    capture: &CapturedSource,
    hyps: &[Hypothesis],
    route: &WriteRoute,
    inventory: &Inventory,
    runtime: Option<&Runtime>,
) -> Result<Option<crate::ordinary_page_capture::PageCapture>> {
    let entry = &route.paths()[0];
    let layout = T::Layout::for_entry(
        entry
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| error("invalid_path"))?,
    )?;
    let view_path = entry.parent().unwrap().join(layout.view);
    let Some(raw) = inventory.files.get(&view_path) else {
        return Ok(None);
    };
    let base = captured_projection(capture, runtime)?;
    let mut candidate = capture.ordinary_document().clone();
    for h in hyps {
        candidate = layer(&candidate, &h.doc)?;
    }
    let candidate = Reader::new(&candidate, runtime)?;
    let reads_page = |body: &V, fields: &Map| {
        let dep = text(&fields["deps"]).unwrap_or("");
        strings(get(body, dep)).iter().any(|id| {
            id.starts_with("page.") && crate::reasoning_fields::BUILTINS.contains(&id.as_str())
        }) || R::predicate_refs(&R::predicate_of(body, fields))
            .iter()
            .any(|id| {
                id.starts_with("page.") && crate::reasoning_fields::BUILTINS.contains(&id.as_str())
            })
    };
    let relevant = hyps.iter().flat_map(|h| h.raw.iter()).any(|(id, body)| {
        let old = base.base.reader.raw.get(id).unwrap_or(&V::Null);
        matches!(body, V::Map(_))
            && body != old
            && (G::arrangement(&candidate, body)
                || (base.base.judgments.contains_key(id) && G::arrangement(&base.base.reader, old)))
            && (reads_page(body, &candidate.fields) || reads_page(old, &base.base.reader.fields))
    });
    if !relevant {
        return Ok(None);
    }
    let page = crate::ordinary_page_capture::PageCapture::capture(
        route.paths(),
        &route.project().root,
        None,
        runtime,
    )
    .map_err(|e| error(&format!("presentation contribution cannot be checked: {e}")))?;
    require(
        page.view_path == view_path && page.view_before.as_deref() == Some(raw.as_slice()),
        "snapshot_changed",
    )?;
    Ok(Some(page))
}
/// An incidental malformed brief is still bound by bytes, but is only interpreted
/// when this consolidation actually crosses into the optional page application.
pub(super) fn ancillary(entry: &Path, inventory: &mut Inventory) -> Result<Ancillary> {
    let layout = T::Layout::for_entry(
        entry
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| error("invalid_path"))?,
    )?;
    let root = entry.parent().ok_or_else(|| error("invalid_path"))?;
    let view = root.join(layout.view);
    let replaced = root.join(layout.replaced);
    let mut inputs = BTreeMap::new();
    let brief = if inventory.exists(&view)? {
        let raw = inventory.read(&view)?;
        inputs.insert(view, Some(crate::identity::sha256(&raw)));
        crate::history_yaml::decode_ordinary_source_value(&raw).unwrap_or(O::Scalar(V::Null))
    } else {
        inputs.insert(view, None);
        O::Scalar(V::Null)
    };
    let replaced_value = if inventory.file(&replaced)? {
        let raw = inventory.read(&replaced)?;
        inputs.insert(replaced, Some(crate::identity::sha256(&raw)));
        crate::history_yaml::decode_ordinary_source_value(&raw)
            .map(|v| v.projected())
            .unwrap_or(V::Null)
    } else {
        if !inventory.exists(&replaced)? {
            inputs.insert(replaced, None);
        }
        V::Null
    };
    Ok(Ancillary {
        brief,
        replaced: replaced_value,
        inputs,
    })
}

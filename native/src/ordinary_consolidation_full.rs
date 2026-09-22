//! Read-only full ordinary-domain consolidation. No value here is admitted to a writer.
use super::{CommandOutput, Options};
use crate::{
    Result,
    history_contract::error,
    ordinary_domain_counts as C, ordinary_fields as F, ordinary_language as L,
    ordinary_semantics::{self as R, Reader},
    ordinary_value::{Map, Value as V, map, map_mut, named, py, python_equal, same, text, truth},
    ordinary_views::{Projection, World, predicate_text, short},
    project_modes::WriteRoute,
    reasoning_runtime::Runtime,
    require,
    source_capture::{OrdinaryCapture as CapturedSource, ReadMode},
    source_inventory::Inventory,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
#[path = "ordinary_consolidation_guards.rs"]
mod guards;
#[path = "ordinary_consolidation_report.rs"]
mod report;
use guards as G;
#[path = "ordinary_consolidation_candidates.rs"]
mod candidates;
#[path = "ordinary_consolidation_page.rs"]
mod page;
#[path = "ordinary_consolidation_pending.rs"]
mod pending;
use candidates::near;
fn s(value: &str) -> V {
    V::Text(value.into())
}
#[derive(Clone)]
struct Hypothesis {
    name: String,
    path: Option<PathBuf>,
    doc: V,
    head: V,
    raw: Map,
    ids: BTreeSet<String>,
}
include!("ordinary_consolidation_engine.rs");
fn captured_projection<'a>(
    capture: &CapturedSource,
    runtime: Option<&'a Runtime>,
) -> Result<Projection<'a>> {
    let context = V::from_typed(&capture.ordinary_context());
    Projection::new(
        capture.ordinary_document(),
        map(capture.hypotheses())?,
        map(get(&context, "conflicts"))?,
        capture.reader_lines()?,
        runtime,
    )
    .map_err(|e| ordinary_error(capture.ordinary_document(), e))
}
fn read_hypotheses(
    capture: &CapturedSource,
    requested: &[String],
    runtime: Option<&Runtime>,
) -> Result<Vec<Hypothesis>> {
    let all = map(capture.hypotheses())?;
    let mut pool = BTreeMap::new();
    for (name, h) in all {
        if get(h, "kind") == &s("contribution") {
            continue;
        }
        require(
            !truth(get(h, "error")),
            &format!(
                "refused - hypothesis {name} could not be read: {}",
                py(get(h, "error"))
            ),
        )?;
        let doc = get(h, "doc");
        let merged = layer(capture.ordinary_document(), doc)?;
        Reader::new(&merged, runtime).map_err(|e| {
            error(&format!(
                "refused - hypothesis {name} cannot be read over the base: {}",
                ordinary_error(&merged, e)
                    .0
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(160)
                    .collect::<String>()
            ))
        })?;
        let path = PathBuf::from(text(get(h, "path"))?);
        let body = entries(doc)?;
        let ids = body.keys().cloned().collect();
        pool.insert(
            name.clone(),
            Hypothesis {
                name: name.clone(),
                path: Some(path),
                doc: doc.clone(),
                head: get(h, "head").clone(),
                raw: body,
                ids,
            },
        );
    }
    let missing = requested
        .iter()
        .filter(|n| !pool.contains_key(*n))
        .cloned()
        .collect::<Vec<_>>();
    require(
        missing.is_empty(),
        &format!(
            "refused - no hypothesis named {} beside the record (there: {})",
            missing.join(", "),
            if pool.is_empty() {
                "none".into()
            } else {
                pool.keys().cloned().collect::<Vec<_>>().join(", ")
            }
        ),
    )?;
    if requested.is_empty() {
        Ok(pool.into_values().collect())
    } else {
        Ok(requested
            .iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|n| pool.remove(n).unwrap())
            .collect())
    }
}

pub(super) fn preview(
    request: &super::OrdinaryPreviewRequest<'_>,
    evidence: &super::PreviewEvidence<'_>,
) -> Result<super::OrdinaryPreview> {
    let empty = Map::new();
    let context = request.context.map(map).transpose()?;
    let conflicts = context
        .and_then(|m| m.get("conflicts"))
        .map(map)
        .transpose()?
        .unwrap_or(&empty);
    let base = Projection::new(
        request.document,
        request.hypotheses,
        conflicts,
        vec![],
        request.runtime,
    )
    .map_err(|e| ordinary_error(request.document, e))?;
    let mut pool = BTreeMap::new();
    for (name, h) in request.hypotheses {
        if get(h, "kind") == &s("contribution") {
            continue;
        }
        require(
            !truth(get(h, "error")),
            &format!(
                "refused - hypothesis {name} could not be read: {}",
                py(get(h, "error"))
            ),
        )?;
        let doc = if get(h, "doc") != &V::Null {
            get(h, "doc")
        } else {
            get(h, "document")
        };
        let merged = layer(request.document, doc)?;
        Reader::new(&merged, request.runtime).map_err(|e| {
            error(&format!(
                "refused - hypothesis {name} cannot be read over the base: {}",
                ordinary_error(&merged, e)
                    .0
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(160)
                    .collect::<String>()
            ))
        })?;
        let raw = entries(doc)?;
        pool.insert(
            name.clone(),
            Hypothesis {
                name: name.clone(),
                path: text(get(h, "path"))
                    .ok()
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from),
                doc: doc.clone(),
                head: get(h, "head").clone(),
                ids: raw.keys().cloned().collect(),
                raw,
            },
        );
    }
    for proposal in request.proposals {
        require(
            !pool.contains_key(&proposal.name),
            &format!(
                "refused - {} names both an existing and a supplied hypothesis",
                proposal.name
            ),
        )?;
        let raw = entries(&proposal.document)?;
        pool.insert(
            proposal.name.clone(),
            Hypothesis {
                name: proposal.name.clone(),
                path: None,
                doc: proposal.document.clone(),
                head: proposal.head.clone(),
                ids: raw.keys().cloned().collect(),
                raw,
            },
        );
    }
    let proposals = pool.into_values().collect::<Vec<_>>();
    let brief = evidence
        .brief
        .and_then(|raw| crate::history_yaml::decode_full_ordinary_source_value(raw).ok())
        .map(|v| v.projected());
    let page = if let Some(raw) = evidence.brief {
        if needs_page(request.document, &base, &proposals, request.runtime)? {
            Some(
                page_evidence(
                    request.document,
                    request.hypotheses,
                    request.context,
                    raw,
                    request.runtime,
                )
                .map_err(|e| error(&format!("presentation contribution cannot be checked: {e}")))?,
            )
        } else {
            None
        }
    } else {
        None
    };
    let stamp = request
        .as_of
        .map(str::to_owned)
        .unwrap_or_else(crate::source_clock::latest_day);
    let c = union(UnionInput {
        doc: request.document,
        all: request.hypotheses,
        hyps: proposals,
        base,
        runtime: request.runtime,
        stamp: &stamp,
        take: &[],
        drops: &empty,
        brief: brief.as_ref(),
        page: page.as_ref(),
    })?;
    let report = report::lines(&c, chrono::Local::now().date_naive())?.join("\n") + "\n";
    Ok(super::OrdinaryPreview {
        report,
        exit_code: i32::from(c.red() || !c.drops_needed.is_empty()),
        blocked: c.blocked() || !c.drops_needed.is_empty(),
        candidate_document: c.view.as_ref().map(|_| c.doc.clone()),
        facts: c.facts(),
    })
}

pub(super) fn page_evidence(
    document: &V,
    hypotheses: &Map,
    context: Option<&V>,
    raw: &[u8],
    runtime: Option<&Runtime>,
) -> Result<V> {
    page::from_document(
        document,
        hypotheses,
        context.unwrap_or(&V::Map(Map::new())),
        vec![],
        raw,
        runtime,
    )
}

pub(super) fn run(
    options: &Options,
    route: &WriteRoute,
    runtime_override: Option<&Runtime>,
) -> Result<CommandOutput> {
    let entry = &route.paths()[0];
    let layout = crate::history_transaction::Layout::for_entry(
        entry
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| error("invalid_path"))?,
    )?;
    require(
        crate::history_transaction_fs::read(&entry.parent().unwrap().join(&layout.journal))?
            .is_none(),
        "recovery_required",
    )?;
    let loaded_runtime;
    let runtime = if runtime_override.is_some() {
        runtime_override
    } else {
        loaded_runtime = crate::public_workspace::runtime()?;
        loaded_runtime.as_ref()
    };
    let capture = crate::source_capture::capture_ordinary_source_with_runtime(
        route.paths(),
        &route.project().root,
        if options.frozen {
            ReadMode::Frozen
        } else {
            ReadMode::Live
        },
        None,
        runtime,
    )?;
    require(
        get(
            &F::capabilities(capture.ordinary_document(), None)?,
            "profile",
        ) != &s("core/v1"),
        "unsupported_capability: use core/v1 consumer",
    )?;
    let hyps = read_hypotheses(&capture, &options.names, runtime)?;
    // The run over every hypothesis also tests what the pending ledger would bring; a run
    // asked about named hypotheses is about those alone.
    let finds = if options.names.is_empty() {
        pending::findings(&capture)?
    } else {
        vec![]
    };
    if hyps.is_empty() && finds.is_empty() {
        capture.verify()?;
        route.verify()?;
        return Ok(CommandOutput {
            stdout: "no hypotheses beside the record - nothing to consolidate\n".into(),
            stderr: String::new(),
            code: 0,
        });
    }
    let stamp = options
        .as_of
        .clone()
        .unwrap_or_else(crate::source_clock::latest_day);
    let drops = V::from_typed(&super::drops(&options.drops)?);
    let mut inventory = Inventory::default();
    let view_path = entry.parent().unwrap().join(layout.view);
    let brief_raw = if inventory.exists(&view_path)? {
        Some(inventory.read(&view_path)?)
    } else {
        None
    };
    let brief = brief_raw
        .as_ref()
        .and_then(|raw| crate::history_yaml::decode_full_ordinary_source_value(raw).ok())
        .map(|v| v.projected())
        .unwrap_or(V::Null);
    let page_for = |proposals: &[Hypothesis]| -> Result<Option<V>> {
        let Some(raw) = &brief_raw else {
            return Ok(None);
        };
        let base = captured_projection(&capture, runtime)?;
        if !needs_page(capture.ordinary_document(), &base, proposals, runtime)? {
            return Ok(None);
        }
        page::facts(&capture, raw, runtime)
            .map(Some)
            .map_err(|e| error(&format!("presentation contribution cannot be checked: {e}")))
    };
    let (mut output, mut red, mut stray) = (String::new(), false, None);
    if hyps.is_empty() {
        output.push_str("no hypotheses beside the record - nothing to consolidate\n");
    } else {
        let page = page_for(&hyps)?;
        let c = union(UnionInput {
            doc: capture.ordinary_document(),
            all: map(capture.hypotheses())?,
            hyps,
            base: captured_projection(&capture, runtime)?,
            runtime,
            stamp: &stamp,
            take: &options.take,
            drops: map(&drops)?,
            brief: Some(&brief),
            page: page.as_ref(),
        })?;
        output = report::lines(&c, chrono::Local::now().date_naive())?.join("\n") + "\n";
        red = c.red() || !c.drops_needed.is_empty();
        stray = report::stray(&c, &options.take);
    }
    if !finds.is_empty() {
        let p = pending::test(
            &pending::Input {
                capture: &capture,
                stamp: &stamp,
                brief: Some(&brief),
                page_for: &page_for,
            },
            finds,
            runtime,
        )?;
        output.push('\n');
        output.push_str(&(pending::lines(&p)?.join("\n") + "\n"));
        red |= p.red();
    }
    capture.verify()?;
    inventory.verify()?;
    route.verify()?;
    if let Some(why) = &stray {
        output.push('\n');
        output.push_str(why);
        output.push('\n');
    }
    Ok(CommandOutput {
        stdout: output,
        stderr: String::new(),
        code: i32::from(stray.is_some() || red),
    })
}

//! Source-report publication on explicitly marked node-history records.
use super::*;
use crate::{
    history_authoring::{self as A, obj, s},
    history_contract::field,
    history_node_capture::Capture,
    history_node_publication as P, history_node_writer as W,
    history_view::map_mut,
    reasoning_runtime::Runtime,
    recording_privacy as Privacy, require,
};

pub(super) struct Context<'a> {
    pub route: &'a WriteRoute,
    pub original: &'a [PathBuf],
    pub record: &'a Path,
    pub state: &'a Path,
    pub source: &'a Path,
    pub envelope_sha: &'a str,
    pub expected_target: Option<&'a J>,
    pub supplied_runtime: Option<&'a Runtime>,
}
impl Context<'_> {
    fn root(&self) -> &Path {
        self.record.parent().unwrap()
    }
    fn journal(&self, event: &str) -> PathBuf {
        self.state.join("journals").join(format!("{event}.json"))
    }
    fn guard(&self) -> Result<V> {
        Ok(obj([
            ("kind", s("public-node-report/v1")),
            (
                "routing",
                crate::direct_history::routing(self.route, self.original)?,
            ),
        ]))
    }
    fn intent(&self, report: &Report, event: &str, target_hash: Option<&V>) -> Result<V> {
        let mut context = obj([
            ("kind", s("source-report-node/v1")),
            ("event_id", s(event)),
            (
                "source_sha256",
                s(&crate::identity::sha256(report.quote.as_bytes())),
            ),
            ("envelope_sha256", s(self.envelope_sha)),
            (
                "record",
                s(self.record.to_str().ok_or_else(|| error("invalid_path"))?),
            ),
            (
                "state_dir",
                s(self.state.to_str().ok_or_else(|| error("invalid_path"))?),
            ),
            ("policy_sha256", s(&self.route.config().digest()?)),
            (
                "routing_sha256",
                s(&crate::source_capture::routing_observation(
                    self.route.paths(),
                    &self.route.project().root,
                )?
                .digest()?),
            ),
        ]);
        if let Some(hash) = target_hash {
            map_mut(&mut context)?.insert("target_sha256".into(), hash.clone());
        }
        Ok(context)
    }
    fn sources(&self, report: &Report, event: &str) -> Result<()> {
        self.route.verify()?;
        require(
            F::read(self.source)?.as_deref() == Some(report.quote.as_bytes()),
            "report_source_changed",
        )?;
        let envelope = F::read(&self.state.join("envelopes").join(format!("{event}.json")))?
            .ok_or_else(|| error("report_envelope_missing"))?;
        require(
            crate::identity::sha256(&envelope) == self.envelope_sha,
            "report_envelope_changed",
        )
    }
}
fn portable(event: &str) -> String {
    format!("evidence/reports/{event}.txt")
}
/// Report admission needs only the request's caller context. Compact node
/// transactions retain that request directly; older transactions retain it in
/// their receipt-shaped authoring intent.
fn report_context(prepared: &P::Prepared, after: &Capture) -> Result<V> {
    if let Some(context) = prepared
        .context()?
        .as_ref()
        .filter(|context| crate::history_node_transaction::is_context(context))
    {
        let compact = crate::history_node_transaction::validate(context)?;
        let action = map(&compact["action"])?;
        require(
            action.get("kind").is_some_and(|kind| kind == &s("batch")),
            "report_actions_changed",
        )?;
        return Ok(field(action, "context")?.clone());
    }
    let receipt = W::receipt(&after.snapshot, prepared.operation())?;
    Ok(field(W::intent(&receipt)?, "context")?.clone())
}

/// The public report result keeps the semantic output needed by callers,
/// without reconstructing a document-sized historical receipt.
fn report_result(prepared: &P::Prepared, before: &Capture, after: &Capture) -> Result<V> {
    if let Some(context) = prepared
        .context()?
        .as_ref()
        .filter(|context| crate::history_node_transaction::is_context(context))
    {
        let compact = crate::history_node_transaction::validate(context)?;
        let objects = after
            .history
            .objects()
            .iter()
            .filter(|(id, _)| !before.history.objects().contains_key(*id))
            .map(|(id, object)| {
                after.object(crate::history_contract::text(&map(object)?["subject"])?, id)
            })
            .collect::<Result<Vec<_>>>()?;
        let diagnostics = map(&compact["result"])?
            .get("diagnostics")
            .cloned()
            .unwrap_or_else(|| V::List(Vec::new()));
        return Ok(obj([
            ("format", s("node-ledger-report/v1")),
            ("action", compact["action"].clone()),
            (
                "context",
                field(map(&compact["action"])?, "context")?.clone(),
            ),
            ("objects", V::List(objects)),
            ("result", compact["result"].clone()),
            (
                "after",
                obj([("batch", obj([("diagnostics", diagnostics)]))]),
            ),
        ]));
    }
    W::receipt(&after.snapshot, prepared.operation())
}
fn journal(prepared: &P::Prepared, graphs: &(J, J), phase: &str) -> Result<J> {
    Ok(
        json!({"version":1,"kind":"native-node-report/v1","phase":phase,
        "prepared":base64::engine::general_purpose::STANDARD.encode(prepared.to_bytes()?),
        "graph_before":graphs.0,"graph_after":graphs.1}),
    )
}
fn decode(value: &J) -> Result<P::Prepared> {
    require(
        value["version"] == 1
            && value["kind"] == "native-node-report/v1"
            && ["prepared", "committed"]
                .iter()
                .any(|phase| value["phase"] == *phase),
        "invalid report journal",
    )?;
    let encoded = value["prepared"]
        .as_str()
        .ok_or_else(|| error("invalid report journal"))?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| error("invalid report journal"))?;
    P::Prepared::from_bytes(&raw)
}
fn graphs(
    before: &Capture,
    after: &Capture,
    report: &Report,
    runtime: Option<&Runtime>,
) -> Result<(J, J)> {
    let seeds = report
        .updates
        .iter()
        .map(|v| v["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let graphs = (
        graph_document(before.entry_bytes(), before.document(), &seeds, runtime)?,
        graph_document(after.entry_bytes(), after.document(), &seeds, runtime)?,
    );
    if let Some(profile) = report.raw.get("profile").and_then(J::as_str) {
        require(
            graphs.1["assessment_profile"] == profile,
            "requested writer profile requires explicit record migration",
        )?;
    }
    Ok(graphs)
}
/// Rebuild exact report actions from the retained envelope before any publication or recovery.
fn verify(
    ctx: &Context<'_>,
    report: &Report,
    event: &str,
    prepared: &P::Prepared,
    runtime: Option<&Runtime>,
) -> Result<(J, J, V)> {
    crate::public_node_history::scope(ctx.route)?;
    require(
        prepared.guard()?.as_ref() == Some(&ctx.guard()?),
        "node_history_route_changed",
    )?;
    ctx.sources(report, event)?;
    require(
        private_reason(report)?.is_none(),
        "private or unclear source permission",
    )?;
    let (before, after, action, objects) =
        crate::public_node_history::candidate(ctx.root(), prepared)?;
    for document in [before.document(), after.document(), &objects] {
        require(
            !Privacy::private_marker(document),
            "private or unclear prepared source permission",
        )?;
    }
    let context = report_context(prepared, &after)?;
    let target_hash = map(&context)?.get("target_sha256");
    require(
        context == ctx.intent(report, event, target_hash)?,
        "report_preparation_stale: report policy or routing changed",
    )?;
    if let Some(expected) = ctx.expected_target {
        require(
            target_hash == Some(&V::from_json(&expected["body_sha256"])?),
            "report_target_changed",
        )?;
    }
    let target = target_hash
        .map(|hash| hash.to_json().map(|hash| json!({"body_sha256":hash})))
        .transpose()?;
    verify_captured_target(
        before.document(),
        before.entry_bytes(),
        report,
        target.as_ref(),
    )?;
    let collection = source_collection(before.document(), report)?;
    let (planned, _) = actions(
        report,
        event,
        Path::new(&portable(event)),
        &collection,
        true,
    )?;
    require(
        field(map(&action)?, "actions")? == &V::List(planned),
        "report_actions_changed",
    )?;
    require(
        prepared.operation() == format!("report-{event}")
            && prepared.evidence()?
                == BTreeMap::from([(portable(event), report.quote.as_bytes().to_vec())]),
        "report_evidence_changed",
    )?;
    W::verify(ctx.root(), prepared, runtime)?;
    ctx.sources(report, event)?;
    let (before_graph, after_graph) = graphs(&before, &after, report, runtime)?;
    Ok((
        before_graph,
        after_graph,
        report_result(prepared, &before, &after)?,
    ))
}
fn retained_matches(
    ctx: &Context<'_>,
    event: &str,
    prepared: &P::Prepared,
    graphs: &(J, J),
) -> Result<()> {
    let raw = F::read(&ctx.journal(event))?.ok_or_else(|| error("report_journal_missing"))?;
    let value: J =
        crate::json_ingress::parse_slice(&raw, crate::json_ingress::DuplicateKeys::Reject)?;
    require(
        decode(&value)?.to_bytes()? == prepared.to_bytes()?
            && value["graph_before"] == graphs.0
            && value["graph_after"] == graphs.1,
        "history report recovery journal mismatch",
    )
}
fn retained_sources(ctx: &Context<'_>, prepared: &P::Prepared) -> Result<()> {
    for raw in [
        prepared.before_view()?.unwrap_or_default(),
        prepared.after_view()?,
    ] {
        let document = crate::history_yaml::decode_document(&raw)?;
        crate::history_sources::capture(ctx.root(), "GROUNDING.yaml", &document)?;
    }
    Ok(())
}
fn publish(
    ctx: &Context<'_>,
    report: &Report,
    event: &str,
    prepared: &P::Prepared,
    graphs: &(J, J),
    runtime: Option<&Runtime>,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<()> {
    P::publish(
        ctx.root(),
        prepared,
        |p| {
            retained_matches(ctx, event, p, graphs)?;
            let (before, after, _) = verify(ctx, report, event, p, runtime)?;
            require((before, after) == *graphs, "report_graph_changed")
        },
        |phase| {
            ctx.sources(report, event)?;
            retained_sources(ctx, prepared)?;
            retained_matches(ctx, event, prepared, graphs)?;
            if phase == P::Phase::Commit {
                save(
                    &ctx.journal(event),
                    &journal(prepared, graphs, "committed")?,
                )?;
            }
            probe(match phase {
                P::Phase::Journal => "journal",
                P::Phase::Append(_) => "append",
                P::Phase::Evidence(_) => "evidence",
                P::Phase::Import(_) => "import",
                P::Phase::Commit => "committed",
                P::Phase::View => "view",
            })?;
            ctx.sources(report, event)?;
            retained_sources(ctx, prepared)?;
            retained_matches(ctx, event, prepared, graphs)
        },
    )
}
/// Generic history recovery uses the same report verifier as resubmission. Private
/// ingestion state remains required; the permanent manifest cannot confer public admission.
pub(crate) fn verify_recovery(
    route: &WriteRoute,
    original: &[PathBuf],
    prepared: &P::Prepared,
    runtime_override: Option<&Runtime>,
) -> Result<()> {
    let record = &route.paths()[0];
    let root = record.parent().unwrap();
    let (before, after) = prepared.snapshots(root)?;
    let before = Capture::from_snapshot(before)?;
    let after = Capture::from_snapshot(after)?;
    let retained_context = report_context(prepared, &after)?;
    let context = map(&retained_context)?;
    // Preserve the original lexical owner path while accepting another route spelling
    // for that same file (for example macOS /var versus /private/var).
    let recorded_record = PathBuf::from(crate::history_contract::text(field(context, "record")?)?);
    require(
        recorded_record.is_absolute()
            && crate::project_modes::resolved(&recorded_record)?
                == crate::project_modes::resolved(record)?,
        "ingestion_state_owner_mismatch",
    )?;
    let record = &recorded_record;
    let state = PathBuf::from(crate::history_contract::text(field(context, "state_dir")?)?);
    let event = crate::history_contract::text(field(context, "event_id")?)?;
    require(state.is_absolute(), "invalid report state directory")?;
    let owner =
        F::read(&state.join("record.json"))?.ok_or_else(|| error("report_state_owner_missing"))?;
    require(
        crate::json_ingress::parse_slice(&owner, crate::json_ingress::DuplicateKeys::Reject)?
            == json!({"record":record}),
        "ingestion_state_owner_mismatch",
    )?;
    let raw = F::read(&state.join("envelopes").join(format!("{event}.json")))?
        .ok_or_else(|| error("report_envelope_missing"))?;
    let report = parse(&raw)?;
    let envelope_sha = crate::identity::sha256(&raw);
    let source = state.join("sources").join(format!("{event}.txt"));
    let owned_runtime = if runtime_override.is_none() {
        crate::public_workspace::runtime_for_document(before.document())?
    } else {
        None
    };
    let runtime = runtime_override.or(owned_runtime.as_ref());
    let ctx = Context {
        route,
        original,
        record,
        state: &state,
        source: &source,
        envelope_sha: &envelope_sha,
        expected_target: None,
        supplied_runtime: runtime,
    };
    let (before_graph, after_graph, _) = verify(&ctx, &report, event, prepared, runtime)?;
    retained_matches(&ctx, event, prepared, &(before_graph, after_graph))
}
pub(super) fn run(
    report: &Report,
    event: &str,
    ctx: Context<'_>,
    retained: Option<&[u8]>,
    probe: &mut dyn FnMut(&str) -> Result<()>,
) -> Result<Output> {
    let outcome: Result<(P::Prepared, (J, J), V)> = (|| {
        crate::public_node_history::scope(ctx.route)?;
        require(
            private_reason(report)?.is_none(),
            "private or unclear source permission",
        )?;
        let prepared;
        let expected_graphs;
        let owned_runtime;
        let runtime;
        if let Some(raw) = retained {
            let retained: J =
                crate::json_ingress::parse_slice(raw, crate::json_ingress::DuplicateKeys::Reject)?;
            prepared = decode(&retained)?;
            let (before, _) = prepared.snapshots(ctx.root())?;
            let before = Capture::from_snapshot(before)?;
            owned_runtime = if ctx.supplied_runtime.is_none() {
                crate::public_workspace::runtime_for_document(before.document())?
            } else {
                None
            };
            runtime = ctx.supplied_runtime.or(owned_runtime.as_ref());
            let (before_graph, after_graph, _) = verify(&ctx, report, event, &prepared, runtime)?;
            require(
                retained["graph_before"] == before_graph && retained["graph_after"] == after_graph,
                "report_graph_changed",
            )?;
            expected_graphs = (before_graph, after_graph);
            if F::read(&ctx.root().join(".kpopper/.history-node-publication.json"))?.is_some() {
                P::recover(ctx.root(), |pending| {
                    require(
                        pending.to_bytes()? == prepared.to_bytes()?,
                        "history report recovery journal mismatch",
                    )?;
                    verify(&ctx, report, event, pending, runtime).map(|_| ())
                })?;
            }
        } else {
            let before = Capture::read(ctx.root())?;
            verify_captured_target(
                before.document(),
                before.entry_bytes(),
                report,
                ctx.expected_target,
            )?;
            require(
                !Privacy::private_marker(before.document()),
                "private or unclear source permission in history report",
            )?;
            let collection = source_collection(before.document(), report)?;
            let (planned, _) = actions(
                report,
                event,
                Path::new(&portable(event)),
                &collection,
                true,
            )?;
            owned_runtime = if ctx.supplied_runtime.is_none() {
                crate::public_workspace::runtime_for_document(before.document())?
            } else {
                None
            };
            runtime = ctx.supplied_runtime.or(owned_runtime.as_ref());
            let target = ctx
                .expected_target
                .map(|v| V::from_json(&v["body_sha256"]))
                .transpose()?;
            let batch = crate::history_authoring_batch::BatchOptions {
                authoring: A::Options {
                    operation: format!("report-{event}"),
                    recorded_at: chrono::Utc::now().to_rfc3339(),
                    recording_day: report.date.clone(),
                    by: V::Null,
                    strict: true,
                    paths: crate::history_paths::Scheme::Hashed,
                    receipt_version: None,
                },
                receipt_version: 8,
                context: ctx.intent(report, event, target.as_ref())?,
                evidence: BTreeMap::from([(portable(event), report.quote.as_bytes().to_vec())]),
            };
            ctx.sources(report, event)?;
            prepared = W::prepare_batch(ctx.root(), &planned, &batch, runtime)?
                .with_guard(&ctx.guard()?)?;
            let (before_graph, after_graph, _) = verify(&ctx, report, event, &prepared, runtime)?;
            expected_graphs = (before_graph, after_graph);
            save(
                &ctx.journal(event),
                &journal(&prepared, &expected_graphs, "prepared")?,
            )?;
            probe("prepared")?;
        }
        publish(
            &ctx,
            report,
            event,
            &prepared,
            &expected_graphs,
            runtime,
            probe,
        )?;
        if retained.is_some() {
            probe("recovered")?;
        }
        crate::session_activity::published(
            ctx.root(),
            &[crate::history_transaction::FileImage {
                path: "GROUNDING.yaml".into(),
                role: "record".into(),
                before: prepared.before_view()?,
                after: Some(prepared.after_view()?),
            }],
            Some(
                &report
                    .updates
                    .iter()
                    .map(|v| v["id"].as_str().unwrap().to_owned())
                    .collect(),
            ),
        );
        let (before, after) = prepared.snapshots(ctx.root())?;
        let result = report_result(
            &prepared,
            &Capture::from_snapshot(before)?,
            &Capture::from_snapshot(after)?,
        )?;
        Ok((prepared, expected_graphs, result))
    })();
    let (answer, signals, code) = match outcome {
        Ok((prepared, graphs, semantic)) => {
            let data = json!({"operation":prepared.operation(),"digest":prepared.digest(),"receipt":semantic.to_json()?});
            let (answer, signals) = receipt_parts(
                report,
                ctx.record,
                ctx.state,
                event,
                ctx.source,
                ctx.envelope_sha,
                "applied",
                None,
                retained.is_some(),
                Some(&data),
                Some(&prepared.after_view()?),
                Some(&graphs),
            )?;
            (answer, signals, 0)
        }
        Err(e) => {
            let stale = retained.is_some()
                && [
                    "node_publication_stale_frontier",
                    "node_publication_stale_view",
                    "node_history_route_changed",
                    "node_history_pending_unsupported",
                    "report_preparation_stale:",
                ]
                .iter()
                .any(|prefix| e.0.starts_with(prefix))
                && F::read(&ctx.root().join(".kpopper/.history-node-publication.json"))?.is_none()
                && F::read(
                    &ctx.root()
                        .join(format!(".kpopper/history-commits/report-{event}.json")),
                )?
                .is_none();
            if ctx.journal(event).is_file() && !stale {
                return Err(e);
            }
            let (answer, signals) = receipt_parts(
                report,
                ctx.record,
                ctx.state,
                event,
                ctx.source,
                ctx.envelope_sha,
                "needs_primary",
                Some(&e.0),
                retained.is_some(),
                None,
                None,
                None,
            )?;
            (answer, signals, 1)
        }
    };
    let _state_lock = F::DirectoryGuard::acquire(ctx.state, true)?;
    let path = ctx.state.join("receipts").join(format!("{event}.json"));
    if let Some(raw) = F::read(&path)? {
        let value: J =
            crate::json_ingress::parse_slice(&raw, crate::json_ingress::DuplicateKeys::Reject)?;
        return Ok(Output {
            text: String::from_utf8(raw).map_err(|_| error("invalid receipt"))?,
            code: receipt_code(&value),
        });
    }
    for signal in &signals {
        save(
            &ctx.state
                .join("signals")
                .join(format!("{}.json", signal["id"].as_str().unwrap())),
            signal,
        )?;
    }
    save(&path, &answer)?;
    save(
        &ctx.state.join("results").join(format!("{event}.json")),
        &json!({"receipt":answer,"signals":signals}),
    )?;
    Ok(Output {
        text: format!("{}\n", serde_json::to_string(&answer)?),
        code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Runtime) {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join(".kpopper")).unwrap();
        fs::write(root.path().join(".kpopper/history.yaml"), "version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: fixture\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
        fs::write(root.path().join("GROUNDING.yaml"), "meta:\n  purpose: Fixture\nschema:\n  deps: rests_on\n  snapshot: seen\n  predicate: wrong_if\nknown: {}\n").unwrap();
        let runtime = A::tests::runtime(&root.path().join("runtime"))
            .with_ordinary_program(crate::ordinary_reader::tests::program());
        for (index, action) in [
            json!({"kind":"add","id":"s.old","into":"known","body":{"url":"https://example.test","read":"2026-09-01"}}),
            json!({"kind":"add","id":"p.x","into":"known","body":{"v":1,"from":"s.old","at":"table"}}),
            json!({"kind":"add","id":"d.x","into":"known","body":{"rests_on":["p.x"],"verdict":"ok","wrong_if":"p.x > 1"}}),
        ].iter().enumerate() {
            let p = W::prepare(root.path(), &V::from_json(action).unwrap(), &A::Options {
                operation:format!("seed-{index}"), recorded_at:"2026-09-01T00:00:00Z".into(), recording_day:"2026-09-01".into(), by:V::Null, strict:true, paths:crate::history_paths::Scheme::Hashed, receipt_version:None,
            }, Some(&runtime)).unwrap();
            W::publish(root.path(), &p, Some(&runtime), |_| Ok(())).unwrap();
        }
        (root, runtime)
    }
    fn options(root: &Path) -> Options {
        Options {
            file: "-".into(),
            record: None,
            state_dir: Some(root.join("reports")),
        }
    }
    fn report() -> J {
        json!({"event_id":"node-report","date":"2026-09-24","source_quote":"exact quote: x is 2\r\n","updates":[{"kind":"set","id":"p.x","value":2}]})
    }
    #[test]
    fn node_report_applies_with_semantic_result_signals_and_portable_evidence() {
        let (root, runtime) = fixture();
        let report = report();
        let bytes = serde_json::to_vec(&report).unwrap();
        let output = run_with_probe(
            &options(root.path()),
            root.path(),
            Some(&bytes),
            Some(&runtime),
            &mut |_| Ok(()),
        )
        .unwrap();
        assert_eq!(output.code, 0, "{}", output.text);
        let receipt: J = serde_json::from_str(&output.text).unwrap();
        assert_eq!(receipt["newly_fired_judgments"], json!(["d.x"]));
        assert_eq!(receipt["state"], "applied");
        assert!(receipt["target_after_sha256"].as_str().is_some());
        let event = receipt["event_id"].as_str().unwrap();
        let portable = portable(event);
        assert_eq!(
            fs::read(root.path().join(&portable)).unwrap(),
            report["source_quote"].as_str().unwrap().as_bytes()
        );
        let semantic = &receipt["mutation"]["receipt"];
        assert_eq!(semantic["format"], "node-ledger-report/v1");
        assert_eq!(semantic["context"]["event_id"], event);
        assert_eq!(semantic["action"]["kind"], "batch");
        assert!(
            semantic["action"]["actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|action| action["id"] == "p.x")
        );
        let ids = semantic["objects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|object| object["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert!(!ids.is_empty());
        assert_eq!(
            receipt["diagnostics"],
            semantic["after"]["batch"]["diagnostics"]
        );
        let copy = P::export(root.path()).unwrap().reconstruct().unwrap();
        let copied = Capture::read(copy.path()).unwrap();
        for id in ids {
            assert!(copied.history.objects().contains_key(&id));
        }
        let again = run_with_probe(
            &options(root.path()),
            root.path(),
            Some(&bytes),
            Some(&runtime),
            &mut |_| panic!("retry wrote"),
        )
        .unwrap();
        assert_eq!(serde_json::from_str::<J>(&again.text).unwrap(), receipt);
        let manifest = fs::read_to_string(
            root.path()
                .join(format!(".kpopper/history-commits/report-{event}.json")),
        )
        .unwrap();
        for absent in ["graph_before", "exact quote", "source_quote"] {
            assert!(!manifest.contains(absent));
        }
    }
    #[test]
    fn node_report_recovers_each_publication_boundary() {
        for phase in [
            "prepared",
            "journal",
            "append",
            "evidence",
            "committed",
            "view",
        ] {
            let (root, runtime) = fixture();
            let bytes = serde_json::to_vec(&report()).unwrap();
            let error = run_with_probe(
                &options(root.path()),
                root.path(),
                Some(&bytes),
                Some(&runtime),
                &mut |at| {
                    if at == phase {
                        Err(error("crash"))
                    } else {
                        Ok(())
                    }
                },
            )
            .unwrap_err();
            assert!(error.0.contains("crash"), "{phase}: {}", error.0);
            if ["journal", "committed"].contains(&phase) {
                let result = crate::direct_history::recover_with_runtime(
                    &[root.path().join("GROUNDING.yaml")],
                    root.path(),
                    false,
                    Some(&runtime),
                )
                .unwrap();
                assert_eq!(
                    map(&result).unwrap()["state"],
                    s(if phase == "committed" {
                        "committed"
                    } else {
                        "rolled_back"
                    })
                );
            }
            let output = run_with_probe(
                &options(root.path()),
                root.path(),
                Some(&bytes),
                Some(&runtime),
                &mut |_| Ok(()),
            )
            .unwrap();
            assert_eq!(output.code, 0, "{phase}: {}", output.text);
            let receipt: J = serde_json::from_str(&output.text).unwrap();
            assert_eq!(receipt["recovered"], true);
            assert_eq!(receipt["newly_fired_judgments"], json!(["d.x"]));
            assert_eq!(
                Capture::read(root.path())
                    .unwrap()
                    .document()
                    .to_json()
                    .unwrap()["known"]["p.x"]["v"],
                2
            );
        }
    }
    #[test]
    fn node_report_rechecks_source_and_prepared_graph_before_recovery() {
        for change in ["source", "graph", "envelope"] {
            let (root, runtime) = fixture();
            let bytes = serde_json::to_vec(&report()).unwrap();
            run_with_probe(
                &options(root.path()),
                root.path(),
                Some(&bytes),
                Some(&runtime),
                &mut |at| {
                    if at == "prepared" {
                        Err(error("crash"))
                    } else {
                        Ok(())
                    }
                },
            )
            .unwrap_err();
            let state = root.path().join("reports");
            let journal_path = fs::read_dir(state.join("journals"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let mut journal: J = serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
            if change == "graph" {
                journal["graph_after"]["hash"] = json!("tampered");
                save(&journal_path, &journal).unwrap();
            } else {
                let dir = if change == "source" {
                    "sources"
                } else {
                    "envelopes"
                };
                let path = fs::read_dir(state.join(dir))
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                fs::write(path, b"changed").unwrap();
            }
            let before = fs::read(root.path().join("GROUNDING.yaml")).unwrap();
            assert!(
                run_with_probe(
                    &options(root.path()),
                    root.path(),
                    Some(&bytes),
                    Some(&runtime),
                    &mut |_| Ok(())
                )
                .is_err()
            );
            assert_eq!(
                fs::read(root.path().join("GROUNDING.yaml")).unwrap(),
                before
            );
            assert!(journal_path.exists());
        }
    }
    #[test]
    fn node_report_target_profile_privacy_and_stale_preparation_guards() {
        let (root, runtime) = fixture();
        let entry = root.path().join("GROUNDING.yaml");
        let before = fs::read(&entry).unwrap();
        for (tag, extra) in [
            ("profile", json!({"profile":"core/v1"})),
            ("hash", json!({"record_sha256":"0".repeat(64)})),
            ("private", json!({"shareability":"private"})),
        ] {
            let mut value = report();
            value["event_id"] = json!(tag);
            value
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let output = run_with_probe(
                &options(root.path()),
                root.path(),
                Some(&serde_json::to_vec(&value).unwrap()),
                Some(&runtime),
                &mut |_| Ok(()),
            )
            .unwrap();
            assert_eq!(output.code, 1, "{tag}: {}", output.text);
            assert_eq!(fs::read(&entry).unwrap(), before);
        }
        let mut value = report();
        value["event_id"] = json!("captured");
        let target = capture_target(&value, &entry, root.path()).unwrap();
        let output = run_bound(
            &options(root.path()),
            root.path(),
            Some(&serde_json::to_vec(&value).unwrap()),
            None,
            Some(&json!({"body_sha256":"0".repeat(64)})),
            Some(&runtime),
            &mut |_| Ok(()),
        )
        .unwrap();
        assert_eq!(output.code, 1);
        assert!(
            serde_json::from_str::<J>(&output.text).unwrap()["reason"]
                .as_str()
                .unwrap()
                .contains("target changed")
        );
        value["event_id"] = json!("stale");
        let bytes = serde_json::to_vec(&value).unwrap();
        run_bound(
            &options(root.path()),
            root.path(),
            Some(&bytes),
            None,
            Some(&target),
            Some(&runtime),
            &mut |at| {
                if at == "prepared" {
                    Err(error("crash"))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        let p = W::prepare(
            root.path(),
            &V::from_json(&json!({"kind":"set","id":"p.x","value":3})).unwrap(),
            &A::Options {
                operation: "intervening".into(),
                recorded_at: "2026-09-24T12:00:00Z".into(),
                recording_day: "2026-09-24".into(),
                by: V::Null,
                strict: true,
                paths: crate::history_paths::Scheme::Hashed,
                receipt_version: None,
            },
            Some(&runtime),
        )
        .unwrap();
        W::publish(root.path(), &p, Some(&runtime), |_| Ok(())).unwrap();
        let changed = fs::read(&entry).unwrap();
        let output = run_bound(
            &options(root.path()),
            root.path(),
            Some(&bytes),
            None,
            Some(&target),
            Some(&runtime),
            &mut |_| Ok(()),
        )
        .unwrap();
        assert_eq!(output.code, 1);
        assert_eq!(
            serde_json::from_str::<J>(&output.text).unwrap()["state"],
            "needs_primary"
        );
        assert_eq!(fs::read(&entry).unwrap(), changed);
    }
}

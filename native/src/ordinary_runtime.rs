//! Explicit, verified access to the ordinary reader's existing Lean program.
//! This JSON protocol is distinct from the core/v1 KP2/KP3/KP4 protocols.
use crate::{
    Error, Result,
    identity::sha256,
    reasoning_runtime::{OperationalBounds, run_command_bounded_with_status},
    require,
};
use serde_json::Value as J;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// Python's expressions.wire_payload lowers only executable positions. Source
/// bodies in the session graph, revision identity and view evidence stay intact.
fn session_payload(mut request: J, bounds: &OperationalBounds) -> Result<Vec<u8>> {
    require(
        serde_json::to_vec(&request)?.len() <= bounds.input_bytes,
        "ordinary_input_limit",
    )?;
    fn expression(value: &mut J, predicate: bool) {
        if value
            .as_object()
            .is_some_and(|object| object.contains_key("expr"))
            && let Ok(typed) = crate::value::TypedValue::from_json(value)
            && let Ok(tree) =
                crate::reasoning_language::legacy_expression_detailed(&typed, predicate)
            && let Ok(tree) = tree.to_json()
        {
            *value = tree;
        }
    }
    fn body(value: Option<&mut J>, predicates: &[&str], snapshots: &[&str]) {
        let Some(value) = value.and_then(J::as_object_mut) else {
            return;
        };
        if let Some(rule) = value.get_mut("rule") {
            expression(rule, false);
        }
        for field in predicates {
            if let Some(predicate) = value.get_mut(*field) {
                expression(predicate, true);
            }
        }
        for field in snapshots {
            if let Some(seen) = value.get_mut(*field).and_then(J::as_object_mut) {
                for snapshot in seen.values_mut() {
                    if let Some(rule) = snapshot
                        .get_mut("computed")
                        .and_then(J::as_object_mut)
                        .and_then(|computed| computed.get_mut("rule"))
                    {
                        expression(rule, false);
                    }
                }
            }
        }
    }
    if let Some(nodes) = request
        .get_mut("record")
        .and_then(|record| record.get_mut("nodes"))
        .and_then(J::as_object_mut)
    {
        for node in nodes.values_mut() {
            let predicate = node["assessment_fields"]["predicate"]
                .as_str()
                .unwrap_or("wrong_if")
                .to_owned();
            let snapshot = node["assessment_fields"]["snapshot"]
                .as_str()
                .unwrap_or("seen")
                .to_owned();
            body(node.get_mut("body"), &[&predicate], &[&snapshot]);
            body(
                node.get_mut("assessment_body"),
                &[&predicate, "wrong_if"],
                &[&snapshot, "seen"],
            );
        }
    }
    if let Some(predicate) = request.get_mut("predicate") {
        expression(predicate, true);
    }
    let payload = serde_json::to_vec(&request)?;
    require(payload.len() <= bounds.input_bytes, "ordinary_input_limit")?;
    Ok(payload)
}

#[derive(Debug)]
pub struct Program {
    root: PathBuf,
    binary: PathBuf,
    manifest: Vec<u8>,
    binary_sha256: String,
    bounds: OperationalBounds,
}
impl Program {
    /// Describe the selected ordinary program without executing it. Only its
    /// consumed build manifest and binary are covered, not system libraries.
    pub(crate) fn inspect(root: &Path, directory: &str) -> Result<J> {
        let manifest_name = format!("{directory}/build.json");
        let raw = crate::reasoning_runtime::resource_read(root, &manifest_name, 16384)?;
        let manifest =
            crate::json_ingress::parse_slice(&raw, crate::json_ingress::DuplicateKeys::Reject)?;
        require(
            manifest.is_object()
                && manifest["source_sha256"] == env!("KPOP_ORDINARY_SOURCE_SHA256"),
            "ordinary_source_changed",
        )?;
        let name = if cfg!(windows) {
            "epistemic-core.exe"
        } else {
            "epistemic-core"
        };
        let hash = crate::reasoning_runtime::resource_hash(
            root,
            &format!("{directory}/{name}"),
            128 * 1024 * 1024,
        )?;
        require(
            manifest["binary_sha256"] == hash,
            "ordinary_program_changed",
        )?;
        Ok(
            serde_json::json!({"status":"files_validated", "directory":directory, "manifest":manifest,
            "files":{"build.json":sha256(&raw),name:hash}}),
        )
    }
    fn valid_bundle(bundle: &J, judgment: &str) -> bool {
        bundle.is_object()
            && bundle["id"] == judgment
            && bundle["recorded_claim"].is_string()
            && bundle["premises"].is_array()
            && bundle["falsifier"].is_object()
            && (bundle["falsifier"]["holds_on_current_values"].is_boolean()
                || bundle["falsifier"]["holds_on_current_values"].is_null())
            && (bundle["mechanical_review_trigger"].is_boolean()
                || bundle["mechanical_review_trigger"].is_null())
            && bundle["human_reopener"].is_object()
            && bundle["declared_gap"].is_object()
            && bundle["scope"].is_string()
    }

    pub fn provenance(&self) -> Result<J> {
        let manifest = crate::json_ingress::parse_slice(
            &self.manifest,
            crate::json_ingress::DuplicateKeys::LastWins,
        )?;
        Ok(serde_json::json!({
            "profile":"checked-reader/v1",
            "source_sha256":manifest["source_sha256"],
            "binary_sha256":self.binary_sha256,
        }))
    }
    /// Open a caller-selected build of the bundled ordinary Lean source. No
    /// program discovery, build, installation or fallback happens during reads.
    pub fn open(root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        let manifest = fs::read(root.join("build.json"))?;
        require(manifest.len() <= 16384, "ordinary_manifest_limit")?;
        let data = crate::json_ingress::parse_slice(
            &manifest,
            crate::json_ingress::DuplicateKeys::LastWins,
        )?;
        require(
            data["source_sha256"] == env!("KPOP_ORDINARY_SOURCE_SHA256"),
            "ordinary_source_changed",
        )?;
        let binary_sha256 = data["binary_sha256"]
            .as_str()
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| Error("invalid_ordinary_manifest".into()))?
            .to_owned();
        let binary = root.join(if cfg!(windows) {
            "epistemic-core.exe"
        } else {
            "epistemic-core"
        });
        let program = Self {
            root,
            binary,
            manifest,
            binary_sha256,
            bounds: OperationalBounds::default(),
        };
        program.verify()?;
        Ok(program)
    }
    fn verify(&self) -> Result<()> {
        require(
            fs::read(self.root.join("build.json"))? == self.manifest
                && sha256(&fs::read(&self.binary)?) == self.binary_sha256,
            "ordinary_program_changed",
        )
    }
    pub(crate) fn limit(&mut self, bounds: &OperationalBounds) {
        self.bounds = bounds.clone();
    }
    fn effective_bounds(&self, bounds: &OperationalBounds) -> Result<OperationalBounds> {
        bounds.validate()?;
        Ok(OperationalBounds {
            timeout: bounds.timeout.min(self.bounds.timeout),
            batch_requests: bounds.batch_requests.min(self.bounds.batch_requests),
            input_bytes: bounds.input_bytes.min(self.bounds.input_bytes),
            output_bytes: bounds.output_bytes.min(self.bounds.output_bytes),
        })
    }
    /// Ordinary session operations retain the Python subprocess contract of
    /// twenty seconds. Compute requests retain their configured runtime bound.
    fn session_bounds(&self, bounds: &OperationalBounds) -> Result<OperationalBounds> {
        let mut bounds = self.effective_bounds(bounds)?;
        bounds.timeout = bounds.timeout.min(Duration::from_secs(20));
        Ok(bounds)
    }
    pub fn request(&self, request: &J, bounds: &OperationalBounds) -> Result<J> {
        let bounds = self.effective_bounds(bounds)?;
        let payload = serde_json::to_vec(request)?;
        require(payload.len() <= bounds.input_bytes, "ordinary_input_limit")?;
        self.verify()?;
        let (_, result) = self.execute(payload, &bounds, &[0])?;
        require(
            request["operation"] == "compute"
                && result["values"].as_object().is_some_and(|values| {
                    values.values().all(|value| {
                        value.as_object().is_some_and(|m| {
                            m.contains_key("value")
                                && m.get("role").is_some_and(J::is_string)
                                && m.get("reason").is_some_and(J::is_string)
                        })
                    })
                })
                && result["predicate"].as_object().is_some_and(|m| {
                    m.get("holds_on_current_values")
                        .is_some_and(|v| v.is_null() || v.is_boolean())
                        && m.get("reason").is_some_and(J::is_string)
                }),
            "invalid_ordinary_response",
        )?;
        Ok(result)
    }

    fn execute(
        &self,
        payload: Vec<u8>,
        bounds: &OperationalBounds,
        accepted: &[i32],
    ) -> Result<(i32, J)> {
        self.verify()?;
        let mut command = Command::new(&self.binary);
        let (status, output) = run_command_bounded_with_status(
            &mut command,
            payload,
            bounds.timeout,
            bounds.output_bytes,
        )?;
        self.verify()?;
        let code = status
            .code()
            .ok_or_else(|| Error("native reasoning process failed".into()))?;
        require(accepted.contains(&code), "native reasoning process failed")?;
        let result = crate::json_ingress::parse_slice(
            &output,
            crate::json_ingress::DuplicateKeys::LastWins,
        )?;
        Ok((code, result))
    }

    /// Evaluate the complete ordinary checked-reader graph once.
    pub fn scan(&self, record: &J, bounds: &OperationalBounds) -> Result<J> {
        let bounds = self.session_bounds(bounds)?;
        let request = serde_json::json!({"operation":"scan","record":record});
        let payload = session_payload(request, &bounds)?;
        let (_, result) = self.execute(payload, &bounds, &[0])?;
        require(
            result["counts"].is_object()
                && result["events"].is_object()
                && result["conditions"].is_object()
                && result["assessments"].is_array()
                && result["errors"].is_array()
                && result["assessments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|bundle| {
                        bundle["id"]
                            .as_str()
                            .is_some_and(|id| Self::valid_bundle(bundle, id))
                    }),
            "invalid_ordinary_scan_response",
        )?;
        Ok(result)
    }

    /// Check the caller's exact assertion list against one ordinary judgment.
    /// Exit 2 is accepted only for a complete, explicitly rejected result.
    pub fn assess(
        &self,
        record: &J,
        judgment: &str,
        assertions: &[J],
        bounds: &OperationalBounds,
    ) -> Result<J> {
        let bounds = self.session_bounds(bounds)?;
        let request = serde_json::json!({"record":record,"id":judgment,"assertions":assertions});
        let payload = session_payload(request, &bounds)?;
        let (code, result) = self.execute(payload, &bounds, &[0, 2])?;
        require(
            result["assertion_checks"].is_array()
                && result["assertions_requested"].as_u64() == Some(assertions.len() as u64)
                && result["assertions_accepted"].is_boolean()
                && Self::valid_bundle(&result["bundle"], judgment),
            "invalid_ordinary_assessment_response",
        )?;
        let accepted = result["assertions_accepted"] == true;
        require(
            result["assertion_checks"]
                .as_array()
                .is_some_and(|checks| checks.len() == assertions.len())
                && result["assertion_checks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|check| {
                        check.is_object()
                            && check["accepted"].is_boolean()
                            && check["kind"].is_string()
                            && check["reason"].is_string()
                    })
                && accepted
                    == result["assertion_checks"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|check| check["accepted"] == true),
            "invalid_ordinary_assessment_response",
        )?;
        require(
            (code == 0 && accepted) || (code == 2 && !accepted),
            "invalid_ordinary_assessment_status",
        )?;
        Ok(result)
    }

    /// Verify a complete navigation projection with the same checked program
    /// that computed the ordinary assessment. No caller-authored acceptance is
    /// trusted, and record identity is re-derived before invoking Lean.
    pub fn guard_view(&self, record: &J, view: &J, bounds: &OperationalBounds) -> Result<J> {
        let project = record["project_context"].as_str().unwrap_or("");
        let snapshot = sha256(&serde_json::to_vec(record)?);
        let revision = sha256(&serde_json::to_vec(
            &serde_json::json!({"project":project,"graph":snapshot}),
        )?);
        require(
            !project.is_empty() && view["project"] == project && view["revision"] == revision,
            "view context does not match this record snapshot",
        )?;
        let orientation = record
            .get("orientation")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        require(
            view["orientation"] == orientation
                && view["rules"] == include_str!("../shared/session/rules.txt").trim(),
            "view orientation or rules changed",
        )?;
        let bounds = self.session_bounds(bounds)?;
        let payload = session_payload(
            serde_json::json!({"operation":"guard_view","record":record,"view":view}),
            &bounds,
        )?;
        let (_, result) = self.execute(payload, &bounds, &[0])?;
        let checks = [
            "accepted",
            "coverage",
            "conflicts",
            "links",
            "cell_ownership",
            "counts",
            "conditions_addressed",
            "link_map",
            "events",
            "raw_links",
        ];
        require(
            checks.iter().all(|key| result[*key].is_boolean()),
            "invalid_ordinary_guard_response",
        )?;
        require(
            checks.iter().all(|key| result[*key] == true),
            &format!("invalid projection: {}", serde_json::to_string(&result)?),
        )?;
        Ok(result)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn scripted(bytes: &[u8]) -> (tempfile::TempDir, Program) {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("epistemic-core");
        fs::write(&binary, bytes).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let manifest = serde_json::json!({"source_sha256":env!("KPOP_ORDINARY_SOURCE_SHA256"),"binary_sha256":sha256(bytes)});
        fs::write(
            temp.path().join("build.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let program = Program::open(temp.path()).unwrap();
        (temp, program)
    }

    #[test]
    fn changed_program_and_wrong_protocol_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("epistemic-core");
        let bytes = b"#!/bin/sh\nprintf '{}'\n";
        fs::write(&binary, bytes).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let manifest = serde_json::json!({"source_sha256":env!("KPOP_ORDINARY_SOURCE_SHA256"),"binary_sha256":sha256(bytes)});
        fs::write(
            temp.path().join("build.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let mut p = Program::open(temp.path()).unwrap();
        let request = serde_json::json!({"operation":"compute","record":{"nodes":{}},"predicate":null,"dependencies":[]});
        p.limit(&OperationalBounds {
            input_bytes: 1,
            ..Default::default()
        });
        assert_eq!(
            p.request(&request, &OperationalBounds::default())
                .unwrap_err()
                .0,
            "ordinary_input_limit"
        );
        p.limit(&OperationalBounds::default());
        assert_eq!(
            p.request(&request, &OperationalBounds::default())
                .unwrap_err()
                .0,
            "invalid_ordinary_response"
        );
        fs::write(&binary, b"changed").unwrap();
        assert_eq!(
            p.request(&request, &OperationalBounds::default())
                .unwrap_err()
                .0,
            "ordinary_program_changed"
        );
    }

    #[test]
    fn configured_limits_bound_scan_and_assess() {
        let (_temp, mut input) = scripted(b"#!/bin/sh\nprintf '{}'");
        input.limit(&OperationalBounds {
            input_bytes: 1,
            ..Default::default()
        });
        assert_eq!(
            input
                .scan(&serde_json::json!({}), &OperationalBounds::default())
                .unwrap_err()
                .0,
            "ordinary_input_limit"
        );

        let (_temp, mut output) = scripted(b"#!/bin/sh\nprintf '012345678901234567890123456789'");
        output.limit(&OperationalBounds {
            output_bytes: 16,
            ..Default::default()
        });
        assert_eq!(
            output
                .scan(&serde_json::json!({}), &OperationalBounds::default())
                .unwrap_err()
                .0,
            "output_limit"
        );

        let (_temp, mut timeout) = scripted(b"#!/bin/sh\nsleep 1\nprintf '{}'");
        timeout.limit(&OperationalBounds {
            timeout: Duration::from_millis(20),
            ..Default::default()
        });
        assert_eq!(
            timeout
                .assess(
                    &serde_json::json!({}),
                    "d.test",
                    &[serde_json::json!({"kind":"current","id":"p.a","expected":1})],
                    &OperationalBounds::default(),
                )
                .unwrap_err()
                .0,
            "runtime_timeout"
        );
    }

    fn empty_view() -> (J, J) {
        let record = serde_json::json!({"project_context":"fixture","nodes":{},"edges":[]});
        let revision = sha256(&serde_json::to_vec(&serde_json::json!({"project":"fixture","graph":sha256(&serde_json::to_vec(&record).unwrap())})).unwrap());
        let view = serde_json::json!({"project":"fixture","revision":revision,"orientation":{},"rules":include_str!("../shared/session/rules.txt").trim()});
        (record, view)
    }

    #[test]
    fn session_executable_lowering_matches_python_without_rewriting_evidence() {
        let fixture: J = serde_json::from_str(include_str!(
            "../tests/fixtures/ordinary-session-view-oracle.json"
        ))
        .unwrap();
        for case in fixture["wire_requests"].as_array().unwrap() {
            let actual: J = serde_json::from_slice(
                &session_payload(case["input"].clone(), &OperationalBounds::default()).unwrap(),
            )
            .unwrap();
            assert_eq!(actual, case["wire"]);
        }
    }

    #[test]
    fn compute_keeps_configured_timeout_and_sessions_have_explicit_twenty_second_cap() {
        let (_temp, mut program) = scripted(b"#!/bin/sh\nprintf '{}'");
        let bounds = OperationalBounds {
            timeout: Duration::from_secs(25),
            ..Default::default()
        };
        assert_eq!(
            program.effective_bounds(&bounds).unwrap().timeout,
            Duration::from_secs(25)
        );
        assert_eq!(
            program.session_bounds(&bounds).unwrap().timeout,
            Duration::from_secs(20)
        );
        program.limit(&OperationalBounds {
            timeout: Duration::from_millis(20),
            ..Default::default()
        });
        assert_eq!(
            program.effective_bounds(&bounds).unwrap().timeout,
            Duration::from_millis(20)
        );
        assert_eq!(
            program.session_bounds(&bounds).unwrap().timeout,
            Duration::from_millis(20)
        );
    }

    #[test]
    fn guard_protocol_and_process_limits_fail_closed() {
        let (record, view) = empty_view();
        let (temp, mut program) = scripted(b"#!/bin/sh\nprintf '{\"accepted\":true}'");
        assert_eq!(
            program
                .guard_view(&record, &view, &OperationalBounds::default())
                .unwrap_err()
                .0,
            "invalid_ordinary_guard_response"
        );
        program.limit(&OperationalBounds {
            input_bytes: 1,
            ..Default::default()
        });
        assert_eq!(
            program
                .guard_view(&record, &view, &OperationalBounds::default())
                .unwrap_err()
                .0,
            "ordinary_input_limit"
        );
        program.limit(&OperationalBounds {
            output_bytes: 1,
            ..Default::default()
        });
        assert_eq!(
            program
                .guard_view(&record, &view, &OperationalBounds::default())
                .unwrap_err()
                .0,
            "output_limit"
        );
        program.limit(&OperationalBounds::default());
        fs::write(temp.path().join("epistemic-core"), b"changed").unwrap();
        assert_eq!(
            program
                .guard_view(&record, &view, &OperationalBounds::default())
                .unwrap_err()
                .0,
            "ordinary_program_changed"
        );
        let (_temp, mut timeout) = scripted(b"#!/bin/sh\nsleep 1\nprintf '{}'");
        timeout.limit(&OperationalBounds {
            timeout: Duration::from_millis(20),
            ..Default::default()
        });
        assert_eq!(
            timeout
                .guard_view(&record, &view, &OperationalBounds::default())
                .unwrap_err()
                .0,
            "runtime_timeout"
        );
        let (_temp, rejected) = scripted(b"#!/bin/sh\nprintf '{\"accepted\":true,\"coverage\":false,\"conflicts\":true,\"links\":true,\"cell_ownership\":true,\"counts\":true,\"conditions_addressed\":true,\"link_map\":true,\"events\":true,\"raw_links\":true}'");
        assert!(
            rejected
                .guard_view(&record, &view, &OperationalBounds::default())
                .unwrap_err()
                .0
                .starts_with("invalid projection:")
        );
    }
}

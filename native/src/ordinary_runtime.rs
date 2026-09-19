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

#[derive(Debug)]
pub struct Program {
    root: PathBuf,
    binary: PathBuf,
    manifest: Vec<u8>,
    binary_sha256: String,
    bounds: OperationalBounds,
}
impl Program {
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
            Duration::from_secs(20).min(bounds.timeout),
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
        let bounds = self.effective_bounds(bounds)?;
        let request = serde_json::json!({"operation":"scan","record":record});
        let payload = serde_json::to_vec(&request)?;
        require(payload.len() <= bounds.input_bytes, "ordinary_input_limit")?;
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
        let bounds = self.effective_bounds(bounds)?;
        let request = serde_json::json!({"record":record,"id":judgment,"assertions":assertions});
        let payload = serde_json::to_vec(&request)?;
        require(payload.len() <= bounds.input_bytes, "ordinary_input_limit")?;
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
}

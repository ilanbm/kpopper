//! Explicit, verified access to the ordinary reader's existing Lean program.
//! This JSON protocol is distinct from the core/v1 KP2/KP3/KP4 protocols.
use crate::{
    Error, Result,
    identity::sha256,
    reasoning_runtime::{OperationalBounds, run_bounded},
    require,
};
use serde_json::Value as J;
use std::{
    fs,
    path::{Path, PathBuf},
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
    pub fn request(&self, request: &J, bounds: &OperationalBounds) -> Result<J> {
        bounds.validate()?;
        let bounds = OperationalBounds {
            timeout: bounds.timeout.min(self.bounds.timeout),
            batch_requests: bounds.batch_requests.min(self.bounds.batch_requests),
            input_bytes: bounds.input_bytes.min(self.bounds.input_bytes),
            output_bytes: bounds.output_bytes.min(self.bounds.output_bytes),
        };
        let payload = serde_json::to_vec(request)?;
        require(payload.len() <= bounds.input_bytes, "ordinary_input_limit")?;
        self.verify()?;
        let output = run_bounded(
            &self.binary,
            &[],
            payload,
            bounds.timeout,
            bounds.output_bytes,
        )?;
        self.verify()?;
        let result = crate::json_ingress::parse_slice(
            &output,
            crate::json_ingress::DuplicateKeys::LastWins,
        )?;
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
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
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
}

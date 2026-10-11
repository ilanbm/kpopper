use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const PACKAGE_FORMAT: &str = "kpop-model-package/v1";
pub const LOCK_FORMAT: &str = "kpop-model-lock/v1";
pub const TARGET: &str = "darwin-arm64";
pub const ENGINE_VERSION: &str = "0.15.1";
pub const ENGINE_SHA256: &str = "4b1c3390f4c68f3b8bd50c2e49fee6e5ac5360631ad26dbb5a095b6460d6e1b5";
pub const ENGINE_SIZE: u64 = 54_947_168;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Capability {
    Read,
    Application,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Engine {
    pub version: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Adapter {
    pub protocol: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Smoke {
    Read {
        id: String,
    },
    Application {
        case: String,
        as_of: String,
        expect: BTreeMap<String, Value>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub format: String,
    pub id: String,
    pub version: String,
    pub target: String,
    pub capability: Capability,
    pub model: String,
    pub skill: String,
    pub include: Vec<String>,
    pub engine: Engine,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<Adapter>,
    pub smoke: Smoke,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileIdentity {
    pub sha256: String,
    pub size: u64,
    pub executable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageLock {
    pub format: String,
    pub descriptor_sha256: String,
    pub source_commit: String,
    pub engine: Engine,
    pub files: BTreeMap<String, FileIdentity>,
}

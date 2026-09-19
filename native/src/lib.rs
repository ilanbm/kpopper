#![forbid(unsafe_code)]

pub mod history_adapter;
pub mod history_authority;
pub mod history_cancellation;
pub mod history_capture;
pub mod history_contract;
pub mod history_emit;
pub mod history_group;
pub mod history_paths;
pub mod history_preparation;
pub mod history_projection;
pub mod history_reduce;
pub mod history_store;
pub mod history_transaction;
pub mod history_transaction_fs;
pub mod history_view;
pub mod history_yaml;
pub mod identity;
mod python_identifiers;
pub mod reasoning_fields;
pub mod reasoning_language;
pub mod reasoning_query;
mod reasoning_query_bounds;
pub mod reasoning_runtime;
pub mod reasoning_transport;
pub mod source_clock;
pub mod source_text;
pub mod store;
pub mod value;

use std::fmt;

#[derive(Debug)]
pub struct Error(pub String);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self(format!("io: {error}"))
    }
}
impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self(format!("invalid_json: {error}"))
    }
}
pub type Result<T> = std::result::Result<T, Error>;
pub fn require(condition: bool, code: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Error(code.into()))
    }
}

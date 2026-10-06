//! Shared types for the versioned native model package companion.
pub mod manifest;
pub mod package;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub mod runtime;

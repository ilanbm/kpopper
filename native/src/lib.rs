#![forbid(unsafe_code)]

#[cfg(test)]
mod test_runtime_provenance;

pub mod annotated_document;
mod annotated_document_html;
pub mod authoring_source;
pub mod checked_session;
pub mod checked_session_store;
pub mod core_html;
pub mod direct_history;
mod followup_core;
pub mod followup_store;
pub mod followup_triggers;
pub mod history_activation;
mod history_activation_verify;
pub mod history_adapter;
pub mod history_authoring;
mod history_authoring_audit;
pub mod history_authoring_batch;
mod history_authoring_reader;
pub mod history_authority;
pub mod history_bootstrap;
pub mod history_branch;
pub mod history_branch_adoption;
pub mod history_branch_audit;
pub mod history_branch_git;
pub mod history_branch_preview;
pub mod history_bundle;
pub mod history_cancellation;
pub mod history_capture;
pub mod history_contract;
pub mod history_edits;
pub mod history_emit;
pub mod history_group;
pub mod history_group_activation;
pub mod history_hypotheses;
pub mod ingestion_target;
pub mod history_hypothesis_authoring;
pub mod history_hypothesis_import;
pub mod history_hypothesis_import_prepare;
pub mod history_identity;
pub mod history_identity_brief;
mod history_identity_merge;
pub mod history_identity_rewrite;
mod history_identity_text;
pub mod history_migration;
mod history_migration_copy;
mod history_migration_source;
pub mod history_native_declaration;
pub mod history_paths;
pub mod history_preparation;
pub mod history_projection;
pub mod history_prospective;
pub mod history_reduce;
pub mod history_runtime;
pub mod history_sources;
pub mod history_store;
mod history_temporal;
mod history_temporal_capture;
pub mod history_transaction;
pub mod history_transaction_fs;
pub mod history_view;
pub mod history_watch;
pub mod history_yaml;
pub mod identity;
pub mod ingestion_orchestration;
pub mod ingestion_delivery;
pub mod ingestion_state;
pub mod ingestion_hooks;
pub mod json_ingress;
pub mod legacy_authoring;
mod legacy_batch;
pub mod ordinary_assessment;
mod ordinary_assessment_report;
pub mod ordinary_checked_session;
mod ordinary_counts;
mod ordinary_document;
mod ordinary_domain_counts;
mod ordinary_fields;
mod ordinary_findings;
pub mod ordinary_hub;
mod ordinary_language;
mod ordinary_overlay;
pub(crate) mod ordinary_page_capture;
pub mod ordinary_reader;
mod ordinary_report;
pub mod ordinary_runtime;
mod ordinary_semantics;
mod ordinary_source;
pub mod ordinary_value;
mod ordinary_views;
pub(crate) mod ordinary_write_report;
mod ordinary_yaml_diagnostic;
pub mod pending_bundle;
pub mod pending_control;
pub mod pending_publication;
pub mod pending_state;
pub mod project_modes;
pub mod public_annotated_document;
pub mod public_assessment;
pub mod public_authoring;
pub mod public_checked_session;
pub mod public_config;
pub mod public_consolidation;
pub mod public_export;
pub mod public_followups;
pub mod public_history;
pub mod public_hub;
pub mod public_identity;
pub mod public_ingestion;
pub mod public_knowledge;
pub mod public_pending;
pub mod public_update;
pub mod public_workspace;
pub mod publication_provider;
mod python_identifiers;
pub mod reasoning_assessment;
pub mod reasoning_authoring;
mod reasoning_authoring_guards;
pub mod reasoning_authoring_preparation;
pub mod reasoning_basis;
pub mod reasoning_capabilities;
pub mod reasoning_context;
pub mod reasoning_declaration_text;
pub mod reasoning_evaluate;
pub mod reasoning_fields;
pub mod reasoning_history_assessment;
pub mod reasoning_history_support;
pub mod reasoning_language;
pub mod reasoning_operations;
pub mod reasoning_projection;
pub mod reasoning_query;
pub mod reasoning_query_adapter;
mod reasoning_query_bounds;
pub mod reasoning_runtime;
pub mod reasoning_scenario;
pub mod reasoning_scope;
pub mod reasoning_snapshot;
mod reasoning_temporal;
pub mod reasoning_transport;
pub mod reasoning_values;
mod recording_privacy;
pub mod recording_receipt;
pub mod session_activity;
pub mod session_gate;
pub mod session_mcp;
pub mod session_search;
pub mod source_capture;
pub mod source_clock;
mod source_document;
mod source_inventory;
mod source_overlay;
mod source_target;
pub mod source_text;
pub mod store;
pub mod tokenizer;
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

pub mod core_page;
pub mod public_core_readers;
pub mod public_ordinary_readers;
pub mod public_readers;
pub mod public_remeasure;
pub mod public_search;
mod public_search_rank;
pub mod public_session;

pub mod application_cli;
pub mod followup_daily;
pub mod history_contribution_adoption;
pub(crate) mod history_contribution_prepare;
pub mod public_expressions;
mod public_history_adopt;
pub mod public_watch;
pub mod onboarding;
pub mod public_map;
pub mod watch_capture;
pub mod watch_compare;
pub mod watch_delivery;
pub mod watch_shared;
pub mod watch_store;

mod session_settings;
pub mod session_admin;

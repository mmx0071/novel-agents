//! Harness: pipeline constants, content rules, review priority, gates.

pub mod activation;
pub mod chapter_budget;
pub mod content_rules;
pub mod gates;
pub mod naming_rules;
pub mod pipeline_config;
pub mod policies;
pub mod review_priority;

pub use activation::{
    collect_signals, evaluate_activation, resolve_pipeline_agents, ActivationSignals,
    ActivationSuggestion,
};
pub use chapter_budget::ChapterBudget;
pub use content_rules::check_draft;
pub use gates::{GateDecision, on_consistency_result, on_pacing_result, should_publish};
pub use naming_rules::NamingRules;
pub use pipeline_config::{HandlerKind, HandlerSpec, PipelineConfig};
pub use policies::StudioPolicies;
pub use review_priority::{
    issue_priority, normalize_consistency_issues, normalize_priority, partition_issues,
    AUTO_FIX_PRIORITIES,
};

/// Full-rewrite detection from `config/policies.yaml` (embedded defaults).
pub fn needs_full_rewrite(message: &str) -> bool {
    StudioPolicies::defaults().needs_full_rewrite(message)
}

/// Embedded defaults for callers that lack a config_root (tests / early init).
pub fn default_pipeline() -> PipelineConfig {
    PipelineConfig::defaults()
}

//! Harness: pipeline constants, content rules, review priority, gates.

pub mod activation;
pub mod chapter_budget;
pub mod content_rules;
pub mod continuity;
pub mod gates;
pub mod longform;
pub mod naming_rules;
pub mod pipeline_config;
pub mod policies;
pub mod review_priority;
pub mod revise_plan;
pub mod script_shape;
pub mod unattended;

pub use activation::{
    collect_signals, evaluate_activation, resolve_pipeline_agents, resolve_pipeline_agents_filtered,
    resolve_pipeline_agents_with_tier, ActivationSignals, ActivationSuggestion,
};
pub use chapter_budget::{
    consecutive_soft_short_from_meta, set_consecutive_soft_short, ChapterBudget, LengthAssessment,
};
pub use script_shape::ScriptShapeConfig;
pub use longform::{
    AuditTier, ForeshadowDebtConfig, ImpactScanMode, LongformConfig, QualityTier,
};
pub use continuity::{ContinuityBudget, ContinuityTier};
pub use content_rules::{
    check_draft, check_draft_with, has_blocking_violation, rewrite_meta_chapter_refs_in_body,
    rewrite_meta_chapter_refs_with, ContentRuleViolation, ContentRulesConfig,
};
pub use gates::{
    consistency_human_option_labels, on_consistency_result, on_pacing_result, should_publish,
    GateDecision,
};
pub use naming_rules::NamingRules;
pub use pipeline_config::{HandlerKind, HandlerSpec, PipelineConfig};
pub use policies::StudioPolicies;
pub use unattended::{BatchSoftSkips, SoftSkipPolicy, UnattendedPolicy};
pub use review_priority::{
    filter_issues_by_ids, has_timeline_p0, issue_fingerprint, issue_priority, issue_type,
    merge_verify_audit, normalize_consistency_issues, normalize_priority, partition_issues,
    with_issue_ids, AUTO_FIX_PRIORITIES,
};
pub use revise_plan::{
    apply_plan_to_steer_args, build_revise_plan, hard_gate_ids_from_violations,
    load_revise_streak, persist_revise_context, RevisePlan, RevisePlanConfig, ReviseScope,
};

/// Full-rewrite detection from `config/policies.yaml` (embedded defaults).
pub fn needs_full_rewrite(message: &str) -> bool {
    StudioPolicies::defaults().needs_full_rewrite(message)
}

/// Embedded defaults for callers that lack a config_root (tests / early init).
pub fn default_pipeline() -> PipelineConfig {
    PipelineConfig::defaults()
}

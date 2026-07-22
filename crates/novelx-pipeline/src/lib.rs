//! Chapter pipeline — Codex-style step events, local-patch-first revision.

pub mod cards;
pub mod context;
pub mod lore;
pub mod memory;
pub mod phases;
pub mod plots;
pub mod project;
pub mod run;
pub mod schemas;
pub mod volume;
pub mod volume_audit;
pub mod volume_sync;

pub use cards::{
    collect_entity_gaps, entity_status_is_active, load_markdown_cards, normalize_entity_status,
    read_arc_outline_excerpt, MarkdownCard,
};
pub use context::{build_chapter_context, read_world_doc, ChapterContextPack, ContextProfile};
pub use lore::{lore_assert_from_summary, lore_query};
pub use memory::{
    apply_summary_json, load_memory, recall_archived_summaries, save_memory, ProjectMemory,
};
pub use plots::{
    accept_verdict_is_pass, accept_verdict_is_pass_against, active_plot_exit_context,
    advance_plots_for_published_chapter, check_plot_write_gate, check_plot_write_gate_with,
    complete_active_plot_on_accept, complete_bridging_plots_after_publish,
    in_progress_plot_missing_exit, ensure_bridge_plot_active, ensure_plot_card_lifecycle_frontmatter,
    extract_plot_exit_condition, list_plots_summary, load_plot_cards_by_progress, load_plot_index,
    materialize_plot_card_markdown, normalize_status, pending_bridge_plot,
    plot_design_blocked_reason, rebuild_plot_index, select_plots_for_chapter,
    sort_plot_cards_by_progress, sort_plot_entries_by_progress, update_plot_card, PlotAdvanceEvent,
    PlotIndex, PlotWriteGate, PlotWriteMode,
};
pub use phases::{
    confirm_setup_approve, confirm_setup_revise, design_plot_force_allowed, load_meta_json,
    lock_brief, mark_volume_sync_skipped, maybe_advance_setup_after_outlines, resolve_setup_phase,
    resolve_volume_phase, set_setup_phase, set_volume_phase, setup_write_block_reason,
    volume_write_block_reason, PhaseEnforceFlags, SetupPhase, VolumePhase,
};
pub use project::{
    chapter_dir, delete_chapter, init_project, list_chapter_numbers, list_projects,
    load_project_state, project_dir, read_chapter_draft, read_chapter_outline, refresh_meta_flags,
    save_project_state, sync_records_novel_json, write_chapter_draft, write_chapter_outline,
    DeleteChapterResult, ProjectState,
};
pub use schemas::{
    display_arc_outline, display_bible, display_chapter_outline, display_draft, display_entity_card,
    display_entity_gaps, display_master_outline, display_plot_card_body,
    migrate_project_reader_formats, parse_chapter_outline_text, validate_arc_outline, validate_bible,
    normalize_draft_best_effort, validate_chapter_outline, validate_draft, validate_entity_card,
    validate_master_outline, validate_plot_card, ChapterOutline, EntityKind, MigrateReport,
    SchemaError, MIN_DRAFT_BODY_CHARS,
};
pub use run::{
    execute_pipeline, execute_pipeline_with_steps, execute_single_agent_step, needs_full_rewrite,
    finalize_chapter_publish, plan_steps_for_mode, steer_revision_options, PipelineEvent,
    PipelineRun, PipelineStep, PublishFinalizeResult, RevisionOptions, RunMode,
};
pub use volume::{
    active_volume_for_chapter, bound_for_volume, evaluate_volume_end, load_volume_bounds,
    mark_volume_completed, migrate_legacy_arc_planner, parse_bounds_from_markdown,
    parse_volume_meta_from_markdown, split_ending_conditions, sync_act_chapter_bounds,
    volume_chapter_span, volume_just_ended, VolumeBound, VolumeEndDecision,
};
pub use volume_audit::{
    gather_volume_audit_pack, heuristic_deep_audit_chapters, run_volume_audit, VolumeAuditReport,
};
pub use volume_sync::{apply_volume_sync_json, run_volume_sync, VolumeSyncReport};

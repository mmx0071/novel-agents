//! Chapter pipeline — Codex-style step events, local-patch-first revision.

pub mod activation_hints;
pub mod batch;
pub mod body_state;
pub mod cards;
pub mod chapter_gate;
pub mod chapter_index;
pub mod context;
pub mod expected_events;
pub mod foreshadow;
pub mod impact;
pub mod lore;
pub mod memory;
pub mod phases;
pub mod plots;
pub mod project;
pub mod run;
pub mod schemas;
pub mod setting_audit;
pub mod setting_pass;
pub mod split_chapter;
pub mod volume;
pub mod volume_audit;
pub mod volume_drift;
pub mod volume_pack;
pub mod volume_sync;
pub mod lore_index;
pub mod materials;
pub mod cold_archive;
pub mod cost_log;
pub mod version_nodes;
pub mod volume_audit_gate;
pub mod volume_checklist;
pub mod volume_qa;
pub mod foreshadow_phase;

pub use cards::{
    collect_entity_gaps, detect_location_parent, entity_names_equivalent, entity_status_is_active,
    fold_location_child_into_card, load_markdown_cards, location_name_looks_dependent,
    location_name_looks_parent_scale, normalize_entity_status, read_arc_outline_excerpt,
    resolve_entity_card_path, resolve_location_write_target, MarkdownCard,
};
pub use batch::{
    apply_unattended_batch_policy, run_continue_batch, BatchChapterResult, BatchContinueOpts,
    BatchContinueResult,
};
pub use body_state::{
    check_body_state_conflicts, check_body_state_locus_conflicts, check_body_state_side_conflicts,
    format_body_state_board, format_body_state_board_for_character,
};
pub use chapter_gate::{check_chapter_order, check_revise_target, ChapterOrderBlock};
pub use context::{
    build_chapter_context, format_chapter_bridge, read_world_doc, ChapterContextPack, ContextProfile,
};
pub use lore::{lore_assert_from_summary, lore_query};
pub use expected_events::{
    agent_fit_actionable, build_eval_context, conditions_from_value, count_by_status,
    enqueue_expected_event, eval_hard_ok, format_conditions_line, format_events_summary,
    format_expected_for_context, format_expected_for_lore, list_events_for_preview,
    list_gate_candidates, list_hard_ok_candidates, load_expected_events, needs_fresh_review,
    resolve_expected_event, save_expected_events, set_event_review, update_expected_event,
    ExpectedConditions, ExpectedEvent, ExpectedEventStore, ExpectedReview,
};
pub use foreshadow::{
    classify_foreshadow_debt, format_dangling_for_context, format_dangling_for_tracker,
    foreshadow_debt_breakdown, load_foreshadow_index, normalize_foreshadow_horizon,
    rebuild_foreshadow_index, ForeshadowDebtBreakdown, ForeshadowDebtClass, ForeshadowIndex,
};
pub use activation_hints::{
    collect_studio_activation_hints, format_studio_activation_hints_block,
    studio_activation_hints_for_project, StudioActivationHint,
};
pub use chapter_index::{
    bm25_recall, list_indexed_chapters, load_index, rebuild_index, remove_chapter,
    upsert_chapter_summary, ChapterIndex,
};
pub use memory::{
    apply_summary_json, build_volume_rollup_from_summaries, confirm_volume_memory,
    format_volume_memory_preview, load_all_volume_rollups, load_facts_archive,
    load_foreshadow_archive, load_memory, longform_health_snapshot,
    prune_asserted_facts_to_archive, prune_open_threads_into_archive, recall_archived_summaries,
    recall_archived_summaries_with, recall_archived_threads, recall_archived_threads_in,
    foreshadow_already_known, resolve_foreshadow_archive, save_memory,
    select_asserted_facts_for_context, select_asserted_facts_for_context_in,
    select_dangling_age_boosted, select_entity_timeline_for_context, upsert_volume_rollup,
    EntityTimelineFact, ProjectMemory, VolumeRollup, HOT_ENTITY_TIMELINE_LIMIT, HOT_FACTS_LIMIT,
};
pub use plots::{
    accept_verdict_is_pass, accept_verdict_is_pass_against, active_plot_exit_context,
    advance_plots_for_published_chapter, check_plot_write_gate, check_plot_write_gate_with,
    complete_active_plot_on_accept, complete_bridging_plots_after_publish,
    in_progress_plot_missing_exit, ensure_bridge_plot_active, ensure_next_plot_card,
    ensure_plot_card_lifecycle_frontmatter,
    extract_plot_exit_condition, format_plot_progress_report, format_plot_progress_report_for,
    list_plots_summary,
    load_plot_cards_by_progress, load_plot_index, volume_has_open_plot_work,
    materialize_plot_card_markdown, normalize_status, pending_bridge_plot,
    plot_design_blocked_reason, rebuild_plot_index, select_plots_for_chapter,
    sort_plot_cards_by_progress, sort_plot_entries_by_progress, update_plot_card, PlotAdvanceEvent,
    PlotIndex, PlotWriteGate, PlotWriteMode,
};
pub use phases::{
    bible_setup_gap_reason, confirm_setup_approve, confirm_setup_revise, design_plot_force_allowed,
    has_valid_bible, load_meta_json, lock_brief, mark_volume_sync_skipped,
    maybe_advance_setup_after_outlines, recover_false_volume_end, resolve_setup_next_step,
    resolve_setup_phase, resolve_volume_phase, set_setup_phase, set_volume_phase,
    setup_write_block_reason, volume_write_block_reason, PhaseEnforceFlags, SetupNextStep,
    SetupPhase, VolumePhase,
};
pub use project::{
    chapter_dir, chapter_memory_artifact_matches_draft, delete_chapter, draft_fingerprint,
    init_project, init_project_with_mode, is_short_drama, list_chapter_numbers, list_projects,
    load_project_state, project_dir, read_chapter_draft, read_chapter_outline, refresh_meta_flags,
    resolve_project_mode, save_project_state, sync_records_novel_json, unit_body_filename,
    update_target_chapters, write_chapter_draft, write_chapter_memory_artifact, write_chapter_outline,
    write_chapter_outline_budget, DeleteChapterResult, ProjectMode, ProjectState,
};
pub use schemas::{
    display_arc_outline, display_bible, display_chapter_outline, display_draft, display_entity_card,
    display_entity_gaps, display_master_outline, display_plot_card_body, display_script,
    draft_body_chars, migrate_project_reader_formats, normalize_script_best_effort,
    outline_budget_repair_hint, outline_entity_roster, parse_chapter_outline_text,
    parse_chapter_outline_text_budget, validate_arc_outline, validate_bible,
    normalize_draft_best_effort, normalize_plot_card_best_effort, validate_chapter_outline,
    validate_chapter_outline_budget, validate_draft, validate_entity_card, validate_master_outline,
    validate_plot_card, validate_script, ChapterOutline, EntityKind, MigrateReport,
    OutlineEntityRoster, OutlineValidateMode, SchemaError, MAX_KEY_EVENTS, MIN_DRAFT_BODY_CHARS,
    MIN_SCRIPT_BODY_CHARS,
};
pub use run::{
    apply_cached_local_patches, execute_pipeline, execute_pipeline_with_steps,
    execute_single_agent_step, finalize_chapter_publish, looks_like_revision_instruction_leak,
    needs_full_rewrite, plan_full_revision_preview, plan_local_revision_preview, plan_steps_for_mode,
    steer_revision_options, LocalPatchPreviewItem, LocalRevisionPreview, PipelineEvent, PipelineRun,
    PipelineStep, PublishFinalizeResult, RevisionOptions, RunMode,
};
pub use volume::{
    active_volume_for_chapter, arc_outline_path, bound_for_volume, evaluate_volume_end,
    has_any_arc_outline, list_arc_outline_volumes, list_arc_outlines_for_preview,
    load_volume_bounds, mark_volume_completed, migrate_arc_outlines, migrate_legacy_arc_planner,
    parse_bounds_from_markdown, parse_volume_meta_from_markdown, read_arc_outline_text,
    reopen_volume_act, resolve_arc_outline_volume, split_ending_conditions, sync_act_chapter_bounds,
    volume_chapter_span, volume_just_ended, write_arc_outline_text, VolumeBound, VolumeEndDecision,
};
pub use volume_audit::{
    gather_volume_audit_pack, heuristic_deep_audit_chapters, persist_volume_audit_report,
    run_volume_audit, volume_audit_report_path, volume_has_audit_report, VolumeAuditReport,
};
pub use volume_audit_gate::{
    check_volume_audit_for_continue, check_volume_audit_for_continue_ex,
    check_volume_audit_for_sync, find_config_root, mid_audit_threshold_resolved,
    thick_volume_threshold_resolved, VolumeAuditGateBlock,
};
pub use volume_qa::{
    clear_volume_qa_mid_due, mark_volume_qa_mid_skipped, resolve_volume_qa_phase,
    set_volume_qa_phase, VolumeQaPhase,
};
pub use foreshadow_phase::{
    foreshadow_batch_block_message, foreshadow_phase_advice, resolve_foreshadow_phase,
    set_foreshadow_phase, ForeshadowPhase, ForeshadowPhaseSnapshot,
};
pub use volume_checklist::{
    run_volume_memory_checklist, ChecklistItem, VolumeMemoryChecklist,
};
pub use volume_drift::{
    run_volume_drift_check, run_volume_drift_check_with, DriftCheckOpts, DriftNote,
    VolumeDriftReport,
};
pub use volume_pack::{gather_volume_layered_pack, LayeredVolumePack, VOLUME_PACK_CHAR_CAP};
pub use lore_index::{
    ensure_lore_index, list_chapters_from_index, query_entities_from_index,
    query_open_foreshadow_from_index, rebuild_lore_index_from_disk, remove_chapter_index_row,
    upsert_chapter_index_row, upsert_entity_index_row, upsert_foreshadow_index_row, LoreIndex,
};
pub use materials::{
    clear_drought_flag, clear_inspiration_flag, drought_flag_active,
    format_material_hooks_for_context, inspiration_flag_active, load_material_runtime_config,
    load_materials_index, material_cooldown_allows, material_research_prompt,
    parse_material_cards_json, read_project_brief, save_material_cards, set_drought_flag,
    set_inspiration_flag, MaterialCard, MaterialRuntimeConfig, MaterialsIndex,
};
pub use cold_archive::{
    maybe_cold_archive_volume, read_chapter_draft_resolved, ColdArchiveResult,
};
pub use cost_log::{
    append_cost_entry, format_cost_by_agent_line, format_cost_status_line, load_recent_entries,
    summarize_cost_by_agent, CostEntry, CostAgentSummary,
};
pub use impact::{
    impact_source_arc, impact_source_bible, impact_source_draft, impact_source_entity,
    impact_source_master, impact_source_outline, scan_impact, scan_impact_with_opts, ImpactHit,
    ImpactReport, ImpactScanOpts, ImpactSource, ImpactSourceKind, ImpactTargetKind,
};
pub use setting_audit::{
    build_setting_audit_pack, build_structure_audit_candidate, run_setting_audit,
    SettingAuditPackOpts, SettingAuditResult,
};
pub use setting_pass::{
    run_chapter_setting_pass, run_plot_setting_pass, ChapterSettingPassFlags,
    PlotSettingPassFlags, PlotSettingPassResult,
};
pub use split_chapter::{chapter_looks_overlong, split_chapter_draft, SplitChapterResult};
pub use version_nodes::{
    commit_node, ensure_repo as ensure_version_repo, git_available as version_git_available,
    list_nodes as list_version_nodes, post_restore_disk_hooks,
    restore_node as restore_version_node, VersionNode,
};
pub use volume_sync::{
    apply_volume_sync_json, run_chapter_sync, run_plot_sync, run_volume_sync, VolumeSyncReport,
};

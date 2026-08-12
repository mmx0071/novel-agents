//! Locked on-disk / display schemas for reader surfaces (genre-neutral).

mod arc;
mod bible;
mod chapter_outline;
mod draft;
mod entity;
mod error;
mod master;
mod md;
mod migrate;
mod plot;
mod script;

pub use arc::{display_arc_outline, normalize_arc_outline_input, validate_arc_outline};
pub use bible::{display_bible, validate_bible};
pub use chapter_outline::{
    display_chapter_outline, outline_budget_repair_hint, outline_entity_roster,
    parse_chapter_outline_text, parse_chapter_outline_text_budget, validate_chapter_outline,
    validate_chapter_outline_budget, validate_chapter_outline_with, ChapterOutline,
    OutlineEntityRoster, OutlineValidateMode, MAX_KEY_EVENTS,
};
pub use draft::{
    display_draft, draft_body_chars, normalize_draft_best_effort, validate_draft,
    MIN_DRAFT_BODY_CHARS,
};
pub use entity::{
    display_entity_card, minimal_entity_skeleton, validate_entity_body_edit, validate_entity_card,
    EntityKind,
};
pub use error::SchemaError;
pub use master::{display_master_outline, normalize_master_outline_input, validate_master_outline};
pub use migrate::{migrate_project_reader_formats, MigrateReport};
pub use plot::{
    display_plot_card_body, normalize_plot_card_best_effort, validate_plot_card,
    validate_plot_card_body_edit,
};
pub use script::{normalize_script_best_effort, validate_script, MIN_SCRIPT_BODY_CHARS};

/// Fixed entity-gaps reader text (computed list; no on-disk file).
pub fn display_entity_gaps(gaps: &[String]) -> String {
    if gaps.is_empty() {
        return "设定卡与世界观暂无明显缺口。".into();
    }
    let mut lines = vec![
        "（软提示·不阻断写章）待补全：".to_string(),
        String::new(),
    ];
    for g in gaps {
        lines.push(format!("- {g}"));
    }
    lines.join("\n")
}

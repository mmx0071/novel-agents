//! One-shot project migration toward locked reader schemas.

use super::chapter_outline::parse_chapter_outline_text;
use super::entity::{minimal_entity_skeleton, EntityKind};
use super::plot::validate_plot_card;
use std::fs;
use std::path::Path;

#[derive(Debug, Default)]
pub struct MigrateReport {
    pub outlines_migrated: Vec<u32>,
    pub outlines_failed: Vec<(u32, String)>,
    pub entities_padded: Vec<String>,
    pub plots_normalized: Vec<String>,
    pub plot_failed: Vec<(String, String)>,
    pub notes: Vec<String>,
}

/// Migrate legacy chapter outlines + pad thin entity/plot cards (no invented plot).
pub fn migrate_project_reader_formats(project_dir: &Path) -> MigrateReport {
    let mut report = MigrateReport::default();
    migrate_outlines(project_dir, &mut report);
    migrate_entities(project_dir, &mut report);
    migrate_plots(project_dir, &mut report);
    migrate_bible_note(project_dir, &mut report);
    report
}

fn migrate_outlines(project_dir: &Path, report: &mut MigrateReport) {
    let root = project_dir.join("chapters");
    let Ok(rd) = fs::read_dir(&root) else {
        return;
    };
    for e in rd.flatten() {
        let path = e.path();
        if !path.is_dir() {
            continue;
        }
        let Some(n) = path
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let json_path = path.join("outline.json");
        if json_path.is_file() {
            continue;
        }
        let md_path = path.join("outline.md");
        let Ok(md) = fs::read_to_string(&md_path) else {
            continue;
        };
        match parse_chapter_outline_text(&md) {
            Ok(o) => {
                if let Ok(pretty) = serde_json::to_string_pretty(&o) {
                    let _ = fs::write(&json_path, pretty);
                    report.outlines_migrated.push(n);
                }
            }
            Err(err) => report.outlines_failed.push((n, err.to_string())),
        }
    }
    report.outlines_migrated.sort_unstable();
}

fn migrate_entities(project_dir: &Path, report: &mut MigrateReport) {
    for (group, kind) in [
        ("characters", EntityKind::Character),
        ("items", EntityKind::Item),
        ("locations", EntityKind::Location),
    ] {
        let dir = project_dir.join("entities").join(group);
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("未命名");
            let before = text.clone();
            let padded = minimal_entity_skeleton(kind, name, &text);
            if padded != before {
                let _ = fs::write(&path, &padded);
                report
                    .entities_padded
                    .push(format!("{group}/{name}"));
            }
        }
    }
    report.entities_padded.sort();
}

fn migrate_plots(project_dir: &Path, report: &mut MigrateReport) {
    let dir = project_dir.join("plots");
    let Ok(rd) = fs::read_dir(&dir) else {
        return;
    };
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        if path.file_name().and_then(|s| s.to_str()) == Some("index.json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("plot")
            .to_string();
        match validate_plot_card(&text) {
            Ok(norm) => {
                if norm != text {
                    let _ = fs::write(&path, &norm);
                    report.plots_normalized.push(name);
                }
            }
            Err(err) => report.plot_failed.push((name, err.to_string())),
        }
    }
    report.plots_normalized.sort();
}

fn migrate_bible_note(project_dir: &Path, report: &mut MigrateReport) {
    let path = project_dir.join("artifacts/bible.md");
    let Ok(text) = fs::read_to_string(&path) else {
        report.notes.push("bible.md 不存在".into());
        return;
    };
    if super::bible::validate_bible(&text).is_err() {
        report.notes.push(
            "bible.md 缺必填节（# 世界观 + ## 0./1./2./7.）— 请用最小合法骨架手工补齐，勿静默造设定"
                .into(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn migrate_synthetic_project_outlines() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("novelx-migrate-{stamp}"));
        let ch = root.join("chapters/001");
        fs::create_dir_all(&ch).unwrap();
        fs::write(
            ch.join("outline.md"),
            r#"```json
{
  "title": "试章",
  "pov": "甲",
  "time_location": "夜·城",
  "goal": "到达",
  "conflict": "阻拦",
  "emotion_curve": "紧→松",
  "key_events": ["开场危机", "中段转折"],
  "characters": ["甲"],
  "scene_tags": ["chase"],
  "cliffhanger": "门开了",
  "lore_queries": ["甲伤势"]
}
```"#,
        )
        .unwrap();
        let report = migrate_project_reader_formats(&root);
        assert!(report.outlines_failed.is_empty(), "{:?}", report.outlines_failed);
        assert_eq!(report.outlines_migrated, vec![1]);
        assert!(root.join("chapters/001/outline.json").is_file());
        let _ = fs::remove_dir_all(&root);
    }
}

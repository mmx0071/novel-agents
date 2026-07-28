//! One-shot project migration toward locked reader schemas.

use super::bible::validate_bible;
use super::chapter_outline::parse_chapter_outline_text;
use super::entity::{minimal_entity_skeleton, validate_entity_card, EntityKind};
use super::md::{has_h2, join_fm, split_fm};
use super::plot::validate_plot_card;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Default)]
pub struct MigrateReport {
    pub outlines_migrated: Vec<u32>,
    pub outlines_padded: Vec<u32>,
    pub outlines_failed: Vec<(u32, String)>,
    pub entities_padded: Vec<String>,
    pub entities_stub_folded: Vec<String>,
    pub plots_normalized: Vec<String>,
    pub plot_failed: Vec<(String, String)>,
    pub bible_rewritten: bool,
    pub notes: Vec<String>,
}

/// Migrate legacy chapter outlines + pad thin entity/plot cards (no invented plot).
pub fn migrate_project_reader_formats(project_dir: &Path) -> MigrateReport {
    let mut report = MigrateReport::default();
    migrate_outlines(project_dir, &mut report);
    merge_paren_duplicate_entities(project_dir, &mut report);
    migrate_entities(project_dir, &mut report);
    migrate_plots(project_dir, &mut report);
    migrate_bible(project_dir, &mut report);
    report
}

/// Merge halfwidth/fullwidth paren twins, and swapped alias forms
/// (`A（B）.md` + `B（A）.md` → keep the longer card).
/// Covers characters / items / locations (items & locations often get「名（说明）」twins).
fn merge_paren_duplicate_entities(project_dir: &Path, report: &mut MigrateReport) {
    for (group, label) in [
        ("characters", "人物"),
        ("items", "物品"),
        ("locations", "地点"),
    ] {
        merge_paren_duplicates_in_dir(&project_dir.join("entities").join(group), label, report);
    }
}

fn merge_paren_duplicates_in_dir(dir: &Path, label: &str, report: &mut MigrateReport) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let names: Vec<String> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("md") {
                return None;
            }
            p.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .collect();
    for name in &names {
        if !name.contains('(') {
            continue;
        }
        let full = name.replace('(', "（").replace(')', "）");
        if full == *name || !names.contains(&full) {
            continue;
        }
        let half_path = dir.join(format!("{name}.md"));
        let full_path = dir.join(format!("{full}.md"));
        let Ok(half) = fs::read_to_string(&half_path) else {
            continue;
        };
        let Ok(full_text) = fs::read_to_string(&full_path) else {
            continue;
        };
        // Keep the longer body; prepend note about merge.
        let keep = if half.len() >= full_text.len() {
            half
        } else {
            full_text
        };
        let _ = fs::write(&full_path, keep);
        let _ = fs::remove_file(&half_path);
        report
            .notes
            .push(format!("合并重复{label}卡：{name}.md → {full}.md"));
    }

    // Swapped paren aliases / bare vs `正式名（别名）`: 旧钥 vs 旧钥（入门引导）
    let names: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension().and_then(|s| s.to_str()) == Some("md"))
                .then(|| p.file_stem()?.to_str().map(|s| s.to_string()))
                .flatten()
        })
        .collect();
    let mut removed = std::collections::HashSet::new();
    for i in 0..names.len() {
        if removed.contains(&names[i]) {
            continue;
        }
        for j in (i + 1)..names.len() {
            if removed.contains(&names[j]) {
                continue;
            }
            if !crate::cards::entity_names_equivalent(&names[i], &names[j]) {
                continue;
            }
            let a_path = dir.join(format!("{}.md", names[i]));
            let b_path = dir.join(format!("{}.md", names[j]));
            let Ok(a_text) = fs::read_to_string(&a_path) else {
                continue;
            };
            let Ok(b_text) = fs::read_to_string(&b_path) else {
                continue;
            };
            let (keep_path, drop_path, keep_name, drop_name) = if a_text.len() >= b_text.len() {
                (&a_path, &b_path, &names[i], &names[j])
            } else {
                (&b_path, &a_path, &names[j], &names[i])
            };
            let keep = if a_text.len() >= b_text.len() {
                a_text
            } else {
                b_text
            };
            let _ = fs::write(keep_path, keep);
            let _ = fs::remove_file(drop_path);
            removed.insert(drop_name.clone());
            report.notes.push(format!(
                "合并别名重复{label}卡：{drop_name}.md → {keep_name}.md"
            ));
        }
    }
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
        if !json_path.is_file() {
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
            continue;
        }
        // Pad missing items/locations for legacy JSON.
        let Ok(raw) = fs::read_to_string(&json_path) else {
            continue;
        };
        let Ok(mut v) = serde_json::from_str::<Value>(&raw) else {
            report
                .outlines_failed
                .push((n, "outline.json 解析失败".into()));
            continue;
        };
        let Some(obj) = v.as_object_mut() else {
            continue;
        };
        let mut changed = false;
        for key in ["items", "locations"] {
            if !obj.contains_key(key) {
                obj.insert(key.to_string(), json!([]));
                changed = true;
            }
        }
        if changed {
            if let Ok(pretty) = serde_json::to_string_pretty(&v) {
                let _ = fs::write(&json_path, pretty);
                report.outlines_padded.push(n);
            }
        }
    }
    report.outlines_migrated.sort_unstable();
    report.outlines_padded.sort_unstable();
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
            let (folded, did_fold) = fold_sync_stubs_into_current_status(&text);
            if did_fold {
                report
                    .entities_stub_folded
                    .push(format!("{group}/{name}"));
            }
            let before = folded.clone();
            let padded = minimal_entity_skeleton(kind, name, &folded);
            // Prefer validate path when possible (normalizes H2 aliases).
            let final_text = match validate_entity_card(kind, &padded) {
                Ok(norm) => norm,
                Err(_) => padded,
            };
            if final_text != text {
                let _ = fs::write(&path, &final_text);
                if final_text != before || did_fold {
                    report
                        .entities_padded
                        .push(format!("{group}/{name}"));
                }
            }
        }
    }
    report.entities_padded.sort();
    report.entities_padded.dedup();
    report.entities_stub_folded.sort();
}

const SYNC_STUB_TITLES: &[&str] = &[
    "卷末同步摘要",
    "章后同步摘要",
    "剧情同步摘要",
];

/// Move sync-stub sections into `## Current status` and remove stub headings.
fn fold_sync_stubs_into_current_status(text: &str) -> (String, bool) {
    let (meta, body) = split_fm(text);
    let lines: Vec<&str> = body.lines().collect();
    let mut out_lines: Vec<String> = Vec::new();
    let mut stub_chunks: Vec<String> = Vec::new();
    let mut i = 0;
    let mut folded = false;
    while i < lines.len() {
        let t = lines[i].trim();
        if let Some(rest) = t.strip_prefix("## ") {
            let title = rest.trim();
            if SYNC_STUB_TITLES.iter().any(|s| title == *s) {
                folded = true;
                i += 1;
                let mut chunk = Vec::new();
                while i < lines.len() {
                    let l = lines[i];
                    let lt = l.trim();
                    if lt.starts_with("## ") {
                        break;
                    }
                    if lt.starts_with("### ") {
                        // drop "### 更新" label; keep content lines after
                        i += 1;
                        continue;
                    }
                    if !lt.is_empty() {
                        chunk.push(lt.to_string());
                    }
                    i += 1;
                }
                if !chunk.is_empty() {
                    stub_chunks.push(chunk.join("\n"));
                }
                continue;
            }
        }
        out_lines.push(lines[i].to_string());
        i += 1;
    }

    if !folded {
        return (text.to_string(), false);
    }

    let mut body = out_lines.join("\n");
    let status_blob = stub_chunks.join("\n\n");
    if !status_blob.trim().is_empty() {
        if has_h2(
            &body,
            &[
                "Current status",
                "Current Status",
                "当前状态",
                "现状",
            ],
        ) {
            // Append under existing section end — simple: append note block.
            body = format!(
                "{}\n\n### 同步终态\n\n{}\n",
                body.trim_end(),
                status_blob.trim()
            );
        } else {
            body = format!(
                "{}\n\n## Current status\n\n{}\n",
                body.trim_end(),
                status_blob.trim()
            );
        }
    }

    // Ensure status in FM.
    let mut meta = meta;
    if !meta.contains_key("status") {
        meta.insert("status".into(), "active".into());
    }
    (join_fm(&meta, body.trim()), true)
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

/// Restructure bible to required H1 + ## 0/1/2/7 without inventing new lore.
fn migrate_bible(project_dir: &Path, report: &mut MigrateReport) {
    let path = project_dir.join("artifacts/bible.md");
    let Ok(text) = fs::read_to_string(&path) else {
        report.notes.push("bible.md 不存在".into());
        return;
    };
    if validate_bible(&text).is_ok() {
        return;
    }

    let sections = split_markdown_h2_sections(&text);
    let mut by_topic: HashMap<&'static str, Vec<String>> = HashMap::new();
    let mut genre_line = String::new();
    let mut preamble = Vec::new();

    for (title, body) in &sections {
        let t = title.trim();
        let b = body.trim();
        if t.is_empty() {
            for line in b.lines() {
                let l = line.trim();
                if l.starts_with("题材：") || l.starts_with("题材:") {
                    genre_line = l.to_string();
                } else if !l.is_empty() && !l.starts_with('#') {
                    preamble.push(l.to_string());
                }
            }
            continue;
        }
        let key = classify_bible_topic(t);
        by_topic.entry(key).or_default().push(format!("### {t}\n\n{b}"));
    }

    let one_liner = if !genre_line.is_empty() {
        genre_line
            .trim_start_matches("题材：")
            .trim_start_matches("题材:")
            .trim()
            .to_string()
    } else if !preamble.is_empty() {
        preamble[0].clone()
    } else {
        "（待补：一句话世界）".into()
    };

    let join = |key: &str, fallback: &str| -> String {
        by_topic
            .get(key)
            .map(|v| v.join("\n\n"))
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| fallback.to_string())
    };

    let rebuilt = format!(
        "# 世界观 Bible\n\n\
## 0. 一句话世界\n\n{one_liner}\n\n\
## 1. 时代与叙事框架\n\n{}\n\n\
## 2. 全局势力与阵营\n\n{}\n\n\
## 3. 规则 / 能力 / 职业体系（若有）\n\n{}\n\n\
## 4. 关键地理索引（一句话/地，细节进地点卡）\n\n{}\n\n\
## 5. 历史与传说\n\n{}\n\n\
## 6. 硬性禁忌\n\n{}\n\n\
## 7. 开放问题\n\n{}\n",
        join("era", "（待从正文/卷纲补全时代与叙事框架）"),
        join("factions", "（待补：全局势力与阵营）"),
        join("rules", "（待补：规则与代价；无超自然则写明「无」）"),
        join("geo", "（待补：关键地理索引）"),
        join("history", "（待补：历史与传说）"),
        join("taboo", "（待从正文提炼硬性禁忌）"),
        join("open", "（待揭开放问题）"),
    );

    match validate_bible(&rebuilt) {
        Ok(norm) => {
            let _ = fs::write(&path, norm);
            report.bible_rewritten = true;
            report
                .notes
                .push("bible.md 已按必填节重排（内容来自原同步段落，未新造设定）".into());
        }
        Err(e) => {
            report.notes.push(format!(
                "bible.md 重排后仍未通过校验：{} — 请手工补齐",
                e.message
            ));
        }
    }
}

fn classify_bible_topic(title: &str) -> &'static str {
    let t = title;
    if t.contains("秘书处") || t.contains("势力") || t.contains("稽察") {
        return "factions";
    }
    if t.contains("碎片")
        || t.contains("路径")
        || t.contains("规则")
        || t.contains("锚点")
        || t.contains("裂缝")
        || t.contains("倒计时")
    {
        return "rules";
    }
    if t.contains("锁龙井")
        || t.contains("地下")
        || t.contains("文津")
        || t.contains("投影")
        || t.contains("地点")
        || t.contains("遗址")
        || t.contains("心室")
        || t.contains("子宫")
    {
        return "geo";
    }
    if t.contains("顾维钧") || t.contains("历史") || t.contains("死亡") || t.contains("传说") {
        return "history";
    }
    if t.contains("禁忌") {
        return "taboo";
    }
    if t.contains("开放") || t.contains("待揭") {
        return "open";
    }
    "era"
}

/// Split into (h2_title, body). Preamble uses empty title.
fn split_markdown_h2_sections(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cur_title = String::new();
    let mut cur_body = String::new();
    for line in text.lines() {
        let t = line.trim_end();
        if let Some(rest) = t.trim_start().strip_prefix("## ") {
            if !cur_title.is_empty() || !cur_body.trim().is_empty() {
                out.push((cur_title.clone(), cur_body.clone()));
            }
            cur_title = rest.trim().to_string();
            cur_body.clear();
        } else if t.trim_start().starts_with("# ") && !t.trim_start().starts_with("## ") {
            // skip H1 in body accumulation for topic split; keep as preamble marker
            if cur_title.is_empty() {
                continue;
            }
            cur_body.push_str(line);
            cur_body.push('\n');
        } else {
            cur_body.push_str(line);
            cur_body.push('\n');
        }
    }
    if !cur_title.is_empty() || !cur_body.trim().is_empty() {
        out.push((cur_title, cur_body));
    }
    out
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

    #[test]
    fn folds_volume_sync_stub_into_current_status() {
        let raw = r#"---
name: 甲
status: active
---

# 甲

## 卷末同步摘要

旧摘要。

### 更新
终态备注。
"#;
        let (out, folded) = fold_sync_stubs_into_current_status(raw);
        assert!(folded);
        assert!(!out.contains("卷末同步摘要"));
        assert!(out.contains("## Current status") || out.contains("## 当前状态"));
        assert!(out.contains("终态备注"));
    }
}

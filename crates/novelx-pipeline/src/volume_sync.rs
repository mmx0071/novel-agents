//! End-of-volume batch sync: entities, nomenclature, bible, arc progress.
//! Does not write plots or relation graphs.

use crate::memory::load_memory;
use crate::volume::{sync_act_chapter_bounds, volume_chapter_span, VolumeBound};
use anyhow::Result;
use novelx_llm::LlmClient;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::run::PipelineEvent;

const VOLUME_SYNC_SYSTEM: &str = r#"你是小说设定库卷末「状态」同步员。根据本卷各章摘要与现有设定，输出 JSON（不要 Markdown 围栏），直接写入实体终态。
字段：
{
  "entities":[{"kind":"character|item|location","name":"","status":"active|background|exited|consumed","holdings":"物品名逗号分隔，人物可选","note":"可选，40字内状态备注"}],
  "nomenclature":[{"canonical_name":"","category":"","aliases":[],"ability_or_trait":""}],
  "bible_patches":[{"topic":"","content":"补丁段落"}],
  "arc_progress":"本卷完成进度说明（写入 progress_notes）",
  "master_revision":"对总纲后续卷目标/悬念的修订要点（1–3句；无则空字符串）"
}
规则：
- 禁止输出 plots / 剧情卡：剧情卡只由 design_plot / 写作流水线管理，卷末同步不得新建或改写剧情卡。
- 禁止输出 relation_graph / 关系图谱。
- **实体以 status / holdings 为准**直接同步终态；不要写长篇摘要。可选 note 仅作一句备注。
- 兼容旧字段 summary：若出现则当作 note。
- 必须覆盖本卷终态：主角与主要配角、关键地点/物品均须给出 status（及人物 holdings）；勿因「已有卡片」而输出空 entities。
- status：人物/地点用 active|background|exited；物品用 active|consumed。已退场角色、已消耗物品必须标 exited/consumed。
- holdings：人物当前持有关键物品（规范名，逗号分隔）；无则 ""。
- master_revision：本卷兑现后，全书后续卷需要怎么改（目标偏移/新悬念）；只写要点，勿重写全文。
- 人物名遵守禁名与已有命名。
- 只输出 JSON，不要解释。"#;

#[derive(Debug, Clone)]
pub struct VolumeSyncReport {
    pub volume_index: u32,
    pub entities_upserted: usize,
    pub plots_written: usize,
    pub nomenclature_added: usize,
    pub bible_patches: usize,
    /// Entity names written/updated as short stubs (need design_entity / Web).
    pub incomplete_entities: Vec<String>,
    pub message: String,
}

pub async fn run_volume_sync(
    project_dir: &Path,
    volume: &VolumeBound,
    llm: Arc<LlmClient>,
    tx: Option<mpsc::UnboundedSender<PipelineEvent>>,
) -> Result<VolumeSyncReport> {
    let emit = |tx: &Option<mpsc::UnboundedSender<PipelineEvent>>, msg: String| {
        if let Some(t) = tx.as_ref() {
            let _ = t.send(PipelineEvent::LlmDelta {
                agent: "volume_sync".into(),
                delta: format!("{msg}\n"),
            });
        }
    };
    let (span_start, span_end) = volume_chapter_span(volume, volume.end_chapter.max(1));
    emit(
        &tx,
        format!(
            "（卷末同步：第{}卷「{}」第{}–{}章…）",
            volume.volume_index, volume.name, span_start, span_end
        ),
    );

    let pack = gather_volume_context(project_dir, volume)?;
    let user = format!(
        "项目目录设定同步。卷：第{}卷 {}\n章范围：{}–{}\n终止条件：{}\n\n# 本卷摘要\n{}\n\n# 现有设定节选\n{}\n\n只输出 JSON。",
        volume.volume_index,
        volume.name,
        span_start,
        span_end,
        volume.ending_conditions.join("；"),
        pack.summaries,
        pack.existing_excerpt
    );

    // analysis 默认 4096 会截断整卷 JSON → 解析失败 → 全 0 写入；卷末同步单独抬高上限。
    const VOLUME_SYNC_MAX_TOKENS: u32 = 32768;
    let model = llm.model_for_agent("lore_librarian");
    let raw = match llm
        .complete_limited(
            VOLUME_SYNC_SYSTEM,
            &user,
            Some(&model),
            Some(VOLUME_SYNC_MAX_TOKENS),
        )
        .await
    {
        Ok(s) if !s.trim().is_empty() => s,
        _ => {
            llm.complete_limited(
                VOLUME_SYNC_SYSTEM,
                &user,
                Some(&llm.model_for_agent("summarizer")),
                Some(VOLUME_SYNC_MAX_TOKENS),
            )
            .await?
        }
    };

    let v = extract_json_value(&raw).unwrap_or_else(|| {
        tracing::warn!(
            volume = volume.volume_index,
            raw_chars = raw.chars().count(),
            "volume sync JSON parse failed; using empty object"
        );
        json!({})
    });
    let mut report =
        apply_sync_json(project_dir, volume, &v, SyncApplyMode::VolumeEnd)?;
    // Empty apply usually means truncated / invalid JSON or empty arrays — one forced retry.
    if report.entities_upserted + report.bible_patches + report.nomenclature_added == 0 {
        tracing::warn!(
            volume = volume.volume_index,
            raw_preview = %raw.chars().take(400).collect::<String>(),
            "volume sync wrote nothing; retrying with stricter prompt"
        );
        let retry_user = format!(
            "{user}\n\n上次输出未能写入设定库（常为 JSON 被截断或非法）。请重新输出完整合法 JSON：entities 至少 5 条（每条含 name+status，人物尽量含 holdings）、bible_patches 至少 1 条、nomenclature 含本卷新词。note≤40字。不要输出 relation_graph。"
        );
        let raw2 = llm
            .complete_limited(
                VOLUME_SYNC_SYSTEM,
                &retry_user,
                Some(&model),
                Some(VOLUME_SYNC_MAX_TOKENS),
            )
            .await
            .unwrap_or_default();
        if let Some(v2) = extract_json_value(&raw2) {
            report = apply_sync_json(project_dir, volume, &v2, SyncApplyMode::VolumeEnd)?;
        } else {
            tracing::warn!(
                volume = volume.volume_index,
                raw_chars = raw2.chars().count(),
                "volume sync retry JSON parse failed"
            );
        }
    }
    let _ = sync_act_chapter_bounds(project_dir, volume);
    emit(&tx, format!("（卷末同步完成：{}）", report.message));
    Ok(report)
}

const PLOT_SYNC_SYSTEM: &str = r#"你是小说设定库「剧情收束」同步员。根据刚完成的剧情卡与近章摘要，输出 JSON（不要 Markdown 围栏），增量更新设定库。
字段（禁止 arc_progress / master_revision — 那是卷末专用）：
{
  "entities":[{"kind":"character|item|location","name":"","summary":"80字内要点","status":"active|background|exited|consumed","holdings":""}],
  "nomenclature":[{"canonical_name":"","category":"","aliases":[],"ability_or_trait":""}],
  "bible_patches":[{"topic":"","content":"补丁段落"}]
}
规则：
- 禁止输出 plots / relation_graph / arc_progress / master_revision。
- 只写本剧情期间确实变化的实体/名词/世界观补丁；无变化可少写，勿编造。
- 实体 summary 写收束后的最新状态；新建卡会标为 stub 待补全。
- 只输出 JSON。"#;

const CHAPTER_SYNC_SYSTEM: &str = r#"你是小说设定库「章后」同步员。根据刚发布的本章摘要，输出 JSON（不要 Markdown 围栏），增量更新设定库。
字段（禁止 arc_progress / master_revision / plots / relation_graph）：
{
  "entities":[{"kind":"character|item|location","name":"","summary":"60字内要点","status":"active|background|exited|consumed","holdings":""}],
  "nomenclature":[{"canonical_name":"","category":"","aliases":[],"ability_or_trait":""}],
  "bible_patches":[{"topic":"","content":"补丁段落"}]
}
规则：
- 只写本章正文/摘要里**确实发生变化**的实体状态（伤势、持有物、位置、退场/消耗等）；无变化则 entities 可为空数组。
- status / holdings 必须反映本章结束后的最新态，供下一章设定卡读取。
- 勿编造未出现的人物/物品；新建卡会标为 stub 待补全。
- 只输出 JSON。"#;

/// Light setting sync after each published chapter (when the active plot did not complete).
/// Does **not** change `volume_phase`. Keeps entity status/holdings fresh for the next chapter.
pub async fn run_chapter_sync(
    project_dir: &Path,
    chapter: u32,
    llm: Arc<LlmClient>,
    tx: Option<mpsc::UnboundedSender<PipelineEvent>>,
) -> Result<VolumeSyncReport> {
    let emit = |tx: &Option<mpsc::UnboundedSender<PipelineEvent>>, msg: String| {
        if let Some(t) = tx.as_ref() {
            let _ = t.send(PipelineEvent::LlmDelta {
                agent: "chapter_sync".into(),
                delta: format!("{msg}\n"),
            });
        }
    };
    let volume = crate::volume::active_volume_for_chapter(project_dir, chapter).unwrap_or_else(|| {
        crate::volume::default_bound(1)
    });
    emit(&tx, format!("（章后同步：第{chapter}章设定…）"));

    let pack = gather_chapter_range_context(project_dir, chapter, chapter, "")?;
    let user = format!(
        "第{chapter}章刚发布。卷：第{}卷 {}\n\n# 本章摘要\n{}\n\n# 现有设定节选\n{}\n\n只输出 JSON。",
        volume.volume_index, volume.name, pack.summaries, pack.existing_excerpt
    );

    const CHAPTER_SYNC_MAX_TOKENS: u32 = 8192;
    let model = llm.model_for_agent("lore_librarian");
    let raw = match llm
        .complete_limited(
            CHAPTER_SYNC_SYSTEM,
            &user,
            Some(&model),
            Some(CHAPTER_SYNC_MAX_TOKENS),
        )
        .await
    {
        Ok(s) if !s.trim().is_empty() => s,
        _ => llm
            .complete_limited(
                CHAPTER_SYNC_SYSTEM,
                &user,
                Some(&llm.model_for_agent("summarizer")),
                Some(CHAPTER_SYNC_MAX_TOKENS),
            )
            .await
            .unwrap_or_default(),
    };

    let mut v = extract_json_value(&raw).unwrap_or_else(|| json!({}));
    tag_entities_chapter_sync(&mut v);
    if let Some(obj) = v.as_object_mut() {
        obj.remove("arc_progress");
        obj.remove("master_revision");
    }
    let mut report =
        apply_sync_json(project_dir, &volume, &v, SyncApplyMode::PlotLight)?;
    report.message = format!("第{chapter}章：{}", report.message);
    emit(&tx, format!("（章后同步完成：{}）", report.message));
    Ok(report)
}

/// Light setting sync after a plot card completes. Does **not** change `volume_phase`.
pub async fn run_plot_sync(
    project_dir: &Path,
    plot_title: &str,
    chapter: u32,
    llm: Arc<LlmClient>,
    tx: Option<mpsc::UnboundedSender<PipelineEvent>>,
) -> Result<VolumeSyncReport> {
    let emit = |tx: &Option<mpsc::UnboundedSender<PipelineEvent>>, msg: String| {
        if let Some(t) = tx.as_ref() {
            let _ = t.send(PipelineEvent::LlmDelta {
                agent: "plot_sync".into(),
                delta: format!("{msg}\n"),
            });
        }
    };
    let volume = crate::volume::active_volume_for_chapter(project_dir, chapter).unwrap_or_else(|| {
        crate::volume::default_bound(1)
    });
    let span_start = {
        let vol_start = if volume.start_chapter == 0 {
            1
        } else {
            volume.start_chapter
        };
        chapter.saturating_sub(6).max(vol_start).max(1)
    };
    emit(
        &tx,
        format!("（剧情同步：「{plot_title}」第{span_start}–{chapter}章…）"),
    );

    let pack = gather_chapter_range_context(project_dir, span_start, chapter, plot_title)?;
    let user = format!(
        "剧情卡「{plot_title}」刚收束。卷：第{}卷 {}\n章范围：{span_start}–{chapter}\n\n# 近章摘要与剧情卡\n{}\n\n# 现有设定节选\n{}\n\n只输出 JSON。",
        volume.volume_index, volume.name, pack.summaries, pack.existing_excerpt
    );

    const PLOT_SYNC_MAX_TOKENS: u32 = 16384;
    let model = llm.model_for_agent("lore_librarian");
    let raw = match llm
        .complete_limited(
            PLOT_SYNC_SYSTEM,
            &user,
            Some(&model),
            Some(PLOT_SYNC_MAX_TOKENS),
        )
        .await
    {
        Ok(s) if !s.trim().is_empty() => s,
        _ => llm
            .complete_limited(
                PLOT_SYNC_SYSTEM,
                &user,
                Some(&llm.model_for_agent("summarizer")),
                Some(PLOT_SYNC_MAX_TOKENS),
            )
            .await
            .unwrap_or_default(),
    };

    let mut v = extract_json_value(&raw).unwrap_or_else(|| json!({}));
    tag_entities_plot_sync(&mut v);
    // Never write volume-end progress / master revision from plot-scoped sync.
    if let Some(obj) = v.as_object_mut() {
        obj.remove("arc_progress");
        obj.remove("master_revision");
    }
    let mut report =
        apply_sync_json(project_dir, &volume, &v, SyncApplyMode::PlotLight)?;
    report.message = format!("剧情「{plot_title}」：{}", report.message);
    emit(&tx, format!("（剧情同步完成：{}）", report.message));
    Ok(report)
}

fn tag_entities_plot_sync(v: &mut Value) {
    if let Some(arr) = v.get_mut("entities").and_then(|x| x.as_array_mut()) {
        for ent in arr {
            if let Some(obj) = ent.as_object_mut() {
                obj.insert("source".into(), Value::String("plot_sync".into()));
            }
        }
    }
}

fn tag_entities_chapter_sync(v: &mut Value) {
    if let Some(arr) = v.get_mut("entities").and_then(|x| x.as_array_mut()) {
        for ent in arr {
            if let Some(obj) = ent.as_object_mut() {
                obj.insert("source".into(), Value::String("chapter_sync".into()));
            }
        }
    }
}

fn gather_chapter_range_context(
    project_dir: &Path,
    from: u32,
    to: u32,
    plot_title: &str,
) -> Result<VolumeContextPack> {
    let mut summary_parts = Vec::new();
    for ch in from..=to {
        let path = project_dir
            .join("chapters")
            .join(format!("{ch:03}"))
            .join("summary.json");
        if let Ok(text) = std::fs::read_to_string(&path) {
            let excerpt: String = text.chars().take(1200).collect();
            summary_parts.push(format!("## 第{ch}章\n{excerpt}"));
        }
    }
    // Attach matching plot card when a title is provided.
    if !plot_title.trim().is_empty() {
        let plots_dir = project_dir.join("plots");
        if let Ok(rd) = std::fs::read_dir(&plots_dir) {
            for e in rd.flatten() {
                let path = e.path();
                if path.extension().and_then(|s| s.to_str()) != Some("md") {
                    continue;
                }
                if let Ok(text) = std::fs::read_to_string(&path) {
                    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    if stem == plot_title
                        || text.contains(&format!("title: {plot_title}"))
                        || text.contains(&format!("# {plot_title}"))
                    {
                        summary_parts.push(format!(
                            "## 剧情卡「{plot_title}」\n{}",
                            text.chars().take(2000).collect::<String>()
                        ));
                        break;
                    }
                }
            }
        }
    }
    if summary_parts.is_empty() {
        summary_parts.push("（尚无 summary.json，请依据剧情卡与现有设定做最小同步）".into());
    }

    let mem = load_memory(project_dir);
    let mem_excerpt: String = serde_json::to_string_pretty(&mem)
        .unwrap_or_default()
        .chars()
        .take(2000)
        .collect();
    let bible = read_first_existing(project_dir, &["artifacts/bible.md"]);
    let bible_excerpt: String = bible.chars().take(1800).collect();
    let nom = std::fs::read_to_string(project_dir.join("lore/nomenclature.json"))
        .or_else(|_| std::fs::read_to_string(project_dir.join("artifacts/nomenclature.md")))
        .unwrap_or_default();
    let nom_excerpt: String = nom.chars().take(1200).collect();
    let entity_list = list_entity_names(project_dir);
    let existing_excerpt = format!(
        "## Memory\n{mem_excerpt}\n\n## Bible\n{bible_excerpt}\n\n## Nomenclature\n{nom_excerpt}\n\n## Entities\n{}",
        entity_list.join("、")
    );
    Ok(VolumeContextPack {
        summaries: summary_parts.join("\n\n"),
        existing_excerpt,
    })
}

fn max_existing_chapter(project_dir: &Path) -> u32 {
    let dir = project_dir.join("chapters");
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 1;
    };
    rd.flatten()
        .filter_map(|e| e.file_name().to_string_lossy().parse::<u32>().ok())
        .max()
        .unwrap_or(1)
}

struct VolumeContextPack {
    summaries: String,
    existing_excerpt: String,
}

fn gather_volume_context(project_dir: &Path, volume: &VolumeBound) -> Result<VolumeContextPack> {
    let mut summary_parts = Vec::new();
    let upto = if volume.end_chapter > 0 {
        volume.end_chapter
    } else {
        max_existing_chapter(project_dir).max(1)
    };
    let (from, to) = volume_chapter_span(volume, upto);
    for ch in from..=to {
        let path = project_dir
            .join("chapters")
            .join(format!("{ch:03}"))
            .join("summary.json");
        if let Ok(text) = std::fs::read_to_string(&path) {
            let excerpt: String = text.chars().take(1200).collect();
            summary_parts.push(format!("## 第{ch}章\n{excerpt}"));
        }
    }
    if summary_parts.is_empty() {
        summary_parts.push("（本卷尚无 summary.json，请依据现有设定做最小同步）".into());
    }

    let mem = load_memory(project_dir);
    let mem_excerpt: String = serde_json::to_string_pretty(&mem)
        .unwrap_or_default()
        .chars()
        .take(2500)
        .collect();

    // Prefer bible only — legacy world_architect may diverge after volume syncs.
    let bible = read_first_existing(project_dir, &["artifacts/bible.md"]);
    let bible_excerpt: String = bible.chars().take(2000).collect();

    let nom = std::fs::read_to_string(project_dir.join("lore/nomenclature.json"))
        .or_else(|_| std::fs::read_to_string(project_dir.join("artifacts/nomenclature.md")))
        .unwrap_or_default();
    let nom_excerpt: String = nom.chars().take(1500).collect();

    let entity_list = list_entity_names(project_dir);

    let existing_excerpt = format!(
        "## Memory\n{mem_excerpt}\n\n## Bible\n{bible_excerpt}\n\n## Nomenclature\n{nom_excerpt}\n\n## Entities\n{}",
        entity_list.join("、")
    );

    Ok(VolumeContextPack {
        summaries: summary_parts.join("\n\n"),
        existing_excerpt,
    })
}

fn list_entity_names(project_dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    for group in ["characters", "items", "locations"] {
        let dir = project_dir.join("entities").join(group);
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n.ends_with(".md") {
                    names.push(n.trim_end_matches(".md").to_string());
                }
            }
        }
    }
    names
}

fn read_first_existing(project_dir: &Path, rels: &[&str]) -> String {
    for r in rels {
        let p = project_dir.join(r);
        if let Ok(t) = std::fs::read_to_string(&p) {
            if !t.trim().is_empty() {
                return t;
            }
        }
    }
    String::new()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncApplyMode {
    /// Full volume-end sync: may write arc progress / master revision.
    VolumeEnd,
    /// Post-plot light sync: entities / nomenclature / bible only — never volume-end notes.
    PlotLight,
}

/// Apply volume-end sync JSON (entities, nomenclature, bible, arc progress).
pub fn apply_volume_sync_json(
    project_dir: &Path,
    volume: &VolumeBound,
    v: &Value,
) -> Result<VolumeSyncReport> {
    apply_sync_json(project_dir, volume, v, SyncApplyMode::VolumeEnd)
}

fn apply_sync_json(
    project_dir: &Path,
    volume: &VolumeBound,
    v: &Value,
    mode: SyncApplyMode,
) -> Result<VolumeSyncReport> {
    let mut entities_upserted = 0usize;
    let mut nomenclature_added = 0usize;
    let mut bible_patches = 0usize;
    // Sync must not invent plot cards (managed by design_plot / pipeline).
    let plots_written = 0usize;
    let mut incomplete_entities = Vec::new();

    if let Some(arr) = v.get("entities").and_then(|x| x.as_array()) {
        for ent in arr {
            match upsert_entity_card(project_dir, ent, mode)? {
                Some(UpsertEntityOutcome { name, incomplete }) => {
                    entities_upserted += 1;
                    if incomplete {
                        incomplete_entities.push(name);
                    }
                }
                None => {}
            }
        }
    }

    if let Some(arr) = v.get("nomenclature").and_then(|x| x.as_array()) {
        nomenclature_added = merge_nomenclature(project_dir, arr)?;
    }

    if let Some(arr) = v.get("bible_patches").and_then(|x| x.as_array()) {
        for patch in arr {
            if append_bible_patch(project_dir, volume, patch, mode)? {
                bible_patches += 1;
            }
        }
    }

    if mode == SyncApplyMode::VolumeEnd {
        if let Some(progress) = v.get("arc_progress").and_then(|x| x.as_str()) {
            let revision = v
                .get("master_revision")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let _ = write_arc_progress(project_dir, volume, progress, revision);
        }
    }

    let stub_hint = if incomplete_entities.is_empty() {
        String::new()
    } else {
        format!(
            "；待补全实体 {} 个（请 design_entity 或 Web「设定缺口」）：{}",
            incomplete_entities.len(),
            incomplete_entities
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join("、")
        )
    };
    let scope = match mode {
        SyncApplyMode::VolumeEnd => "卷末",
        SyncApplyMode::PlotLight => "剧情",
    };
    let message = format!(
        "第{}卷（{scope}）：实体{} 剧情卡{} 名词+{} Bible补丁{}{}",
        volume.volume_index,
        entities_upserted,
        plots_written,
        nomenclature_added,
        bible_patches,
        stub_hint
    );

    Ok(VolumeSyncReport {
        volume_index: volume.volume_index,
        entities_upserted,
        plots_written,
        nomenclature_added,
        bible_patches,
        incomplete_entities,
        message,
    })
}

struct UpsertEntityOutcome {
    name: String,
    /// True when a new stub card was created (needs design_entity / Web).
    incomplete: bool,
}

/// Upsert entity card from sync JSON.
/// - VolumeEnd: write status/holdings (+ optional note) into「当前状态」; no summary stubs.
/// - PlotLight: keep short sync notes for plot/chapter light sync.
fn upsert_entity_card(
    project_dir: &Path,
    ent: &Value,
    mode: SyncApplyMode,
) -> Result<Option<UpsertEntityOutcome>> {
    let kind = ent.get("kind").and_then(|x| x.as_str()).unwrap_or("character");
    let name = ent.get("name").and_then(|x| x.as_str()).unwrap_or("").trim();
    if name.is_empty() {
        return Ok(None);
    }
    let note = ent
        .get("note")
        .or_else(|| ent.get("summary"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let status_raw = ent.get("status").and_then(|x| x.as_str()).unwrap_or("");
    let holdings = ent
        .get("holdings")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let status = crate::cards::normalize_entity_status(status_raw);
    // Volume end may update status-only; light sync still wants a note/summary.
    let has_payload = !status_raw.trim().is_empty() || !holdings.is_empty() || !note.is_empty();
    if !has_payload {
        return Ok(None);
    }
    if mode == SyncApplyMode::PlotLight && note.is_empty() {
        return Ok(None);
    }

    let source = ent
        .get("source")
        .and_then(|x| x.as_str())
        .unwrap_or(match mode {
            SyncApplyMode::VolumeEnd => "volume_sync",
            SyncApplyMode::PlotLight => "plot_sync",
        });
    let source = match source {
        "plot_sync" => "plot_sync",
        "chapter_sync" => "chapter_sync",
        _ => "volume_sync",
    };
    let group = match kind {
        "item" => "items",
        "location" => "locations",
        _ => "characters",
    };
    let folder = project_dir.join("entities").join(group);
    std::fs::create_dir_all(&folder)?;
    let path = folder.join(format!("{name}.md"));
    let existed = path.exists();

    if mode == SyncApplyMode::VolumeEnd {
        if existed {
            let mut existing = std::fs::read_to_string(&path)?;
            existing = mark_frontmatter_sync_fields(
                &existing,
                &status,
                &holdings,
                source,
                false, // do not force complete:false on volume state sync
            );
            existing = upsert_current_state_section(&existing, &status, &holdings, &note);
            std::fs::write(&path, existing)?;
            return Ok(Some(UpsertEntityOutcome {
                name: name.to_string(),
                incomplete: false,
            }));
        }
        let hold_line = if holdings.is_empty() {
            String::new()
        } else {
            format!("holdings: {holdings}\n")
        };
        let state_body = format_current_state_section(&status, &holdings, &note);
        let body = format!(
            "---\nname: {name}\nkind: {kind}\ncomplete: false\nsource: {source}\nstatus: {status}\n{hold_line}---\n\n# {name}\n\n- 类型：{kind}\n\n{state_body}\n"
        );
        std::fs::write(&path, body)?;
        return Ok(Some(UpsertEntityOutcome {
            name: name.to_string(),
            incomplete: true,
        }));
    }

    // Plot / chapter light sync: short note section (legacy stub path).
    let section_h2 = match source {
        "plot_sync" => "剧情同步摘要",
        "chapter_sync" => "章后同步摘要",
        _ => "卷末同步摘要",
    };
    if existed {
        let mut existing = std::fs::read_to_string(&path)?;
        existing = mark_frontmatter_sync_fields(&existing, &status, &holdings, source, true);
        if existing.contains(&format!("## {section_h2}"))
            || existing.contains("## 卷末同步摘要")
            || existing.contains("## 剧情同步摘要")
            || existing.contains("## 章后同步摘要")
        {
            existing.push_str(&format!("\n### 更新\n{note}\n"));
        } else {
            existing.push_str(&format!("\n\n## {section_h2}\n\n{note}\n"));
        }
        std::fs::write(&path, existing)?;
        return Ok(Some(UpsertEntityOutcome {
            name: name.to_string(),
            incomplete: false,
        }));
    }
    let hold_line = if holdings.is_empty() {
        String::new()
    } else {
        format!("holdings: {holdings}\n")
    };
    let body = format!(
        "---\nname: {name}\nkind: {kind}\ncomplete: false\nsource: {source}\nstatus: {status}\n{hold_line}---\n\n# {name}\n\n- 类型：{kind}\n\n## {section_h2}\n\n{note}\n"
    );
    std::fs::write(&path, body)?;
    Ok(Some(UpsertEntityOutcome {
        name: name.to_string(),
        incomplete: true,
    }))
}

fn format_current_state_section(status: &str, holdings: &str, note: &str) -> String {
    let mut lines = vec!["## 当前状态".into(), String::new()];
    if !status.is_empty() {
        lines.push(format!("- 状态：{status}"));
    }
    if !holdings.is_empty() {
        lines.push(format!("- 持有：{holdings}"));
    }
    if !note.is_empty() {
        lines.push(format!("- 备注：{note}"));
    }
    lines.join("\n")
}

/// Replace or append `## 当前状态` (does not accumulate sync summary stubs).
fn upsert_current_state_section(text: &str, status: &str, holdings: &str, note: &str) -> String {
    let section = format_current_state_section(status, holdings, note);
    let (fm, body) = split_frontmatter_body(text);
    let body = body.trim_start_matches('\n');
    let replaced = replace_h2_section(body, "当前状态", &section);
    if let Some(new_body) = replaced {
        return join_frontmatter_body(&fm, &new_body);
    }
    let mut new_body = body.to_string();
    if !new_body.is_empty() && !new_body.ends_with('\n') {
        new_body.push('\n');
    }
    new_body.push('\n');
    new_body.push_str(&section);
    new_body.push('\n');
    join_frontmatter_body(&fm, &new_body)
}

fn split_frontmatter_body(text: &str) -> (Option<String>, String) {
    let trimmed = text.trim_start_matches('\u{feff}');
    if !trimmed.starts_with("---") {
        return (None, text.to_string());
    }
    let rest = &trimmed[3..];
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    if let Some(end) = rest.find("\n---") {
        let yaml = rest[..end].to_string();
        let body = rest[end + 4..].to_string();
        return (Some(yaml), body);
    }
    (None, text.to_string())
}

fn join_frontmatter_body(fm: &Option<String>, body: &str) -> String {
    match fm {
        Some(yaml) => format!("---\n{yaml}\n---{}", body),
        None => body.to_string(),
    }
}

fn replace_h2_section(body: &str, title: &str, new_section: &str) -> Option<String> {
    let marker = format!("## {title}");
    let start = body.find(&marker)?;
    let after = start + marker.len();
    let rest = &body[after..];
    let end_rel = rest
        .find("\n## ")
        .map(|i| i)
        .unwrap_or(rest.len());
    let end = after + end_rel;
    let mut out = String::new();
    out.push_str(&body[..start]);
    out.push_str(new_section.trim_end());
    out.push('\n');
    let tail = body[end..].trim_start_matches('\n');
    if !tail.is_empty() {
        out.push('\n');
        out.push_str(tail);
    }
    Some(out)
}

/// Update YAML frontmatter lifecycle fields after a sync.
/// When `mark_incomplete`, force `complete: false` (light stub sync).
fn mark_frontmatter_sync_fields(
    text: &str,
    status: &str,
    holdings: &str,
    source: &str,
    mark_incomplete: bool,
) -> String {
    let source = match source {
        "plot_sync" => "plot_sync",
        "chapter_sync" => "chapter_sync",
        _ => "volume_sync",
    };
    let trimmed = text.trim_start_matches('\u{feff}');
    if !trimmed.starts_with("---") {
        let hold_line = if holdings.is_empty() {
            String::new()
        } else {
            format!("holdings: {holdings}\n")
        };
        let complete_line = if mark_incomplete {
            "complete: false\n"
        } else {
            ""
        };
        return format!(
            "---\n{complete_line}source: {source}\nstatus: {status}\n{hold_line}---\n\n{}",
            text.trim_start()
        );
    }
    let rest = &trimmed[3..];
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let Some(end) = rest.find("\n---") else {
        let hold_line = if holdings.is_empty() {
            String::new()
        } else {
            format!("holdings: {holdings}\n")
        };
        let complete_line = if mark_incomplete {
            "complete: false\n"
        } else {
            ""
        };
        return format!(
            "---\n{complete_line}source: {source}\nstatus: {status}\n{hold_line}---\n\n{text}"
        );
    };
    let yaml = &rest[..end];
    let body = &rest[end + 4..];
    let mut lines: Vec<String> = Vec::new();
    let mut saw_complete = false;
    let mut saw_source = false;
    let mut saw_status = false;
    let mut saw_holdings = false;
    for line in yaml.lines() {
        let t = line.trim();
        if t.starts_with("complete:") {
            if mark_incomplete {
                lines.push("complete: false".into());
            } else {
                lines.push(line.to_string());
            }
            saw_complete = true;
        } else if t.starts_with("source:") {
            lines.push(format!("source: {source}"));
            saw_source = true;
        } else if t.starts_with("status:") {
            lines.push(format!("status: {status}"));
            saw_status = true;
        } else if t.starts_with("holdings:") {
            if !holdings.is_empty() {
                lines.push(format!("holdings: {holdings}"));
            } else {
                lines.push(line.to_string());
            }
            saw_holdings = true;
        } else {
            lines.push(line.to_string());
        }
    }
    if mark_incomplete && !saw_complete {
        lines.push("complete: false".into());
    }
    if !saw_source {
        lines.push(format!("source: {source}"));
    }
    if !saw_status {
        lines.push(format!("status: {status}"));
    }
    if !saw_holdings && !holdings.is_empty() {
        lines.push(format!("holdings: {holdings}"));
    }
    format!("---\n{}\n---{}", lines.join("\n"), body)
}

fn merge_nomenclature(project_dir: &Path, items: &[Value]) -> Result<usize> {
    let path = project_dir.join("lore/nomenclature.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut root: Value = if path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&path)?).unwrap_or_else(|_| json!({}))
    } else {
        json!({"entities": []})
    };
    let arr = root
        .as_object_mut()
        .map(|o| {
            o.entry("entities")
                .or_insert_with(|| Value::Array(vec![]))
                .as_array_mut()
        })
        .flatten();
    let Some(arr) = arr else {
        return Ok(0);
    };
    let mut added = 0usize;
    for item in items {
        let name = item
            .get("canonical_name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim();
        if name.is_empty() {
            continue;
        }
        let exists = arr.iter().any(|e| {
            e.get("canonical_name").and_then(|x| x.as_str()) == Some(name)
                || e.get("name").and_then(|x| x.as_str()) == Some(name)
        });
        if exists {
            continue;
        }
        arr.push(json!({
            "canonical_name": name,
            "category": item.get("category").and_then(|x| x.as_str()).unwrap_or("concept"),
            "aliases": item.get("aliases").cloned().unwrap_or(json!([])),
            "ability_or_trait": item.get("ability_or_trait").and_then(|x| x.as_str()).unwrap_or(""),
            "source": "volume_sync",
        }));
        added += 1;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&root)?)?;
    Ok(added)
}

fn append_bible_patch(
    project_dir: &Path,
    volume: &VolumeBound,
    patch: &Value,
    mode: SyncApplyMode,
) -> Result<bool> {
    let default_topic = match mode {
        SyncApplyMode::VolumeEnd => "卷末补丁",
        SyncApplyMode::PlotLight => "剧情补丁",
    };
    let topic = patch
        .get("topic")
        .and_then(|x| x.as_str())
        .unwrap_or(default_topic);
    let content = patch.get("content").and_then(|x| x.as_str()).unwrap_or("").trim();
    if content.is_empty() {
        return Ok(false);
    }
    let art = project_dir.join("artifacts");
    std::fs::create_dir_all(&art)?;
    let path = art.join("bible.md");
    // Write bible only; seed empty file if missing (do not dual-write world_architect).
    let mut existing = if path.exists() {
        std::fs::read_to_string(&path).unwrap_or_default()
    } else {
        String::new()
    };
    let label = match mode {
        SyncApplyMode::VolumeEnd => format!("第{}卷同步", volume.volume_index),
        SyncApplyMode::PlotLight => format!("第{}卷·剧情同步", volume.volume_index),
    };
    let block = format!("\n\n## {topic}（{label}）\n\n{content}\n");
    existing.push_str(&block);
    std::fs::write(&path, &existing)?;
    Ok(true)
}

fn strip_volume_end_progress_notes(prev: &str) -> String {
    prev.lines()
        .filter(|line| !line.trim_start().starts_with("[卷末]"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn write_arc_progress(
    project_dir: &Path,
    volume: &VolumeBound,
    progress: &str,
    master_revision: &str,
) -> Result<()> {
    let path = project_dir.join("artifacts/story_outline.json");
    let mut root: Value = if path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&path)?)?
    } else {
        json!({"acts": [], "markdown": ""})
    };
    let (s, e) = volume_chapter_span(volume, volume.end_chapter.max(1));
    if let Some(acts) = root
        .as_object_mut()
        .and_then(|o| o.get_mut("acts"))
        .and_then(|a| a.as_array_mut())
    {
        for act in acts.iter_mut() {
            let vi = act
                .get("volume_index")
                .and_then(|x| x.as_u64())
                .unwrap_or(0) as u32;
            if vi == volume.volume_index {
                if let Some(obj) = act.as_object_mut() {
                    let prev = obj
                        .get("progress_notes")
                        .and_then(|x| x.as_str())
                        .unwrap_or("");
                    let base = strip_volume_end_progress_notes(prev);
                    let note = if base.is_empty() {
                        format!("[卷末] 第{s}–{e}章已完成：{}", progress.trim())
                    } else {
                        format!("{base}\n[卷末] 第{s}–{e}章已完成：{}", progress.trim())
                    };
                    obj.insert("progress_notes".into(), Value::String(note));
                    if volume.start_chapter > 0 {
                        obj.insert("start_chapter".into(), Value::from(volume.start_chapter));
                    }
                    if volume.end_chapter > 0 {
                        obj.insert("end_chapter".into(), Value::from(volume.end_chapter));
                    }
                    obj.insert("completed".into(), Value::Bool(true));
                }
                break;
            }
        }
    }
    // Replace (not append) the auto progress section on *this volume's* 卷纲.
    let arc_path = crate::volume::arc_outline_path(project_dir, volume.volume_index.max(1));
    if let Some(parent) = arc_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let t = std::fs::read_to_string(&arc_path)
        .or_else(|_| std::fs::read_to_string(project_dir.join("artifacts/arc_outline.md")))
        .unwrap_or_default();
    let block = format!(
        "## 卷末进度（自动）\n\n- 第{s}–{e}章已发布\n- {}\n",
        progress.trim()
    );
    let rewritten = replace_auto_section(&t, "## 卷末进度（自动）", &block);
    let _ = crate::volume::write_arc_outline_text(project_dir, volume.volume_index.max(1), &rewritten);
    let _ = append_master_outline_volume_revision(
        project_dir,
        volume.volume_index,
        s,
        e,
        progress,
        master_revision,
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&root)?)?;
    Ok(())
}

/// Append/replace an auto revision block on master_outline.md after a volume ends.
fn append_master_outline_volume_revision(
    project_dir: &Path,
    volume_index: u32,
    start_ch: u32,
    end_ch: u32,
    progress: &str,
    master_revision: &str,
) -> Result<()> {
    let path = project_dir.join("artifacts/master_outline.md");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let marker = format!("## 卷末修订（自动）·第{volume_index}卷");
    let rev = master_revision.trim();
    let rev_line = if rev.is_empty() {
        "- （本卷无额外总纲修订要点；后续卷目标以卷纲为准）".to_string()
    } else {
        format!("- 后续卷修订要点：{rev}")
    };
    let block = format!(
        "{marker}\n\n- 第{start_ch}–{end_ch}章已完成：{}\n{rev_line}\n",
        progress.trim()
    );
    let rewritten = replace_auto_section(&existing, &marker, &block);
    std::fs::write(&path, rewritten)?;
    Ok(())
}

fn replace_auto_section(markdown: &str, marker: &str, replacement: &str) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with(marker) {
            skipping = true;
            continue;
        }
        if skipping {
            if line.starts_with("## ") {
                skipping = false;
            } else {
                continue;
            }
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    out = out.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(replacement.trim_start());
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn strip_json_fence(raw: &str) -> &str {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = rest
            .strip_prefix("json")
            .or_else(|| rest.strip_prefix("JSON"))
            .unwrap_or(rest);
        let rest = rest.trim_start_matches('\n');
        return rest.strip_suffix("```").unwrap_or(rest).trim();
    }
    trimmed
}

fn extract_json_value(raw: &str) -> Option<Value> {
    let unfenced = strip_json_fence(raw);
    if let Ok(v) = serde_json::from_str::<Value>(unfenced) {
        return Some(v);
    }
    if let (Some(start), Some(end)) = (unfenced.find('{'), unfenced.rfind('}')) {
        if end > start {
            if let Ok(v) = serde_json::from_str::<Value>(&unfenced[start..=end]) {
                return Some(v);
            }
        }
    }
    // Model often truncates mid-object; salvage complete array items.
    salvage_volume_sync_json(unfenced)
}

/// Recover partial volume-sync payloads when the root JSON is truncated/invalid.
fn salvage_volume_sync_json(raw: &str) -> Option<Value> {
    let mut obj = serde_json::Map::new();
    let mut any = false;
    for key in ["entities", "nomenclature", "bible_patches"] {
        let items = extract_complete_json_objects_in_array(raw, key);
        if !items.is_empty() {
            obj.insert(key.to_string(), Value::Array(items));
            any = true;
        }
    }
    if let Some(progress) = extract_json_string_after_key(raw, "arc_progress") {
        obj.insert("arc_progress".into(), Value::String(progress));
        any = true;
    }
    any.then_some(Value::Object(obj))
}

fn find_key_value_start(raw: &str, key: &str) -> Option<usize> {
    let pat = format!("\"{key}\"");
    let bytes = raw.as_bytes();
    let mut search_from = 0;
    while let Some(rel) = raw[search_from..].find(&pat) {
        let mut i = search_from + rel + pat.len();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes.get(i) == Some(&b':') {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            return Some(i);
        }
        search_from = search_from + rel + 1;
    }
    None
}

fn extract_json_string_after_key(raw: &str, key: &str) -> Option<String> {
    let start = find_key_value_start(raw, key)?;
    let rest = raw[start..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut esc = false;
    for ch in rest.chars() {
        if esc {
            out.push(ch);
            esc = false;
            continue;
        }
        match ch {
            '\\' => esc = true,
            '"' => return Some(out),
            c => out.push(c),
        }
    }
    None
}

/// Collect fully-closed `{...}` objects inside `"key": [ ... ]`, ignoring a truncated tail.
fn extract_complete_json_objects_in_array(raw: &str, key: &str) -> Vec<Value> {
    let Some(start) = find_key_value_start(raw, key) else {
        return Vec::new();
    };
    let bytes = raw.as_bytes();
    let mut i = start;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'[' {
        return Vec::new();
    }
    i += 1;
    let mut out = Vec::new();
    let mut in_str = false;
    let mut esc = false;
    let mut obj_start: Option<usize> = None;
    let mut depth = 0i32;
    let chars: Vec<(usize, char)> = raw[i..].char_indices().collect();
    for &(rel, ch) in &chars {
        let abs = i + rel;
        if in_str {
            if esc {
                esc = false;
            } else if ch == '\\' {
                esc = true;
            } else if ch == '"' {
                in_str = false;
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            '{' => {
                if depth == 0 {
                    obj_start = Some(abs);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = obj_start.take() {
                        let end = abs + ch.len_utf8();
                        if let Ok(v) = serde_json::from_str::<Value>(&raw[s..end]) {
                            out.push(v);
                        }
                    }
                } else if depth < 0 {
                    break;
                }
            }
            ']' if depth == 0 => break,
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salvage_truncated_entities_array() {
        let raw = r#"{
  "entities": [
    {"kind":"character","name":"测试甲","summary":"终态A","status":"ok"},
    {"kind":"character","name":"测试乙","summary":"终态B","status":"ok"},
    {"kind":"location","name":"安全屋","summary":"截断"#;
        let v = extract_json_value(raw).expect("salvage");
        let ents = v["entities"].as_array().expect("entities");
        assert_eq!(ents.len(), 2);
        assert_eq!(ents[0]["name"], "测试甲");
    }

    #[test]
    fn apply_json_writes_entities_ignores_plots_and_graph() {
        let root = std::env::temp_dir().join(format!(
            "novelx_vol_sync_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(root.join("artifacts")).unwrap();
        let vol = VolumeBound {
            volume_index: 1,
            start_chapter: 1,
            end_chapter: 15,
            name: "测试卷".into(),
            ending_conditions: vec!["测试终止".into()],
            goal: String::new(),
            completed: false,
        };
        std::fs::write(
            root.join("artifacts/master_outline.md"),
            "# 总纲\n\n主线追查异变。\n",
        )
        .unwrap();
        let v = json!({
            "entities":[{
                "kind":"character",
                "name":"测试角色",
                "summary":"一卷要点",
                "status":"退场",
                "holdings":"旧钥"
            }],
            "plots":[{"title":"主线收束","body":"# 主线\n收束"}],
            "nomenclature":[{"canonical_name":"裂隙气","category":"concept","aliases":[]}],
            "bible_patches":[{"topic":"裂隙","content":"本卷确立裂隙呼吸规则"}],
            "arc_progress":"第一卷收束",
            "master_revision":"第二卷重心转向城市对抗",
            "relation_graph":{
                "nodes":[{"id":"c_test","kind":"character","label":"测试角色"}],
                "edges":[{"from":"c_test","to":"c_test","label":"自指"}]
            }
        });
        let report = apply_volume_sync_json(&root, &vol, &v).unwrap();
        assert_eq!(report.entities_upserted, 1);
        assert_eq!(report.plots_written, 0, "volume sync must ignore plots");
        assert_eq!(report.incomplete_entities, vec!["测试角色".to_string()]);
        let card = std::fs::read_to_string(root.join("entities/characters/测试角色.md")).unwrap();
        assert!(card.contains("complete: false"));
        assert!(card.contains("source: volume_sync"));
        assert!(card.contains("status: exited"));
        assert!(card.contains("holdings: 旧钥"));
        assert!(card.contains("## 当前状态"), "{card}");
        assert!(!card.contains("卷末同步摘要"), "{card}");
        // Re-sync existing card: state only, must not force incomplete / append stubs.
        let report2 = apply_volume_sync_json(
            &root,
            &vol,
            &json!({
                "entities":[{
                    "kind":"character",
                    "name":"测试角色",
                    "status":"active",
                    "holdings":"新钥",
                    "note":"卷末复位"
                }]
            }),
        )
        .unwrap();
        assert_eq!(report2.entities_upserted, 1);
        assert!(report2.incomplete_entities.is_empty());
        let card2 = std::fs::read_to_string(root.join("entities/characters/测试角色.md")).unwrap();
        assert!(card2.contains("status: active"));
        assert!(card2.contains("holdings: 新钥"));
        assert!(card2.contains("备注：卷末复位"));
        assert_eq!(card2.matches("## 当前状态").count(), 1);
        assert!(!card2.contains("### 更新"));
        assert!(!root.join("plots").exists() || root.join("plots").read_dir().unwrap().count() == 0);
        assert!(!root.join("artifacts/relation_graph.json").exists());
        assert!(root.join("lore/nomenclature.json").exists());
        let master = std::fs::read_to_string(root.join("artifacts/master_outline.md")).unwrap();
        assert!(master.contains("## 卷末修订（自动）·第1卷"));
        assert!(master.contains("第二卷重心转向城市对抗"));
        // Re-running progress must not stack duplicate [卷末] lines.
        let _ = apply_volume_sync_json(
            &root,
            &vol,
            &json!({
                "arc_progress": "再次同步",
                "master_revision": "修订二次"
            }),
        )
        .unwrap();
        let outline: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("artifacts/story_outline.json")).unwrap_or_else(
                |_| {
                    // write_arc_progress creates outline even without acts
                    "{}".into()
                },
            ),
        )
        .unwrap_or(json!({}));
        let notes = outline["acts"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|a| a.get("progress_notes"))
            .and_then(|x| x.as_str())
            .unwrap_or("");
        // No acts in empty outline — ensure per-volume 卷纲 has single auto section.
        let arc = crate::volume::read_arc_outline_text(&root, 1).unwrap();
        assert_eq!(arc.matches("## 卷末进度（自动）").count(), 1);
        assert!(arc.contains("再次同步"));
        let master2 = std::fs::read_to_string(root.join("artifacts/master_outline.md")).unwrap();
        assert_eq!(master2.matches("## 卷末修订（自动）·第1卷").count(), 1);
        assert!(master2.contains("修订二次"));
        let _ = notes;
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn plot_light_apply_skips_volume_end_progress() {
        let root = std::env::temp_dir().join(format!(
            "novelx_plot_sync_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(root.join("artifacts")).unwrap();
        std::fs::write(
            root.join("artifacts/story_outline.json"),
            r#"{
  "acts": [
    {
      "volume_index": 1,
      "name": "试卷",
      "start_chapter": 1,
      "end_chapter": 10,
      "progress_notes": "既有进度"
    }
  ]
}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("artifacts/bible.md"),
            "# 世界观 Bible\n\n## 0. 一句话世界\n测。\n",
        )
        .unwrap();
        let vol = VolumeBound {
            volume_index: 1,
            start_chapter: 1,
            end_chapter: 10,
            name: "试卷".into(),
            ending_conditions: vec![],
            goal: String::new(),
            completed: false,
        };
        let v = json!({
            "entities":[{
                "kind":"character",
                "name":"剧情角色",
                "summary":"本段要点",
                "status":"active",
                "source":"plot_sync"
            }],
            "bible_patches":[{"topic":"线索","content":"剧情内补丁"}],
            "arc_progress":"不该写成卷末",
            "master_revision":"不该改总纲"
        });
        let report = apply_sync_json(&root, &vol, &v, SyncApplyMode::PlotLight).unwrap();
        assert_eq!(report.entities_upserted, 1);
        assert!(report.message.contains("剧情"));
        let outline: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("artifacts/story_outline.json")).unwrap(),
        )
        .unwrap();
        let notes = outline["acts"][0]["progress_notes"].as_str().unwrap_or("");
        assert_eq!(notes, "既有进度");
        assert!(!notes.contains("卷末"));
        let bible = std::fs::read_to_string(root.join("artifacts/bible.md")).unwrap();
        assert!(bible.contains("剧情同步"));
        assert!(!bible.contains("卷末同步"));
        let master_path = root.join("artifacts/master_outline.md");
        assert!(!master_path.exists() || {
            let t = std::fs::read_to_string(master_path).unwrap_or_default();
            !t.contains("卷末修订")
        });
        let card = std::fs::read_to_string(root.join("entities/characters/剧情角色.md")).unwrap();
        assert!(card.contains("source: plot_sync"));
        assert!(card.contains("剧情同步摘要"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn collect_gaps_flags_stubs_and_duplicates() {
        let root = std::env::temp_dir().join(format!(
            "novelx_gaps_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(root.join("entities/items")).unwrap();
        std::fs::write(
            root.join("entities/items/旧钥.md"),
            "---\nname: 旧钥\ncomplete: false\nsource: volume_sync\n---\n\n# 旧钥\n\n## 卷末同步摘要\n\n短\n",
        )
        .unwrap();
        std::fs::write(
            root.join("entities/items/旧钥（入门引导）.md"),
            "---\nname: 旧钥（入门引导）\ncomplete: false\n---\n\n# 旧钥（入门引导）\n\n## 卷末同步摘要\n\n短\n",
        )
        .unwrap();
        let gaps = crate::cards::collect_entity_gaps(&root);
        assert!(gaps.iter().any(|g| g.contains("待补全") && g.contains("旧钥")));
        assert!(gaps.iter().any(|g| g.contains("疑似重复")));
        let _ = std::fs::remove_dir_all(&root);
    }
}

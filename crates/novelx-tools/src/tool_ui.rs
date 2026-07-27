//! Compact tool lines for chat/WebSocket UI (full output still goes to the model).

use serde_json::Value;

/// Pipeline step / tool id → Chinese label for progress lines & tool cards.
pub fn agent_label_zh(id: &str) -> &str {
    match id {
        "chapter_planner" => "章纲规划",
        "lore_librarian" => "设定检索",
        "writer" => "正文写作",
        "continue_writing" => "继续创作",
        "local_reviser" => "局部修订",
        "scene_specialist" => "场景专改",
        "dialogue_specialist" => "对话专改",
        "consistency_auditor" => "一致性审计",
        "foreshadow_tracker" => "伏笔追踪",
        "pacing_reviewer" => "节奏审查",
        "literary_editor" => "文学润色",
        "nomenclature_curator" => "名词管理",
        "summarizer" => "章节摘要",
        "plot_acceptor" => "剧情验收",
        "autofix" => "自动修复",
        "revise_chapter" => "修订章节",
        "audit_chapter" => "审校章节",
        "audit_chapters" => "审阅队列",
        "audit_volume" => "整卷复盘",
        "steer_run" => "门控续作",
        "offer_decisions" => "提出决策",
        "expectation_reviewer" => "预期检阅",
        "enqueue_expected_event" => "登记预期",
        "update_expected_event" => "更新预期",
        "list_expected_events" => "预期列表",
        "review_expected_events" => "检阅预期",
        "resolve_expected_event" => "处理预期",
        other => other,
    }
}

/// Tools whose full payload should not be streamed into the chat timeline.
pub(crate) fn is_bulk_context_tool(name: &str) -> bool {
    matches!(
        name,
        "read_chapter"
            | "query_lore"
            | "query_memory"
            | "list_entities"
            | "list_plots"
            | "list_expected_events"
            | "get_project_status"
    )
}

/// Short Chinese status for the timeline. `None` = show original `output`.
pub fn tool_output_for_ui(name: &str, args: &Value, output: &str, data: &Value) -> Option<String> {
    if !is_bulk_context_tool(name) {
        return None;
    }
    // Keep real errors visible.
    if looks_like_error(output) {
        return None;
    }
    let title = book_title(args, data);
    Some(match name {
        "read_chapter" => {
            let ch = chapter_num(args, data);
            format!("正在阅读《{title}》第{ch}章·正文 / 章纲")
        }
        "query_lore" => {
            let surface = lore_surface(args, data);
            format!("正在阅读《{title}》{surface}")
        }
        "query_memory" => format!("正在查阅《{title}》记忆摘要"),
        "list_entities" => {
            let kind = args.get("kind").and_then(|v| v.as_str()).unwrap_or("all");
            let label = match kind {
                "character" => "人物",
                "item" => "物品",
                "location" => "地点",
                _ => "人物 / 物品 / 地点",
            };
            let n = data
                .get("entities")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("正在浏览《{title}》{label}（{n} 项）")
        }
        "list_plots" => {
            let n = data
                .get("plots")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .or_else(|| {
                    Some(
                        output
                            .lines()
                            .filter(|l| !l.trim().is_empty())
                            .count(),
                    )
                })
                .unwrap_or(0);
            format!("正在阅读《{title}》剧情（{n} 项）")
        }
        "list_expected_events" => {
            let n = data
                .get("events")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("正在查阅《{title}》预处理预期（{n} 项）")
        }
        "get_project_status" => format!("正在查看《{title}》项目状态"),
        _ => format!("正在查阅《{title}》"),
    })
}

fn looks_like_error(output: &str) -> bool {
    let t = output.trim();
    t.starts_with("⛔")
        || t.contains("BLOCKER")
        || t.contains("格式不合规")
        || t.contains("必填")
        || t.starts_with("error:")
        || t.starts_with("Error")
}

fn book_title(args: &Value, data: &Value) -> String {
    data.get("state")
        .and_then(|s| s.get("name"))
        .and_then(|v| v.as_str())
        .or_else(|| data.get("project").and_then(|v| v.as_str()))
        .or_else(|| args.get("project").and_then(|v| v.as_str()))
        .filter(|s| !s.is_empty())
        .unwrap_or("当前小说")
        .to_string()
}

fn chapter_num(args: &Value, data: &Value) -> u32 {
    data.get("chapter")
        .and_then(|v| v.as_u64())
        .or_else(|| args.get("chapter").and_then(|v| v.as_u64()))
        .unwrap_or(1) as u32
}

fn lore_surface(args: &Value, data: &Value) -> String {
    let q = args
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    let hits = data
        .get("hits")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        })
        .unwrap_or_default();
    let blob = format!("{q} {hits}");
    if blob.contains("总纲") || blob.contains("master") {
        return "总纲".into();
    }
    if blob.contains("卷纲") || blob.contains("arc_outline") || blob.contains("arc ") {
        return "卷纲".into();
    }
    if blob.contains("世界观") || blob.contains("bible") {
        return "世界观".into();
    }
    if blob.contains("剧情") || blob.contains("plot") {
        return "剧情".into();
    }
    if blob.contains("人物") || blob.contains("character") {
        return "人物".into();
    }
    if blob.contains("物品") || blob.contains("item") {
        return "物品".into();
    }
    if blob.contains("地点") || blob.contains("location") {
        return "地点".into();
    }
    if blob.contains("设定") || blob.contains("名词") {
        return "设定".into();
    }
    if let Some(ch) = args.get("chapter").and_then(|v| v.as_u64()) {
        return format!("第{ch}章相关设定");
    }
    "设定与上下文".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn read_chapter_summary() {
        let s = tool_output_for_ui(
            "read_chapter",
            &json!({"project": "sample-novel", "chapter": 3}),
            &"x".repeat(5000),
            &json!({"chapter": 3, "project": "sample-novel"}),
        )
        .unwrap();
        assert!(s.contains("第3章"));
        assert!(s.contains("sample-novel"));
        assert!(!s.contains("详见右侧"));
        assert!(!s.contains(&"x".repeat(100)));
    }

    #[test]
    fn query_lore_surface() {
        let s = tool_output_for_ui(
            "query_lore",
            &json!({"project": "sample-novel", "query": "看一下卷纲终止条件"}),
            "huge context…",
            &json!({"state": {"name": "样例小说"}, "hits": ["arc_outline"]}),
        )
        .unwrap();
        assert!(s.contains("卷纲"));
        assert!(s.contains("样例小说"));
    }

    #[test]
    fn agent_labels_are_chinese() {
        assert_eq!(agent_label_zh("chapter_planner"), "章纲规划");
        assert_eq!(agent_label_zh("lore_librarian"), "设定检索");
        assert_eq!(agent_label_zh("writer"), "正文写作");
        assert_eq!(agent_label_zh("continue_writing"), "继续创作");
        assert_eq!(agent_label_zh("scene_specialist"), "场景专改");
        assert_eq!(agent_label_zh("consistency_auditor"), "一致性审计");
        assert_eq!(agent_label_zh("foreshadow_tracker"), "伏笔追踪");
        assert_eq!(agent_label_zh("autofix"), "自动修复");
        assert_eq!(agent_label_zh("pacing_reviewer"), "节奏审查");
        assert_eq!(agent_label_zh("literary_editor"), "文学润色");
    }
}

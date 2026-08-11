//! Compact tool lines for chat/WebSocket UI (full output still goes to the model).

use serde_json::Value;

/// Pipeline step / tool id → Chinese label for progress lines & tool cards.
/// Unknown English ids fall back to a generic phrase (never leak snake_case to UI).
pub fn agent_label_zh(id: &str) -> &str {
    match id {
        "chapter_planner" => "规划章纲",
        "lore_librarian" => "查阅设定",
        "writer" => "撰写正文",
        "continue_writing" => "继续创作",
        "continue_writing_batch" => "批量续写",
        "continue_episode" => "续写短剧",
        "local_reviser" => "局部改稿",
        "scene_specialist" => "打磨场景",
        "dialogue_specialist" => "打磨对白",
        "consistency_auditor" => "核对前后文",
        "foreshadow_tracker" => "梳理伏笔",
        "pacing_reviewer" => "检查节奏",
        "literary_editor" => "润色文笔",
        "nomenclature_curator" => "统一用词",
        "summarizer" => "整理章摘要",
        "plot_acceptor" => "核对剧情进度",
        "autofix" => "自动修正",
        "revise_chapter" => "修订章节",
        "revise_local" => "局部改稿",
        "revise_outline" => "修订章纲",
        "revise_episode" => "修订短剧",
        "split_chapter" => "拆成两章",
        "apply_draft_patch" => "局部改稿",
        "replan_volume" => "重排本卷计划",
        "audit_chapter" => "审校章节",
        "audit_chapters" => "审阅队列",
        "audit_volume" => "整卷复盘",
        "research_materials" => "搜集素材",
        "read_chapter" => "阅读章节",
        "read_episode" => "阅读短剧",
        "steer_run" => "按你的选择继续",
        "offer_decisions" => "请你选择下一步",
        "expectation_reviewer" => "检阅预期",
        "enqueue_expected_event" => "登记预期",
        "update_expected_event" => "更新预期",
        "list_expected_events" => "查看预期列表",
        "review_expected_events" => "检阅预期",
        "resolve_expected_event" => "处理预期",
        "query_lore" => "查询设定",
        "query_memory" => "查阅记忆",
        "list_entities" => "浏览设定卡",
        "list_plots" => "查看剧情进度",
        "get_project_status" => "查看项目进度",
        "design_entity" => "设计设定卡",
        "design_plot" => "设计剧情卡",
        "update_plot" => "更新剧情卡",
        "design_master_outline" => "设计总纲",
        "design_arc_outline" => "设计卷纲",
        "sync_volume" => "同步设定库",
        "confirm_volume_memory" => "确认卷记忆",
        "create_novel" | "init_novel" => "创建小说",
        "lock_brief" => "锁定灵感",
        "confirm_setup" => "确认定稿",
        "spawn_agent" => "启动协作角色",
        "wait_agent" => "等待协作角色",
        "list_agents" => "查看协作角色",
        "interrupt_agent" => "中断协作角色",
        "activate_agents" => "启用写作角色",
        other if looks_like_internal_id(other) => "后台步骤",
        other => other,
    }
}

fn looks_like_internal_id(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    id.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
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
        assert_eq!(agent_label_zh("chapter_planner"), "规划章纲");
        assert_eq!(agent_label_zh("lore_librarian"), "查阅设定");
        assert_eq!(agent_label_zh("writer"), "撰写正文");
        assert_eq!(agent_label_zh("continue_writing"), "继续创作");
        assert_eq!(agent_label_zh("continue_writing_batch"), "批量续写");
        assert_eq!(agent_label_zh("scene_specialist"), "打磨场景");
        assert_eq!(agent_label_zh("consistency_auditor"), "核对前后文");
        assert_eq!(agent_label_zh("foreshadow_tracker"), "梳理伏笔");
        assert_eq!(agent_label_zh("autofix"), "自动修正");
        assert_eq!(agent_label_zh("pacing_reviewer"), "检查节奏");
        assert_eq!(agent_label_zh("literary_editor"), "润色文笔");
        assert_eq!(agent_label_zh("unknown_snake_tool"), "后台步骤");
    }
}

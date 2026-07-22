use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContentRuleViolation {
    pub rule: String,
    pub message: String,
}

/// Deterministic hard rules on a draft (full scan — cheap).
pub fn check_draft(draft: &str, banned_names: &[String]) -> Vec<ContentRuleViolation> {
    let mut out = Vec::new();
    for name in banned_names {
        if !name.is_empty() && draft.contains(name) {
            out.push(ContentRuleViolation {
                rule: "banned_name".into(),
                message: format!("正文出现禁名「{name}」"),
            });
        }
    }
    if draft.contains("[CONTRADICTION]") {
        out.push(ContentRuleViolation {
            rule: "contradiction_marker".into(),
            message: "正文残留 [CONTRADICTION] 标记".into(),
        });
    }
    let re = Regex::new(r"\[NEW_FACT:[^\]]+\]").unwrap();
    let count = re.find_iter(draft).count();
    if count > 8 {
        out.push(ContentRuleViolation {
            rule: "too_many_new_facts".into(),
            message: format!("[NEW_FACT] 过多（{count}），需收敛"),
        });
    }
    out.extend(check_meta_chapter_refs(draft));
    out
}

/// Prose must not refer to serial chapter numbers (breaks immersion).
/// Heading lines like `# 第10章 标题` are allowed.
fn check_meta_chapter_refs(draft: &str) -> Vec<ContentRuleViolation> {
    let re = Regex::new(r"第[一二三四五六七八九十百千零〇两\d]+章").expect("meta chapter regex");
    let mut out = Vec::new();
    for (idx, line) in draft.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some(m) = re.find(line) {
            out.push(ContentRuleViolation {
                rule: "meta_chapter_ref".into(),
                message: format!(
                    "正文出现章号元叙述「{}」（约第{}行）：角色不应知道「第N章」；改用故事内时间/事件指称（如「上次会面时」「那次事故之后」）",
                    m.as_str(),
                    idx + 1
                ),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_title_heading_but_flags_body_meta_chapter() {
        let draft = "# 第10章 开局\n\n第八章跟对方联系时，他感到不安。\n";
        let v = check_draft(draft, &[]);
        assert!(
            v.iter().any(|x| x.rule == "meta_chapter_ref"),
            "expected meta_chapter_ref, got {v:?}"
        );
    }

    #[test]
    fn does_not_flag_numbered_proper_noun() {
        // 「第十三条军规」是故事内专名，不应被当成「第N章」元叙述。
        let draft = "# 第10章\n\n他想起第十三条军规与石碑上的记号。\n";
        let v = check_draft(draft, &[]);
        assert!(v.is_empty(), "got {v:?}");
    }
}

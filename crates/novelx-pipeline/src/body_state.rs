//! Continuity board for injury sites & ability loci — injected into writer CanonContext.
//! Genre-neutral: only structural side/site / host cues, never story-specific names.

use crate::cards::truncate_chars;
use crate::memory::load_memory;
use crate::project::chapter_dir;
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

/// How many prior chapters to scan for injury / ability-locus facts.
/// Wider than early MVP (4) so 800–1000ch longform keeps mid-arc wounds/loci hot.
pub const BODY_STATE_LOOKBACK: usize = 8;

/// Build a short locked board of injury / ability-locus facts for chapter N
/// (sourced from prior digests + previous chapter summary).
pub fn format_body_state_board(project_dir: &Path, chapter: u32) -> String {
    render_board(&collect_body_state_lines(project_dir, chapter))
}

/// Per-character board for Web / entity cards.
///
/// - Fact mentions any of `self_keys` → include.
/// - Fact mentions some other name in `all_keys` but not self → exclude.
/// - Fact mentions nobody in `all_keys` → include only when `is_default_owner`
///   (always-include / protagonist card).
pub fn format_body_state_board_for_character(
    project_dir: &Path,
    chapter: u32,
    self_keys: &[String],
    all_keys: &[String],
    is_default_owner: bool,
) -> String {
    let lines = collect_body_state_lines(project_dir, chapter);
    let self_keys = normalize_keys(self_keys);
    let all_keys = normalize_keys(all_keys);
    if self_keys.is_empty() && !is_default_owner {
        return String::new();
    }

    let mut owned = Vec::new();
    for (ch, fact) in lines {
        let mentioned: Vec<&str> = all_keys
            .iter()
            .map(|s| s.as_str())
            .filter(|k| fact.contains(k))
            .collect();
        let mine = mentioned.iter().any(|k| self_keys.iter().any(|s| s == k));
        let include = if mentioned.is_empty() {
            is_default_owner
        } else {
            mine
        };
        if include {
            owned.push((ch, fact));
        }
    }
    render_board(&owned)
}

fn normalize_keys(keys: &[String]) -> Vec<String> {
    let mut out: Vec<String> = keys
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| s.chars().count() >= 1)
        .collect();
    out.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()));
    out.dedup();
    out
}

fn collect_body_state_lines(project_dir: &Path, chapter: u32) -> Vec<(u32, String)> {
    let mut lines: Vec<(u32, String)> = Vec::new();
    let mut seen = HashSet::new();

    let mem = load_memory(project_dir);
    let prior_chs: Vec<u32> = {
        let mut cs: Vec<u32> = mem
            .recent_digests
            .iter()
            .map(|d| d.chapter)
            .filter(|c| *c < chapter || chapter == 0)
            .collect();
        if chapter > 1 {
            cs.push(chapter - 1);
        }
        cs.sort_unstable();
        cs.dedup();
        cs.into_iter().rev().take(BODY_STATE_LOOKBACK).collect()
    };

    for ch in prior_chs.iter().copied().rev() {
        if let Some(d) = mem.recent_digests.iter().find(|d| d.chapter == ch) {
            for fact in &d.key_facts {
                push_fact(&mut lines, &mut seen, ch, fact);
            }
        }
        if let Some(v) = load_chapter_summary_json(project_dir, ch) {
            collect_from_summary_value(&mut lines, &mut seen, ch, &v);
        }
    }

    lines.sort_by(|a, b| b.0.cmp(&a.0));
    lines
}

fn render_board(lines: &[(u32, String)]) -> String {
    let mut injuries = Vec::new();
    let mut abilities = Vec::new();
    for (ch, fact) in lines {
        let tagged = format!("[第{ch}章] {fact}");
        if is_injury_fact(fact) {
            if injuries.len() < 8 {
                injuries.push(tagged);
            }
        } else if is_ability_locus_fact(fact) || is_control_side_fact(fact) {
            if abilities.len() < 8 {
                abilities.push(tagged);
            }
        }
    }

    if injuries.is_empty() && abilities.is_empty() {
        return String::new();
    }

    let mut out = Vec::new();
    out.push(
        "开写前锁定：脑中固定下表伤势侧别/部位与能力寄宿/附着/载体；\
场面承接优先用症状与动作限制（颤抖、冷汗、握力发虚等），非分侧剧情勿反复点名左右；\
必须分侧或变更时须与下表一致，并写出可见转移/愈合过程（不得默默挪位）。"
            .into(),
    );
    if !injuries.is_empty() {
        out.push(String::new());
        out.push("伤势：".into());
        for l in &injuries {
            out.push(format!("- {l}"));
        }
    }
    if !abilities.is_empty() {
        out.push(String::new());
        out.push("能力位置/载体/控制：".into());
        for l in &abilities {
            out.push(format!("- {l}"));
        }
    }
    out.join("\n")
}

fn load_chapter_summary_json(project_dir: &Path, chapter: u32) -> Option<Value> {
    let path = chapter_dir(project_dir, chapter).join("summary.json");
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn collect_from_summary_value(
    lines: &mut Vec<(u32, String)>,
    seen: &mut HashSet<String>,
    chapter: u32,
    v: &Value,
) {
    if let Some(bs) = v.get("body_state") {
        if let Some(arr) = bs.get("injuries").and_then(|x| x.as_array()) {
            for item in arr {
                if let Some(s) = item.as_str() {
                    push_fact(lines, seen, chapter, &format!("伤势：{s}"));
                }
            }
        }
        if let Some(arr) = bs.get("ability_loci").and_then(|x| x.as_array()) {
            for item in arr {
                if let Some(s) = item.as_str() {
                    push_fact(lines, seen, chapter, &format!("能力位置：{s}"));
                }
            }
        }
    }
    if let Some(arr) = v.get("new_facts").and_then(|x| x.as_array()) {
        for item in arr {
            if let Some(s) = item.as_str() {
                push_fact(lines, seen, chapter, s);
            }
        }
    }
}

fn push_fact(lines: &mut Vec<(u32, String)>, seen: &mut HashSet<String>, chapter: u32, raw: &str) {
    let fact = truncate_chars(raw.trim(), 160);
    if fact.is_empty() || !is_body_state_fact(&fact) {
        return;
    }
    let key = fact.chars().take(48).collect::<String>();
    if !seen.insert(key) {
        return;
    }
    lines.push((chapter, fact));
}

fn is_body_state_fact(s: &str) -> bool {
    is_injury_fact(s) || is_ability_locus_fact(s) || is_control_side_fact(s)
}

fn has_side_or_site(s: &str) -> bool {
    const SITES: &[&str] = &[
        "左", "右", "肩", "臂", "手", "掌", "腕", "腿", "膝", "踝", "腹", "胸", "肋", "背", "颈",
        "头", "额", "眼", "指",
    ];
    SITES.iter().any(|k| s.contains(k))
}

fn is_injury_fact(s: &str) -> bool {
    // Ability-locus board lines may say «尚未造成伤痕» — not an injury side-lock.
    if s.contains("能力位置：") {
        return false;
    }
    if s.starts_with("伤势：") || s.contains("伤势：") {
        return has_side_or_site(s) || s.contains("伤");
    }
    let injury_kw = s.contains("伤")
        || s.contains("伤口")
        || s.contains("骨折")
        || s.contains("淤血")
        || s.contains("血痕")
        || s.contains("撕裂")
        || s.contains("挫伤")
        || s.contains("扭伤")
        || s.contains("灼伤");
    injury_kw && has_side_or_site(s)
}

fn is_ability_locus_fact(s: &str) -> bool {
    if s.starts_with("能力位置：") || s.contains("能力位置：") {
        return true;
    }
    let mark_on_site = (s.contains("掌心") || s.contains("手腕") || s.contains("腕内侧") || s.contains("手背"))
        && (s.contains("符号") || s.contains("印记") || s.contains("纹") || s.contains("图案"));
    let locus_kw = s.contains("寄宿")
        || s.contains("附着")
        || s.contains("载体")
        || s.contains("印记")
        || s.contains("操控权")
        || s.contains("控制权")
        || s.contains("所在位置")
        || s.contains("附着点")
        || mark_on_site
        || (s.contains("能力") && (s.contains("位于") || s.contains("在其") || has_side_or_site(s)))
        || (s.contains("权能") && has_side_or_site(s))
        || (s.contains("异能") && has_side_or_site(s));
    locus_kw
}

fn is_control_side_fact(s: &str) -> bool {
    let control = s.contains("操控")
        || s.contains("控制")
        || s.contains("夺取")
        || s.contains("接管")
        || s.contains("不归")
        || s.contains("自主");
    control && has_side_or_site(s)
}

const BODY_SITES: &[&str] = &[
    "肩", "臂", "手", "掌", "腕", "腿", "膝", "踝", "指", "肋",
];

fn fact_mentions_side_site(fact: &str, side: char, site: &str) -> bool {
    let direct = format!("{side}{site}");
    if fact.contains(&direct) {
        return true;
    }
    // «左小腿» / «右大腿» still count as the 腿 site.
    for mid in ["小", "大"] {
        if fact.contains(&format!("{side}{mid}{site}")) {
            return true;
        }
    }
    false
}

fn extract_side_site_locks(fact: &str) -> Vec<(char, &'static str)> {
    let mut out = Vec::new();
    for site in BODY_SITES {
        let has_l = fact_mentions_side_site(fact, '左', site);
        let has_r = fact_mentions_side_site(fact, '右', site);
        // Comparative board lines («左腿伤、右腿可支撑») mention both sides — not an
        // exclusive single-side lock; locking both makes normal prose impossible.
        if has_l && has_r {
            continue;
        }
        if has_l {
            out.push(('左', *site));
        }
        if has_r {
            out.push(('右', *site));
        }
    }
    out
}

fn has_transfer_or_heal_marker(draft: &str) -> bool {
    // Whole-draft exemption: only clear transfer/heal phrasing.
    // Do NOT include bare「另一只手」— that would skip opposite-injury checks for the chapter.
    const MARKERS: &[&str] = &[
        "转移到",
        "挪到",
        "换到",
        "改用",
        "已经愈合",
        "伤已愈合",
        "伤势已愈合",
        "伤势已恢复",
        "包扎好",
        "换手",
        "换到另一",
        "移到另一",
        "改用另一",
        "换另一只",
    ];
    MARKERS.iter().any(|m| draft.contains(m))
}

/// Healed / residual injury lines must not act as exclusive side locks.
fn is_inactive_side_lock_fact(fact: &str) -> bool {
    // Avoid bare「伤势已」— it matches「伤势已加重/恶化」active worsenings.
    const CUES: &[&str] = &[
        "已愈合",
        "已经愈合",
        "伤已愈合",
        "伤势已愈合",
        "伤势已恢复",
        "伤势已消退",
        "消退至接近不可见",
        "接近不可见",
        "不影响动作",
        "已恢复",
        "已经恢复",
        "并恢复",
        "仅残留",
        "残留痕迹",
        "钝压感",
        "退至",
        "退回真皮",
        "映射完成后退回",
    ];
    CUES.iter().any(|c| fact.contains(c))
}

/// Injury / wound cues — opposite-side words only block when co-occurring in-sentence.
fn sentence_has_injury_cue(seg: &str) -> bool {
    const CUES: &[&str] = &[
        "伤", "伤口", "撕裂", "骨折", "渗血", "淤", "愈合", "痛得", "无法伸", "挫伤", "灼伤",
        "扭伤", "血痕",
    ];
    CUES.iter().any(|c| seg.contains(c))
}

fn draft_has_opposite_injury_claim(draft: &str, opp: &str) -> bool {
    draft
        .split(|c| matches!(c, '。' | '！' | '？' | '\n' | '；' | ';'))
        .any(|seg| {
            let seg = seg.trim();
            !seg.is_empty() && seg.contains(opp) && sentence_has_injury_cue(seg)
        })
}

/// Deterministic light check: if the board locks 左X / 右X and a sentence both
/// names the opposite side and carries an injury cue, block publish.
///
/// Incidental opposite-side actions (e.g. 「右手按内袋」) without injury language pass.
/// Cross-fact bilateral locks on the same site skip exclusive opposite-side blocking.
pub fn check_body_state_side_conflicts(
    project_dir: &Path,
    chapter: u32,
    draft: &str,
) -> Vec<String> {
    if draft.trim().is_empty() {
        return Vec::new();
    }
    if has_transfer_or_heal_marker(draft) {
        return Vec::new();
    }
    let lines = collect_body_state_lines(project_dir, chapter);
    let mut sides_by_site: std::collections::HashMap<&'static str, HashSet<char>> =
        std::collections::HashMap::new();
    let mut lock_facts: Vec<(char, &'static str, String)> = Vec::new();
    for (_ch, fact) in &lines {
        // Ability locus flips → check_body_state_locus_conflicts.
        // Side exclusivity is for injury / control only.
        if !(is_injury_fact(fact) || is_control_side_fact(fact)) {
            continue;
        }
        if is_inactive_side_lock_fact(fact) {
            continue;
        }
        for (side, site) in extract_side_site_locks(fact) {
            sides_by_site.entry(site).or_default().insert(side);
            lock_facts.push((side, site, fact.clone()));
        }
    }
    let bilateral_sites: HashSet<&'static str> = sides_by_site
        .iter()
        .filter(|(_, sides)| sides.len() >= 2)
        .map(|(site, _)| *site)
        .collect();

    let mut msgs = Vec::new();
    let mut seen = HashSet::new();
    for (side, site, fact) in &lock_facts {
        if bilateral_sites.contains(site) {
            continue;
        }
        let opposite = if *side == '左' { '右' } else { '左' };
        let opp = format!("{opposite}{site}");
        let key = format!("{side}{site}->{opp}");
        if !seen.insert(key) {
            continue;
        }
        if draft_has_opposite_injury_claim(draft, &opp) {
            msgs.push(format!(
                "身体状态板锁定「{side}{site}」（依据：{}），正文却在同一句把伤势/伤情写到「{opp}」且未见转移/愈合交代。请对齐侧别或改为症状/动作限制承接，或写出可见变更过程。",
                truncate_chars(fact, 60)
            ));
        }
    }
    msgs
}

const LOCUS_PREFIXES: &[&str] = &["寄宿在", "寄宿于", "附着于", "附着在", "附着到", "印记在"];

/// Ability / mark cues — required in the same sentence as a draft locus host
/// so metaphors like「寒意附着在脊背」do not false-positive.
const ABILITY_LOCUS_CUES: &[&str] = &[
    "印记", "异能", "权能", "能力", "载体", "符号", "操控权", "控制权", "魔纹", "咒印",
];

fn sentence_has_ability_cue(seg: &str) -> bool {
    ABILITY_LOCUS_CUES.iter().any(|k| seg.contains(k))
}

/// Extract short host fragments after 寄宿/附着 cues (e.g. 「右手腕内侧」).
fn extract_locus_hosts(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for pref in LOCUS_PREFIXES {
        let mut rest = text;
        while let Some(idx) = rest.find(pref) {
            let after = &rest[idx + pref.len()..];
            let host: String = after
                .chars()
                .take_while(|c| {
                    !matches!(
                        *c,
                        '，' | ',' | '。' | '；' | ';' | '、' | ' ' | '\n' | '（' | '('
                    )
                })
                .take(12)
                .collect();
            let host = host.trim().to_string();
            if host.chars().count() >= 2 {
                out.push(host);
            }
            rest = &rest[idx + pref.len()..];
        }
    }
    out.sort();
    out.dedup();
    out
}

fn fact_shares_ability_cue_with_sentence(fact: &str, seg: &str) -> bool {
    ABILITY_LOCUS_CUES
        .iter()
        .any(|k| fact.contains(k) && seg.contains(k))
        || (is_ability_locus_fact(fact) && sentence_has_ability_cue(seg))
}

/// If the board locks an ability host and the draft attaches the same ability to a
/// different host without a transfer marker, block publish.
pub fn check_body_state_locus_conflicts(
    project_dir: &Path,
    chapter: u32,
    draft: &str,
) -> Vec<String> {
    if draft.trim().is_empty() || has_transfer_or_heal_marker(draft) {
        return Vec::new();
    }
    let mut msgs = Vec::new();
    let mut seen = HashSet::new();
    for (_ch, fact) in collect_body_state_lines(project_dir, chapter) {
        if !is_ability_locus_fact(&fact) {
            continue;
        }
        let locked = extract_locus_hosts(&fact);
        if locked.is_empty() {
            continue;
        }
        for seg in draft.split(|c| matches!(c, '。' | '！' | '？' | '\n' | '；' | ';')) {
            let seg = seg.trim();
            if seg.is_empty()
                || !sentence_has_ability_cue(seg)
                || !fact_shares_ability_cue_with_sentence(&fact, seg)
            {
                continue;
            }
            let draft_hosts = extract_locus_hosts(seg);
            for host in &locked {
                for other in &draft_hosts {
                    if other == host || other.contains(host) || host.contains(other.as_str()) {
                        continue;
                    }
                    let key = format!("{host}->{other}");
                    if !seen.insert(key) {
                        continue;
                    }
                    msgs.push(format!(
                        "身体状态板锁定能力载体「{host}」（依据：{}），正文却写到「{other}」且未见转移交代。请对齐载体或写出可见转移过程。",
                        truncate_chars(&fact, 60)
                    ));
                }
            }
        }
    }
    msgs
}

/// Side flip + ability-host locus checks.
pub fn check_body_state_conflicts(
    project_dir: &Path,
    chapter: u32,
    draft: &str,
) -> Vec<String> {
    let mut msgs = check_body_state_side_conflicts(project_dir, chapter, draft);
    msgs.extend(check_body_state_locus_conflicts(project_dir, chapter, draft));
    msgs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{save_memory, ChapterDigest, ProjectMemory};
    use crate::project::init_project;
    use std::path::PathBuf;

    fn tmp_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "novelx-body-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros()
        ));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn board_pulls_injury_and_ability_from_summary() {
        let projects = tmp_root("sum");
        let root = init_project(&projects, "sample-novel", "未定", 10).unwrap();
        let ch_dir = chapter_dir(&root, 1);
        std::fs::create_dir_all(&ch_dir).unwrap();
        std::fs::write(
            ch_dir.join("summary.json"),
            r#"{
              "event_summary":"对峙。",
              "ending_hook":"门开了。",
              "new_facts":[
                "主角左肩贯穿伤，暂不能抬弓",
                "异能印记寄宿在右手腕内侧"
              ],
              "body_state":{
                "injuries":["左肩贯穿伤"],
                "ability_loci":["异能印记在右手腕内侧"]
              }
            }"#,
        )
        .unwrap();
        let board = format_body_state_board(&root, 2);
        assert!(board.contains("伤势"), "{board}");
        assert!(board.contains("左肩"), "{board}");
        assert!(board.contains("能力位置") || board.contains("右手腕"), "{board}");
        assert!(
            board.contains("不得默默挪位")
                || board.contains("可见转移")
                || board.contains("症状"),
            "{board}"
        );
        let flip = check_body_state_side_conflicts(
            &root,
            2,
            "他抬起右肩拉弓，伤口完全不在话下。\n",
        );
        assert!(
            flip.iter().any(|m| m.contains("左肩") && m.contains("右肩")),
            "expected side flip, got {flip:?}"
        );
        let ok = check_body_state_side_conflicts(
            &root,
            2,
            "他护着左肩，右手仍握着剑。\n",
        );
        assert!(ok.is_empty(), "no false positive: {ok:?}");
        // Comparative injury fact locks neither side exclusively.
        let ch2 = chapter_dir(&root, 2);
        std::fs::create_dir_all(&ch2).unwrap();
        std::fs::write(
            ch2.join("summary.json"),
            r#"{
              "event_summary":"伤",
              "ending_hook":"",
              "new_facts":[
                "周荣左小腿伤口仍在渗血，右腿可支撑、左腿不能负重"
              ],
              "body_state":{
                "injuries":["周荣左小腿伤口，右腿可支撑"],
                "ability_loci":["异能印记寄宿在左手掌心"]
              }
            }"#,
        )
        .unwrap();
        let bilateral = check_body_state_side_conflicts(
            &root,
            3,
            "周荣左腿发抖，右腿单独承重；陈衍用右手拾起铜戒，左手掌心的印记发紧。\n",
        );
        assert!(
            bilateral.is_empty(),
            "bilateral board + incidental opposite hand must not block: {bilateral:?}"
        );
        // Ability line mentioning「伤痕」must not become an injury side-lock on 左手.
        assert!(
            !is_injury_fact("能力位置：陈衍左手掌心：描摹密文符号后产生阻力，尚未造成可见伤痕。")
        );
        let locus = check_body_state_locus_conflicts(
            &root,
            2,
            "异能印记寄宿在左掌心，隐隐发热。\n",
        );
        assert!(
            locus.iter().any(|m| m.contains("右手腕") && m.contains("左掌")),
            "expected locus host flip, got {locus:?}"
        );
        let locus_ok = check_body_state_locus_conflicts(
            &root,
            2,
            "异能印记仍寄宿在右手腕内侧，隐隐发热。\n",
        );
        assert!(locus_ok.is_empty(), "no false positive locus: {locus_ok:?}");
        let metaphor = check_body_state_locus_conflicts(
            &root,
            2,
            "寒意附着在脊背，目光寄宿在她脸上。\n",
        );
        assert!(
            metaphor.is_empty(),
            "metaphor must not trip locus gate: {metaphor:?}"
        );
        let _ = std::fs::remove_dir_all(&projects);
    }

    #[test]
    fn residual_and_cross_fact_bilateral_skip_exclusive_side_lock() {
        let projects = tmp_root("res");
        let root = init_project(&projects, "sample-novel", "未定", 10).unwrap();
        let ch_dir = chapter_dir(&root, 1);
        std::fs::create_dir_all(&ch_dir).unwrap();
        std::fs::write(
            ch_dir.join("summary.json"),
            r#"{
              "event_summary":"伤。",
              "ending_hook":"门开了。",
              "new_facts":[],
              "body_state":{
                "injuries":[
                  "主角左手中指伤势仍在，握力受限",
                  "主角右手手背暗铜色细线消退至接近不可见，仅残留痕迹"
                ],
                "ability_loci":[]
              }
            }"#,
        )
        .unwrap();
        // Residual right-hand line is inactive; left lock remains — symptom prose OK.
        let symptoms = check_body_state_side_conflicts(
            &root,
            2,
            "他抬手时指尖微微颤抖，额角渗出冷汗，不敢用力握物。\n",
        );
        assert!(symptoms.is_empty(), "symptom carry must pass: {symptoms:?}");
        // Incidental 右手 without injury cue must pass.
        let right = check_body_state_side_conflicts(&root, 2, "他抬起右手去开门。\n");
        assert!(
            right.is_empty(),
            "opposite hand action without injury cue must pass: {right:?}"
        );
        // Same-sentence opposite side + injury cue still blocks.
        let flip = check_body_state_side_conflicts(&root, 2, "他护着右手，伤口完全不在话下。\n");
        assert!(
            flip.iter().any(|m| m.contains("左手")),
            "true flip with injury cue must block: {flip:?}"
        );
        let wound = check_body_state_side_conflicts(&root, 2, "右手伤口撕裂，鲜血渗出。\n");
        assert!(
            wound.iter().any(|m| m.contains("右手")),
            "opposite wound claim must block: {wound:?}"
        );
        // Bare「另一只手」must NOT exempt a later opposite-injury sentence.
        let other_hand = check_body_state_side_conflicts(
            &root,
            2,
            "他伸出另一只手开门。右手伤口撕裂，鲜血渗出。\n",
        );
        assert!(
            other_hand.iter().any(|m| m.contains("右手")),
            "另一只手 must not whole-draft bypass: {other_hand:?}"
        );
        assert!(
            !is_inactive_side_lock_fact("主角左手伤势已加重，握力更差"),
            "伤势已加重 must stay an active side lock"
        );
        assert!(
            is_inactive_side_lock_fact("主角左手伤势已愈合，仅残留痕迹"),
            "伤势已愈合 must be inactive"
        );
        // Two active opposite-side injuries on 手 → bilateral skip.
        let ch2 = chapter_dir(&root, 2);
        std::fs::create_dir_all(&ch2).unwrap();
        std::fs::write(
            ch2.join("summary.json"),
            r#"{
              "event_summary":"双侧伤。",
              "ending_hook":"",
              "new_facts":[],
              "body_state":{
                "injuries":[
                  "主角左手掌心灼伤，握拳疼痛",
                  "主角右手手背割伤，渗血"
                ],
                "ability_loci":[]
              }
            }"#,
        )
        .unwrap();
        let both = check_body_state_side_conflicts(
            &root,
            3,
            "他左手扶墙，右手推门，额角冒冷汗。\n",
        );
        assert!(
            both.is_empty(),
            "cross-fact bilateral must not deadlock: {both:?}"
        );
        let _ = std::fs::remove_dir_all(&projects);
    }

    #[test]
    fn board_uses_digest_facts_when_no_summary() {
        let projects = tmp_root("dig");
        let root = init_project(&projects, "sample-novel", "未定", 10).unwrap();
        let mut mem = ProjectMemory {
            version: 1,
            ..Default::default()
        };
        mem.recent_digests.push(ChapterDigest {
            chapter: 3,
            event_summary: "夜袭".into(),
            hook: "追兵将近".into(),
            key_facts: vec![
                "主角右膝旧伤复发，奔跑跛行".into(),
                "权能附着于左掌心铜环".into(),
            ],
            ..Default::default()
        });
        save_memory(&root, &mem).unwrap();
        let board = format_body_state_board(&root, 4);
        assert!(board.contains("右膝"), "{board}");
        assert!(board.contains("左掌") || board.contains("铜环"), "{board}");
        let _ = std::fs::remove_dir_all(&projects);
    }

    #[test]
    fn board_keeps_limb_control_beside_ability_locus() {
        let projects = tmp_root("ctrl");
        let root = init_project(&projects, "sample-novel", "未定", 10).unwrap();
        let ch_dir = chapter_dir(&root, 5);
        std::fs::create_dir_all(&ch_dir).unwrap();
        std::fs::write(
            ch_dir.join("summary.json"),
            r#"{
              "event_summary":"对峙。",
              "ending_hook":"门开了。",
              "new_facts":[
                "异能印记寄宿在右手腕内侧",
                "主角左臂约七成操控权被夺取，暂不归他控制"
              ]
            }"#,
        )
        .unwrap();
        let board = format_body_state_board(&root, 6);
        assert!(board.contains("右手腕") || board.contains("印记"), "{board}");
        assert!(board.contains("左臂") && board.contains("控制"), "{board}");
        assert!(board.contains("能力位置/载体/控制"), "{board}");
        let _ = std::fs::remove_dir_all(&projects);
    }

    #[test]
    fn per_character_board_filters_by_name() {
        let projects = tmp_root("per");
        let root = init_project(&projects, "sample-novel", "未定", 10).unwrap();
        let ch_dir = chapter_dir(&root, 1);
        std::fs::create_dir_all(&ch_dir).unwrap();
        std::fs::write(
            ch_dir.join("summary.json"),
            r#"{
              "event_summary":"对峙。",
              "ending_hook":"门开了。",
              "new_facts":[
                "张甲左肩贯穿伤，暂不能抬弓",
                "李乙右膝旧伤复发",
                "异能印记寄宿在张甲右手腕内侧",
                "左踝擦伤未点名"
              ],
              "body_state":{
                "injuries":["张甲左肩贯穿伤","李乙右膝旧伤","左踝擦伤"],
                "ability_loci":["张甲：异能印记在右手腕内侧"]
              }
            }"#,
        )
        .unwrap();
        let all = vec!["张甲".into(), "李乙".into()];
        let a = format_body_state_board_for_character(
            &root,
            2,
            &["张甲".into()],
            &all,
            true,
        );
        let b = format_body_state_board_for_character(
            &root,
            2,
            &["李乙".into()],
            &all,
            false,
        );
        assert!(a.contains("左肩"), "{a}");
        assert!(a.contains("左踝") || a.contains("擦伤"), "default owner gets unscoped: {a}");
        assert!(!a.contains("右膝"), "{a}");
        assert!(b.contains("右膝"), "{b}");
        assert!(!b.contains("左肩"), "{b}");
        assert!(!b.contains("右手腕"), "{b}");
        assert!(!b.contains("左踝"), "{b}");
        let _ = std::fs::remove_dir_all(&projects);
    }
}

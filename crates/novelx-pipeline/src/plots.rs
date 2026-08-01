//! Plot-card lifecycle: volume index, status machine, chapter selection.

use crate::cards::{load_markdown_cards, truncate_chars, MarkdownCard};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const PLOT_STATUSES: &[&str] = &[
    "planned",
    "in_progress",
    "bridging",
    "completed",
    "abandoned",
];

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlotIndex {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub volumes: Vec<PlotVolumeIndex>,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlotVolumeIndex {
    pub volume_index: u32,
    #[serde(default)]
    pub active_main_plot: String,
    #[serde(default)]
    pub plots: Vec<PlotIndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlotIndexEntry {
    pub id: String,
    pub slug: String,
    pub title: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default = "default_plot_type")]
    pub plot_type: String,
    /// Optional retrospective only — plot cards must not plan chapter spans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapter_from: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapter_to: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_chapter: Option<u32>,
    /// Agent decision: after main beats land, write at most one digest chapter (0 or 1).
    #[serde(default)]
    pub needs_bridge: bool,
    /// True after that single bridge chapter was published (never bridge twice).
    #[serde(default)]
    pub bridge_done: bool,
    #[serde(default)]
    pub next_plot: String,
}

fn default_status() -> String {
    "planned".into()
}

fn default_plot_type() -> String {
    "local".into()
}

#[derive(Debug, Clone)]
pub struct PlotAdvanceEvent {
    pub title: String,
    pub from: String,
    pub to: String,
}

pub fn plot_index_path(project_dir: &Path) -> PathBuf {
    project_dir.join("plots/index.json")
}

pub fn load_plot_index(project_dir: &Path) -> PlotIndex {
    let path = plot_index_path(project_dir);
    if !path.exists() {
        return rebuild_plot_index(project_dir);
    }
    let mut index = fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<PlotIndex>(&t).ok())
        .unwrap_or_else(|| rebuild_plot_index(project_dir));
    for vol in &mut index.volumes {
        sort_plot_entries_by_progress(&mut vol.plots);
    }
    index.volumes.sort_by_key(|v| v.volume_index);
    index
}

pub fn save_plot_index(project_dir: &Path, index: &PlotIndex) -> anyhow::Result<()> {
    let path = plot_index_path(project_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(index)?;
    fs::write(path, text)?;
    Ok(())
}

/// Scan `plots/*.md` and rebuild index (preserves active_main when still valid).
pub fn rebuild_plot_index(project_dir: &Path) -> PlotIndex {
    let prev = {
        let path = plot_index_path(project_dir);
        fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<PlotIndex>(&t).ok())
    };
    let prev_active: HashMap<u32, String> = prev
        .as_ref()
        .map(|p| {
            p.volumes
                .iter()
                .map(|v| (v.volume_index, v.active_main_plot.clone()))
                .collect()
        })
        .unwrap_or_default();

    let cards = load_plot_cards(project_dir);
    let mut by_vol: HashMap<u32, Vec<PlotIndexEntry>> = HashMap::new();
    for card in &cards {
        let vol = volume_index_of(card);
        let entry = entry_from_card(card);
        by_vol.entry(vol).or_default().push(entry);
    }

    let mut volumes: Vec<PlotVolumeIndex> = by_vol
        .into_iter()
        .map(|(volume_index, mut plots)| {
            sort_plot_entries_by_progress(&mut plots);
            let mut active = prev_active
                .get(&volume_index)
                .cloned()
                .unwrap_or_default();
            if !active.is_empty()
                && !plots.iter().any(|p| {
                    (p.title == active || p.slug == active)
                        && matches!(p.status.as_str(), "in_progress" | "bridging")
                })
            {
                active.clear();
            }
            if active.is_empty() {
                if let Some(p) = plots.iter().find(|p| {
                    is_main_type(&p.plot_type)
                        && matches!(p.status.as_str(), "in_progress" | "bridging")
                }) {
                    active = p.title.clone();
                }
            }
            PlotVolumeIndex {
                volume_index,
                active_main_plot: active,
                plots,
            }
        })
        .collect();
    volumes.sort_by_key(|v| v.volume_index);

    let index = PlotIndex {
        version: 1,
        volumes,
    };
    let _ = save_plot_index(project_dir, &index);
    index
}

pub fn load_plot_cards(project_dir: &Path) -> Vec<MarkdownCard> {
    let mut cards = load_markdown_cards(&project_dir.join("plots"), "plot");
    cards.retain(|c| c.slug != "index");
    for card in &mut cards {
        enrich_plot_meta(card);
        normalize_plot_status_meta(card);
    }
    cards
}

/// Load plot cards sorted by story progress (volume → next_plot chain → status).
pub fn load_plot_cards_by_progress(project_dir: &Path) -> Vec<MarkdownCard> {
    let mut cards = load_plot_cards(project_dir);
    sort_plot_cards_by_progress(&mut cards);
    cards
}

fn enrich_plot_meta(card: &mut MarkdownCard) {
    // Pull fields from trailing / embedded JSON produced by plot-designer.
    if let Some(json_slice) = extract_json_object(&card.markdown) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_slice) {
            let fields = v.get("fields").cloned().unwrap_or(v.clone());
            for (src, key) in [
                ("status", "status"),
                ("plot_type", "plot_type"),
                ("scope", "scope"),
                ("arc", "arc"),
                ("arc_index", "arc_index"),
                ("volume_index", "volume_index"),
            ] {
                if !card.meta.contains_key(key) {
                    if let Some(s) = v.get(src).and_then(|x| x.as_str()) {
                        card.meta.insert(key.into(), s.to_string());
                    } else if let Some(n) = v.get(src).and_then(|x| x.as_u64()) {
                        card.meta.insert(key.into(), n.to_string());
                    }
                }
            }
            for (src, key) in [
                ("chapter_from", "chapter_from"),
                ("chapter_to", "chapter_to"),
                ("bridge_chapter", "bridge_chapter"),
                ("next_plot", "next_plot"),
            ] {
                if !card.meta.contains_key(key) {
                    if let Some(n) = fields.get(src).and_then(|x| x.as_u64()) {
                        card.meta.insert(key.into(), n.to_string());
                    } else if let Some(s) = fields.get(src).and_then(|x| x.as_str()) {
                        if !s.trim().is_empty() {
                            card.meta.insert(key.into(), s.trim().to_string());
                        }
                    }
                }
            }
            if !card.meta.contains_key("status") {
                if let Some(s) = fields.get("status").and_then(|x| x.as_str()) {
                    card.meta.insert("status".into(), s.to_string());
                }
            }
        }
    }
    // Human-readable Chinese status in body header.
    if !card.meta.contains_key("status") {
        if card.markdown.contains("状态：进行中") || card.markdown.contains("状态: 进行中") {
            card.meta.insert("status".into(), "in_progress".into());
        } else if card.markdown.contains("状态：已完成") || card.markdown.contains("状态: 已完成")
        {
            card.meta.insert("status".into(), "completed".into());
        } else if card.markdown.contains("状态：衔接") {
            card.meta.insert("status".into(), "bridging".into());
        } else if card.markdown.contains("状态：规划") || card.markdown.contains("状态：待开始") {
            card.meta.insert("status".into(), "planned".into());
        }
    }
}

fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.rfind('{')?;
    let slice = &text[start..];
    if slice.contains("\"title\"") || slice.contains("\"fields\"") || slice.contains("\"status\"") {
        // Find matching closing brace from the end.
        let end = slice.rfind('}')?;
        Some(&slice[..=end])
    } else {
        None
    }
}

fn normalize_plot_status_meta(card: &mut MarkdownCard) {
    let raw = card
        .meta
        .get("status")
        .cloned()
        .unwrap_or_else(|| "planned".into());
    card.meta
        .insert("status".into(), normalize_status(&raw));
    if let Some(pt) = card.meta.get("plot_type").cloned() {
        card.meta
            .insert("plot_type".into(), normalize_plot_type(&pt));
    } else if card
        .meta
        .get("scope")
        .map(|s| s == "volume")
        .unwrap_or(false)
    {
        card.meta.insert("plot_type".into(), "main".into());
    } else {
        card.meta
            .insert("plot_type".into(), "local".into());
    }
}

pub fn normalize_status(raw: &str) -> String {
    let s = raw.trim().to_lowercase();
    match s.as_str() {
        "planned" | "plan" | "draft" | "todo" | "待开始" | "规划" => "planned".into(),
        "in_progress" | "inprogress" | "active" | "running" | "进行中" | "进行" => {
            "in_progress".into()
        }
        "bridging" | "bridge" | "衔接" | "衔接中" => "bridging".into(),
        "completed" | "complete" | "done" | "finished" | "已完成" | "完成" => "completed".into(),
        "abandoned" | "dropped" | "cancelled" | "canceled" | "废弃" | "放弃" => {
            "abandoned".into()
        }
        _ => {
            if PLOT_STATUSES.contains(&s.as_str()) {
                s
            } else {
                "planned".into()
            }
        }
    }
}

pub fn normalize_plot_type(raw: &str) -> String {
    let s = raw.trim().to_lowercase();
    match s.as_str() {
        "main" | "mainline" | "volume" | "主线" => "main".into(),
        "subplot" | "local" | "side" | "支线" | "局部" => "local".into(),
        _ => {
            if s.is_empty() {
                "local".into()
            } else {
                s
            }
        }
    }
}

fn is_main_type(plot_type: &str) -> bool {
    matches!(
        normalize_plot_type(plot_type).as_str(),
        "main" | "mainline" | "volume"
    )
}

/// Story-progress rank: earlier arcs first, then active, then planned.
fn progress_rank(status: &str) -> u8 {
    match normalize_status(status).as_str() {
        "completed" => 0,
        "bridging" => 1,
        "in_progress" => 2,
        "planned" => 3,
        "abandoned" => 4,
        _ => 5,
    }
}

fn plot_ref_matches(entry_title: &str, entry_slug: &str, next: &str) -> bool {
    let n = next.trim();
    if n.is_empty() {
        return false;
    }
    n == entry_title || n == entry_slug || entry_title.starts_with(n) || n.starts_with(entry_title)
}

/// Order entries by narrative progress: follow `next_plot` chains, then status.
pub fn sort_plot_entries_by_progress(plots: &mut [PlotIndexEntry]) {
    if plots.len() <= 1 {
        return;
    }
    let n = plots.len();
    let mut incoming = vec![0u32; n];
    let mut edges: Vec<Vec<usize>> = vec![vec![]; n];
    for (i, a) in plots.iter().enumerate() {
        if a.next_plot.trim().is_empty() {
            continue;
        }
        if let Some(j) = plots.iter().enumerate().find_map(|(j, b)| {
            if i != j && plot_ref_matches(&b.title, &b.slug, &a.next_plot) {
                Some(j)
            } else {
                None
            }
        }) {
            edges[i].push(j);
            incoming[j] += 1;
        }
    }

    let mut order = Vec::with_capacity(n);
    let mut ready: Vec<usize> = (0..n).filter(|&i| incoming[i] == 0).collect();
    ready.sort_by(|&i, &j| {
        progress_rank(&plots[i].status)
            .cmp(&progress_rank(&plots[j].status))
            .then(plots[i].slug.cmp(&plots[j].slug))
    });
    while let Some(i) = ready.first().copied() {
        ready.remove(0);
        order.push(i);
        for &j in &edges[i] {
            incoming[j] = incoming[j].saturating_sub(1);
            if incoming[j] == 0 {
                ready.push(j);
                ready.sort_by(|&a, &b| {
                    progress_rank(&plots[a].status)
                        .cmp(&progress_rank(&plots[b].status))
                        .then(plots[a].slug.cmp(&plots[b].slug))
                });
            }
        }
    }
    // Cycles / leftovers: append by progress rank.
    if order.len() < n {
        let mut rest: Vec<usize> = (0..n).filter(|i| !order.contains(i)).collect();
        rest.sort_by(|&i, &j| {
            progress_rank(&plots[i].status)
                .cmp(&progress_rank(&plots[j].status))
                .then(plots[i].slug.cmp(&plots[j].slug))
        });
        order.extend(rest);
    }

    let sorted: Vec<PlotIndexEntry> = order.into_iter().map(|i| plots[i].clone()).collect();
    plots.clone_from_slice(&sorted);
}

/// Order markdown plot cards the same way as the volume index.
pub fn sort_plot_cards_by_progress(cards: &mut [MarkdownCard]) {
    if cards.len() <= 1 {
        return;
    }
    let mut by_vol: Vec<(u32, usize, PlotIndexEntry)> = cards
        .iter()
        .enumerate()
        .map(|(i, c)| (volume_index_of(c), i, entry_from_card(c)))
        .collect();
    by_vol.sort_by_key(|(vol, idx, _)| (*vol, *idx));

    let mut out_order = Vec::with_capacity(cards.len());
    let mut start = 0;
    while start < by_vol.len() {
        let vol = by_vol[start].0;
        let mut end = start + 1;
        while end < by_vol.len() && by_vol[end].0 == vol {
            end += 1;
        }
        let mut group: Vec<PlotIndexEntry> = by_vol[start..end].iter().map(|x| x.2.clone()).collect();
        let card_indices: Vec<usize> = by_vol[start..end].iter().map(|x| x.1).collect();
        sort_plot_entries_by_progress(&mut group);
        let mut used = vec![false; card_indices.len()];
        for entry in &group {
            if let Some(pos) = card_indices.iter().enumerate().find_map(|(k, &ci)| {
                if used[k] {
                    return None;
                }
                let c = &cards[ci];
                if c.title == entry.title || c.slug == entry.slug || c.id == entry.id {
                    Some(k)
                } else {
                    None
                }
            }) {
                used[pos] = true;
                out_order.push(card_indices[pos]);
            }
        }
        for (k, &ci) in card_indices.iter().enumerate() {
            if !used[k] {
                out_order.push(ci);
            }
        }
        start = end;
    }

    let sorted: Vec<MarkdownCard> = out_order.into_iter().map(|i| cards[i].clone()).collect();
    cards.clone_from_slice(&sorted);
}

pub fn volume_index_of(card: &MarkdownCard) -> u32 {
    card.meta
        .get("volume_index")
        .or_else(|| card.meta.get("arc_index"))
        .or_else(|| card.meta.get("act"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
}

fn meta_flag(card: &MarkdownCard, key: &str) -> bool {
    card.meta
        .get(key)
        .map(|s| matches!(s.trim().to_lowercase().as_str(), "true" | "yes" | "1"))
        .unwrap_or(false)
}

fn entry_from_card(card: &MarkdownCard) -> PlotIndexEntry {
    PlotIndexEntry {
        id: card.id.clone(),
        slug: card.slug.clone(),
        title: card.title.clone(),
        status: card
            .meta
            .get("status")
            .cloned()
            .map(|s| normalize_status(&s))
            .unwrap_or_else(|| "planned".into()),
        plot_type: card
            .meta
            .get("plot_type")
            .cloned()
            .map(|s| normalize_plot_type(&s))
            .unwrap_or_else(|| "local".into()),
        // Chapter spans are legacy/optional retrospective — never required for guidance cards.
        chapter_from: card.meta.get("chapter_from").and_then(|s| s.parse().ok()),
        chapter_to: card.meta.get("chapter_to").and_then(|s| s.parse().ok()),
        bridge_chapter: card
            .meta
            .get("bridge_chapter")
            .and_then(|s| s.parse().ok()),
        needs_bridge: meta_flag(card, "needs_bridge"),
        bridge_done: meta_flag(card, "bridge_done"),
        next_plot: card
            .meta
            .get("next_plot")
            .cloned()
            .unwrap_or_default(),
    }
}

/// After publish: rebuild plot index / active_main only.
///
/// Status is **never** advanced by `chapter_from`/`chapter_to`/`bridge_chapter`.
/// Completion is owned by `plot_acceptor` (and manual `update_plot`); bridging is
/// entered via `ensure_bridge_plot_active` before writing the bridge chapter, then
/// closed by `complete_bridging_plots_after_publish` after that chapter publishes.
pub fn advance_plots_for_published_chapter(
    project_dir: &Path,
    _published_chapter: u32,
) -> anyhow::Result<Vec<PlotAdvanceEvent>> {
    let mut index = rebuild_plot_index(project_dir);
    for vol in &mut index.volumes {
        if vol.plots.iter().any(|p| {
            (p.title == vol.active_main_plot || p.slug == vol.active_main_plot)
                && matches!(p.status.as_str(), "completed" | "abandoned")
        }) {
            vol.active_main_plot.clear();
        }
        if vol.active_main_plot.is_empty() {
            if let Some(p) = vol.plots.iter().find(|p| {
                is_main_type(&p.plot_type)
                    && matches!(p.status.as_str(), "in_progress" | "bridging")
            }) {
                vol.active_main_plot = p.title.clone();
            }
        }
    }
    save_plot_index(project_dir, &index)?;
    Ok(Vec::new())
}

fn patch_plot_card_status(
    path: &Path,
    card: &MarkdownCard,
    new_status: &str,
) -> anyhow::Result<()> {
    let text = fs::read_to_string(path)?;
    let updated = set_status_in_markdown(&text, new_status);
    fs::write(path, updated)?;
    let _ = card;
    Ok(())
}

fn set_status_in_markdown(text: &str, new_status: &str) -> String {
    let mut text = text.to_string();
    // Frontmatter status:
    if text.trim_start().starts_with("---") {
        let parts: Vec<&str> = text.splitn(3, "---").collect();
        if parts.len() >= 3 {
            let mut fm = parts[1].to_string();
            if fm.contains("status:") {
                let mut lines = Vec::new();
                for line in fm.lines() {
                    if line.trim_start().starts_with("status:") {
                        lines.push(format!("status: {new_status}"));
                    } else {
                        lines.push(line.to_string());
                    }
                }
                fm = lines.join("\n");
                if !fm.ends_with('\n') {
                    fm.push('\n');
                }
            } else {
                if !fm.ends_with('\n') {
                    fm.push('\n');
                }
                fm.push_str(&format!("status: {new_status}\n"));
            }
            text = format!("---\n{}---{}", fm.trim_start_matches('\n'), parts[2]);
        }
    }

    // Replace JSON "status" near end if present.
    if let Some(start) = text.rfind('{') {
        let head = &text[..start];
        let tail = &text[start..];
        if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(tail) {
            if let Some(obj) = v.as_object_mut() {
                obj.insert("status".into(), json!(new_status));
                if let Some(fields) = obj.get_mut("fields").and_then(|f| f.as_object_mut()) {
                    fields.insert("status".into(), json!(new_status));
                }
                text = format!("{head}{}", serde_json::to_string_pretty(&v).unwrap_or_else(|_| tail.to_string()));
            }
        }
    }

    // Chinese label in body.
    let label = match new_status {
        "planned" => "规划中",
        "in_progress" => "进行中",
        "bridging" => "衔接中",
        "completed" => "已完成",
        "abandoned" => "已废弃",
        _ => new_status,
    };
    for (old, _) in [
        ("状态：进行中", ()),
        ("状态: 进行中", ()),
        ("状态：已完成", ()),
        ("状态：衔接中", ()),
        ("状态：规划中", ()),
        ("状态：待开始", ()),
        ("状态：active", ()),
    ] {
        if text.contains(old) {
            text = text.replace(old, &format!("状态：{label}"));
        }
    }
    text
}

/// Manual status / field update for a plot card (+ rebuild index).
pub fn update_plot_card(
    project_dir: &Path,
    title_or_slug: &str,
    status: Option<&str>,
    chapter_from: Option<u32>,
    chapter_to: Option<u32>,
    bridge_chapter: Option<u32>,
    next_plot: Option<&str>,
    set_active_main: bool,
) -> anyhow::Result<serde_json::Value> {
    let cards = load_plot_cards(project_dir);
    let card = cards
        .iter()
        .find(|c| {
            c.title == title_or_slug
                || c.slug == title_or_slug
                || c.id == title_or_slug
                || c.title.contains(title_or_slug)
        })
        .ok_or_else(|| anyhow::anyhow!("未找到剧情卡：{title_or_slug}"))?;
    let path = project_dir.join("plots").join(format!("{}.md", card.slug));
    let mut text = fs::read_to_string(&path)?;

    if let Some(st) = status {
        let st = normalize_status(st);
        text = set_status_in_markdown(&text, &st);
    }
    // chapter_from/to/bridge_chapter are deprecated metadata (ignored by lifecycle).
    // Still persist if callers send them, for display / migration only.
    if chapter_from.is_some() || chapter_to.is_some() || bridge_chapter.is_some() {
        tracing::warn!(
            plot = %card.title,
            "chapter_from/to/bridge_chapter are ignored for plot lifecycle; use 收束条件 + plot_acceptor"
        );
    }
    text = upsert_frontmatter_fields(
        &text,
        &[
            ("chapter_from", chapter_from.map(|n| n.to_string())),
            ("chapter_to", chapter_to.map(|n| n.to_string())),
            ("bridge_chapter", bridge_chapter.map(|n| n.to_string())),
            ("next_plot", next_plot.map(|s| s.to_string())),
            (
                "volume_index",
                Some(volume_index_of(card).to_string()),
            ),
        ],
    );
    fs::write(&path, &text)?;

    let mut index = rebuild_plot_index(project_dir);
    let vol_i = volume_index_of(card);
    let new_status = status.map(normalize_status);
    if set_active_main || new_status.as_deref() == Some("in_progress") {
        if let Some(vol) = index.volumes.iter_mut().find(|v| v.volume_index == vol_i) {
            if is_main_type(
                card.meta
                    .get("plot_type")
                    .map(|s| s.as_str())
                    .unwrap_or("main"),
            ) || set_active_main
            {
                vol.active_main_plot = card.title.clone();
            }
        }
        save_plot_index(project_dir, &index)?;
    }
    // Activating a main plot closes volume handoff / unlocks drafting.
    if new_status.as_deref() == Some("in_progress")
        || new_status.as_deref() == Some("bridging")
        || set_active_main
    {
        let _ = crate::phases::set_volume_phase(
            project_dir,
            crate::phases::VolumePhase::DraftingVolume,
        );
    }

    Ok(json!({
        "title": card.title,
        "slug": card.slug,
        "path": path.display().to_string(),
        "status": new_status,
        "index": index,
    }))
}

fn upsert_frontmatter_fields(text: &str, fields: &[(&str, Option<String>)]) -> String {
    if !text.trim_start().starts_with("---") {
        let mut fm = String::from("---\n");
        for (k, v) in fields {
            if let Some(val) = v {
                fm.push_str(&format!("{k}: {val}\n"));
            }
        }
        fm.push_str("---\n\n");
        return format!("{fm}{text}");
    }
    let parts: Vec<&str> = text.splitn(3, "---").collect();
    if parts.len() < 3 {
        return text.to_string();
    }
    let mut map: HashMap<String, String> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for line in parts[1].lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_string();
            order.push(key.clone());
            map.insert(key, v.trim().to_string());
        }
    }
    for (k, v) in fields {
        if let Some(val) = v {
            if !map.contains_key(*k) {
                order.push((*k).into());
            }
            map.insert((*k).into(), val.clone());
        }
    }
    let mut fm = String::new();
    for k in order {
        if let Some(v) = map.get(&k) {
            fm.push_str(&format!("{k}: {v}\n"));
        }
    }
    format!("---\n{fm}---{}", parts[2])
}

/// Select plot cards for chapter context (lifecycle-aware).
pub fn select_plots_for_chapter(
    project_dir: &Path,
    chapter: u32,
    haystack: &str,
) -> (String, Vec<String>) {
    let index = load_plot_index(project_dir);
    let cards = load_plot_cards(project_dir);
    if cards.is_empty() {
        return (String::new(), Vec::new());
    }

    let mut scored: Vec<(i32, &MarkdownCard)> = Vec::new();
    for card in &cards {
        let status = card
            .meta
            .get("status")
            .map(|s| normalize_status(s))
            .unwrap_or_else(|| "planned".into());
        if status == "abandoned" {
            continue;
        }
        let vol = volume_index_of(card);
        let active_main = index
            .volumes
            .iter()
            .find(|v| v.volume_index == vol)
            .map(|v| v.active_main_plot.as_str())
            .unwrap_or("");

        let _ = chapter; // guidance cards are not bound to chapter spans
        let keys = card.match_keys();
        let hit = keys
            .iter()
            .any(|k| k.chars().count() >= 2 && haystack.contains(k));

        let mut score = 0i32;
        if card.title == active_main || card.slug == active_main {
            score += 100;
        }
        match status.as_str() {
            "in_progress" => score += 80,
            "bridging" => score += 70,
            "planned" => score += 10,
            "completed" => score -= 40,
            _ => {}
        }
        // Legacy retrospective spans only (optional); not used for planning.
        if let Some((f, t)) = card.chapter_range() {
            if chapter >= f && chapter <= t {
                score += 15;
            }
        }
        if hit {
            score += 20;
        }
        if is_main_type(
            card.meta
                .get("plot_type")
                .map(|s| s.as_str())
                .unwrap_or(""),
        ) {
            score += 5;
        }
        // Drop completed cards unless keyword-hit (reference only).
        if status == "completed" && !hit {
            continue;
        }
        if score > 0 {
            scored.push((score, card));
        }
    }

    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.truncate(2);

    // Soft fallback: active/in_progress only, never a random first file.
    if scored.is_empty() {
        if let Some(card) = cards.iter().find(|c| {
            matches!(
                c.meta
                    .get("status")
                    .map(|s| normalize_status(s))
                    .as_deref(),
                Some("in_progress") | Some("bridging")
            )
        }) {
            scored.push((1, card));
        }
    }

    let mut hits = Vec::new();
    let mut blocks = Vec::new();
    for (_, p) in scored {
        hits.push(format!("plot:{}", p.title));
        let status = p
            .meta
            .get("status")
            .cloned()
            .unwrap_or_else(|| "planned".into());
        let body = truncate_chars(p.markdown.trim(), 700);
        blocks.push(format!(
            "### {} 〔status={status}〕\n{body}",
            p.title
        ));
    }
    (blocks.join("\n\n"), hits)
}

pub fn list_plots_summary(
    project_dir: &Path,
    volume_filter: Option<u32>,
    status_filter: Option<&str>,
) -> serde_json::Value {
    let index = load_plot_index(project_dir);
    let status_filter = status_filter.map(normalize_status);
    let mut rows = Vec::new();
    for vol in &index.volumes {
        if volume_filter.is_some_and(|v| v != vol.volume_index) {
            continue;
        }
        for p in &vol.plots {
            if status_filter
                .as_ref()
                .is_some_and(|s| s != &p.status)
            {
                continue;
            }
            rows.push(json!({
                "volume_index": vol.volume_index,
                "active_main_plot": vol.active_main_plot,
                "id": p.id,
                "slug": p.slug,
                "title": p.title,
                "status": p.status,
                "plot_type": p.plot_type,
                "needs_bridge": p.needs_bridge,
                "bridge_done": p.bridge_done,
                "next_plot": p.next_plot,
            }));
        }
    }
    json!({
        "version": index.version,
        "plots": rows,
        "volumes": index.volumes,
    })
}

fn status_label_zh(status: &str) -> &'static str {
    match normalize_status(status).as_str() {
        "completed" => "已完成",
        "in_progress" => "进行中",
        "bridging" => "衔接中",
        "planned" => "待开始",
        "abandoned" => "已废弃",
        _ => "未知",
    }
}

/// User-facing Chinese progress report (for「剧情进行到哪了」/ `list_plots`).
/// `volume_focus`：问「第N卷」时只展开该卷；`None` 则按当前写作进度选卷。
pub fn format_plot_progress_report(project_dir: &Path) -> String {
    format_plot_progress_report_for(project_dir, None)
}

pub fn format_plot_progress_report_for(project_dir: &Path, volume_focus: Option<u32>) -> String {
    let _ = rebuild_plot_index(project_dir);
    let index = load_plot_index(project_dir);
    let state = crate::load_project_state(project_dir).ok();
    let published = state.as_ref().map(|s| s.published_count).unwrap_or(0);
    let next_ch = state
        .as_ref()
        .map(|s| s.next_chapter.max(1))
        .unwrap_or(1);
    let chapter_for_vol = published.max(1);
    let cur_vol = volume_focus.unwrap_or_else(|| {
        crate::volume::active_volume_for_chapter(project_dir, chapter_for_vol)
            .map(|b| b.volume_index)
            .or_else(|| {
                index
                    .volumes
                    .iter()
                    .find(|v| {
                        v.plots
                            .iter()
                            .any(|p| matches!(p.status.as_str(), "in_progress" | "bridging"))
                    })
                    .map(|v| v.volume_index)
            })
            .or_else(|| index.volumes.last().map(|v| v.volume_index))
            .unwrap_or(1)
    });

    let mut out = Vec::new();
    if volume_focus.is_some() {
        out.push(format!(
            "第{cur_vol}卷进度：全书已写 {published} 章，下一章为第 {next_ch} 章。"
        ));
    } else {
        out.push(format!(
            "进度：已写 {published} 章，下一章为第 {next_ch} 章；当前卷约第 {cur_vol} 卷。"
        ));
    }

    let mut active: Option<(u32, &PlotIndexEntry)> = None;
    // Prefer in-progress card inside the focused volume when asking「第N卷」.
    for vol in &index.volumes {
        if volume_focus.is_some_and(|v| v != vol.volume_index) {
            continue;
        }
        for p in &vol.plots {
            if matches!(p.status.as_str(), "in_progress" | "bridging") && is_main_type(&p.plot_type)
            {
                active = Some((vol.volume_index, p));
            }
        }
    }
    if active.is_none() && volume_focus.is_some() {
        for vol in &index.volumes {
            for p in &vol.plots {
                if matches!(p.status.as_str(), "in_progress" | "bridging")
                    && is_main_type(&p.plot_type)
                {
                    active = Some((vol.volume_index, p));
                }
            }
        }
    }

    if let Some((vi, p)) = active {
        let cards = load_plot_cards(project_dir);
        let exit = cards
            .iter()
            .find(|c| c.title == p.title || c.slug == p.slug)
            .map(|c| extract_plot_exit_condition(&c.markdown))
            .filter(|s| !s.trim().is_empty());
        if p.status == "bridging" {
            out.push(format!(
                "当前阶段：第{vi}卷「{}」衔接章（消化上一落点，勿开全新主线高潮）。",
                p.title
            ));
        } else {
            out.push(format!("当前主推：第{vi}卷「{}」。", p.title));
        }
        if let Some(ex) = exit {
            out.push(format!("本卡收束条件：{ex}"));
            out.push("该收束尚未验收完结前，主线停在本卡。".into());
        }
        if !p.next_plot.trim().is_empty() {
            out.push(format!("收束后下一张：{}", p.next_plot.trim()));
        }
    } else if let Some(pending) = pending_bridge_plot(&index) {
        out.push(format!(
            "当前阶段：上一卡「{}」已完成，仍欠 1 章衔接（needs_bridge）。续写将先写衔接章。",
            pending.title
        ));
    } else {
        out.push("当前没有进行中的主线剧情卡。若要继续写章，请先 design_plot 并 update_plot(in_progress)。".into());
    }

    out.push(String::new());
    out.push(format!("第{cur_vol}卷剧情卡："));
    if let Some(vol) = index.volumes.iter().find(|v| v.volume_index == cur_vol) {
        if vol.plots.is_empty() {
            out.push("（本卷尚无剧情卡）".into());
        } else {
            for p in &vol.plots {
                let mark = match p.status.as_str() {
                    "in_progress" | "bridging" => "▶",
                    "completed" => "✓",
                    "planned" => "·",
                    _ => "×",
                };
                out.push(format!(
                    "{mark} {}（{}）",
                    p.title,
                    status_label_zh(&p.status)
                ));
            }
        }
    } else {
        out.push("（未找到当前卷索引）".into());
    }

    // Other volumes: one-line summary only.
    let other: Vec<String> = index
        .volumes
        .iter()
        .filter(|v| v.volume_index != cur_vol)
        .map(|v| {
            let done = v.plots.iter().filter(|p| p.status == "completed").count();
            let total = v.plots.len();
            format!("第{}卷 {done}/{total} 张已完成", v.volume_index)
        })
        .collect();
    if !other.is_empty() {
        out.push(String::new());
        out.push(format!("其他卷：{}", other.join("；")));
    }

    out.push(String::new());
    out.push("若要续写，直接说「继续创作」。".into());

    out.join("\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlotWriteGate {
    Allow { mode: PlotWriteMode, detail: String },
    Block {
        message: String,
        /// Stable code for UI gates (`planned_inactive`, `need_design_plot`, …).
        reason: &'static str,
        /// Planned / active plot title when relevant.
        plot_title: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlotWriteMode {
    /// Writing against an in_progress card.
    ActivePlot,
    /// System/agent-scheduled digest chapter (at most one per plot).
    Bridge,
}

/// Latest completed main plot that still owes a bridge chapter (agent said needs_bridge, not yet done).
pub fn pending_bridge_plot<'a>(index: &'a PlotIndex) -> Option<&'a PlotIndexEntry> {
    index
        .volumes
        .iter()
        .flat_map(|v| &v.plots)
        .rev()
        .find(|p| {
            is_main_type(&p.plot_type)
                && p.status == "completed"
                && p.needs_bridge
                && !p.bridge_done
        })
}

/// Volume that should accept the next chapter: pending bridge / active plot / open act.
fn writing_focus_volume(index: &PlotIndex, project_dir: &Path) -> Option<u32> {
    if let Some(p) = pending_bridge_plot(index) {
        for v in &index.volumes {
            if v.plots.iter().any(|x| x.id == p.id || x.title == p.title) {
                return Some(v.volume_index);
            }
        }
    }
    for v in &index.volumes {
        if v.plots
            .iter()
            .any(|p| matches!(p.status.as_str(), "in_progress" | "bridging"))
        {
            return Some(v.volume_index);
        }
    }
    if let Ok(state) = crate::project::load_project_state(project_dir) {
        let ch = state.next_chapter.max(1);
        if let Some(b) = crate::volume::active_volume_for_chapter(project_dir, ch) {
            return Some(b.volume_index);
        }
    }
    index.volumes.last().map(|v| v.volume_index)
}

/// Gate continue_writing.
/// Bridge chapters are decided by the plot card (`needs_bridge`), not the user — 0 or 1 only.
///
/// Guidance cards have no chapter spans: completion is driven by **收束条件**
/// (`plot_acceptor` pass), not a default chapter count.
pub fn check_plot_write_gate(project_dir: &Path) -> PlotWriteGate {
    check_plot_write_gate_with(project_dir, crate::phases::PhaseEnforceFlags::default())
}

/// Like [`check_plot_write_gate`], but setup/volume phase hard-blocks follow feature flags.
pub fn check_plot_write_gate_with(
    project_dir: &Path,
    enforce: crate::phases::PhaseEnforceFlags,
) -> PlotWriteGate {
    if enforce.setup {
        if let Some(msg) = crate::phases::setup_write_block_reason(project_dir) {
            return PlotWriteGate::Block {
                message: msg,
                reason: "setup",
                plot_title: None,
            };
        }
    }
    if enforce.volume {
        if let Some(msg) = crate::phases::volume_write_block_reason(project_dir) {
            return PlotWriteGate::Block {
                message: msg,
                reason: "volume_phase",
                plot_title: None,
            };
        }
    }

    let index = load_plot_index(project_dir);
    let all: Vec<&PlotIndexEntry> = index.volumes.iter().flat_map(|v| v.plots.iter()).collect();
    if all.is_empty() {
        return PlotWriteGate::Block {
            message: "尚无剧情卡。请先调用 design_plot 创建指引，再写章（剧情卡不预估章数）。".into(),
            reason: "need_design_plot",
            plot_title: None,
        };
    }

    if let Some(p) = all
        .iter()
        .find(|p| matches!(p.status.as_str(), "in_progress" | "bridging"))
    {
        if p.status == "in_progress" {
            if let Some((title, _)) = in_progress_plot_missing_exit(project_dir) {
                return PlotWriteGate::Block {
                    message: format!(
                        "进行中剧情卡「{title}」缺少可检验的「收束条件」。请先补全卡面收束条件后再写章，否则无法验收完结。"
                    ),
                    reason: "missing_exit",
                    plot_title: Some(title),
                };
            }
        }
        return PlotWriteGate::Allow {
            mode: if p.status == "bridging" {
                PlotWriteMode::Bridge
            } else {
                PlotWriteMode::ActivePlot
            },
            detail: format!("使用剧情卡「{}」({})", p.title, p.status),
        };
    }

    // Bridge owes the current volume — must win over leftover `planned` cards on prior volumes
    // (e.g. vol2 planned while vol3 already needs a bridge chapter).
    if let Some(p) = pending_bridge_plot(&index) {
        return PlotWriteGate::Allow {
            mode: PlotWriteMode::Bridge,
            detail: format!(
                "Agent 判定「{}」需要 1 章衔接（消化余波→下一剧情）；本章自动按衔接章创作",
                p.title
            ),
        };
    }

    // Only block on planned cards in the volume that currently needs writing.
    let focus_vol = writing_focus_volume(&index, project_dir);
    if let Some(p) = index
        .volumes
        .iter()
        .filter(|v| focus_vol.map(|fv| v.volume_index == fv).unwrap_or(true))
        .flat_map(|v| v.plots.iter())
        .find(|p| is_main_type(&p.plot_type) && p.status == "planned")
    {
        return PlotWriteGate::Block {
            message: format!(
                "已有规划中的剧情卡「{}」，但尚未激活。请先 update_plot(title=\"{}\", status=\"in_progress\", set_active_main=true)，再写章。",
                p.title, p.title
            ),
            reason: "planned_inactive",
            plot_title: Some(p.title.clone()),
        };
    }

    let prev = all
        .iter()
        .rev()
        .find(|p| is_main_type(&p.plot_type) && p.status == "completed");
    let prev_title = prev.map(|p| p.title.as_str()).unwrap_or("前序剧情");
    let next_hint = prev
        .and_then(|p| {
            let n = p.next_plot.trim();
            (!n.is_empty()).then_some(n.to_string())
        })
        .map(|n| format!("上一卡指向下一情节：「{n}」。"))
        .unwrap_or_default();
    let next_title = prev
        .and_then(|p| {
            let n = p.next_plot.trim();
            (!n.is_empty()).then_some(n.to_string())
        })
        .unwrap_or_else(|| format!("{prev_title}·下一段"));
    PlotWriteGate::Block {
        message: format!(
            "「{prev_title}」已结束（无需衔接，或衔接章已写完）。{next_hint}\n\
             卷间交接：①（建议）design_arc_outline 细化下卷 → ② design_plot 开下一张剧情卡 → ③ update_plot(status=in_progress, set_active_main=true) → ④ 再 continue_writing。\n\
             勿在无进行中剧情卡时直接续写。"
        ),
        reason: "need_design_plot",
        plot_title: Some(next_title),
    }
}

/// Active in-progress plot that has no extractable 收束条件.
pub fn in_progress_plot_missing_exit(project_dir: &Path) -> Option<(String, String)> {
    let index = load_plot_index(project_dir);
    let entry = index
        .volumes
        .iter()
        .flat_map(|v| &v.plots)
        .find(|p| p.status == "in_progress")?;
    let cards = load_plot_cards(project_dir);
    let card = cards
        .iter()
        .find(|c| c.slug == entry.slug || c.title == entry.title || c.id == entry.id)?;
    let exit = extract_plot_exit_condition(&card.markdown);
    if exit.trim().is_empty() {
        Some((entry.title.clone(), card.slug.clone()))
    } else {
        None
    }
}

/// Active in-progress plot card + its narrative exit condition (收束).
pub fn active_plot_exit_context(project_dir: &Path) -> Option<(PlotIndexEntry, String, String)> {
    let index = load_plot_index(project_dir);
    let entry = index
        .volumes
        .iter()
        .flat_map(|v| &v.plots)
        .find(|p| p.status == "in_progress")?
        .clone();
    let cards = load_plot_cards(project_dir);
    let card = cards
        .iter()
        .find(|c| c.slug == entry.slug || c.title == entry.title || c.id == entry.id)?;
    let exit = extract_plot_exit_condition(&card.markdown);
    if exit.trim().is_empty() {
        return None;
    }
    Some((entry, exit, card.markdown.clone()))
}

/// Pull 收束条件 / exit_condition from plot card (YAML frontmatter, then body).
pub fn extract_plot_exit_condition(markdown: &str) -> String {
    // Frontmatter wins — avoids "## 收束" matching inside "## 收束条件".
    if let Some(fm) = yaml_frontmatter_exit_condition(markdown) {
        if is_usable_exit_condition(&fm) {
            return truncate_chars(&fm, 400);
        }
    }
    // Longer / more specific headings first; match only as a full heading line.
    for heading in ["## 进入 / 收束条件", "## 收束条件", "### 收束条件", "## 收束"] {
        if let Some(section) = section_after_exact_heading(markdown, heading) {
            let mut lines: Vec<&str> = Vec::new();
            for line in section.lines() {
                let t = line.trim();
                if t.starts_with("## ") {
                    break;
                }
                if t.is_empty() {
                    continue;
                }
                if t.contains("进入") && !t.contains("收束") && lines.is_empty() {
                    continue;
                }
                if t.starts_with("- **进入**") || t.starts_with("- **进入**：") {
                    continue;
                }
                let cleaned = t
                    .trim_start_matches("- ")
                    .trim_start_matches("**收束**：")
                    .trim_start_matches("**收束**:")
                    .trim()
                    .trim_start_matches('：')
                    .trim_start_matches(':')
                    .trim();
                // Leftover from a prefix match (should not happen with exact heading).
                if cleaned == "条件" || cleaned.eq_ignore_ascii_case("condition") {
                    continue;
                }
                if cleaned.is_empty() || cleaned.starts_with("**进入**") {
                    continue;
                }
                lines.push(cleaned);
            }
            let joined = lines.join("；");
            if is_usable_exit_condition(&joined) {
                return truncate_chars(&joined, 400);
            }
        }
    }
    // Fallback: scan for 收束： inline.
    for line in markdown.lines() {
        let t = line.trim();
        if let Some(rest) = t
            .strip_prefix("- **收束**：")
            .or_else(|| t.strip_prefix("- **收束**:"))
            .or_else(|| t.strip_prefix("**收束**："))
            .or_else(|| t.strip_prefix("收束："))
            .or_else(|| t.strip_prefix("收束:"))
        {
            let s = rest.trim();
            if is_usable_exit_condition(s) {
                return truncate_chars(s, 400);
            }
        }
    }
    String::new()
}

fn is_usable_exit_condition(s: &str) -> bool {
    let t = s.trim();
    t.chars().count() >= 8
        && t != "条件"
        && !t.eq_ignore_ascii_case("condition")
        && t != "exit"
        && t != "exit_condition"
}

fn yaml_frontmatter_exit_condition(markdown: &str) -> Option<String> {
    let t = markdown.trim_start();
    if !t.starts_with("---") {
        return None;
    }
    let rest = t.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    let block = &rest[..end];
    for line in block.lines() {
        let line = line.trim();
        if let Some(v) = line
            .strip_prefix("exit_condition:")
            .or_else(|| line.strip_prefix("exit_condition："))
        {
            let v = v.trim().trim_matches('"').trim_matches('\'').trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Heading must end the line (or file) so `## 收束` does not eat `## 收束条件`.
fn section_after_exact_heading<'a>(markdown: &'a str, heading: &str) -> Option<&'a str> {
    let mut search_from = 0;
    while let Some(rel) = markdown[search_from..].find(heading) {
        let idx = search_from + rel;
        let after_head = idx + heading.len();
        let rest = &markdown[after_head..];
        let ok = rest.is_empty()
            || rest.starts_with('\n')
            || rest.starts_with("\r\n")
            || rest.starts_with('\r');
        if ok {
            let after = rest
                .strip_prefix("\r\n")
                .or_else(|| rest.strip_prefix('\n'))
                .or_else(|| rest.strip_prefix('\r'))
                .unwrap_or(rest);
            return Some(after);
        }
        search_from = idx + heading.len();
    }
    None
}

/// Volume still has plot-card work — must not fire volume-end / sync gate.
pub fn volume_has_open_plot_work(project_dir: &Path, volume_index: u32) -> bool {
    let index = load_plot_index(project_dir);
    let Some(vol) = index.volumes.iter().find(|v| v.volume_index == volume_index) else {
        return false;
    };
    if vol.plots.is_empty() {
        return false;
    }
    // planned / in_progress / bridging all mean the volume ladder is unfinished —
    // a just-completed card without next_plot must not end the volume if peers remain.
    for p in &vol.plots {
        if matches!(
            p.status.as_str(),
            "planned" | "in_progress" | "bridging"
        ) {
            return true;
        }
    }
    for p in &vol.plots {
        let next = p.next_plot.trim();
        if next.is_empty() {
            continue;
        }
        if !matches!(
            p.status.as_str(),
            "completed" | "in_progress" | "bridging"
        ) {
            continue;
        }
        let next_entry = vol
            .plots
            .iter()
            .find(|x| x.title == next || x.id == next || x.slug == next);
        match next_entry {
            None => return true, // next_plot not designed yet
            Some(n) if n.status != "completed" => return true,
            _ => {}
        }
    }
    false
}

fn parse_json_object(raw: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(raw).unwrap_or_else(|_| {
        if let Some(start) = raw.find('{') {
            if let Some(end) = raw.rfind('}') {
                return serde_json::from_str(&raw[start..=end]).unwrap_or_default();
            }
        }
        json!({})
    })
}

fn json_truthy_flag(v: &serde_json::Value, keys: &[&str]) -> bool {
    for key in keys {
        match v.get(*key) {
            Some(serde_json::Value::Bool(b)) => return *b,
            Some(serde_json::Value::String(s)) => {
                return matches!(
                    s.trim().to_lowercase().as_str(),
                    "true" | "yes" | "1" | "已兑现" | "是" | "pass" | "通过"
                );
            }
            _ => {}
        }
    }
    false
}

/// Whether `plot_acceptor` JSON marks a pass (ignores skipped).
/// Soft/cheating passes (rewritten exit, non-empty gaps) are rejected.
pub fn accept_verdict_is_pass(accept_raw: &str) -> bool {
    accept_verdict_is_pass_against(accept_raw, None)
}

/// Like [`accept_verdict_is_pass`], optionally checking the reported exit against the card.
pub fn accept_verdict_is_pass_against(
    accept_raw: &str,
    card_exit: Option<&str>,
) -> bool {
    let v = parse_json_object(accept_raw);
    if v.get("skipped")
        .and_then(|x| x.as_bool())
        .unwrap_or(false)
    {
        return false;
    }
    if !json_truthy_flag(&v, &["pass", "accepted"]) {
        return false;
    }
    // pass + remaining gaps ⇒ not truly done
    if let Some(gaps) = v.get("gaps").and_then(|g| g.as_array()) {
        if gaps.iter().any(|x| {
            x.as_str()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false)
        }) {
            tracing::warn!("plot_acceptor pass ignored: gaps still present");
            return false;
        }
    }
    if let Some(card_exit) = card_exit.map(str::trim).filter(|s| !s.is_empty()) {
        let reported = v
            .get("exit_condition")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim();
        if !reported.is_empty() && !exit_conditions_align(card_exit, reported) {
            tracing::warn!(
                card_exit = %card_exit.chars().take(80).collect::<String>(),
                reported = %reported.chars().take(80).collect::<String>(),
                "plot_acceptor pass ignored: exit_condition rewritten"
            );
            return false;
        }
    }
    true
}

/// Reject passes that swapped the card exit for a different beat (e.g. tease of next plot).
fn exit_conditions_align(card_exit: &str, reported: &str) -> bool {
    let phrases = distinctive_exit_phrases(card_exit);
    if phrases.is_empty() {
        return true;
    }
    let hit = phrases.iter().filter(|p| reported.contains(p.as_str())).count();
    // At least half of distinctive card phrases must appear in the reported exit.
    hit * 2 >= phrases.len()
}

fn distinctive_exit_phrases(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let flush = |buf: &mut String, out: &mut Vec<String>| {
        let t = buf.trim().to_string();
        buf.clear();
        if t.chars().count() >= 4 {
            out.push(t);
        }
    };
    for ch in s.chars() {
        if ch.is_alphanumeric()
            || matches!(ch, '\u{4e00}'..='\u{9fff}' | '\u{3400}'..='\u{4dbf}')
        {
            buf.push(ch);
        } else if !buf.is_empty() {
            flush(&mut buf, &mut out);
        }
    }
    if !buf.is_empty() {
        flush(&mut buf, &mut out);
    }
    out
}

/// Block `design_plot` while a card is still active or a bridge chapter is owed.
pub fn plot_design_blocked_reason(project_dir: &Path) -> Option<String> {
    let _ = rebuild_plot_index(project_dir);
    let index = load_plot_index(project_dir);
    let all: Vec<&PlotIndexEntry> = index.volumes.iter().flat_map(|v| v.plots.iter()).collect();
    if let Some(p) = all
        .iter()
        .find(|p| matches!(p.status.as_str(), "in_progress" | "bridging"))
    {
        return Some(format!(
            "已有进行中的剧情卡「{}」（{}）。请先写完并验收收束；不要叠开新卡。\
             （紧急可 force=true）",
            p.title, p.status
        ));
    }
    if let Some(p) = pending_bridge_plot(&index) {
        return Some(format!(
            "剧情卡「{}」已完成但仍欠 1 章衔接（needs_bridge）。请先 continue_writing 写衔接章，\
             再 design_plot。（紧急可 force=true）",
            p.title
        ));
    }
    None
}

/// Complete the active in_progress card when `plot_acceptor` marks `pass=true`.
/// Caller must only invoke this after the chapter has passed the publish gate.
pub fn complete_active_plot_on_accept(
    project_dir: &Path,
    accept_raw: &str,
) -> anyhow::Result<Option<PlotAdvanceEvent>> {
    let card_exit = active_plot_exit_context(project_dir).map(|(_, exit, _)| exit);
    if !accept_verdict_is_pass_against(accept_raw, card_exit.as_deref()) {
        return Ok(None);
    }
    let v = parse_json_object(accept_raw);
    complete_active_in_progress_plot(
        project_dir,
        v.get("rationale")
            .or_else(|| v.get("plot_exit_rationale"))
            .and_then(|x| x.as_str())
            .unwrap_or(""),
    )
}

fn complete_active_in_progress_plot(
    project_dir: &Path,
    rationale: &str,
) -> anyhow::Result<Option<PlotAdvanceEvent>> {
    let index = load_plot_index(project_dir);
    let Some(active) = index
        .volumes
        .iter()
        .flat_map(|v| &v.plots)
        .find(|p| p.status == "in_progress")
        .cloned()
    else {
        return Ok(None);
    };
    let vol_i = index
        .volumes
        .iter()
        .find(|v| v.plots.iter().any(|p| p.title == active.title || p.slug == active.slug))
        .map(|v| v.volume_index)
        .unwrap_or(1);
    update_plot_card(
        project_dir,
        &active.title,
        Some("completed"),
        None,
        None,
        None,
        None,
        false,
    )?;
    // Clear active_main if it pointed at this card.
    let mut index = load_plot_index(project_dir);
    for vol in &mut index.volumes {
        if vol.active_main_plot == active.title || vol.active_main_plot == active.slug {
            vol.active_main_plot.clear();
        }
    }
    save_plot_index(project_dir, &index)?;
    // Bridge first when owed; else ensure next_plot card exists and promote.
    continue_plot_ladder_after(project_dir, &active, vol_i)?;
    tracing::info!(
        plot = %active.title,
        %rationale,
        "plot card completed by plot_acceptor / exit condition"
    );
    Ok(Some(PlotAdvanceEvent {
        title: active.title,
        from: "in_progress".into(),
        to: "completed".into(),
    }))
}

fn next_plot_ref_actionable(next: &str) -> bool {
    let n = next.trim();
    if n.chars().count() < 2 {
        return false;
    }
    !matches!(
        n,
        "待定" | "待补" | "暂无" | "无" | "TBD" | "tbd" | "n/a" | "N/A" | "-" | "—"
    )
}

fn sanitize_plot_filename(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect();
    let s = s.trim().to_string();
    if s.is_empty() {
        "untitled".into()
    } else {
        s.chars().take(40).collect()
    }
}

fn find_plot_by_ref<'a>(index: &'a PlotIndex, next_ref: &str) -> Option<&'a PlotIndexEntry> {
    let next_ref = next_ref.trim();
    index.volumes.iter().flat_map(|v| &v.plots).find(|p| {
        plot_ref_matches(&p.title, &p.slug, next_ref)
            || p.title == next_ref
            || p.slug == next_ref
            || p.id == next_ref
    })
}

/// Ensure a planned card exists for `next_ref` (create stub when missing).
/// Returns the card title to promote, if any.
pub fn ensure_next_plot_card(
    project_dir: &Path,
    next_ref: &str,
    volume_index: u32,
) -> anyhow::Result<Option<String>> {
    let next_ref = next_ref.trim();
    if !next_plot_ref_actionable(next_ref) {
        return Ok(None);
    }
    let index = load_plot_index(project_dir);
    if let Some(p) = find_plot_by_ref(&index, next_ref) {
        return Ok(Some(p.title.clone()));
    }
    let folder = project_dir.join("plots");
    fs::create_dir_all(&folder)?;
    let safe = sanitize_plot_filename(next_ref);
    let mut path = folder.join(format!("{safe}.md"));
    let mut n = 2u32;
    while path.exists() {
        path = folder.join(format!("{safe}-{n}.md"));
        n += 1;
        if n > 50 {
            anyhow::bail!("无法为 next_plot 分配唯一文件名：{next_ref}");
        }
    }
    let (body, _) = crate::schemas::normalize_plot_card_best_effort(next_ref, "");
    // Seed 概览 with the ladder hint so the stub is not empty of intent.
    let body = body.replacen("（待补全）", next_ref, 1);
    fs::write(&path, &body)?;
    ensure_plot_card_lifecycle_frontmatter(&path, volume_index, "planned")?;
    tracing::info!(
        path = %path.display(),
        title = %next_ref,
        volume_index,
        "created stub next_plot card for unattended ladder"
    );
    Ok(Some(next_ref.to_string()))
}

fn try_promote_plot_to_active(project_dir: &Path, title: &str) -> anyhow::Result<()> {
    let index = load_plot_index(project_dir);
    let Some(p) = find_plot_by_ref(&index, title).cloned() else {
        return Ok(());
    };
    if matches!(p.status.as_str(), "in_progress" | "bridging") {
        return Ok(());
    }
    if p.status != "planned" {
        tracing::warn!(
            plot = %p.title,
            status = %p.status,
            "next_plot card is not planned; skip auto-promote"
        );
        return Ok(());
    }
    update_plot_card(
        project_dir,
        &p.title,
        Some("in_progress"),
        None,
        None,
        None,
        None,
        true,
    )?;
    tracing::info!(next = %p.title, "auto-promoted next_plot to in_progress");
    Ok(())
}

/// After a main card finishes writing work: enter bridging if owed, else ensure+promote next.
fn continue_plot_ladder_after(
    project_dir: &Path,
    from: &PlotIndexEntry,
    volume_index: u32,
) -> anyhow::Result<()> {
    if from.needs_bridge && !from.bridge_done {
        match ensure_bridge_plot_active(project_dir) {
            Ok(Some(t)) => tracing::info!(
                plot = %t,
                "auto-entered bridging after plot complete"
            ),
            Ok(None) => tracing::debug!(
                plot = %from.title,
                "bridge owed but ensure_bridge_plot_active returned none"
            ),
            Err(e) => tracing::warn!(
                error = %e,
                plot = %from.title,
                "failed to enter bridging after plot complete"
            ),
        }
        return Ok(());
    }
    let next_ref = from.next_plot.trim();
    if next_ref.is_empty() {
        return Ok(());
    }
    match ensure_next_plot_card(project_dir, next_ref, volume_index) {
        Ok(Some(title)) => {
            if let Err(e) = try_promote_plot_to_active(project_dir, &title) {
                tracing::warn!(error = %e, next = %title, "failed to auto-promote next_plot");
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(
            error = %e,
            next = %next_ref,
            "failed to ensure next_plot card"
        ),
    }
    Ok(())
}

/// Flip pending-bridge completed card → bridging before writing the digest chapter.
pub fn ensure_bridge_plot_active(project_dir: &Path) -> anyhow::Result<Option<String>> {
    let index = load_plot_index(project_dir);
    if index
        .volumes
        .iter()
        .flat_map(|v| &v.plots)
        .any(|p| p.status == "bridging" || p.status == "in_progress")
    {
        return Ok(None);
    }
    let Some(p) = pending_bridge_plot(&index).cloned() else {
        return Ok(None);
    };
    let data = update_plot_card(
        project_dir,
        &p.title,
        Some("bridging"),
        None,
        None,
        None,
        None,
        true,
    )?;
    Ok(data
        .get("title")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string()))
}

fn patch_plot_bridge_done(path: &Path, done: bool) -> anyhow::Result<()> {
    let text = fs::read_to_string(path)?;
    let text = upsert_frontmatter_fields(
        &text,
        &[("bridge_done", Some(if done { "true" } else { "false" }.into()))],
    );
    fs::write(path, text)?;
    Ok(())
}

/// After a **bridge** chapter publishes: `bridging` → `completed` + `bridge_done`.
/// Only cards already in `bridging` (set before writing that chapter) are closed —
/// never enter and exit bridging in the same publish tick.
pub fn complete_bridging_plots_after_publish(
    project_dir: &Path,
) -> anyhow::Result<Vec<PlotAdvanceEvent>> {
    let mut events = Vec::new();
    let mut sealed: Vec<(PlotIndexEntry, u32)> = Vec::new();
    let cards = load_plot_cards(project_dir);
    for card in cards {
        let status = card
            .meta
            .get("status")
            .map(|s| normalize_status(s))
            .unwrap_or_else(|| "planned".into());
        if status != "bridging" {
            continue;
        }
        let path = project_dir.join("plots").join(format!("{}.md", card.slug));
        if !path.exists() {
            continue;
        }
        let mut entry = entry_from_card(&card);
        let vol_i = volume_index_of(&card);
        patch_plot_card_status(&path, &card, "completed")?;
        patch_plot_bridge_done(&path, true)?;
        entry.status = "completed".into();
        entry.bridge_done = true;
        sealed.push((entry, vol_i));
        events.push(PlotAdvanceEvent {
            title: card.title.clone(),
            from: "bridging".into(),
            to: "completed+bridge_done".into(),
        });
    }
    if !events.is_empty() {
        let mut index = rebuild_plot_index(project_dir);
        for vol in &mut index.volumes {
            if vol.plots.iter().any(|p| {
                (p.title == vol.active_main_plot || p.slug == vol.active_main_plot)
                    && p.status == "completed"
            }) {
                vol.active_main_plot.clear();
            }
        }
        save_plot_index(project_dir, &index)?;
        // After bridge seals, promote next_plot (create stub if needed).
        for (entry, vol_i) in &sealed {
            let _ = continue_plot_ladder_after(project_dir, entry, *vol_i);
        }
    }
    Ok(events)
}

/// Normalize raw model output into a single frontmatter + markdown plot card.
/// Strips ``` fences, converts plot-designer JSON, unwraps nested cards.
pub fn materialize_plot_card_markdown(raw: &str, title: &str, act: Option<u64>) -> String {
    let stripped = strip_md_fence(raw.trim());
    // Pure JSON first (avoid recurse-through-nested on the same payload).
    let out = if let Some(v) = extract_plot_json(&stripped) {
        render_plot_json_markdown(&v, title, act)
    } else if let Some(inner) = extract_nested_plot_payload(&stripped) {
        if inner.trim() != stripped.trim() {
            let nested = materialize_plot_card_markdown(&inner, title, act);
            // If nested JSON was corrupt, materialize may have wrapped raw `{…}`;
            // fall back to the prose stub above the blob.
            if nested.contains("\n{\n") || nested.trim_start().starts_with('{') {
                if let Some(stub) = strip_trailing_json_blob(&stripped) {
                    materialize_plot_card_markdown(&stub, title, act)
                } else {
                    nested
                }
            } else {
                nested
            }
        } else if stripped.starts_with("---") {
            materialize_plot_fm_passthrough(&stripped)
        } else {
            materialize_plot_prose_fallback(title, act, &stripped)
        }
    } else if stripped.starts_with("---") {
        materialize_plot_fm_passthrough(&stripped)
    } else {
        materialize_plot_prose_fallback(title, act, &stripped)
    };
    fill_required_plot_frontmatter(&out, title, act)
}

fn materialize_plot_fm_passthrough(stripped: &str) -> String {
    // Drop a trailing corrupt JSON payload left after a valid stub.
    if let Some(stub) = strip_trailing_json_blob(stripped) {
        if stub.trim() != stripped.trim() {
            return stub;
        }
    }
    stripped.to_string()
}

fn materialize_plot_prose_fallback(title: &str, act: Option<u64>, body: &str) -> String {
    let vol_line = act
        .map(|a| format!("volume_index: {a}\n"))
        .unwrap_or_default();
    format!(
        "---\ntitle: {title}\n{vol_line}status: planned\nscope: local\nplot_type: main\nneeds_bridge: false\ncategory: plot\n---\n\n# {title}\n\n{body}\n"
    )
}

/// Fill schema-required FM keys when the model omitted them (prose / incomplete FM path).
fn fill_required_plot_frontmatter(text: &str, title: &str, act: Option<u64>) -> String {
    let (meta, _) = crate::cards::split_simple_frontmatter(text);
    let missing = |k: &str| meta.get(k).map(|s| s.trim().is_empty()).unwrap_or(true);
    let mut fields: Vec<(&str, Option<String>)> = Vec::new();
    if missing("title") {
        fields.push(("title", Some(title.to_string())));
    }
    if missing("status") {
        fields.push(("status", Some("planned".into())));
    }
    if missing("scope") {
        fields.push(("scope", Some("local".into())));
    }
    if missing("plot_type") {
        fields.push(("plot_type", Some("main".into())));
    }
    if missing("needs_bridge") {
        fields.push(("needs_bridge", Some("false".into())));
    }
    if missing("category") {
        fields.push(("category", Some("plot".into())));
    }
    if missing("volume_index") {
        if let Some(a) = act {
            fields.push(("volume_index", Some(a.to_string())));
        }
    }
    if fields.is_empty() {
        text.to_string()
    } else {
        upsert_frontmatter_fields(text, &fields)
    }
}

fn strip_md_fence(text: &str) -> String {
    let t = text.trim();
    let Some(rest) = t.strip_prefix("```") else {
        return t.to_string();
    };
    let mut lines = rest.lines();
    let first = lines.next().unwrap_or("");
    // language tag on first line (markdown/json/…)
    let body = if first.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        lines.collect::<Vec<_>>().join("\n")
    } else {
        format!("{first}\n{}", lines.collect::<Vec<_>>().join("\n"))
    };
    let body = body.trim();
    body.strip_suffix("```").unwrap_or(body).trim().to_string()
}

fn extract_nested_plot_payload(text: &str) -> Option<String> {
    // ```markdown … ``` or ```json … ``` buried after a stub.
    if let Some(start) = text.find("```") {
        if start > 0 {
            let after = &text[start..];
            let closed = strip_md_fence(after);
            if closed.contains("---") || closed.trim_start().starts_with('{') {
                return Some(closed);
            }
        }
    }
    // Trailing JSON object after a prose stub (not the whole file).
    if let Some(pos) = text.find("\n{") {
        if pos > 0 {
            let candidate = text[pos..].trim();
            if candidate.starts_with('{')
                && (candidate.contains("\"fields\"") || candidate.contains("\"overview\""))
            {
                return Some(candidate.to_string());
            }
        }
    }
    None
}

fn extract_plot_json(text: &str) -> Option<serde_json::Value> {
    let t = text.trim();
    // Only treat as plot JSON when the document is primarily JSON (not md + JSON).
    if !t.starts_with('{') {
        return None;
    }
    let candidates = [t.to_string(), soften_inner_quotes(t)];
    for c in &candidates {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(c) {
            if v.get("fields").is_some()
                || v.get("plot_direction").is_some()
                || v.get("overview").is_some()
            {
                return Some(v);
            }
        }
    }
    None
}

/// Replace Chinese-context bare `"…"` with 「…」 so broken model JSON can parse.
fn soften_inner_quotes(s: &str) -> String {
    let mut out = s.to_string();
    for _ in 0..8 {
        let next = regex_lite_replace_cjk_quotes(&out);
        if next == out {
            break;
        }
        out = next;
    }
    out
}

fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{4e00}'..='\u{9fff}'
        | '\u{3400}'..='\u{4dbf}'
        | '\u{f900}'..='\u{faff}'
    )
}

fn regex_lite_replace_cjk_quotes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '"' {
            if let Some(j) = (i + 1..chars.len().min(i + 42)).find(|&k| chars[k] == '"') {
                let inner: String = chars[i + 1..j].iter().collect();
                // Only rewrite CJK-context quotes (e.g. 文"门"出). Never touch JSON keys
                // like "title": "…" where left is ASCII.
                let left_ok = i > 0
                    && (is_cjk(chars[i - 1])
                        || matches!(chars[i - 1], '、' | '，' | '。' | '；' | '：' | '）' | ')' | '】'));
                let right_ok = j + 1 < chars.len()
                    && (is_cjk(chars[j + 1])
                        || matches!(
                            chars[j + 1],
                            '、' | '，' | '。' | '；' | '：' | '（' | '(' | '）' | ')' | '【' | '】'
                        ));
                let inner_ok = !inner.is_empty()
                    && !inner.contains('\n')
                    && !inner.contains(':')
                    && inner.chars().all(|c| is_cjk(c) || c.is_ascii_alphanumeric() || matches!(c, '·' | '—' | '-' | '／' | '/'));
                if left_ok && right_ok && inner_ok {
                    out.push('「');
                    out.push_str(&inner);
                    out.push('」');
                    i = j + 1;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn strip_trailing_json_blob(text: &str) -> Option<String> {
    let pos = text.find("\n{")?;
    if pos == 0 {
        return None;
    }
    let stub = text[..pos].trim_end();
    if stub.starts_with("---") || stub.contains('\n') {
        Some(stub.to_string())
    } else {
        None
    }
}

fn json_str(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn json_bool(v: &serde_json::Value, key: &str, default: bool) -> bool {
    v.get(key).and_then(|x| x.as_bool()).unwrap_or(default)
}

fn json_list(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn render_plot_json_markdown(v: &serde_json::Value, fallback_title: &str, act: Option<u64>) -> String {
    let fields = v.get("fields").cloned().unwrap_or_else(|| v.clone());
    let title = {
        let t = json_str(v, "title");
        if t.is_empty() {
            fallback_title.to_string()
        } else {
            t
        }
    };
    let volume_index = v
        .get("volume_index")
        .or_else(|| v.get("arc_index"))
        .and_then(|x| x.as_u64())
        .or(act)
        .unwrap_or(1);
    let status = {
        let s = json_str(v, "status");
        if s.is_empty() {
            "planned".into()
        } else {
            normalize_status(&s)
        }
    };
    // 剧情卡永远是卷内一段；禁止落盘 scope=volume（与卷纲重复）。
    let scope = {
        let s = json_str(v, "scope").to_ascii_lowercase();
        if s.is_empty() || s == "volume" || s == "整卷" {
            "local".into()
        } else {
            s
        }
    };
    let arc = json_str(v, "arc");
    let plot_type = {
        let t = json_str(v, "plot_type");
        if t.is_empty() {
            "main".into()
        } else {
            t
        }
    };
    let needs_bridge = json_bool(v, "needs_bridge", false)
        || json_bool(&fields, "needs_bridge", false);
    let bridge_done = json_bool(v, "bridge_done", false)
        || json_bool(&fields, "bridge_done", false);
    let next_plot = {
        let n = json_str(v, "next_plot");
        if n.is_empty() {
            json_str(&fields, "next_plot")
        } else {
            n
        }
    };
    let entry = {
        let e = json_str(v, "entry_condition");
        if e.is_empty() {
            json_str(&fields, "entry_condition")
        } else {
            e
        }
    };
    let exit = {
        let e = json_str(v, "exit_condition");
        if e.is_empty() {
            json_str(&fields, "exit_condition")
        } else {
            e
        }
    };
    let overview = json_str(&fields, "overview");
    let direction = json_str(&fields, "plot_direction");
    let stakes = json_str(&fields, "stakes");
    let setup = json_str(&fields, "setup");
    let turning = json_str(&fields, "turning_point");
    let notes = json_str(&fields, "notes");
    let characters = json_list(&fields, "characters");
    let items = json_list(&fields, "items");
    let settings = json_list(&fields, "settings");
    let locations = json_list(&fields, "locations");
    let rationale = json_str(v, "rationale");
    let outline_hint = json_str(v, "outline_sync_hint");
    let risks = json_list(v, "risks");

    let mut fm = format!(
        "---\ntitle: {title}\ncategory: plot\nvolume_index: {volume_index}\nstatus: {status}\nplot_type: {plot_type}\n"
    );
    fm.push_str(&format!("scope: {scope}\n"));
    if !arc.is_empty() {
        fm.push_str(&format!("arc: {arc}\n"));
    }
    fm.push_str(&format!(
        "needs_bridge: {}\nbridge_done: {}\n",
        if needs_bridge { "true" } else { "false" },
        if bridge_done { "true" } else { "false" },
    ));
    if !next_plot.is_empty() {
        fm.push_str(&format!("next_plot: {next_plot}\n"));
    }
    if !entry.is_empty() {
        fm.push_str(&format!("entry_condition: {entry}\n"));
    }
    if !exit.is_empty() {
        fm.push_str(&format!("exit_condition: {exit}\n"));
    }
    fm.push_str("---\n\n");

    let mut body = format!("# {title}\n\n");
    // Locked required H2 sections (reader schema).
    body.push_str(&format!(
        "## 概览\n\n{}\n\n",
        if overview.is_empty() {
            "（待补全）"
        } else {
            overview.as_str()
        }
    ));
    body.push_str(&format!(
        "## 剧情走向\n\n{}\n\n",
        if direction.is_empty() {
            "（待补全）"
        } else {
            direction.as_str()
        }
    ));
    body.push_str(&format!(
        "## 冲突与赌注\n\n{}\n\n",
        if stakes.is_empty() {
            "（待补全）"
        } else {
            stakes.as_str()
        }
    ));
    body.push_str("## 出场人物\n\n");
    if characters.is_empty() {
        body.push_str("- （待补全）\n\n");
    } else {
        for c in &characters {
            body.push_str(&format!("- {c}\n"));
        }
        body.push('\n');
    }
    let exit_body = if exit.is_empty() {
        "（待补全）".to_string()
    } else {
        exit.clone()
    };
    body.push_str(&format!("## 收束条件\n\n{exit_body}\n\n"));
    if !setup.is_empty() {
        body.push_str(&format!("## 起因\n\n{setup}\n\n"));
    }
    if !turning.is_empty() {
        body.push_str(&format!("## 转折\n\n{turning}\n\n"));
    }
    if !items.is_empty() {
        body.push_str("## 出场物品\n\n");
        for c in &items {
            body.push_str(&format!("- {c}\n"));
        }
        body.push('\n');
    }
    if !settings.is_empty() {
        body.push_str("## 相关设定\n\n");
        for c in &settings {
            body.push_str(&format!("- {c}\n"));
        }
        body.push('\n');
    }
    if !locations.is_empty() {
        body.push_str("## 相关地点\n\n");
        for c in &locations {
            body.push_str(&format!("- {c}\n"));
        }
        body.push('\n');
    }
    if !entry.is_empty() {
        body.push_str(&format!("## 进入条件\n\n{entry}\n\n"));
    }
    if !notes.is_empty() {
        body.push_str(&format!("## 备注\n\n{notes}\n\n"));
    }
    if !rationale.is_empty() {
        body.push_str(&format!("## 设计理由\n\n{rationale}\n\n"));
    }
    if !outline_hint.is_empty() {
        body.push_str(&format!("## 对卷纲的修订建议\n\n{outline_hint}\n\n"));
    }
    if !risks.is_empty() {
        body.push_str("## 风险\n\n");
        for (i, r) in risks.iter().enumerate() {
            body.push_str(&format!("{}. {r}\n", i + 1));
        }
        body.push('\n');
    }
    format!("{fm}{body}")
}

/// Ensure frontmatter has lifecycle fields after design_plot write.
pub fn ensure_plot_card_lifecycle_frontmatter(
    path: &Path,
    volume_index: u32,
    default_status: &str,
) -> anyhow::Result<()> {
    let text = fs::read_to_string(path)?;
    let text = fill_required_plot_frontmatter(&text, "未命名剧情", Some(volume_index as u64));
    let text = upsert_frontmatter_fields(
        &text,
        &[
            ("category", Some("plot".into())),
            ("volume_index", Some(volume_index.to_string())),
            ("status", Some(normalize_status(default_status))),
        ],
    );
    // If still missing status in fm after upsert with Some — already handled.
    fs::write(path, text)?;
    if let Some(project_dir) = path.parent().and_then(|p| p.parent()) {
        let _ = rebuild_plot_index(project_dir);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "novelx-plots-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros()
        ));
        fs::create_dir_all(p.join("plots")).unwrap();
        p
    }

    #[test]
    fn materialize_unwraps_fenced_nested_card() {
        let raw = "---\ntitle: 外层\nstatus: completed\n---\n\n# 外层\n\n摘要\n\n```markdown\n---\ntitle: 内层\nscope: volume\nstatus: planned\nneeds_bridge: true\n---\n\n# 内层\n\n## 概览\n\n真正内容\n```\n";
        let out = materialize_plot_card_markdown(raw, "外层", Some(1));
        assert!(out.contains("title: 内层"), "{out}");
        assert!(out.contains("真正内容"), "{out}");
        assert!(!out.contains("```"), "{out}");
        assert!(!out.contains("摘要"), "{out}");
    }

    #[test]
    fn materialize_renders_plot_json() {
        let raw = r#"{
          "title": "测试卡",
          "scope": "local",
          "volume_index": 1,
          "plot_type": "main",
          "status": "planned",
          "fields": {
            "overview": "概览文字",
            "plot_direction": "开端→落点",
            "exit_condition": "见到证据",
            "needs_bridge": false,
            "characters": ["甲"],
            "items": ["无"],
            "settings": ["无"],
            "locations": ["站"]
          }
        }"#;
        let out = materialize_plot_card_markdown(raw, "测试卡", Some(1));
        assert!(out.starts_with("---\n"), "{out}");
        assert!(out.contains("## 概览"), "{out}");
        assert!(out.contains("概览文字"), "{out}");
        assert!(out.contains("- 甲"), "{out}");
        assert!(!out.contains("\"fields\""), "{out}");
    }

    #[test]
    fn materialize_prose_fallback_fills_required_fm() {
        let raw = "## 概览\n\n发生什么。\n\n## 剧情走向\n\n开端→落点。\n\n## 冲突与赌注\n\n赌注。\n\n## 出场人物\n\n- 甲\n\n## 收束条件\n\n抵达。\n";
        let out = materialize_plot_card_markdown(raw, "散文卡", Some(1));
        assert!(out.contains("scope: local"), "{out}");
        assert!(out.contains("plot_type: main"), "{out}");
        assert!(out.contains("needs_bridge: false"), "{out}");
        assert!(
            crate::schemas::validate_plot_card(&out).is_ok(),
            "prose fallback must pass schema: {out}"
        );
    }

    #[test]
    fn materialize_incomplete_fm_gets_required_keys() {
        let raw = "---\ntitle: 残缺卡\nstatus: planned\n---\n\n# 残缺卡\n\n## 概览\n\nx\n\n## 剧情走向\n\ny\n\n## 冲突与赌注\n\nz\n\n## 出场人物\n\n- 甲\n\n## 收束条件\n\n落点\n";
        let out = materialize_plot_card_markdown(raw, "残缺卡", Some(1));
        assert!(out.contains("scope: local"), "{out}");
        assert!(out.contains("plot_type: main"), "{out}");
        assert!(out.contains("needs_bridge:"), "{out}");
        assert!(crate::schemas::validate_plot_card(&out).is_ok(), "{out}");
    }

    #[test]
    fn materialize_softens_broken_inner_quotes() {
        let raw = r#"{
          "title": "引号卡",
          "volume_index": 1,
          "status": "planned",
          "fields": {
            "overview": "甲骨文"门"出现",
            "plot_direction": "开端→落点",
            "characters": ["甲"],
            "items": ["标注甲骨文"门"的图"],
            "settings": ["无"],
            "locations": ["站"]
          }
        }"#;
        let out = materialize_plot_card_markdown(raw, "引号卡", Some(1));
        assert!(out.contains("## 概览"), "{out}");
        assert!(out.contains("「门」"), "{out}");
    }

    #[test]
    fn chapter_spans_do_not_advance_status() {
        let root = tmp();
        fs::write(
            root.join("plots/a.md"),
            "---\ntitle: 钥匙\nvolume_index: 1\nplot_type: main\nstatus: planned\nchapter_from: 7\nchapter_to: 10\nbridge_chapter: 11\n---\n\n# 钥匙\n",
        )
        .unwrap();
        let events = advance_plots_for_published_chapter(&root, 10).unwrap();
        assert!(events.is_empty());
        let idx = load_plot_index(&root);
        assert_eq!(idx.volumes[0].plots[0].status, "planned");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn blocks_write_when_in_progress_missing_exit() {
        let root = tmp();
        fs::write(
            root.join("plots/a.md"),
            "---\ntitle: 空收束\nvolume_index: 1\nplot_type: main\nstatus: in_progress\n---\n\n# 空收束\n\n只有概览，没有收束。\n",
        )
        .unwrap();
        let _ = rebuild_plot_index(&root);
        assert!(in_progress_plot_missing_exit(&root).is_some());
        match check_plot_write_gate(&root) {
            PlotWriteGate::Block { message, .. } => {
                assert!(message.contains("收束条件"), "{message}")
            }
            other => panic!("expected block, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn blocks_write_when_volume_awaiting_sync() {
        let projects = std::env::temp_dir().join(format!(
            "novelx-proj-gate-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros()
        ));
        let _ = fs::remove_dir_all(&projects);
        let dir = crate::project::init_project(&projects, "book", "未定", 10).unwrap();
        fs::write(
            dir.join("artifacts/master_outline.md"),
            "# m\n\n".to_string() + &"a".repeat(40),
        )
        .unwrap();
        fs::write(
            dir.join("artifacts/arc_outline.md"),
            "# a\n\n".to_string() + &"b".repeat(40),
        )
        .unwrap();
        fs::write(
            dir.join("artifacts/bible.md"),
            r#"# 世界观 Bible

## 0. 一句话世界
世界。

## 1. 时代与叙事框架
时代。

## 2. 全局势力与阵营
势力。

## 7. 开放问题
待揭。
"#,
        )
        .unwrap();
        crate::phases::confirm_setup_approve(&dir).unwrap();
        // No open plot work — otherwise recover_false_volume_end pulls back to drafting.
        fs::write(
            dir.join("plots/a.md"),
            "---\ntitle: 卡\nvolume_index: 1\nplot_type: main\nstatus: completed\n---\n\n## 收束条件\n\n抵达落点并拿到信物。\n",
        )
        .unwrap();
        let _ = rebuild_plot_index(&dir);
        crate::phases::set_volume_phase(&dir, crate::phases::VolumePhase::AwaitingSync).unwrap();
        match check_plot_write_gate(&dir) {
            PlotWriteGate::Block { message, .. } => {
                assert!(
                    message.contains("awaiting_sync") || message.contains("设定同步"),
                    "{message}"
                )
            }
            other => panic!("expected block, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&projects);
    }

    #[test]
    fn extracts_exit_condition() {
        let root = tmp();
        fs::write(
            root.join("plots/a.md"),
            "---\ntitle: 抉择卡\nvolume_index: 1\nplot_type: main\nstatus: in_progress\n---\n\n# 抉择卡\n\n## 进入 / 收束条件\n\n- **进入**：抵达浅滩。\n- **收束**：做出明确抉择并进入夹层。\n",
        )
        .unwrap();
        let _ = rebuild_plot_index(&root);
        let ctx = active_plot_exit_context(&root).expect("active exit");
        assert!(ctx.1.contains("夹层"), "{}", ctx.1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn exit_condition_prefers_frontmatter_not_shou_shu_prefix() {
        let md = "---\ntitle: 开局卡\nexit_condition: 主角拿到拓片并决定下井\nstatus: in_progress\n---\n\n# 开局卡\n\n## 收束条件\n\n主角拿到拓片并决定下井。\n";
        let exit = extract_plot_exit_condition(md);
        assert!(exit.contains("拓片"), "{exit}");
        assert!(!exit.eq("条件"), "{exit}");
        // Bare "## 收束" must not yield leftover「条件」from「## 收束条件」.
        let only_body = "# 开局卡\n\n## 收束条件\n\n主角拿到拓片并决定下井，秘书处通话结束。\n";
        let exit2 = extract_plot_exit_condition(only_body);
        assert!(exit2.contains("拓片"), "{exit2}");
        assert_ne!(exit2.trim(), "条件");
    }

    #[test]
    fn open_plot_work_blocks_when_next_plot_missing() {
        let root = tmp();
        fs::create_dir_all(root.join("plots")).unwrap();
        fs::write(
            root.join("plots/a.md"),
            "---\ntitle: 开局卡\nvolume_index: 3\nplot_type: main\nstatus: in_progress\nnext_plot: 下一站集合\n---\n\n# 开局卡\n\n## 收束条件\n\n拿到信物并启程。\n",
        )
        .unwrap();
        let _ = rebuild_plot_index(&root);
        assert!(volume_has_open_plot_work(&root, 3));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn open_plot_work_blocks_when_planned_peer_remains() {
        let root = tmp();
        fs::create_dir_all(root.join("plots")).unwrap();
        fs::write(
            root.join("plots/a.md"),
            "---\ntitle: 开局卡\nvolume_index: 2\nplot_type: main\nstatus: completed\n---\n\n# 开局卡\n\n## 收束条件\n已到站\n",
        )
        .unwrap();
        fs::write(
            root.join("plots/b.md"),
            "---\ntitle: 中段卡\nvolume_index: 2\nplot_type: main\nstatus: planned\n---\n\n# 中段卡\n\n## 收束条件\n钥匙到手\n",
        )
        .unwrap();
        let _ = rebuild_plot_index(&root);
        assert!(
            volume_has_open_plot_work(&root, 2),
            "planned peer must block volume end"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn bridge_complete_only_closes_existing_bridging() {
        let root = tmp();
        fs::write(
            root.join("plots/a.md"),
            "---\ntitle: 已收束\nvolume_index: 1\nplot_type: main\nstatus: bridging\nneeds_bridge: true\nbridge_done: false\n---\n\n# 已收束\n",
        )
        .unwrap();
        let _ = rebuild_plot_index(&root);
        let ev = complete_bridging_plots_after_publish(&root).unwrap();
        assert!(ev.iter().any(|e| e.to.contains("completed")));
        let idx = load_plot_index(&root);
        assert_eq!(idx.volumes[0].plots[0].status, "completed");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_rewritten_exit_pass() {
        let card = "主角亲眼目睹关键异象出现在观测舱，不再能否认先前否认的事实";
        let rewritten = r#"{"pass":true,"exit_condition":"主角接触样本触发幻象并接受外部指令前往废弃区","gaps":[],"evidence":["前往废弃区"]}"#;
        assert!(!accept_verdict_is_pass_against(rewritten, Some(card)));
        let aligned = r#"{"pass":true,"exit_condition":"主角亲眼目睹关键异象出现在观测舱，不再能否认先前否认的事实","gaps":[],"evidence":["异象"]}"#;
        assert!(accept_verdict_is_pass_against(aligned, Some(card)));
        let with_gaps = r#"{"pass":true,"exit_condition":"主角亲眼目睹关键异象出现在观测舱，不再能否认先前否认的事实","gaps":["尚未目击异象"],"evidence":[]}"#;
        assert!(!accept_verdict_is_pass_against(with_gaps, Some(card)));
    }

    #[test]
    fn design_plot_blocked_while_in_progress() {
        let root = tmp();
        fs::write(
            root.join("plots/a.md"),
            "---\ntitle: 进行中卡\nvolume_index: 1\nplot_type: main\nstatus: in_progress\n---\n\n# 进行中卡\n\n## 进入 / 收束条件\n\n- **收束**：抵达落点。\n",
        )
        .unwrap();
        let _ = rebuild_plot_index(&root);
        let msg = plot_design_blocked_reason(&root).expect("blocked");
        assert!(msg.contains("进行中"), "{msg}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn completes_on_plot_acceptor_pass() {
        let root = tmp();
        fs::write(
            root.join("plots/a.md"),
            "---\ntitle: 抉择卡\nvolume_index: 1\nplot_type: main\nstatus: in_progress\n---\n\n# 抉择卡\n\n## 收束条件\n\n进入夹层并切断复制体同步。\n",
        )
        .unwrap();
        let _ = rebuild_plot_index(&root);
        assert!(complete_active_plot_on_accept(
            &root,
            r#"{"pass":false,"rationale":"尚未进入"}"#
        )
        .unwrap()
        .is_none());
        let ev = complete_active_plot_on_accept(
            &root,
            r#"{"pass":true,"exit_condition":"进入夹层并切断复制体同步","rationale":"已进夹层并切断同步","gaps":[]}"#,
        )
        .unwrap()
        .expect("event");
        assert_eq!(ev.title, "抉择卡");
        assert_eq!(ev.to, "completed");
        let idx = load_plot_index(&root);
        assert_eq!(idx.volumes[0].plots[0].status, "completed");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn accept_creates_stub_and_promotes_next_plot() {
        let root = tmp();
        fs::write(
            root.join("plots/a.md"),
            "---\ntitle: 开局卡\nvolume_index: 1\nplot_type: main\nstatus: in_progress\nneeds_bridge: false\nnext_plot: 中段：钥匙与关卡\n---\n\n# 开局卡\n\n## 收束条件\n\n拿到线索。\n",
        )
        .unwrap();
        let _ = rebuild_plot_index(&root);
        let _ = complete_active_plot_on_accept(
            &root,
            r#"{"pass":true,"exit_condition":"拿到线索","rationale":"已拿到","gaps":[]}"#,
        )
        .unwrap();
        let idx = load_plot_index(&root);
        let next = idx
            .volumes
            .iter()
            .flat_map(|v| &v.plots)
            .find(|p| p.title.contains("钥匙"))
            .expect("stub next card");
        assert_eq!(next.status, "in_progress");
        assert_eq!(idx.volumes[0].active_main_plot, next.title);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn accept_with_bridge_defers_next_until_bridge_seals() {
        let root = tmp();
        fs::write(
            root.join("plots/a.md"),
            "---\ntitle: 高潮卡\nvolume_index: 1\nplot_type: main\nstatus: in_progress\nneeds_bridge: true\nbridge_done: false\nnext_plot: 余波后的下一段\n---\n\n# 高潮卡\n\n## 收束条件\n\n对峙结束。\n",
        )
        .unwrap();
        let _ = rebuild_plot_index(&root);
        let _ = complete_active_plot_on_accept(
            &root,
            r#"{"pass":true,"exit_condition":"对峙结束","rationale":"已对峙","gaps":[]}"#,
        )
        .unwrap();
        let idx = load_plot_index(&root);
        let a = idx.volumes[0]
            .plots
            .iter()
            .find(|p| p.title == "高潮卡")
            .unwrap();
        assert_eq!(a.status, "bridging");
        assert!(!idx
            .volumes
            .iter()
            .flat_map(|v| &v.plots)
            .any(|p| p.title.contains("余波")));

        let ev = complete_bridging_plots_after_publish(&root).unwrap();
        assert!(ev.iter().any(|e| e.to.contains("bridge_done")));
        let idx = load_plot_index(&root);
        let next = idx
            .volumes
            .iter()
            .flat_map(|v| &v.plots)
            .find(|p| p.title.contains("余波"))
            .expect("next after bridge");
        assert_eq!(next.status, "in_progress");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn write_gate_bridge_beats_stale_planned_on_other_volume() {
        let root = tmp();
        fs::write(
            root.join("plots/old.md"),
            "---\ntitle: 旧卷规划\nvolume_index: 2\nplot_type: main\nstatus: planned\n---\n\n# 旧卷规划\n",
        )
        .unwrap();
        fs::write(
            root.join("plots/a.md"),
            "---\ntitle: 本卷已结束\nvolume_index: 3\nplot_type: main\nstatus: completed\nneeds_bridge: true\nbridge_done: false\n---\n\n# 本卷已结束\n",
        )
        .unwrap();
        let _ = rebuild_plot_index(&root);
        match check_plot_write_gate(&root) {
            PlotWriteGate::Allow {
                mode: PlotWriteMode::Bridge,
                detail,
            } => assert!(detail.contains("本卷已结束"), "{detail}"),
            other => panic!("expected bridge despite stale planned, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn write_gate_auto_bridges_when_agent_needs_bridge() {
        let root = tmp();
        fs::write(
            root.join("plots/a.md"),
            "---\ntitle: 已结束\nvolume_index: 1\nplot_type: main\nstatus: completed\nneeds_bridge: true\nbridge_done: false\n---\n\n# 已结束\n",
        )
        .unwrap();
        let _ = rebuild_plot_index(&root);
        match check_plot_write_gate(&root) {
            PlotWriteGate::Allow {
                mode: PlotWriteMode::Bridge,
                ..
            } => {}
            other => panic!("expected auto bridge, got {other:?}"),
        }
        // After bridge consumed → must design_plot
        fs::write(
            root.join("plots/a.md"),
            "---\ntitle: 已结束\nvolume_index: 1\nplot_type: main\nstatus: completed\nneeds_bridge: true\nbridge_done: true\n---\n\n# 已结束\n",
        )
        .unwrap();
        let _ = rebuild_plot_index(&root);
        match check_plot_write_gate(&root) {
            PlotWriteGate::Block { message, reason, .. } => {
                assert!(message.contains("design_plot"), "{message}");
                assert!(message.contains("卷间交接"), "{message}");
                assert_eq!(reason, "need_design_plot");
            }
            other => panic!("expected block after bridge_done, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sorts_plots_by_story_progress_chain() {
        let mut plots = vec![
            PlotIndexEntry {
                id: "c".into(),
                slug: "支线".into(),
                title: "第一幕·支线冲突".into(),
                status: "in_progress".into(),
                plot_type: "local".into(),
                next_plot: String::new(),
                ..Default::default()
            },
            PlotIndexEntry {
                id: "a".into(),
                slug: "开局".into(),
                title: "第一幕·开局".into(),
                status: "completed".into(),
                plot_type: "main".into(),
                next_plot: "第一幕·中段：钥匙与关卡".into(),
                ..Default::default()
            },
            PlotIndexEntry {
                id: "b".into(),
                slug: "中段".into(),
                title: "第一幕·中段：钥匙与关卡".into(),
                status: "completed".into(),
                plot_type: "main".into(),
                next_plot: "待定".into(),
                ..Default::default()
            },
        ];
        sort_plot_entries_by_progress(&mut plots);
        assert_eq!(plots[0].title, "第一幕·开局");
        assert_eq!(plots[1].title, "第一幕·中段：钥匙与关卡");
        assert_eq!(plots[2].title, "第一幕·支线冲突");
    }

    #[test]
    fn select_prefers_active_over_completed() {
        let root = tmp();
        fs::write(
            root.join("plots/old.md"),
            "---\ntitle: 旧卡\nvolume_index: 1\nplot_type: main\nstatus: completed\nchapter_from: 1\nchapter_to: 5\n---\n\n旧\n",
        )
        .unwrap();
        fs::write(
            root.join("plots/new.md"),
            "---\ntitle: 新卡\nvolume_index: 1\nplot_type: main\nstatus: in_progress\nchapter_from: 7\nchapter_to: 10\n---\n\n新\n",
        )
        .unwrap();
        let _ = rebuild_plot_index(&root);
        let (block, hits) = select_plots_for_chapter(&root, 8, "");
        assert!(hits.iter().any(|h| h.contains("新卡")), "{hits:?}");
        assert!(block.contains("新卡"));
        assert!(!hits.iter().any(|h| h.contains("旧卡")), "{hits:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn progress_report_is_chinese_not_machine_dump() {
        let root = tmp();
        fs::write(
            root.join("plots/active.md"),
            "---\ntitle: 中段卡\nscope: local\nvolume_index: 1\nplot_type: main\nstatus: in_progress\nneeds_bridge: false\n---\n\n# 中段卡\n\n## 概览\nx\n\n## 剧情走向\ny\n\n## 冲突与赌注\nz\n\n## 出场人物\n- 甲\n\n## 收束条件\n抵达落点并拿到信物。\n",
        )
        .unwrap();
        let _ = rebuild_plot_index(&root);
        let report = format_plot_progress_report(&root);
        assert!(report.contains("当前主推"), "{report}");
        assert!(report.contains("中段卡"), "{report}");
        assert!(report.contains("收束条件"), "{report}");
        assert!(!report.contains("needs_bridge="), "{report}");
        assert!(!report.contains("v1 |"), "{report}");

        fs::create_dir_all(root.join("plots")).unwrap();
        fs::write(
            root.join("plots/v2.md"),
            "---\ntitle: 卷二卡\nscope: local\nvolume_index: 2\nplot_type: main\nstatus: in_progress\nneeds_bridge: false\n---\n\n# 卷二卡\n\n## 概览\nx\n\n## 剧情走向\ny\n\n## 冲突与赌注\nz\n\n## 出场人物\n- 甲\n\n## 收束条件\n卷二收束。\n",
        )
        .unwrap();
        let _ = rebuild_plot_index(&root);
        let v2 = format_plot_progress_report_for(&root, Some(2));
        assert!(v2.contains("第2卷进度"), "{v2}");
        assert!(v2.contains("卷二卡"), "{v2}");
        assert!(!v2.contains("v2 |"), "{v2}");
        let _ = fs::remove_dir_all(&root);
    }
}


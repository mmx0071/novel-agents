//! Progressive skill disclosure — Codex-style: metadata first, full body on activate.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SkillError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    System,
    Agent,
    Project,
    Studio,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub scope: SkillScope,
}

#[derive(Debug, Clone)]
pub struct SkillLoadOutcome {
    pub skills: Vec<SkillMetadata>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SkillInjection {
    pub name: String,
    pub path: PathBuf,
    pub body: String,
}

const DEFAULT_METADATA_CHAR_BUDGET: usize = 8_000;
const MAX_SKILL_METADATA_TOKEN_BUDGET: usize = 4_000;
const SKILL_METADATA_CONTEXT_WINDOW_PERCENT: usize = 2;
const APPROX_CHARS_PER_TOKEN: usize = 4;
const MAX_CATALOG_DESCRIPTION_CHARS: usize = 1_024;
const DESCRIPTION_TRUNCATION_WARNING_THRESHOLD_CHARS: usize = 100;

/// Character budget for skill metadata catalog.
/// When `context_window_tokens` is set: ~2% of the window (capped at 4k tokens),
/// converted to chars; otherwise fall back to 8k characters.
pub fn capped_skill_metadata_char_budget(context_window_tokens: Option<usize>) -> usize {
    context_window_tokens
        .filter(|w| *w > 0)
        .map(|window| {
            let tokens = window
                .saturating_mul(SKILL_METADATA_CONTEXT_WINDOW_PERCENT)
                .saturating_div(100)
                .clamp(1, MAX_SKILL_METADATA_TOKEN_BUDGET);
            tokens
                .saturating_mul(APPROX_CHARS_PER_TOKEN)
                .max(2_000)
        })
        .unwrap_or(DEFAULT_METADATA_CHAR_BUDGET)
}

/// Observability for catalog rendering under metadata pressure.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SkillRenderReport {
    pub total_count: usize,
    pub included_count: usize,
    pub omitted_count: usize,
    pub truncated_description_chars: usize,
    pub truncated_description_count: usize,
}

impl SkillRenderReport {
    pub fn warning_message(&self) -> Option<String> {
        if self.omitted_count > 0 {
            return Some(format!(
                "Skill catalog exceeded budget: removed all descriptions and omitted {} skill(s). \
                 Disable unused skills to leave room for the rest.",
                self.omitted_count
            ));
        }
        (self.average_truncated_description_chars()
            > DESCRIPTION_TRUNCATION_WARNING_THRESHOLD_CHARS)
            .then(|| {
                "Skill descriptions were shortened to fit the skills context budget. \
                 Every skill remains listed, but some descriptions are shorter."
                    .to_string()
            })
    }

    fn average_truncated_description_chars(&self) -> usize {
        if self.total_count == 0 || self.truncated_description_chars == 0 {
            return 0;
        }
        self.truncated_description_chars
            .saturating_add(self.total_count.saturating_sub(1))
            / self.total_count
    }
}

/// Rendered skills directory plus pressure report.
#[derive(Debug, Clone)]
pub struct AvailableSkillsRender {
    pub body: String,
    pub report: SkillRenderReport,
}

/// Scan skill roots; only read YAML frontmatter (not full body).
pub fn load_skills(roots: &[(SkillScope, PathBuf)]) -> SkillLoadOutcome {
    let mut skills = Vec::new();
    let mut errors = Vec::new();
    let mut seen = HashSet::new();

    for (scope, root) in roots {
        if !root.exists() {
            continue;
        }
        collect_skills(root, *scope, root, 0, &mut skills, &mut errors, &mut seen);
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    SkillLoadOutcome { skills, errors }
}

fn collect_skills(
    dir: &Path,
    scope: SkillScope,
    _root: &Path,
    depth: usize,
    skills: &mut Vec<SkillMetadata>,
    errors: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    if depth > 6 {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            errors.push(format!("{}: {e}", dir.display()));
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Studio root owns top-level shared *.md only; agent SKILLs live under
            // skills/agents/ and are loaded with SkillScope::Agent — don't retag them.
            if matches!(scope, SkillScope::Studio | SkillScope::System)
                && path.file_name().and_then(|n| n.to_str()) == Some("agents")
            {
                continue;
            }
            collect_skills(&path, scope, _root, depth + 1, skills, errors, seen);
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) != Some("SKILL.md")
            && !(path.extension().and_then(|e| e.to_str()) == Some("md")
                && depth == 0
                && matches!(scope, SkillScope::Studio | SkillScope::System))
        {
            // Allow top-level studio.md style files under skills/
            if path.extension().and_then(|e| e.to_str()) == Some("md")
                && matches!(scope, SkillScope::Studio)
            {
                // fall through
            } else {
                continue;
            }
        }
        match parse_frontmatter(&path, scope) {
            Ok(meta) => {
                if seen.insert(meta.name.clone()) {
                    skills.push(meta);
                }
            }
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
}

fn parse_frontmatter(path: &Path, scope: SkillScope) -> Result<SkillMetadata, SkillError> {
    let content = fs::read_to_string(path)?;
    let (name, description) = if content.starts_with("---") {
        let rest = &content[3..];
        if let Some(end) = rest.find("---") {
            let yaml = &rest[..end];
            let map: serde_yaml::Value = serde_yaml::from_str(yaml)
                .map_err(|e| SkillError::Parse(e.to_string()))?;
            let name = map
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unnamed")
                        .to_string()
                });
            let description = map
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            (name, description)
        } else {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unnamed")
                .to_string();
            (name, first_line_desc(&content))
        }
    } else {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string();
        (name, first_line_desc(&content))
    };
    Ok(SkillMetadata {
        name,
        description,
        path: path.to_path_buf(),
        scope,
    })
}

fn first_line_desc(content: &str) -> String {
    content
        .lines()
        .find(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
        .unwrap_or("")
        .trim()
        .chars()
        .take(120)
        .collect()
}

struct SkillLine<'a> {
    name: &'a str,
    description: String,
    locator: String,
}

impl<'a> SkillLine<'a> {
    fn from_meta(meta: &'a SkillMetadata) -> Self {
        let description: String = meta
            .description
            .chars()
            .take(MAX_CATALOG_DESCRIPTION_CHARS)
            .collect();
        Self {
            name: meta.name.as_str(),
            description,
            locator: meta.path.display().to_string(),
        }
    }

    fn description_chars(&self) -> usize {
        self.description.chars().count()
    }

    fn render_with_description(&self, description: &str) -> String {
        if description.is_empty() {
            format!("- {}: (file: {})", self.name, self.locator)
        } else {
            format!(
                "- {}: {} (file: {})",
                self.name, description, self.locator
            )
        }
    }

    fn render_full(&self) -> String {
        self.render_with_description(&self.description)
    }

    fn render_minimum(&self) -> String {
        self.render_with_description("")
    }

    fn render_with_description_chars(&self, n: usize) -> String {
        let end = self
            .description
            .char_indices()
            .nth(n)
            .map_or(self.description.len(), |(i, _)| i);
        self.render_with_description(&self.description[..end])
    }

    fn line_cost(line: &str) -> usize {
        line.chars().count().saturating_add(1)
    }

    fn full_cost(&self) -> usize {
        Self::line_cost(&self.render_full())
    }

    fn minimum_cost(&self) -> usize {
        Self::line_cost(&self.render_minimum())
    }
}

struct RenderedSkillLine {
    line: String,
    truncated_description_chars: usize,
}

fn omission_notice(omitted: usize) -> String {
    format!("(… {omitted} more skills omitted due to budget)")
}

/// Render available skills directory for system prompt (metadata only).
///
/// Under budget pressure: keep every skill name first, round-robin-share
/// description space, strip descriptions before omitting entries.
pub fn render_available_skills(
    skills: &[SkillMetadata],
    budget: Option<usize>,
) -> AvailableSkillsRender {
    let budget = budget.unwrap_or(DEFAULT_METADATA_CHAR_BUDGET);
    let header = "## Skills";
    let footer =
        "Use `$skill-name` to activate a skill; full SKILL.md is loaded only when activated.";
    let header_cost = SkillLine::line_cost(header);
    let footer_cost = SkillLine::line_cost(footer);
    let framing_cost = header_cost.saturating_add(footer_cost);
    let content_budget = budget.saturating_sub(framing_cost).max(1);

    let skill_lines: Vec<SkillLine<'_>> = skills.iter().map(SkillLine::from_meta).collect();
    let mut rendered = render_skill_lines(&skill_lines, content_budget);

    // If we omit entries, the notice must fit inside the same budget — drop
    // trailing name-only lines until header + lines + notice + footer fit.
    if rendered.omitted_count > 0 {
        loop {
            let notice_cost = SkillLine::line_cost(&omission_notice(rendered.omitted_count));
            let lines_cost: usize = rendered
                .lines
                .iter()
                .map(|l| SkillLine::line_cost(&l.line))
                .sum();
            let total = framing_cost
                .saturating_add(lines_cost)
                .saturating_add(notice_cost);
            if total <= budget || rendered.lines.is_empty() {
                break;
            }
            // Truncation stats already cover every skill from the first pass.
            if rendered.lines.pop().is_some() {
                rendered.omitted_count = rendered.omitted_count.saturating_add(1);
            }
        }
    }

    let mut body_parts = vec![header.to_string()];
    for line in &rendered.lines {
        body_parts.push(line.line.clone());
    }
    if rendered.omitted_count > 0 {
        body_parts.push(omission_notice(rendered.omitted_count));
    }
    body_parts.push(footer.to_string());

    AvailableSkillsRender {
        body: body_parts.join("\n"),
        report: SkillRenderReport {
            total_count: skills.len(),
            included_count: rendered.lines.len(),
            omitted_count: rendered.omitted_count,
            truncated_description_chars: rendered.truncated_description_chars,
            truncated_description_count: rendered.truncated_description_count,
        },
    }
}

/// Convenience wrapper returning only the catalog body.
pub fn build_available_skills(skills: &[SkillMetadata], budget: Option<usize>) -> String {
    render_available_skills(skills, budget).body
}

struct RenderedSkillLines {
    lines: Vec<RenderedSkillLine>,
    omitted_count: usize,
    truncated_description_chars: usize,
    truncated_description_count: usize,
}

fn render_skill_lines(skill_lines: &[SkillLine<'_>], budget: usize) -> RenderedSkillLines {
    let full_cost: usize = skill_lines.iter().map(SkillLine::full_cost).sum();
    if full_cost <= budget {
        return RenderedSkillLines {
            lines: skill_lines
                .iter()
                .map(|line| RenderedSkillLine {
                    line: line.render_full(),
                    truncated_description_chars: 0,
                })
                .collect(),
            omitted_count: 0,
            truncated_description_chars: 0,
            truncated_description_count: 0,
        };
    }

    let minimum_cost: usize = skill_lines.iter().map(SkillLine::minimum_cost).sum();
    if minimum_cost <= budget {
        let lines = allocate_descriptions_round_robin(
            skill_lines,
            budget.saturating_sub(minimum_cost),
        );
        let (truncated_description_chars, truncated_description_count) =
            sum_description_truncation(&lines);
        return RenderedSkillLines {
            lines,
            omitted_count: 0,
            truncated_description_chars,
            truncated_description_count,
        };
    }

    // Even name-only lines overflow: keep as many entries as fit, omit the rest.
    let mut included = Vec::new();
    let mut used = 0usize;
    let mut omitted = 0usize;
    let mut truncated_description_chars = 0usize;
    let mut truncated_description_count = 0usize;
    for line in skill_lines {
        let desc_chars = line.description_chars();
        let cost = line.minimum_cost();
        if used.saturating_add(cost) <= budget {
            used = used.saturating_add(cost);
            included.push(RenderedSkillLine {
                line: line.render_minimum(),
                truncated_description_chars: desc_chars,
            });
        } else {
            omitted = omitted.saturating_add(1);
        }
        truncated_description_chars = truncated_description_chars.saturating_add(desc_chars);
        if desc_chars > 0 {
            truncated_description_count = truncated_description_count.saturating_add(1);
        }
    }
    RenderedSkillLines {
        lines: included,
        omitted_count: omitted,
        truncated_description_chars,
        truncated_description_count,
    }
}

/// Round-robin one description character at a time so no skill monopolizes budget.
fn allocate_descriptions_round_robin(
    skill_lines: &[SkillLine<'_>],
    extra_budget: usize,
) -> Vec<RenderedSkillLine> {
    let desc_lens: Vec<usize> = skill_lines.iter().map(SkillLine::description_chars).collect();
    // Precompute extra cost of using exactly n description chars vs minimum line.
    let extra_costs: Vec<Vec<usize>> = skill_lines
        .iter()
        .map(|line| {
            let min = line.minimum_cost();
            let max = line.description_chars();
            let mut costs = Vec::with_capacity(max.saturating_add(1));
            costs.push(0);
            for n in 1..=max {
                let cost = SkillLine::line_cost(&line.render_with_description_chars(n))
                    .saturating_sub(min);
                costs.push(cost);
            }
            costs
        })
        .collect();

    let mut allocations = vec![0usize; skill_lines.len()];
    let mut current_extra = vec![0usize; skill_lines.len()];
    let mut remaining = extra_budget;

    loop {
        let mut changed = false;
        for (i, &max_chars) in desc_lens.iter().enumerate() {
            if allocations[i] >= max_chars {
                continue;
            }
            let next = allocations[i].saturating_add(1);
            let next_cost = extra_costs[i][next];
            let delta = next_cost.saturating_sub(current_extra[i]);
            if delta <= remaining {
                allocations[i] = next;
                current_extra[i] = next_cost;
                remaining = remaining.saturating_sub(delta);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    skill_lines
        .iter()
        .zip(allocations)
        .map(|(line, n)| RenderedSkillLine {
            line: line.render_with_description_chars(n),
            truncated_description_chars: line.description_chars().saturating_sub(n),
        })
        .collect()
}

fn sum_description_truncation(rendered: &[RenderedSkillLine]) -> (usize, usize) {
    rendered.iter().fold((0usize, 0usize), |(chars, count), line| {
        if line.truncated_description_chars == 0 {
            (chars, count)
        } else {
            (
                chars.saturating_add(line.truncated_description_chars),
                count.saturating_add(1),
            )
        }
    })
}

/// Collect `$skill-name` mentions from user text.
pub fn collect_explicit_skill_mentions(text: &str) -> Vec<String> {
    let re = Regex::new(r"\$([a-zA-Z0-9_-]+)").unwrap();
    let mut names = Vec::new();
    let mut seen = HashSet::new();
    for cap in re.captures_iter(text) {
        let name = cap[1].to_string();
        if seen.insert(name.clone()) {
            names.push(name);
        }
    }
    names
}

/// Agents that receive shared `prose-pitfalls` hard constraints prepended to SKILL body.
const PROSE_PITFALLS_AGENTS: &[&str] = &[
    "writer",
    "consistency-auditor",
    "literary-editor",
    "dialogue-specialist",
    "scene-specialist",
];

/// Agents that produce locked content shapes — prepend `content-formats`.
const CONTENT_FORMATS_AGENTS: &[&str] = &[
    "writer",
    "chapter-planner",
    "master-planner",
    "arc-planner",
    "plot-designer",
    "entity-designer",
    "world-architect",
    "summarizer",
    "nomenclature-curator",
    "setting-auditor",
];

/// Chapter / plot roles that need volume phase + bridge-chapter alignment.
const VOLUME_LIFECYCLE_AGENTS: &[&str] = &[
    "writer",
    "chapter-planner",
    "plot-acceptor",
    "plot-designer",
    "arc-planner",
];

fn wants_prose_pitfalls(agent: &str) -> bool {
    let key = agent.replace('_', "-");
    PROSE_PITFALLS_AGENTS.iter().any(|a| *a == key)
}

fn wants_content_formats(agent: &str) -> bool {
    let key = agent.replace('_', "-");
    CONTENT_FORMATS_AGENTS.iter().any(|a| *a == key)
}

fn wants_volume_lifecycle(agent: &str) -> bool {
    let key = agent.replace('_', "-");
    VOLUME_LIFECYCLE_AGENTS.iter().any(|a| *a == key)
}

fn strip_md_frontmatter(content: &str) -> &str {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content;
    }
    let rest = &trimmed[3..];
    if let Some(end) = rest.find("\n---") {
        let after = &rest[end + 4..];
        return after.trim_start_matches('\n');
    }
    content
}

fn load_shared_skill_body(skills: &[SkillMetadata], name: &str) -> Option<String> {
    let want = name.replace('_', "-");
    let meta = skills.iter().find(|s| {
        s.name == name || s.name.replace('_', "-") == want
    })?;
    let raw = fs::read_to_string(&meta.path).ok()?;
    let body = strip_md_frontmatter(&raw).trim();
    if body.is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

/// Load full skill bodies for activated names.
/// Format producers get `content-formats`; prose agents get `prose-pitfalls`;
/// chapter/plot roles also get `volume-lifecycle`.
pub fn build_skill_injections(
    skills: &[SkillMetadata],
    activate: &[String],
) -> Vec<SkillInjection> {
    let pitfalls = load_shared_skill_body(skills, "prose-pitfalls");
    let formats = load_shared_skill_body(skills, "content-formats");
    let volume_lc = load_shared_skill_body(skills, "volume-lifecycle");
    let mut out = Vec::new();
    for name in activate {
        if let Some(meta) = skills.iter().find(|s| s.name == *name || s.name.replace('_', "-") == *name)
        {
            match fs::read_to_string(&meta.path) {
                Ok(raw) => {
                    let mut prefixes = Vec::new();
                    if wants_content_formats(&meta.name) {
                        if let Some(ref f) = formats {
                            prefixes.push(f.as_str());
                        }
                    }
                    if wants_prose_pitfalls(&meta.name) {
                        if let Some(ref p) = pitfalls {
                            prefixes.push(p.as_str());
                        }
                    }
                    if wants_volume_lifecycle(&meta.name) {
                        if let Some(ref v) = volume_lc {
                            prefixes.push(v.as_str());
                        }
                    }
                    let body = if prefixes.is_empty() {
                        raw
                    } else {
                        format!("{}\n\n---\n\n{raw}", prefixes.join("\n\n---\n\n"))
                    };
                    out.push(SkillInjection {
                        name: meta.name.clone(),
                        path: meta.path.clone(),
                        body,
                    });
                }
                Err(_) => continue,
            }
        }
    }
    out
}

/// Default skill roots relative to repo root.
pub fn default_skill_roots(repo_root: &Path) -> Vec<(SkillScope, PathBuf)> {
    vec![
        (
            SkillScope::Studio,
            repo_root.join("config/skills"),
        ),
        (
            SkillScope::Agent,
            repo_root.join("config/skills/agents"),
        ),
    ]
}

/// Project-as-skill scope: each novel directory becomes a lightweight skill entry.
pub fn load_project_skills(projects_root: &Path) -> Vec<SkillMetadata> {
    let mut skills = Vec::new();
    if !projects_root.exists() {
        return skills;
    }
    for entry in fs::read_dir(projects_root).into_iter().flatten().flatten() {
        let path = entry.path();
        if !path.is_dir() || path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.')) {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project")
            .to_string();
        let brief = fs::read_to_string(path.join("meta.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|j| {
                j.get("brief")
                    .or_else(|| j.get("genre"))
                    .and_then(|x| x.as_str())
                    .map(|s| s.chars().take(120).collect::<String>())
            })
            .unwrap_or_else(|| format!("小说项目 {name}"));
        skills.push(SkillMetadata {
            name: format!("novel-{name}"),
            description: brief,
            path: path.join("meta.json"),
            scope: SkillScope::Project,
        });
    }
    skills
}

// Re-export serde_json for project skills
use serde_json;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn progressive_disclosure() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("novelx_skills_test_{ts}"));
        let agent = dir.join("agents/writer");
        fs::create_dir_all(&agent).unwrap();
        let mut f = fs::File::create(agent.join("SKILL.md")).unwrap();
        writeln!(
            f,
            "---\nname: writer\ndescription: Write chapter drafts.\n---\n\n# Writer\n\nFull body here."
        )
        .unwrap();

        let outcome = load_skills(&[(SkillScope::Agent, dir.join("agents"))]);
        assert_eq!(outcome.skills.len(), 1);
        assert_eq!(outcome.skills[0].name, "writer");
        let listing = build_available_skills(&outcome.skills, Some(2000));
        assert!(listing.contains("writer"));
        assert!(!listing.contains("Full body here"));

        let mentions = collect_explicit_skill_mentions("请用 $writer 写下一章");
        assert_eq!(mentions, vec!["writer"]);
        let inj = build_skill_injections(&outcome.skills, &mentions);
        assert_eq!(inj.len(), 1);
        assert!(inj[0].body.contains("Full body here"));
        let _ = fs::remove_dir_all(&dir);
    }

    fn meta(name: &str, description: &str) -> SkillMetadata {
        SkillMetadata {
            name: name.into(),
            description: description.into(),
            path: PathBuf::from(format!("/tmp/{name}.md")),
            scope: SkillScope::Agent,
        }
    }

    #[test]
    fn catalog_keeps_all_names_when_descriptions_must_shrink() {
        let skills: Vec<_> = (0..8)
            .map(|i| {
                meta(
                    &format!("skill-{i}"),
                    &format!("A very long description for skill number {i} that fills budget."),
                )
            })
            .collect();
        // Tight budget: all names fit, descriptions must share space.
        let rendered = render_available_skills(&skills, Some(480));
        for s in &skills {
            assert!(
                rendered.body.contains(&s.name),
                "missing {}: {}",
                s.name,
                rendered.body
            );
        }
        assert_eq!(rendered.report.omitted_count, 0);
        assert_eq!(rendered.report.included_count, skills.len());
        assert!(rendered.report.truncated_description_chars > 0);
        // Mild truncation may not cross the user-visible warning threshold.
        assert!(
            rendered.report.truncated_description_count > 0
                || rendered.report.warning_message().is_some()
        );
    }

    #[test]
    fn catalog_strips_descriptions_before_omitting_entries() {
        let skills: Vec<_> = (0..6)
            .map(|i| meta(&format!("s{i}"), "desc that will be dropped entirely under pressure"))
            .collect();
        // Budget fits name-only lines for all, but not full descriptions.
        let rendered = render_available_skills(&skills, Some(320));
        assert_eq!(rendered.report.omitted_count, 0);
        assert_eq!(rendered.report.included_count, 6);
        for i in 0..6 {
            assert!(rendered.body.contains(&format!("s{i}")));
        }
        // Descriptions must have been shortened or removed.
        assert!(rendered.report.truncated_description_chars > 0);
    }

    #[test]
    fn catalog_omits_only_when_names_cannot_fit() {
        let skills: Vec<_> = (0..20)
            .map(|i| meta(&format!("long-skill-name-{i:02}"), "x"))
            .collect();
        let budget = 200;
        let rendered = render_available_skills(&skills, Some(budget));
        assert!(rendered.report.omitted_count > 0);
        assert!(rendered.report.included_count > 0);
        assert!(rendered.body.contains("omitted due to budget"));
        assert!(
            rendered.body.chars().count() <= budget,
            "body {} chars exceeds budget {budget}: {}",
            rendered.body.chars().count(),
            rendered.body
        );
        let warn = rendered.report.warning_message().unwrap();
        assert!(warn.contains("omitted"));
    }

    #[test]
    fn catalog_body_stays_within_budget_when_full() {
        let skills: Vec<_> = (0..4)
            .map(|i| meta(&format!("ok-{i}"), "short desc"))
            .collect();
        let budget = 500;
        let rendered = render_available_skills(&skills, Some(budget));
        assert_eq!(rendered.report.omitted_count, 0);
        assert!(rendered.body.chars().count() <= budget);
    }

    #[test]
    fn capped_budget_scales_with_context_window() {
        assert_eq!(capped_skill_metadata_char_budget(None), 8_000);
        // 128k * 2% = 2560 tokens → capped by 4k → 2560*4 = 10240 chars
        assert_eq!(capped_skill_metadata_char_budget(Some(128_000)), 10_240);
        // Huge window still caps at 4k tokens * 4 = 16000
        assert_eq!(capped_skill_metadata_char_budget(Some(1_000_000)), 16_000);
    }

    #[test]
    fn injects_prose_pitfalls_for_writer() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("novelx_skills_pitfalls_{ts}"));
        let agents = dir.join("agents/writer");
        fs::create_dir_all(&agents).unwrap();
        fs::write(
            agents.join("SKILL.md"),
            "---\nname: writer\ndescription: Write.\n---\n\n# Writer\n\nBody.\n",
        )
        .unwrap();
        fs::write(
            dir.join("prose-pitfalls.md"),
            "---\nname: prose-pitfalls\ndescription: Pitfalls.\n---\n\n# 正文常见雷区（硬约束）\n\n1. **章号元叙述**\n",
        )
        .unwrap();

        let outcome = load_skills(&[
            (SkillScope::Studio, dir.clone()),
            (SkillScope::Agent, dir.join("agents")),
        ]);
        assert!(
            outcome.skills.iter().any(|s| s.name == "prose-pitfalls"),
            "skills={:?}",
            outcome.skills.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
        let inj = build_skill_injections(&outcome.skills, &["writer".into()]);
        assert_eq!(inj.len(), 1);
        assert!(
            inj[0].body.contains("正文常见雷区"),
            "missing pitfalls prepend: {}",
            inj[0].body.chars().take(200).collect::<String>()
        );
        assert!(inj[0].body.contains("# Writer"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn studio_scan_skips_agents_subdir() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("novelx_skills_scope_{ts}"));
        let agents = dir.join("agents/writer");
        fs::create_dir_all(&agents).unwrap();
        fs::write(
            dir.join("studio.md"),
            "---\nname: studio\ndescription: Root.\n---\n\n# Studio\n",
        )
        .unwrap();
        fs::write(
            agents.join("SKILL.md"),
            "---\nname: writer\ndescription: Write.\n---\n\n# Writer\n",
        )
        .unwrap();
        let outcome = load_skills(&[
            (SkillScope::Studio, dir.clone()),
            (SkillScope::Agent, dir.join("agents")),
        ]);
        let studio = outcome.skills.iter().find(|s| s.name == "studio").unwrap();
        let writer = outcome.skills.iter().find(|s| s.name == "writer").unwrap();
        assert_eq!(studio.scope, SkillScope::Studio);
        assert_eq!(writer.scope, SkillScope::Agent);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn injects_volume_lifecycle_for_plot_acceptor() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("novelx_skills_vol_{ts}"));
        let agents = dir.join("agents/plot-acceptor");
        fs::create_dir_all(&agents).unwrap();
        fs::write(
            agents.join("SKILL.md"),
            "---\nname: plot-acceptor\ndescription: Accept.\n---\n\n# Acceptor\n\nBody.\n",
        )
        .unwrap();
        fs::write(
            dir.join("volume-lifecycle.md"),
            "---\nname: volume-lifecycle\ndescription: Vol.\n---\n\n# 卷生命周期与衔接章（短约定）\n\n桥接。\n",
        )
        .unwrap();
        let outcome = load_skills(&[
            (SkillScope::Studio, dir.clone()),
            (SkillScope::Agent, dir.join("agents")),
        ]);
        let inj = build_skill_injections(&outcome.skills, &["plot-acceptor".into()]);
        assert_eq!(inj.len(), 1);
        assert!(
            inj[0].body.contains("卷生命周期与衔接章"),
            "missing volume-lifecycle prepend: {}",
            inj[0].body.chars().take(200).collect::<String>()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn injects_content_formats_for_setting_auditor() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("novelx_skills_sa_fmt_{ts}"));
        let agents = dir.join("agents/setting-auditor");
        fs::create_dir_all(&agents).unwrap();
        fs::write(
            agents.join("SKILL.md"),
            "---\nname: setting-auditor\ndescription: Audit.\n---\n\n# Auditor\n\nBody.\n",
        )
        .unwrap();
        fs::write(
            dir.join("content-formats.md"),
            "---\nname: content-formats\ndescription: Formats.\n---\n\n# 内容格式契约（生成锚定）\n\n双层原则。\n",
        )
        .unwrap();

        let outcome = load_skills(&[
            (SkillScope::Studio, dir.clone()),
            (SkillScope::Agent, dir.join("agents")),
        ]);
        let inj = build_skill_injections(&outcome.skills, &["setting-auditor".into()]);
        assert_eq!(inj.len(), 1);
        assert!(
            inj[0].body.contains("内容格式契约"),
            "missing formats prepend: {}",
            inj[0].body.chars().take(200).collect::<String>()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn injects_content_formats_for_chapter_planner() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("novelx_skills_formats_{ts}"));
        let agents = dir.join("agents/chapter-planner");
        fs::create_dir_all(&agents).unwrap();
        fs::write(
            agents.join("SKILL.md"),
            "---\nname: chapter-planner\ndescription: Plan.\n---\n\n# Planner\n\nBody.\n",
        )
        .unwrap();
        fs::write(
            dir.join("content-formats.md"),
            "---\nname: content-formats\ndescription: Formats.\n---\n\n# 内容格式契约（生成锚定）\n\n双层原则。\n",
        )
        .unwrap();

        let outcome = load_skills(&[
            (SkillScope::Studio, dir.clone()),
            (SkillScope::Agent, dir.join("agents")),
        ]);
        let inj = build_skill_injections(&outcome.skills, &["chapter-planner".into()]);
        assert_eq!(inj.len(), 1);
        assert!(
            inj[0].body.contains("内容格式契约"),
            "missing formats prepend: {}",
            inj[0].body.chars().take(200).collect::<String>()
        );
        assert!(inj[0].body.contains("# Planner"));
        let _ = fs::remove_dir_all(&dir);
    }
}

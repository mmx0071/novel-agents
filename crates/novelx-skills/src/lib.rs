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

const DEFAULT_METADATA_BUDGET: usize = 8000;

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

/// Render available skills directory for system prompt (metadata only).
pub fn build_available_skills(skills: &[SkillMetadata], budget: Option<usize>) -> String {
    let budget = budget.unwrap_or(DEFAULT_METADATA_BUDGET);
    let mut lines = vec!["## Skills".to_string()];
    let mut used = lines[0].len();
    for s in skills {
        let line = format!("- {}: {} (file: {})", s.name, s.description, s.path.display());
        if used + line.len() + 1 > budget {
            lines.push(format!(
                "\n(… {} more skills omitted due to budget)",
                skills.len().saturating_sub(lines.len().saturating_sub(1))
            ));
            break;
        }
        used += line.len() + 1;
        lines.push(line);
    }
    lines.push(
        "\nUse `$skill-name` to activate a skill; full SKILL.md is loaded only when activated."
            .into(),
    );
    lines.join("\n")
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

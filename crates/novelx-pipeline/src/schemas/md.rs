//! Markdown heading helpers for locked section trees.

use std::collections::HashMap;

/// Extract H2 titles (stripped of leading `## `).
pub fn h2_titles(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("## ") {
                Some(normalize_heading(rest))
            } else {
                None
            }
        })
        .collect()
}

pub fn h1_line(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let t = line.trim();
        t.strip_prefix("# ")
            .filter(|r| !r.starts_with('#'))
            .map(|r| r.trim().to_string())
    })
}

pub fn normalize_heading(s: &str) -> String {
    s.trim()
        .trim_start_matches(['*', '_'])
        .trim_end_matches(['*', '_'])
        .trim()
        .to_string()
}

/// True if any H2 matches one of the aliases (case-insensitive for ASCII; exact for CJK).
pub fn has_h2(text: &str, aliases: &[&str]) -> bool {
    let titles = h2_titles(text);
    aliases.iter().any(|a| {
        titles.iter().any(|t| heading_eq(t, a))
    })
}

pub fn heading_eq(got: &str, want: &str) -> bool {
    let g = normalize_heading(got);
    let w = normalize_heading(want);
    if g == w {
        return true;
    }
    g.eq_ignore_ascii_case(&w)
        || g.starts_with(&format!("{w}（"))
        || g.starts_with(&format!("{w}("))
        || g.starts_with(&format!("{w} ·"))
        || g.starts_with(&format!("{w}·"))
}

/// Count markdown list items under a named H2 section (until next H1/H2).
pub fn list_items_under_h2(text: &str, aliases: &[&str]) -> usize {
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        if let Some(rest) = t.strip_prefix("## ") {
            if aliases.iter().any(|a| heading_eq(rest, a)) {
                i += 1;
                let mut n = 0;
                while i < lines.len() {
                    let l = lines[i].trim();
                    if l.starts_with("# ") || l.starts_with("## ") {
                        break;
                    }
                    if l.starts_with("- ") || l.starts_with("* ") || looks_like_ordered(l) {
                        n += 1;
                    }
                    i += 1;
                }
                return n;
            }
        }
        i += 1;
    }
    0
}

fn looks_like_ordered(l: &str) -> bool {
    let bytes = l.as_bytes();
    let mut j = 0;
    while j < bytes.len() && bytes[j].is_ascii_digit() {
        j += 1;
    }
    j > 0 && j < bytes.len() && (bytes[j] == b'.' || bytes[j] == b')')
}

/// Collapse 3+ blank lines → 2; trim trailing ws per line.
pub fn normalize_blank_lines(text: &str) -> String {
    let mut out = Vec::new();
    let mut blanks = 0u32;
    for line in text.lines() {
        let trimmed_end: String = line.trim_end().to_string();
        if trimmed_end.is_empty() {
            blanks += 1;
            if blanks <= 2 {
                out.push(String::new());
            }
        } else {
            blanks = 0;
            out.push(trimmed_end);
        }
    }
    let mut s = out.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Rename H2 aliases to canonical title (first alias wins as display name).
pub fn rewrite_h2_aliases(text: &str, map: &[(&str, &[&str])]) -> String {
    let mut lines = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("## ") {
            let mut replaced = false;
            for (canon, aliases) in map {
                if aliases.iter().any(|a| heading_eq(rest, a)) {
                    lines.push(format!("## {canon}"));
                    replaced = true;
                    break;
                }
            }
            if !replaced {
                lines.push(line.to_string());
            }
        } else {
            lines.push(line.to_string());
        }
    }
    normalize_blank_lines(&lines.join("\n"))
}

/// Split YAML frontmatter + body (same rules as cards::split_simple_frontmatter).
pub fn split_fm(text: &str) -> (HashMap<String, String>, String) {
    crate::cards::split_simple_frontmatter(text)
}

pub fn join_fm(meta: &HashMap<String, String>, body: &str) -> String {
    let mut keys: Vec<&String> = meta.keys().collect();
    keys.sort();
    let mut yaml = String::from("---\n");
    for k in keys {
        if let Some(v) = meta.get(k) {
            let needs_quote = v.contains(':') || v.contains('#') || v.contains('\n');
            if needs_quote {
                yaml.push_str(&format!("{k}: \"{}\"\n", v.replace('"', "\\\"")));
            } else {
                yaml.push_str(&format!("{k}: {v}\n"));
            }
        }
    }
    yaml.push_str("---\n\n");
    yaml.push_str(body.trim_start());
    if !yaml.ends_with('\n') {
        yaml.push('\n');
    }
    yaml
}

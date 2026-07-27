//! Optional cold-archive of non-active-volume chapter drafts.

use crate::volume::{active_volume_for_chapter, volume_chapter_span};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct ColdArchiveResult {
    pub archived: Vec<u32>,
    pub skipped: Vec<u32>,
}

#[derive(Debug, Deserialize)]
struct FeaturesFile {
    #[serde(default)]
    features: HashMap<String, bool>,
}

fn cold_archive_enabled(config_root: &Path) -> bool {
    // Default true — keep in sync with FeatureFlags::defaults() / features.yaml.
    let path = config_root.join("features.yaml");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return true;
    };
    serde_yaml::from_str::<FeaturesFile>(&raw)
        .ok()
        .and_then(|f| f.features.get("studio.cold_archive_drafts").copied())
        .unwrap_or(true)
}

fn gzip_write(path: &Path, data: &[u8]) -> Result<()> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    let f = std::fs::File::create(path)?;
    let mut enc = GzEncoder::new(f, Compression::default());
    enc.write_all(data)?;
    enc.finish()?;
    Ok(())
}

fn gzip_read(path: &Path) -> Result<String> {
    use flate2::read::GzDecoder;
    let f = std::fs::File::open(path)?;
    let mut dec = GzDecoder::new(f);
    let mut buf = String::new();
    dec.read_to_string(&mut buf)?;
    Ok(buf)
}

fn looks_like_archive_stub(text: &str) -> bool {
    text.contains("已冷归档") || text.contains("draft.md.gz") || text.contains("chapters_archive/")
}

/// Read draft.md, or inflate from draft.md.gz / follow draft.md.stub.
/// Live `draft.md` always wins so post-archive revisions are visible.
pub fn read_chapter_draft_resolved(project_dir: &Path, chapter: u32) -> Option<String> {
    let dir = project_dir.join("chapters").join(format!("{chapter:03}"));
    let draft = dir.join("draft.md");
    if let Ok(t) = std::fs::read_to_string(&draft) {
        if !t.trim().is_empty() && !looks_like_archive_stub(&t) {
            return Some(t);
        }
    }

    let gz = dir.join("draft.md.gz");
    let stub = dir.join("draft.md.stub");
    if stub.exists() || gz.exists() {
        if let Ok(text) = std::fs::read_to_string(&stub) {
            let target = text.trim();
            if !target.is_empty() {
                let p = if Path::new(target).is_absolute() {
                    Path::new(target).to_path_buf()
                } else {
                    project_dir.join(target)
                };
                if p.extension().and_then(|s| s.to_str()) == Some("gz") {
                    if let Ok(t) = gzip_read(&p) {
                        return Some(t);
                    }
                } else if let Ok(t) = std::fs::read_to_string(&p) {
                    if !looks_like_archive_stub(&t) {
                        return Some(t);
                    }
                }
            }
        }
        if gz.exists() {
            if let Ok(t) = gzip_read(&gz) {
                return Some(t);
            }
        }
    }
    None
}

/// Archive drafts for a completed volume span (feature-flagged).
/// Caller should pass the finished volume's chapter range (e.g. after sync_volume).
/// Chapters already belonging to a *newer* active volume are skipped.
pub fn maybe_cold_archive_volume(
    config_root: &Path,
    project_dir: &Path,
    completed_volume_index: u32,
    start_chapter: u32,
    end_chapter: u32,
) -> Result<ColdArchiveResult> {
    if !cold_archive_enabled(config_root) {
        return Ok(ColdArchiveResult {
            archived: vec![],
            skipped: vec![],
        });
    }
    // Skip chapters that already fall under a newer volume index.
    let newer_active = active_volume_for_chapter(project_dir, end_chapter.saturating_add(1).max(1))
        .filter(|v| v.volume_index > completed_volume_index);
    let newer_span = newer_active.map(|v| volume_chapter_span(&v, end_chapter.max(1)));

    let mut archived = Vec::new();
    let mut skipped = Vec::new();
    let archive_root = project_dir
        .join("chapters_archive")
        .join(format!("{completed_volume_index:02}"));
    std::fs::create_dir_all(&archive_root)?;

    for ch in start_chapter..=end_chapter.max(start_chapter) {
        if let Some((af, at)) = newer_span {
            if ch >= af && ch <= at {
                skipped.push(ch);
                continue;
            }
        }
        let ch_dir = project_dir.join("chapters").join(format!("{ch:03}"));
        let draft_path = ch_dir.join("draft.md");
        let Ok(text) = std::fs::read_to_string(&draft_path) else {
            skipped.push(ch);
            continue;
        };
        if text.trim().is_empty() || looks_like_archive_stub(&text) {
            skipped.push(ch);
            continue;
        }
        let gz_name = format!("{ch:03}.md.gz");
        let gz_path = archive_root.join(&gz_name);
        gzip_write(&gz_path, text.as_bytes())
            .with_context(|| format!("gzip {}", gz_path.display()))?;
        // Keep a compressed copy beside the chapter too for fast resolve.
        let local_gz = ch_dir.join("draft.md.gz");
        gzip_write(&local_gz, text.as_bytes())?;
        let stub = format!(
            "chapters_archive/{completed_volume_index:02}/{gz_name}"
        );
        std::fs::write(ch_dir.join("draft.md.stub"), &stub)?;
        // Replace draft with short stub pointer content (keep file for tools that only look at draft.md).
        std::fs::write(
            &draft_path,
            format!("# 第{ch}章（已冷归档）\n\n见 `{stub}` 或 `draft.md.gz`。\n"),
        )?;
        archived.push(ch);
    }
    Ok(ColdArchiveResult { archived, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn archive_and_resolve() {
        let root = std::env::temp_dir().join(format!(
            "novelx_cold_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&root);
        let config = root.join("config");
        fs::create_dir_all(&config).unwrap();
        fs::write(
            config.join("features.yaml"),
            "features:\n  studio.cold_archive_drafts: true\n",
        )
        .unwrap();
        let proj = root.join("proj");
        let ch = proj.join("chapters/001");
        fs::create_dir_all(&ch).unwrap();
        fs::write(ch.join("draft.md"), "# 第1章\n很长的正文内容用于归档测试。").unwrap();
        let r = maybe_cold_archive_volume(&config, &proj, 1, 1, 1).unwrap();
        assert_eq!(r.archived, vec![1]);
        let resolved = read_chapter_draft_resolved(&proj, 1).unwrap();
        assert!(resolved.contains("很长的正文"));
        let _ = fs::remove_dir_all(&root);
    }
}

//! SQLite acceleration layer for chapter / entity / foreshadow lookups.
//! Markdown files remain the source of truth; index is best-effort.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

pub struct LoreIndex {
    conn: Connection,
}

fn index_path(project_dir: &Path) -> PathBuf {
    project_dir.join("lore/index.sqlite")
}

pub fn ensure_lore_index(project_dir: &Path) -> Result<LoreIndex> {
    let path = index_path(project_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(&path)
        .with_context(|| format!("open lore index {}", path.display()))?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS chapters (
            chapter INTEGER PRIMARY KEY,
            title TEXT NOT NULL DEFAULT '',
            summary_digest TEXT NOT NULL DEFAULT '',
            published INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS entities (
            kind TEXT NOT NULL,
            name TEXT NOT NULL,
            path TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT '',
            PRIMARY KEY (kind, name)
        );
        CREATE TABLE IF NOT EXISTS foreshadow (
            id TEXT PRIMARY KEY,
            planted INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL DEFAULT 'open',
            keywords TEXT NOT NULL DEFAULT ''
        );
        "#,
    )?;
    Ok(LoreIndex { conn })
}

impl LoreIndex {
    pub fn upsert_chapter(
        &self,
        chapter: u32,
        title: &str,
        summary_digest: &str,
        published: bool,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO chapters(chapter, title, summary_digest, published)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(chapter) DO UPDATE SET
               title=excluded.title,
               summary_digest=excluded.summary_digest,
               published=excluded.published",
            params![chapter, title, summary_digest, published as i32],
        )?;
        Ok(())
    }

    pub fn upsert_entity(&self, kind: &str, name: &str, path: &str, status: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO entities(kind, name, path, status)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(kind, name) DO UPDATE SET
               path=excluded.path,
               status=excluded.status",
            params![kind, name, path, status],
        )?;
        Ok(())
    }

    pub fn upsert_foreshadow(
        &self,
        id: &str,
        planted: u32,
        status: &str,
        keywords: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO foreshadow(id, planted, status, keywords)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
               planted=excluded.planted,
               status=excluded.status,
               keywords=excluded.keywords",
            params![id, planted, status, keywords],
        )?;
        Ok(())
    }

    pub fn list_chapters(&self) -> Result<Vec<u32>> {
        let mut stmt = self
            .conn
            .prepare("SELECT chapter FROM chapters ORDER BY chapter ASC")?;
        let rows = stmt.query_map([], |row| row.get::<_, u32>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn remove_chapter(&self, chapter: u32) -> Result<()> {
        self.conn
            .execute("DELETE FROM chapters WHERE chapter = ?1", params![chapter])?;
        Ok(())
    }

    pub fn query_entities(&self, needle: &str, limit: usize) -> Result<Vec<(String, String, String)>> {
        let pattern = format!("%{needle}%");
        let mut stmt = self.conn.prepare(
            "SELECT kind, name, status FROM entities
             WHERE name LIKE ?1 OR status LIKE ?1
             ORDER BY name LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![pattern, limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Open foreshadow rows, oldest planted first (longform debt).
    pub fn query_open_foreshadow(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, u32, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, planted, keywords FROM foreshadow
             WHERE status = 'open' OR status = ''
             ORDER BY planted ASC, id ASC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

pub fn upsert_chapter_index_row(
    project_dir: &Path,
    chapter: u32,
    title: &str,
    summary_digest: &str,
    published: bool,
) -> Result<()> {
    let idx = ensure_lore_index(project_dir)?;
    idx.upsert_chapter(chapter, title, summary_digest, published)
}

pub fn upsert_entity_index_row(
    project_dir: &Path,
    kind: &str,
    name: &str,
    path: &str,
    status: &str,
) -> Result<()> {
    let idx = ensure_lore_index(project_dir)?;
    idx.upsert_entity(kind, name, path, status)
}

pub fn upsert_foreshadow_index_row(
    project_dir: &Path,
    id: &str,
    planted: u32,
    status: &str,
    keywords: &str,
) -> Result<()> {
    let idx = ensure_lore_index(project_dir)?;
    idx.upsert_foreshadow(id, planted, status, keywords)
}

pub fn remove_chapter_index_row(project_dir: &Path, chapter: u32) -> Result<()> {
    let idx = ensure_lore_index(project_dir)?;
    idx.remove_chapter(chapter)
}

pub fn list_chapters_from_index(project_dir: &Path) -> Option<Vec<u32>> {
    let idx = ensure_lore_index(project_dir).ok()?;
    let list = idx.list_chapters().ok()?;
    if list.is_empty() {
        None
    } else {
        Some(list)
    }
}

pub fn query_entities_from_index(
    project_dir: &Path,
    needle: &str,
    limit: usize,
) -> Option<Vec<(String, String, String)>> {
    let idx = ensure_lore_index(project_dir).ok()?;
    idx.query_entities(needle, limit).ok()
}

pub fn query_open_foreshadow_from_index(
    project_dir: &Path,
    limit: usize,
) -> Option<Vec<(String, u32, String)>> {
    let idx = ensure_lore_index(project_dir).ok()?;
    idx.query_open_foreshadow(limit).ok()
}

/// Wipe and rebuild SQLite lore index from on-disk entities + chapters.
/// Used after version-node restore so retrieval matches restored Canon.
pub fn rebuild_lore_index_from_disk(project_dir: &Path) -> Result<()> {
    let path = index_path(project_dir);
    if path.is_file() {
        let _ = std::fs::remove_file(&path);
    }
    // SQLite sidecars.
    for suffix in ["-wal", "-shm", "-journal"] {
        let p = PathBuf::from(format!("{}{suffix}", path.display()));
        let _ = std::fs::remove_file(&p);
    }
    let idx = ensure_lore_index(project_dir)?;
    for (group, kind) in [
        ("characters", "character"),
        ("items", "item"),
        ("locations", "location"),
    ] {
        let folder = project_dir.join("entities").join(group);
        if !folder.is_dir() {
            continue;
        }
        for card in crate::cards::load_markdown_cards(&folder, group) {
            let status = card
                .meta
                .get("status")
                .cloned()
                .unwrap_or_default();
            let rel = format!("entities/{group}/{}.md", card.slug);
            let _ = idx.upsert_entity(kind, &card.title, &rel, &status);
        }
    }
    for ch in crate::project::list_chapter_numbers(project_dir) {
        let title = crate::project::read_chapter_outline(project_dir, ch)
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|o| {
                o.get("title")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();
        let published = project_dir
            .join("chapters")
            .join(format!("{ch:03}"))
            .join("summary.json")
            .is_file();
        let digest = title.chars().take(80).collect::<String>();
        let _ = idx.upsert_chapter(ch, &title, &digest, published);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn upsert_and_list_chapters() {
        let root = std::env::temp_dir().join(format!(
            "novelx_lore_idx_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("lore")).unwrap();
        upsert_chapter_index_row(&root, 999, "九九九", "摘要", true).unwrap();
        upsert_chapter_index_row(&root, 1000, "一千", "摘要2", true).unwrap();
        let list = list_chapters_from_index(&root).unwrap();
        assert_eq!(list, vec![999, 1000]);
        let _ = fs::remove_dir_all(&root);
    }
}

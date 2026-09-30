//! Reading another assistant's workspace into [`SourceEntry`] rows: OpenClaw's
//! `memory/brain.db` and markdown files, and Hermes's three profile files.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use tinymemory_api::types::MemoryCategory;

use crate::keys::{normalize_key, parse_category};
use crate::{MigrationStats, SourceEntry};

pub(crate) fn collect_source_entries(
    source_workspace: &Path,
    stats: &mut MigrationStats,
) -> Result<Vec<SourceEntry>> {
    let mut entries = Vec::new();

    let sqlite_path = source_workspace.join("memory").join("brain.db");
    let sqlite_entries = read_openclaw_sqlite_entries(&sqlite_path)?;
    stats.from_sqlite = sqlite_entries.len();
    entries.extend(sqlite_entries);

    let markdown_entries = read_openclaw_markdown_entries(source_workspace)?;
    stats.from_markdown = markdown_entries.len();
    entries.extend(markdown_entries);

    // De-dup exact duplicates to make re-runs deterministic.
    let mut seen = HashSet::new();
    entries.retain(|entry| {
        let sig = format!("{}\u{0}{}\u{0}{}", entry.key, entry.content, entry.category);
        seen.insert(sig)
    });

    Ok(entries)
}

fn read_openclaw_sqlite_entries(db_path: &Path) -> Result<Vec<SourceEntry>> {
    if !db_path.exists() {
        return Ok(Vec::new());
    }

    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("Failed to open source db {}", db_path.display()))?;

    let table_exists: Option<String> = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='memories' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;

    if table_exists.is_none() {
        return Ok(Vec::new());
    }

    let columns = table_columns(&conn, "memories")?;
    let key_expr = pick_column_expr(&columns, &["key", "id", "name"], "CAST(rowid AS TEXT)");
    let Some(content_expr) =
        pick_optional_column_expr(&columns, &["content", "value", "text", "memory"])
    else {
        bail!("OpenClaw memories table found but no content-like column was detected");
    };
    let category_expr = pick_column_expr(&columns, &["category", "kind", "type"], "'core'");

    let sql = format!(
        "SELECT {key_expr} AS key, {content_expr} AS content, {category_expr} AS category FROM memories"
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;

    let mut entries = Vec::new();
    let mut idx = 0_usize;

    while let Some(row) = rows.next()? {
        let key: String = row
            .get(0)
            .unwrap_or_else(|_| format!("openclaw_sqlite_{idx}"));
        let content: String = row.get(1).unwrap_or_default();
        let category_raw: String = row.get(2).unwrap_or_else(|_| "core".to_string());

        if content.trim().is_empty() {
            continue;
        }

        entries.push(SourceEntry {
            key: normalize_key(&key, idx),
            content: content.trim().to_string(),
            category: parse_category(&category_raw),
        });

        idx += 1;
    }

    Ok(entries)
}

fn read_openclaw_markdown_entries(workspace: &Path) -> Result<Vec<SourceEntry>> {
    let mut entries = Vec::new();

    let top_level = workspace.join("MEMORY.md");
    if top_level.exists() {
        let content = fs::read_to_string(&top_level)
            .with_context(|| format!("Failed to read {}", top_level.display()))?;
        if !content.trim().is_empty() {
            entries.push(SourceEntry {
                key: "openclaw_memory_md".to_string(),
                content: content.trim().to_string(),
                category: MemoryCategory::Core,
            });
        }
    }

    let memory_dir = workspace.join("memory");
    if !memory_dir.exists() {
        return Ok(entries);
    }

    let mut idx = 0_usize;
    for entry in fs::read_dir(&memory_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        if content.trim().is_empty() {
            continue;
        }

        let file_stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("openclaw");

        entries.push(SourceEntry {
            key: normalize_key(file_stem, idx),
            content: content.trim().to_string(),
            category: MemoryCategory::Core,
        });

        idx += 1;
    }

    Ok(entries)
}

fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;

    let mut columns = Vec::new();
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        columns.push(name);
    }

    Ok(columns)
}

fn pick_column_expr<'a>(
    columns: &'a [String],
    candidates: &[&'a str],
    fallback: &'a str,
) -> &'a str {
    for candidate in candidates {
        if columns.iter().any(|c| c.eq_ignore_ascii_case(candidate)) {
            return candidate;
        }
    }
    fallback
}

fn pick_optional_column_expr<'a>(columns: &'a [String], candidates: &[&'a str]) -> Option<&'a str> {
    candidates
        .iter()
        .find(|&candidate| columns.iter().any(|c| c.eq_ignore_ascii_case(candidate)))
        .map(|v| v as _)
}

/// The files a Hermes workspace is imported from: file name, memory key, category.
pub(crate) fn hermes_file_mappings() -> Vec<(&'static str, &'static str, MemoryCategory)> {
    vec![
        ("MEMORY.md", "hermes_memory", MemoryCategory::Core),
        (
            "USER.md",
            "hermes_user_profile",
            MemoryCategory::Custom("user_profile".to_string()),
        ),
        (
            "SOUL.md",
            "hermes_persona",
            MemoryCategory::Custom("persona".to_string()),
        ),
    ]
}

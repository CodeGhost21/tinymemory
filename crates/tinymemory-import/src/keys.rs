//! Key normalisation, category parsing, path comparison and the target backup.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use tinymemory_api::traits::Memory;
use tinymemory_api::types::MemoryCategory;

pub(crate) fn paths_equal(left: &Path, right: &Path) -> bool {
    if let (Ok(left), Ok(right)) = (left.canonicalize(), right.canonicalize()) {
        left == right
    } else {
        left == right
    }
}

pub(crate) fn normalize_key(raw: &str, idx: usize) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return format!("openclaw_{idx}");
    }

    trimmed
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

pub(crate) fn parse_category(raw: &str) -> MemoryCategory {
    match raw.trim().to_lowercase().as_str() {
        "core" => MemoryCategory::Core,
        "daily" => MemoryCategory::Daily,
        "conversation" => MemoryCategory::Conversation,
        "personal" => MemoryCategory::Custom("personal".to_string()),
        "project" => MemoryCategory::Custom("project".to_string()),
        "episode" => MemoryCategory::Custom("episode".to_string()),
        other => MemoryCategory::Custom(other.to_string()),
    }
}

pub(crate) fn backup_target_memory(workspace_dir: &Path) -> Result<Option<PathBuf>> {
    let mem_dir = workspace_dir.join("memory");
    let markdown = workspace_dir.join("MEMORY.md");
    let sqlite = mem_dir.join("brain.db");

    if !mem_dir.exists() && !markdown.exists() && !sqlite.exists() {
        return Ok(None);
    }

    let backup_dir = workspace_dir.join("memory_backup");
    fs::create_dir_all(&backup_dir)?;

    if markdown.exists() {
        let dest = backup_dir.join("MEMORY.md");
        fs::copy(&markdown, &dest).ok();
    }

    if sqlite.exists() {
        let dest = backup_dir.join("brain.db");
        fs::copy(&sqlite, &dest).ok();
    }

    if mem_dir.exists() {
        let dest_dir = backup_dir.join("memory");
        if !dest_dir.exists() {
            fs::create_dir_all(&dest_dir).ok();
        }
        for entry in fs::read_dir(&mem_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let dest = dest_dir.join(
                path.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("memory.md"),
            );
            fs::copy(&path, &dest).ok();
        }
    }

    Ok(Some(backup_dir))
}

pub(crate) async fn next_available_key(memory: &dyn Memory, key: &str) -> Result<String> {
    let mut idx = 1u32;
    loop {
        let candidate = format!("{key}_{idx}");
        if memory.get("", &candidate).await?.is_none() {
            return Ok(candidate);
        }
        idx += 1;
    }
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;

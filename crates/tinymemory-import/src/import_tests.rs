use super::*;
use std::collections::BTreeMap;
use std::sync::Mutex;
use tinymemory_api::types::{MemoryEntry, MemoryTaint, NamespaceSummary, RecallOpts};

/// A `Memory` that keeps entries in a map, for asserting what an import wrote.
#[derive(Default)]
struct MapMemory {
    rows: Mutex<BTreeMap<String, (String, MemoryCategory)>>,
}

impl MapMemory {
    fn rows(&self) -> BTreeMap<String, (String, MemoryCategory)> {
        self.rows.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Memory for MapMemory {
    fn name(&self) -> &str {
        "map"
    }
    async fn store(
        &self,
        _namespace: &str,
        key: &str,
        content: &str,
        category: MemoryCategory,
        _session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        self.rows
            .lock()
            .unwrap()
            .insert(key.to_string(), (content.to_string(), category));
        Ok(())
    }
    async fn recall(
        &self,
        _query: &str,
        _limit: usize,
        _opts: RecallOpts<'_>,
    ) -> anyhow::Result<Vec<MemoryEntry>> {
        Ok(Vec::new())
    }
    async fn get(&self, _namespace: &str, key: &str) -> anyhow::Result<Option<MemoryEntry>> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .get(key)
            .map(|(content, category)| MemoryEntry {
                id: key.to_string(),
                key: key.to_string(),
                content: content.clone(),
                namespace: None,
                category: category.clone(),
                timestamp: String::new(),
                session_id: None,
                score: None,
                taint: MemoryTaint::Internal,
            }))
    }
    async fn list(
        &self,
        _namespace: Option<&str>,
        _category: Option<&MemoryCategory>,
        _session_id: Option<&str>,
    ) -> anyhow::Result<Vec<MemoryEntry>> {
        Ok(Vec::new())
    }
    async fn forget(&self, _namespace: &str, key: &str) -> anyhow::Result<bool> {
        Ok(self.rows.lock().unwrap().remove(key).is_some())
    }
    async fn namespace_summaries(&self) -> anyhow::Result<Vec<NamespaceSummary>> {
        Ok(Vec::new())
    }
    async fn count(&self) -> anyhow::Result<usize> {
        Ok(self.rows.lock().unwrap().len())
    }
    async fn health_check(&self) -> bool {
        true
    }
}

fn target(memory: &Arc<MapMemory>) -> impl FnOnce() -> Result<Arc<dyn Memory>> {
    let memory = memory.clone();
    move || Ok(memory as Arc<dyn Memory>)
}

fn refuse() -> Result<Arc<dyn Memory>> {
    bail!("refusing to import memory into the null driver")
}

#[test]
fn resolve_hermes_workspace_returns_override_when_provided() {
    let custom = PathBuf::from("/custom/hermes");
    assert_eq!(
        resolve_hermes_workspace(Some(custom.clone())).unwrap(),
        custom
    );
}

#[test]
fn resolve_hermes_workspace_defaults_to_home_dot_hermes() {
    let result = resolve_hermes_workspace(None).unwrap();
    #[cfg(windows)]
    {
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            assert_eq!(result, PathBuf::from(local_app_data).join("hermes"));
            return;
        }
    }
    let home = directories::UserDirs::new().unwrap();
    assert_eq!(result, home.home_dir().join(".hermes"));
}

#[test]
fn resolve_openclaw_workspace_defaults_under_home() {
    let result = resolve_openclaw_workspace(None).unwrap();
    assert!(result.ends_with(".openclaw/workspace") || result.ends_with(".openclaw\\workspace"));
}

#[tokio::test]
async fn openclaw_missing_source_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let err = migrate_openclaw_memory(tmp.path(), Some(tmp.path().join("nope")), true, refuse)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("OpenClaw workspace not found"));
}

#[tokio::test]
async fn self_migration_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let err = migrate_openclaw_memory(tmp.path(), Some(tmp.path().to_path_buf()), true, refuse)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("refusing self-migration"));
    let err = migrate_hermes_memory(tmp.path(), Some(tmp.path().to_path_buf()), true, refuse)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("refusing self-migration"));
}

#[tokio::test]
async fn openclaw_empty_source_reports_what_it_checked() {
    let source = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let report = migrate_openclaw_memory(
        dest.path(),
        Some(source.path().to_path_buf()),
        false,
        refuse,
    )
    .await
    .unwrap();
    assert_eq!(report.stats.imported, 0);
    assert_eq!(report.target_workspace, dest.path());
    assert!(report.warnings[0].starts_with("No importable memory found in"));
    assert_eq!(
        report.warnings[1],
        "Checked for: memory/brain.db, MEMORY.md, memory/*.md"
    );
}

fn openclaw_source() -> tempfile::TempDir {
    let source = tempfile::tempdir().unwrap();
    fs::create_dir_all(source.path().join("memory")).unwrap();
    fs::write(source.path().join("MEMORY.md"), "  top level  ").unwrap();
    fs::write(
        source.path().join("memory").join("2024 notes.md"),
        "daily note",
    )
    .unwrap();
    fs::write(source.path().join("memory").join("empty.md"), "   ").unwrap();
    fs::write(source.path().join("memory").join("ignored.txt"), "x").unwrap();
    source
}

#[tokio::test]
async fn openclaw_dry_run_counts_without_opening_the_target() {
    let source = openclaw_source();
    let dest = tempfile::tempdir().unwrap();
    let report =
        migrate_openclaw_memory(dest.path(), Some(source.path().to_path_buf()), true, refuse)
            .await
            .unwrap();
    assert!(report.dry_run);
    assert_eq!(report.stats.from_markdown, 2);
    assert_eq!(report.stats.imported, 0);
    assert!(!dest.path().join("memory_backup").exists());
}

#[tokio::test]
async fn openclaw_apply_writes_markdown_entries_and_reruns_are_idempotent() {
    let source = openclaw_source();
    let dest = tempfile::tempdir().unwrap();
    let memory = Arc::new(MapMemory::default());

    let report = migrate_openclaw_memory(
        dest.path(),
        Some(source.path().to_path_buf()),
        false,
        target(&memory),
    )
    .await
    .unwrap();
    assert_eq!(report.stats.imported, 2);
    let rows = memory.rows();
    assert_eq!(rows["openclaw_memory_md"].0, "top level");
    assert_eq!(rows["2024_notes"].0, "daily note");
    assert_eq!(rows["2024_notes"].1, MemoryCategory::Core);

    let again = migrate_openclaw_memory(
        dest.path(),
        Some(source.path().to_path_buf()),
        false,
        target(&memory),
    )
    .await
    .unwrap();
    assert_eq!(again.stats.imported, 0);
    assert_eq!(again.stats.skipped_unchanged, 2);
}

#[tokio::test]
async fn a_key_holding_different_content_is_renamed_not_overwritten() {
    let source = openclaw_source();
    let dest = tempfile::tempdir().unwrap();
    let memory = Arc::new(MapMemory::default());
    memory
        .store("", "openclaw_memory_md", "mine", MemoryCategory::Core, None)
        .await
        .unwrap();

    let report = migrate_openclaw_memory(
        dest.path(),
        Some(source.path().to_path_buf()),
        false,
        target(&memory),
    )
    .await
    .unwrap();
    assert_eq!(report.stats.renamed_conflicts, 1);
    let rows = memory.rows();
    assert_eq!(rows["openclaw_memory_md"].0, "mine");
    assert_eq!(rows["openclaw_memory_md_1"].0, "top level");
}

#[tokio::test]
async fn a_refused_target_leaves_the_backup_but_writes_nothing() {
    let source = openclaw_source();
    let dest = tempfile::tempdir().unwrap();
    fs::write(dest.path().join("MEMORY.md"), "existing").unwrap();

    let err = migrate_openclaw_memory(
        dest.path(),
        Some(source.path().to_path_buf()),
        false,
        refuse,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("null driver"));
    // The backup is taken before the target is opened: the order hosts rely on.
    assert!(dest.path().join("memory_backup").join("MEMORY.md").exists());
}

#[tokio::test]
async fn openclaw_sqlite_entries_are_read_with_detected_columns() {
    let source = tempfile::tempdir().unwrap();
    fs::create_dir_all(source.path().join("memory")).unwrap();
    let conn = rusqlite::Connection::open(source.path().join("memory").join("brain.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE memories (name TEXT, value TEXT, kind TEXT);
         INSERT INTO memories VALUES ('Project Plan', ' ship it ', 'project');
         INSERT INTO memories VALUES ('blank', '   ', 'core');",
    )
    .unwrap();
    drop(conn);
    let dest = tempfile::tempdir().unwrap();
    let memory = Arc::new(MapMemory::default());

    let report = migrate_openclaw_memory(
        dest.path(),
        Some(source.path().to_path_buf()),
        false,
        target(&memory),
    )
    .await
    .unwrap();
    assert_eq!(report.stats.from_sqlite, 1);
    let rows = memory.rows();
    assert_eq!(rows["Project_Plan"].0, "ship it");
    assert_eq!(
        rows["Project_Plan"].1,
        MemoryCategory::Custom("project".to_string())
    );
}

#[tokio::test]
async fn openclaw_table_without_a_content_column_is_an_error() {
    let source = tempfile::tempdir().unwrap();
    fs::create_dir_all(source.path().join("memory")).unwrap();
    let conn = rusqlite::Connection::open(source.path().join("memory").join("brain.db")).unwrap();
    conn.execute_batch("CREATE TABLE memories (id TEXT, weight INTEGER);")
        .unwrap();
    drop(conn);
    let dest = tempfile::tempdir().unwrap();
    let err = migrate_openclaw_memory(dest.path(), Some(source.path().to_path_buf()), true, refuse)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no content-like column"));
}

#[tokio::test]
async fn hermes_apply_maps_each_profile_file_to_its_key_and_category() {
    let source = tempfile::tempdir().unwrap();
    fs::write(source.path().join("MEMORY.md"), "remember").unwrap();
    fs::write(source.path().join("USER.md"), "who").unwrap();
    fs::write(source.path().join("SOUL.md"), "  ").unwrap();
    let dest = tempfile::tempdir().unwrap();
    let memory = Arc::new(MapMemory::default());

    let report = migrate_hermes_memory(
        dest.path(),
        Some(source.path().to_path_buf()),
        false,
        target(&memory),
    )
    .await
    .unwrap();
    assert_eq!(report.stats.from_markdown, 2);
    assert_eq!(report.stats.imported, 2);
    assert!(report
        .warnings
        .iter()
        .any(|w| w == "SOUL.md is empty, skipping"));
    let rows = memory.rows();
    assert_eq!(rows["hermes_memory"].1, MemoryCategory::Core);
    assert_eq!(
        rows["hermes_user_profile"].1,
        MemoryCategory::Custom("user_profile".to_string())
    );
}

#[tokio::test]
async fn hermes_empty_workspace_reports_what_it_checked() {
    let source = tempfile::tempdir().unwrap();
    let dest = tempfile::tempdir().unwrap();
    let report = migrate_hermes_memory(
        dest.path(),
        Some(source.path().to_path_buf()),
        false,
        refuse,
    )
    .await
    .unwrap();
    assert!(report
        .warnings
        .iter()
        .any(|w| w.starts_with("MEMORY.md not found in")));
    assert!(report
        .warnings
        .iter()
        .any(|w| w == "Checked for: MEMORY.md, USER.md, SOUL.md"));
}

#[test]
fn report_serialises_with_the_shape_the_rpc_surface_documents() {
    let report = MigrationReport {
        source_workspace: PathBuf::from("/s"),
        target_workspace: PathBuf::from("/t"),
        dry_run: true,
        stats: MigrationStats::default(),
        warnings: vec!["w".into()],
    };
    assert_eq!(
        serde_json::to_value(&report).unwrap(),
        serde_json::json!({
            "source_workspace": "/s",
            "target_workspace": "/t",
            "dry_run": true,
            "stats": {
                "from_sqlite": 0,
                "from_markdown": 0,
                "imported": 0,
                "skipped_unchanged": 0,
                "renamed_conflicts": 0
            },
            "warnings": ["w"]
        })
    );
}

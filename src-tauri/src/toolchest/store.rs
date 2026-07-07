//! Tool Set persistence (spec "Tools & Tool Sets"): one versioned JSON file per Tool Set
//! in the app-data dir (own format, sync-friendly per spec section 2), plus a single
//! auto-populated "Recent Tools" list (MRU, capped).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{anyhow, Context as _};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{Tool, ToolSet};

/// Cap on the Recent Tools MRU list.
const RECENT_CAP: usize = 20;
const FILE_VERSION: u32 = 1;

/// On-disk envelope for a Tool Set file - versioned so a future format change can migrate
/// old files on load instead of failing to parse them.
#[derive(Debug, Serialize, Deserialize)]
struct ToolSetFile {
    version: u32,
    #[serde(flatten)]
    set: ToolSet,
}

fn set_file_name(id: Uuid) -> String {
    format!("{id}.toolset.json")
}

/// Manages the on-disk Tool Chest. In-memory cache mirrors disk; every mutation writes
/// through synchronously before updating the cache, so a crash mid-write never leaves the
/// cache ahead of disk.
pub struct ToolChestStore {
    dir: PathBuf,
    sets: Mutex<Vec<ToolSet>>,
    recent: Mutex<Vec<Tool>>,
}

impl ToolChestStore {
    /// Load the store rooted at `app_data_dir/toolchest`. Missing directories are created;
    /// unreadable/corrupt individual Tool Set files are skipped (not fatal - one bad file
    /// must not take down the whole Tool Chest).
    pub fn load(app_data_dir: &Path) -> anyhow::Result<Self> {
        let dir = app_data_dir.join("toolchest");
        let sets_dir = dir.join("toolsets");
        fs::create_dir_all(&sets_dir).with_context(|| format!("create {}", sets_dir.display()))?;

        let mut sets = Vec::new();
        let mut entries: Vec<_> = fs::read_dir(&sets_dir)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(text) = fs::read_to_string(&path) {
                if let Ok(file) = serde_json::from_str::<ToolSetFile>(&text) {
                    sets.push(file.set);
                }
            }
        }

        let recent_path = dir.join("recent.json");
        let recent = fs::read_to_string(&recent_path)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<Tool>>(&s).ok())
            .unwrap_or_default();

        Ok(Self { dir, sets: Mutex::new(sets), recent: Mutex::new(recent) })
    }

    /// An in-memory-only store (no disk I/O) - used for command-layer tests and any
    /// short-lived context where a real app-data dir isn't available.
    pub fn in_memory() -> Self {
        Self { dir: PathBuf::new(), sets: Mutex::new(Vec::new()), recent: Mutex::new(Vec::new()) }
    }

    fn sets_dir(&self) -> PathBuf {
        self.dir.join("toolsets")
    }

    fn recent_path(&self) -> PathBuf {
        self.dir.join("recent.json")
    }

    fn persist_disabled(&self) -> bool {
        self.dir.as_os_str().is_empty()
    }

    fn write_set(&self, set: &ToolSet) -> anyhow::Result<()> {
        if self.persist_disabled() {
            return Ok(());
        }
        fs::create_dir_all(self.sets_dir())?;
        let file = ToolSetFile { version: FILE_VERSION, set: set.clone() };
        let text = serde_json::to_string_pretty(&file)?;
        fs::write(self.sets_dir().join(set_file_name(set.id)), text)?;
        Ok(())
    }

    fn write_recent(&self, recent: &[Tool]) -> anyhow::Result<()> {
        if self.persist_disabled() {
            return Ok(());
        }
        fs::create_dir_all(&self.dir)?;
        fs::write(self.recent_path(), serde_json::to_string_pretty(recent)?)?;
        Ok(())
    }

    pub fn list_sets(&self) -> Vec<ToolSet> {
        self.sets.lock().unwrap().clone()
    }

    pub fn recent(&self) -> Vec<Tool> {
        self.recent.lock().unwrap().clone()
    }

    pub fn create_set(&self, name: String) -> anyhow::Result<ToolSet> {
        let set = ToolSet::new(name);
        self.write_set(&set)?;
        self.sets.lock().unwrap().push(set.clone());
        Ok(set)
    }

    pub fn rename_set(&self, set_id: Uuid, name: String) -> anyhow::Result<()> {
        let snapshot = {
            let mut sets = self.sets.lock().unwrap();
            let set = sets
                .iter_mut()
                .find(|s| s.id == set_id)
                .ok_or_else(|| anyhow!("unknown tool set {set_id}"))?;
            set.name = name;
            set.clone()
        };
        self.write_set(&snapshot)
    }

    pub fn delete_set(&self, set_id: Uuid) -> anyhow::Result<()> {
        self.sets.lock().unwrap().retain(|s| s.id != set_id);
        if !self.persist_disabled() {
            let path = self.sets_dir().join(set_file_name(set_id));
            if path.exists() {
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }

    pub fn add_tool(&self, set_id: Uuid, tool: Tool) -> anyhow::Result<Tool> {
        let snapshot = {
            let mut sets = self.sets.lock().unwrap();
            let set = sets
                .iter_mut()
                .find(|s| s.id == set_id)
                .ok_or_else(|| anyhow!("unknown tool set {set_id}"))?;
            set.tools.push(tool.clone());
            set.clone()
        };
        self.write_set(&snapshot)?;
        Ok(tool)
    }

    pub fn delete_tool(&self, set_id: Uuid, tool_id: Uuid) -> anyhow::Result<()> {
        let snapshot = {
            let mut sets = self.sets.lock().unwrap();
            let set = sets
                .iter_mut()
                .find(|s| s.id == set_id)
                .ok_or_else(|| anyhow!("unknown tool set {set_id}"))?;
            set.tools.retain(|t| t.id != tool_id);
            set.clone()
        };
        self.write_set(&snapshot)
    }

    /// Reorder a set's tools to match `tool_ids` (front to back). Ids not present in the
    /// set are ignored; tools not named in `tool_ids` keep their relative order, appended
    /// after the named ones - so a partial reorder never silently drops a tool.
    pub fn reorder_tools(&self, set_id: Uuid, tool_ids: Vec<Uuid>) -> anyhow::Result<()> {
        let snapshot = {
            let mut sets = self.sets.lock().unwrap();
            let set = sets
                .iter_mut()
                .find(|s| s.id == set_id)
                .ok_or_else(|| anyhow!("unknown tool set {set_id}"))?;
            let mut reordered = Vec::with_capacity(set.tools.len());
            for id in &tool_ids {
                if let Some(pos) = set.tools.iter().position(|t| t.id == *id) {
                    reordered.push(set.tools.remove(pos));
                }
            }
            reordered.append(&mut set.tools);
            set.tools = reordered;
            set.clone()
        };
        self.write_set(&snapshot)
    }

    /// Import an already-built [`ToolSet`] (e.g. from the `.btx` importer) as a new set.
    pub fn import_set(&self, set: ToolSet) -> anyhow::Result<ToolSet> {
        self.write_set(&set)?;
        self.sets.lock().unwrap().push(set.clone());
        Ok(set)
    }

    /// Record a tool as recently used: move-to-front, de-duplicated by tool id, capped at
    /// [`RECENT_CAP`].
    pub fn record_recent(&self, tool: Tool) -> anyhow::Result<()> {
        let snapshot = {
            let mut recent = self.recent.lock().unwrap();
            recent.retain(|t| t.id != tool.id);
            recent.insert(0, tool);
            recent.truncate(RECENT_CAP);
            recent.clone()
        };
        self.write_recent(&snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::{Appearance, MarkupType};
    use crate::toolchest::PlacementMode;

    fn sample_tool(name: &str) -> Tool {
        Tool {
            id: Uuid::new_v4(),
            name: name.to_string(),
            markup_type: MarkupType::Rectangle,
            appearance: Appearance::default(),
            subject: None,
            placement_mode: PlacementMode::Properties,
            geometry: None,
            stamp: None,
        }
    }

    // --- (b) ToolSet JSON persistence load/save ---

    #[test]
    fn create_set_persists_and_reloads() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ToolChestStore::load(tmp.path()).unwrap();
        let set = store.create_set("Doors".to_string()).unwrap();

        // Fresh store instance over the same dir must see the persisted set.
        let reloaded = ToolChestStore::load(tmp.path()).unwrap();
        let sets = reloaded.list_sets();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].id, set.id);
        assert_eq!(sets[0].name, "Doors");
    }

    #[test]
    fn add_tool_persists_across_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ToolChestStore::load(tmp.path()).unwrap();
        let set = store.create_set("Doors".to_string()).unwrap();
        store.add_tool(set.id, sample_tool("Fire Door")).unwrap();

        let reloaded = ToolChestStore::load(tmp.path()).unwrap();
        let sets = reloaded.list_sets();
        assert_eq!(sets[0].tools.len(), 1);
        assert_eq!(sets[0].tools[0].name, "Fire Door");
    }

    #[test]
    fn delete_tool_removes_it_and_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ToolChestStore::load(tmp.path()).unwrap();
        let set = store.create_set("Doors".to_string()).unwrap();
        let tool = store.add_tool(set.id, sample_tool("Fire Door")).unwrap();

        store.delete_tool(set.id, tool.id).unwrap();
        assert!(store.list_sets()[0].tools.is_empty());

        let reloaded = ToolChestStore::load(tmp.path()).unwrap();
        assert!(reloaded.list_sets()[0].tools.is_empty());
    }

    #[test]
    fn delete_set_removes_its_file() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ToolChestStore::load(tmp.path()).unwrap();
        let set = store.create_set("Temp Set".to_string()).unwrap();
        store.delete_set(set.id).unwrap();

        let reloaded = ToolChestStore::load(tmp.path()).unwrap();
        assert!(reloaded.list_sets().is_empty());
    }

    #[test]
    fn rename_set_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ToolChestStore::load(tmp.path()).unwrap();
        let set = store.create_set("Old Name".to_string()).unwrap();
        store.rename_set(set.id, "New Name".to_string()).unwrap();

        let reloaded = ToolChestStore::load(tmp.path()).unwrap();
        assert_eq!(reloaded.list_sets()[0].name, "New Name");
    }

    #[test]
    fn reorder_tools_applies_new_order_and_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ToolChestStore::load(tmp.path()).unwrap();
        let set = store.create_set("Set".to_string()).unwrap();
        let a = store.add_tool(set.id, sample_tool("A")).unwrap();
        let b = store.add_tool(set.id, sample_tool("B")).unwrap();
        let c = store.add_tool(set.id, sample_tool("C")).unwrap();

        store.reorder_tools(set.id, vec![c.id, a.id, b.id]).unwrap();
        let names: Vec<String> = store.list_sets()[0].tools.iter().map(|t| t.name.clone()).collect();
        assert_eq!(names, vec!["C", "A", "B"]);

        let reloaded = ToolChestStore::load(tmp.path()).unwrap();
        let names: Vec<String> = reloaded.list_sets()[0].tools.iter().map(|t| t.name.clone()).collect();
        assert_eq!(names, vec!["C", "A", "B"]);
    }

    #[test]
    fn reorder_tools_appends_unnamed_ids_rather_than_dropping_them() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ToolChestStore::load(tmp.path()).unwrap();
        let set = store.create_set("Set".to_string()).unwrap();
        let a = store.add_tool(set.id, sample_tool("A")).unwrap();
        let _b = store.add_tool(set.id, sample_tool("B")).unwrap();

        // Only naming `a` in the new order: `b` must still be present, appended after.
        store.reorder_tools(set.id, vec![a.id]).unwrap();
        let names: Vec<String> = store.list_sets()[0].tools.iter().map(|t| t.name.clone()).collect();
        assert_eq!(names, vec!["A", "B"]);
    }

    // --- Recent Tools (auto-populated MRU set) ---

    #[test]
    fn record_recent_adds_to_front() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ToolChestStore::load(tmp.path()).unwrap();
        store.record_recent(sample_tool("First")).unwrap();
        store.record_recent(sample_tool("Second")).unwrap();

        let recent = store.recent();
        assert_eq!(recent[0].name, "Second");
        assert_eq!(recent[1].name, "First");
    }

    #[test]
    fn record_recent_deduplicates_by_id_moving_to_front() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ToolChestStore::load(tmp.path()).unwrap();
        let t = sample_tool("Repeated");
        store.record_recent(sample_tool("Other")).unwrap();
        store.record_recent(t.clone()).unwrap();
        store.record_recent(t.clone()).unwrap();

        let recent = store.recent();
        assert_eq!(recent.len(), 2, "re-using the same tool must not duplicate it");
        assert_eq!(recent[0].id, t.id);
    }

    #[test]
    fn record_recent_caps_at_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ToolChestStore::load(tmp.path()).unwrap();
        for i in 0..(RECENT_CAP + 5) {
            store.record_recent(sample_tool(&format!("Tool {i}"))).unwrap();
        }
        assert_eq!(store.recent().len(), RECENT_CAP);
        // Most recent survives; oldest were evicted.
        assert_eq!(store.recent()[0].name, format!("Tool {}", RECENT_CAP + 4));
    }

    #[test]
    fn recent_persists_across_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ToolChestStore::load(tmp.path()).unwrap();
        store.record_recent(sample_tool("Persisted")).unwrap();

        let reloaded = ToolChestStore::load(tmp.path()).unwrap();
        assert_eq!(reloaded.recent().len(), 1);
        assert_eq!(reloaded.recent()[0].name, "Persisted");
    }

    #[test]
    fn corrupt_tool_set_file_is_skipped_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ToolChestStore::load(tmp.path()).unwrap();
        store.create_set("Good Set".to_string()).unwrap();

        // Drop a corrupt file alongside the good one.
        let sets_dir = tmp.path().join("toolchest").join("toolsets");
        fs::write(sets_dir.join("garbage.toolset.json"), "{ not json").unwrap();

        let reloaded = ToolChestStore::load(tmp.path()).unwrap();
        assert_eq!(reloaded.list_sets().len(), 1, "the corrupt file is skipped, not fatal");
        assert_eq!(reloaded.list_sets()[0].name, "Good Set");
    }
}

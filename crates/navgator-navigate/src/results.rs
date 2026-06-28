use crate::config::load_config;
use crate::git::{git_worktree_label, git_worktrees_for_path};
use crate::model::{
    AppResult, BuildItemsResult, GitWorktree, NavigateEntry, NavigateEntryKind, ResultUpdate,
};
use crate::provider_runtime::{
    load_json_cache, save_json_cache, spawn_batched_jobs, unix_timestamp,
};
use crate::search::entry_name;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Duration,
};

const WORKTREE_PROVIDER_PREFIX: &str = "worktree:";
const WORKTREE_CACHE_FILE: &str = "worktrees.json";
const WORKTREE_CACHE_VERSION: u32 = 2;
const BRANCH_ICON: &str = "";

pub(crate) trait ResultProvider {
    fn initial_entries(&self) -> Vec<NavigateEntry>;
    fn spawn_updates(&self, _tx: mpsc::Sender<ResultUpdate>) {}
}

pub(crate) struct ProjectResultProvider {
    paths: Vec<PathBuf>,
}

pub(crate) struct WorktreeResultProvider {
    project_entries: Vec<NavigateEntry>,
}

#[derive(Deserialize, Serialize)]
struct WorktreeCache {
    version: u32,
    generated_at: u64,
    entries: Vec<NavigateEntry>,
}

pub(crate) fn build_items() -> AppResult<BuildItemsResult> {
    let config = load_config()?;
    let project_provider =
        ProjectResultProvider::from_config_paths(config.static_items, config.index_folders);
    let mut entries = project_provider.initial_entries();
    let worktree_provider = WorktreeResultProvider {
        project_entries: entries.clone(),
    };
    entries.extend(worktree_provider.initial_entries());
    Ok(BuildItemsResult {
        entries,
        preview_settings: config.preview_settings,
    })
}

pub(crate) fn spawn_worktree_result_provider(
    project_entries: &[NavigateEntry],
    tx: mpsc::Sender<ResultUpdate>,
) {
    let provider = WorktreeResultProvider {
        project_entries: project_entries.to_vec(),
    };
    provider.spawn_updates(tx);
}

impl ProjectResultProvider {
    fn from_config_paths(static_items: Vec<PathBuf>, index_folders: Vec<PathBuf>) -> Self {
        let mut paths: Vec<PathBuf> = static_items;

        for folder in index_folders {
            paths.push(folder.clone());
            let mut children: Vec<PathBuf> = Vec::new();
            if let Ok(read_dir) = fs::read_dir(&folder) {
                for entry in read_dir.flatten() {
                    let path = entry.path();
                    if is_dir(&path) {
                        children.push(path);
                    }
                }
            }
            children.sort();
            paths.extend(children);
        }

        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for path in paths {
            let key = path.to_string_lossy().to_string();
            if seen.insert(key.clone()) {
                out.push(PathBuf::from(key));
            }
        }
        Self { paths: out }
    }
}

impl ResultProvider for ProjectResultProvider {
    fn initial_entries(&self) -> Vec<NavigateEntry> {
        self.paths
            .iter()
            .map(|path| project_entry(&path.to_string_lossy()))
            .collect()
    }
}

impl ResultProvider for WorktreeResultProvider {
    fn initial_entries(&self) -> Vec<NavigateEntry> {
        let project_roots = self
            .project_entries
            .iter()
            .map(|entry| entry.preview_root_path.as_str())
            .collect::<HashSet<&str>>();
        load_worktree_cache()
            .into_iter()
            .filter(|entry| project_roots.contains(entry.preview_root_path.as_str()))
            .collect()
    }

    fn spawn_updates(&self, tx: mpsc::Sender<ResultUpdate>) {
        let project_entries = self.project_entries.clone();
        thread::spawn(move || {
            let jobs = dedupe_worktree_scan_jobs(project_entries);
            if jobs.is_empty() {
                return;
            }

            let (batch_tx, batch_rx) = mpsc::channel::<Vec<NavigateEntry>>();
            spawn_batched_jobs(
                jobs,
                32,
                Duration::from_millis(100),
                batch_tx,
                scan_worktree_job,
            );

            let mut refreshed = Vec::new();
            let mut seen = HashSet::new();
            for batch in batch_rx {
                let mut unique_batch = Vec::new();
                for entry in batch {
                    if seen.insert(entry.selection_path.clone()) {
                        unique_batch.push(entry.clone());
                        refreshed.push(entry);
                    }
                }
                if !unique_batch.is_empty() {
                    let _ = tx.send(ResultUpdate::Entries {
                        entries: unique_batch,
                    });
                }
            }

            let _ = save_worktree_cache(&refreshed);
            let _ = tx.send(ResultUpdate::ReplaceProviderEntries {
                provider_prefix: WORKTREE_PROVIDER_PREFIX.to_string(),
                entries: refreshed,
            });
        });
    }
}

fn load_worktree_cache() -> Vec<NavigateEntry> {
    load_json_cache::<WorktreeCache>(WORKTREE_CACHE_FILE)
        .filter(|cache| cache.version == WORKTREE_CACHE_VERSION)
        .map(|cache| cache.entries)
        .unwrap_or_default()
}

fn save_worktree_cache(entries: &[NavigateEntry]) -> std::io::Result<()> {
    save_json_cache(
        WORKTREE_CACHE_FILE,
        &WorktreeCache {
            version: WORKTREE_CACHE_VERSION,
            generated_at: unix_timestamp(),
            entries: entries.to_vec(),
        },
    )
}

fn dedupe_worktree_scan_jobs(project_entries: Vec<NavigateEntry>) -> Vec<NavigateEntry> {
    let mut seen = HashSet::new();
    let mut jobs = Vec::new();
    for entry in project_entries {
        if !matches!(entry.kind, NavigateEntryKind::Project) {
            continue;
        }
        if seen.insert(entry.preview_root_path.clone()) {
            jobs.push(entry);
        }
    }
    jobs
}

fn scan_worktree_job(entry: NavigateEntry) -> Vec<NavigateEntry> {
    let repo_path = entry.preview_root_path.clone();
    let worktrees = git_worktrees_for_path(Path::new(&repo_path));
    if worktrees.is_empty() {
        return Vec::new();
    }
    worktrees
        .into_iter()
        .filter(|worktree| !worktree.bare && worktree.path != repo_path)
        .map(|worktree| worktree_entry(&entry, &worktree))
        .collect()
}

fn project_entry(path: &str) -> NavigateEntry {
    let display = entry_name(path);
    NavigateEntry {
        id: format!("project:{path}"),
        display: display.clone(),
        context: None,
        preview_root_path: path.to_string(),
        preferred_preview_path: None,
        selection_path: path.to_string(),
        metadata_path: path.to_string(),
        search_text: vec![display],
        kind: NavigateEntryKind::Project,
    }
}

fn worktree_entry(repo_entry: &NavigateEntry, worktree: &GitWorktree) -> NavigateEntry {
    let branch = git_worktree_label(worktree, true);
    let repo_label = repo_entry.display.clone();
    let display = format!("{BRANCH_ICON} {repo_label} {branch}");
    NavigateEntry {
        id: format!(
            "{WORKTREE_PROVIDER_PREFIX}{}:{}",
            repo_entry.preview_root_path, worktree.path
        ),
        display: display.clone(),
        context: None,
        preview_root_path: repo_entry.preview_root_path.clone(),
        preferred_preview_path: Some(worktree.path.clone()),
        selection_path: worktree.path.clone(),
        metadata_path: worktree.path.clone(),
        search_text: vec![display],
        kind: NavigateEntryKind::Worktree { repo_label, branch },
    }
}

fn is_dir(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_dir())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::{project_entry, worktree_entry, WorktreeCache, WORKTREE_CACHE_VERSION};
    use crate::model::GitWorktree;

    #[test]
    fn worktree_entry_selects_worktree_but_previews_repo() {
        let repo = super::project_entry("/repos/app");
        let worktree = GitWorktree {
            path: "/repos/app-QCDI-8206".to_string(),
            branch: Some("QCDI-8206".to_string()),
            detached: false,
            bare: false,
        };

        let entry = worktree_entry(&repo, &worktree);

        assert_eq!(entry.display, " app QCDI-8206");
        assert!(entry.context.is_none());
        assert_eq!(entry.preview_root_path, "/repos/app");
        assert_eq!(
            entry.preferred_preview_path.as_deref(),
            Some("/repos/app-QCDI-8206")
        );
        assert_eq!(entry.selection_path, "/repos/app-QCDI-8206");
        assert!(matches!(
            entry.kind,
            crate::model::NavigateEntryKind::Worktree { .. }
        ));
    }

    #[test]
    fn worktree_cache_round_trips_entries() {
        let repo = project_entry("/repos/app");
        let worktree = GitWorktree {
            path: "/repos/app-QCDI-8206".to_string(),
            branch: Some("QCDI-8206".to_string()),
            detached: false,
            bare: false,
        };
        let cache = WorktreeCache {
            version: WORKTREE_CACHE_VERSION,
            generated_at: 1,
            entries: vec![worktree_entry(&repo, &worktree)],
        };

        let json = serde_json::to_string(&cache).expect("cache should serialize");
        let restored: WorktreeCache = serde_json::from_str(&json).expect("cache should parse");

        assert_eq!(restored.version, WORKTREE_CACHE_VERSION);
        assert_eq!(restored.entries[0].display, " app QCDI-8206");
        assert_eq!(restored.entries[0].selection_path, "/repos/app-QCDI-8206");
    }
}

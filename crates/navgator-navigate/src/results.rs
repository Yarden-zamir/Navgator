use crate::config::load_config;
use crate::model::{AppResult, BuildItemsResult};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

pub(crate) fn build_items() -> AppResult<BuildItemsResult> {
    let config = load_config()?;
    let mut items: Vec<PathBuf> = config.static_items;
    let index_folders = config.index_folders;
    let preview_settings = config.preview_settings;

    for folder in index_folders {
        items.push(folder.clone());
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
        items.extend(children);
    }

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for path in items {
        let key = path.to_string_lossy().to_string();
        if seen.insert(key.clone()) {
            out.push(key);
        }
    }
    Ok(BuildItemsResult {
        items: out,
        preview_settings,
    })
}

fn is_dir(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_dir())
        .unwrap_or(false)
}

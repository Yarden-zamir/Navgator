use crate::model::{default_preview_settings, AppResult, LoadedConfig, CONFIG_SCHEMA_URL};
use figment::providers::{Format, Toml};
use figment::Figment;
use schemars::{schema_for, JsonSchema};
use serde::Deserialize;
use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Default, Deserialize, JsonSchema)]
#[schemars(
    title = "Navgator Config",
    description = "Configuration file for navgator path indexing and static items."
)]
struct ConfigFile {
    #[serde(default, rename = "$schema")]
    #[schemars(
        title = "Schema URL",
        description = "Optional JSON Schema URL for editor autocompletion and validation."
    )]
    _schema_url: Option<String>,
    #[serde(default)]
    #[schemars(
        title = "Paths",
        description = "Path collection settings used to build the navigation list."
    )]
    paths: Option<ConfigPaths>,
    #[serde(default)]
    #[schemars(title = "Preview", description = "Preview panel settings.")]
    preview: Option<ConfigPreview>,
}

#[derive(Default, Deserialize, JsonSchema)]
#[schemars(
    title = "Path Settings",
    description = "Groups of folders that navgator indexes or always includes."
)]
struct ConfigPaths {
    #[serde(default)]
    #[schemars(
        title = "Index Folders",
        description = "Directories to index; each directory and its direct child directories are included."
    )]
    index_folders: Vec<String>,
    #[serde(default)]
    #[schemars(
        title = "Static Items",
        description = "Directories or files to include as-is without indexing children."
    )]
    static_items: Vec<String>,
}

#[derive(Default, Deserialize, JsonSchema)]
#[schemars(
    title = "Preview Settings",
    description = "Settings for preview and worktree preview tabs."
)]
struct ConfigPreview {
    #[serde(default)]
    #[schemars(
        title = "Shorten Worktree Tab Labels",
        description = "When true, worktree tab labels use only the segment after the last slash, for example feat/yarden/potato becomes potato. Defaults to true."
    )]
    shorten_worktree_tab_labels: Option<bool>,
    #[serde(default)]
    #[schemars(
        title = "Worktree Tab Minimum Characters",
        description = "Minimum label characters to keep before the ellipsis for non-selected worktree preview tabs. Defaults to 6."
    )]
    worktree_tab_min_chars: Option<usize>,
    #[serde(default)]
    #[schemars(
        title = "Selected Worktree Tab Minimum Characters",
        description = "Minimum label characters to keep before the ellipsis for the selected worktree preview tab. Defaults to 10."
    )]
    selected_worktree_tab_min_chars: Option<usize>,
}

pub(crate) fn config_schema_json() -> AppResult<String> {
    let schema = schema_for!(ConfigFile);
    serde_json::to_string_pretty(&schema)
        .map_err(|err| format!("Failed to serialize config schema: {err}").into())
}

pub(crate) fn load_config() -> AppResult<LoadedConfig> {
    let home = home_dir()?;
    let mut index_folders = Vec::new();
    let mut static_items = Vec::new();
    let mut seen_index = HashSet::new();
    let mut seen_static = HashSet::new();
    let mut preview_settings = default_preview_settings();
    let mut found_config = false;

    for path in config_paths(&home) {
        if !path.is_file() {
            continue;
        }
        found_config = true;
        let base_dir = path.parent().unwrap_or(&home);
        let config: ConfigFile = Figment::from(Toml::file(&path)).extract().map_err(|err| {
            let display_path = display_path_for_user(&path.to_string_lossy());
            format!("Failed to parse config {}: {}", display_path, err)
        })?;
        ensure_schema_link_in_config_file(&path, &config);
        if let Some(paths) = config.paths {
            merge_paths(
                &paths.index_folders,
                base_dir,
                &home,
                &mut index_folders,
                &mut seen_index,
            );
            merge_paths(
                &paths.static_items,
                base_dir,
                &home,
                &mut static_items,
                &mut seen_static,
            );
        }
        if let Some(preview) = config.preview {
            if let Some(value) = preview.shorten_worktree_tab_labels {
                preview_settings.shorten_worktree_tab_labels = value;
            }
            if let Some(value) = preview.worktree_tab_min_chars {
                preview_settings.worktree_tab_min_chars = value.max(1);
            }
            if let Some(value) = preview.selected_worktree_tab_min_chars {
                preview_settings.selected_worktree_tab_min_chars = value.max(1);
            }
        }
    }

    if !found_config {
        return Err("No navgator config found. Create one in ~/.config/navgator/config.toml (or set $NAVGATOR_CONFIG).".into());
    }

    Ok(LoadedConfig {
        index_folders,
        static_items,
        preview_settings,
    })
}

pub(crate) fn home_dir() -> AppResult<PathBuf> {
    let value = env::var("HOME").map_err(|_| "HOME is not set")?;
    Ok(PathBuf::from(value))
}

fn ensure_schema_link_in_config_file(path: &Path, config: &ConfigFile) {
    if config._schema_url.is_some() || config.paths.is_none() {
        return;
    }

    let Ok(contents) = fs::read_to_string(path) else {
        return;
    };

    let schema_line = format!("\"$schema\" = \"{CONFIG_SCHEMA_URL}\"");
    let updated = if contents.trim().is_empty() {
        format!("{schema_line}\n")
    } else if contents.starts_with('\n') {
        format!("{schema_line}\n{contents}")
    } else {
        format!("{schema_line}\n\n{contents}")
    };

    if updated != contents {
        let _ = fs::write(path, updated);
    }
}

fn config_paths(home: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(path) = env::var("NAVGATOR_CONFIG") {
        if !path.trim().is_empty() {
            paths.push(PathBuf::from(path));
        }
    }
    paths.push(PathBuf::from("/etc/navgator/config.toml"));
    let xdg = env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".config"));
    paths.push(xdg.join("navgator/config.toml"));
    paths.push(home.join(".config/navgator/config.toml"));
    paths.push(home.join(".navgator.toml"));
    if let Ok(cwd) = env::current_dir() {
        paths.push(cwd.join(".navgator.toml"));
        paths.push(cwd.join(".navgator/config.toml"));
    }

    let mut seen = HashSet::new();
    let mut unique = Vec::new();
    for path in paths {
        let key = path.to_string_lossy().to_string();
        if seen.insert(key) {
            unique.push(path);
        }
    }
    unique
}

fn merge_paths(
    raw_paths: &[String],
    base_dir: &Path,
    home: &Path,
    target: &mut Vec<PathBuf>,
    seen: &mut HashSet<String>,
) {
    for raw in raw_paths {
        if let Some(path) = normalize_path(raw, base_dir, home) {
            let key = path.to_string_lossy().to_string();
            if seen.insert(key) {
                target.push(path);
            }
        }
    }
}

fn normalize_path(raw: &str, base_dir: &Path, home: &Path) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut value = trimmed.to_string();
    if value.starts_with("~/") {
        value = value.replacen("~", &home.to_string_lossy(), 1);
    }
    if value.contains("$HOME") {
        value = value.replace("$HOME", &home.to_string_lossy());
    }
    let mut path = PathBuf::from(value);
    if path.is_relative() {
        path = base_dir.join(path);
    }
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

fn display_path_for_user(path: &str) -> String {
    match env::var("HOME") {
        Ok(home) => display_path_with_home(path, &home),
        Err(_) => path.to_string(),
    }
}

fn display_path_with_home(path: &str, home: &str) -> String {
    if home.is_empty() {
        return path.to_string();
    }
    if path == home {
        return "~".to_string();
    }

    let home_with_separator = format!(
        "{}{}",
        home.trim_end_matches(std::path::MAIN_SEPARATOR),
        std::path::MAIN_SEPARATOR
    );
    if let Some(rest) = path.strip_prefix(&home_with_separator) {
        return format!("~/{}", rest);
    }

    path.to_string()
}

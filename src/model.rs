#![allow(dead_code)]

use ratatui::{layout::Rect, style::Color, text::Text};
use std::{collections::BTreeMap, error::Error, path::PathBuf};

pub(crate) type AppResult<T> = Result<T, Box<dyn Error>>;
pub(crate) type MatchScore = (usize, usize, usize, usize, usize);

pub(crate) const DATE_WIDTH: usize = 16;
pub(crate) const DATE_PLACEHOLDER: &str = "---- -- -- --:--";
pub(crate) const TAB_DIVIDER_WIDTH: usize = 3;
pub(crate) const DEFAULT_WORKTREE_TAB_MIN_CHARS: usize = 6;
pub(crate) const DEFAULT_SELECTED_WORKTREE_TAB_MIN_CHARS: usize = 10;
pub(crate) const MIN_PARTIAL_TAB_WIDTH: usize = 4;
pub(crate) const CONFIG_SCHEMA_URL: &str =
    "https://raw.githubusercontent.com/Yarden-zamir/Navgator/main/config-schema.json";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ProviderId(pub(crate) String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ResultId(pub(crate) String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ContentId(pub(crate) String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct MetadataKey(pub(crate) String);

#[derive(Clone, Debug)]
pub(crate) enum SelectionValue {
    Path(PathBuf),
    Url(String),
    Text(String),
    ProviderSpecific {
        provider_id: ProviderId,
        value: String,
    },
}

#[derive(Clone, Debug)]
pub(crate) enum MetadataValue {
    Text(String),
    Number(i64),
    Decimal(f64),
    Bool(bool),
    DateTime(i64),
    Tags(Vec<String>),
    List(Vec<MetadataValue>),
}

#[derive(Clone, Debug)]
pub(crate) struct MetadataEntry {
    pub(crate) key: MetadataKey,
    pub(crate) value: MetadataValue,
    pub(crate) display: Option<String>,
    pub(crate) sort_value: Option<MetadataValue>,
}

pub(crate) type MetadataMap = BTreeMap<MetadataKey, MetadataEntry>;

#[derive(Clone, Debug)]
pub(crate) struct ResultEntry {
    pub(crate) id: ResultId,
    pub(crate) provider_id: ProviderId,
    pub(crate) display: String,
    pub(crate) metadata: MetadataMap,
}

#[derive(Clone, Debug)]
pub(crate) struct ContentTarget {
    pub(crate) id: ContentId,
    pub(crate) source_result_id: ResultId,
    pub(crate) provider_id: ProviderId,
    pub(crate) display: String,
    pub(crate) metadata: MetadataMap,
    pub(crate) selection_value: SelectionValue,
}

#[derive(Clone, Debug)]
pub(crate) enum ContentBlock {
    Text { lines: Vec<String> },
    List { items: Vec<String> },
    Tree { lines: Vec<String> },
    Empty { message: String },
    Loading { message: String },
    Error { message: String },
}

#[derive(Clone)]
pub(crate) struct PreviewTab {
    pub(crate) path: String,
    pub(crate) label: String,
    pub(crate) text: Text<'static>,
    pub(crate) git: Option<Text<'static>>,
}

#[derive(Clone)]
pub(crate) struct PreviewData {
    pub(crate) previews: Vec<PreviewTab>,
    pub(crate) selected_repo_is_bare: bool,
    pub(crate) git_loaded: bool,
}

pub(crate) struct PreviewTarget {
    pub(crate) path: String,
    pub(crate) label: String,
}

pub(crate) struct GitWorktree {
    pub(crate) path: String,
    pub(crate) branch: Option<String>,
    pub(crate) detached: bool,
    pub(crate) bare: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct PreviewSettings {
    pub(crate) shorten_worktree_tab_labels: bool,
    pub(crate) worktree_tab_min_chars: usize,
    pub(crate) selected_worktree_tab_min_chars: usize,
}

pub(crate) fn default_preview_settings() -> PreviewSettings {
    PreviewSettings {
        shorten_worktree_tab_labels: true,
        worktree_tab_min_chars: DEFAULT_WORKTREE_TAB_MIN_CHARS,
        selected_worktree_tab_min_chars: DEFAULT_SELECTED_WORKTREE_TAB_MIN_CHARS,
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PreviewColors {
    pub(crate) accent: Color,
    pub(crate) muted: Color,
    pub(crate) text: Color,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct SortMeta {
    pub(crate) modified_epoch: Option<i64>,
    pub(crate) created_epoch: Option<i64>,
}

pub(crate) struct MetaResult {
    pub(crate) path: String,
    pub(crate) display: Option<String>,
    pub(crate) modified_epoch: Option<i64>,
    pub(crate) created_epoch: Option<i64>,
}

pub(crate) struct TagResult {
    pub(crate) path: String,
    pub(crate) tags: Vec<String>,
}

pub(crate) struct PreviewResult {
    pub(crate) path: String,
    pub(crate) data: PreviewData,
}

pub(crate) struct GitResult {
    pub(crate) path: String,
    pub(crate) tab_index: usize,
    pub(crate) git: Option<Text<'static>>,
    pub(crate) done: bool,
}

pub(crate) struct BuildItemsResult {
    pub(crate) items: Vec<String>,
    pub(crate) preview_settings: PreviewSettings,
}

pub(crate) struct LoadedConfig {
    pub(crate) index_folders: Vec<PathBuf>,
    pub(crate) static_items: Vec<PathBuf>,
    pub(crate) preview_settings: PreviewSettings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortMode {
    Match,
    AlphaAsc,
    AlphaDesc,
    CreatedAsc,
    CreatedDesc,
    ModifiedAsc,
    ModifiedDesc,
}

impl SortMode {
    pub(crate) fn next(self) -> Self {
        match self {
            SortMode::Match => SortMode::AlphaAsc,
            SortMode::AlphaAsc => SortMode::AlphaDesc,
            SortMode::AlphaDesc => SortMode::CreatedAsc,
            SortMode::CreatedAsc => SortMode::CreatedDesc,
            SortMode::CreatedDesc => SortMode::ModifiedAsc,
            SortMode::ModifiedAsc => SortMode::ModifiedDesc,
            SortMode::ModifiedDesc => SortMode::Match,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            SortMode::Match => "Match",
            SortMode::AlphaAsc => "A->Z",
            SortMode::AlphaDesc => "Z->A",
            SortMode::CreatedAsc => "Created ^",
            SortMode::CreatedDesc => "Created v",
            SortMode::ModifiedAsc => "Modified ^",
            SortMode::ModifiedDesc => "Modified v",
        }
    }

    pub(crate) fn uses_time(self) -> bool {
        matches!(
            self,
            SortMode::CreatedAsc
                | SortMode::CreatedDesc
                | SortMode::ModifiedAsc
                | SortMode::ModifiedDesc
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Search,
    Preview,
    Git,
    TagEdit,
}

#[derive(Clone, Copy)]
pub(crate) struct HelpContext {
    pub(crate) focus: Focus,
    pub(crate) sort_mode: SortMode,
    pub(crate) show_git: bool,
    pub(crate) cursor_at_end: bool,
    pub(crate) has_tag_input: bool,
    pub(crate) preview_tab_index: usize,
    pub(crate) preview_tab_count: usize,
    pub(crate) preview_scroll: usize,
    pub(crate) preview_max_scroll: usize,
    pub(crate) git_scroll: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct HelpColors {
    pub(crate) text: Color,
    pub(crate) accent: Color,
    pub(crate) key_color: Color,
}

pub(crate) struct VisibleListArgs<'a> {
    pub(crate) items: &'a [String],
    pub(crate) filtered: &'a [usize],
    pub(crate) selected: usize,
    pub(crate) offset: usize,
    pub(crate) height: usize,
    pub(crate) text: Color,
    pub(crate) muted: Color,
    pub(crate) dates: &'a std::collections::HashMap<String, String>,
    pub(crate) tags: &'a std::collections::HashMap<String, Vec<String>>,
    pub(crate) inner_width: usize,
    pub(crate) tokens: &'a crate::search::QueryTokens,
    pub(crate) elapsed_ms: u64,
}

pub(crate) struct SidePanelRender<'a> {
    pub(crate) area: Rect,
    pub(crate) preview: &'a Text<'static>,
    pub(crate) git: Option<&'a Text<'static>>,
    pub(crate) preview_title: &'a str,
    pub(crate) preview_tab_labels: &'a [String],
    pub(crate) preview_tab_index: usize,
    pub(crate) preview_settings: PreviewSettings,
    pub(crate) focus: Focus,
    pub(crate) accent: Color,
    pub(crate) text: Color,
    pub(crate) preview_scroll: u16,
    pub(crate) git_scroll: u16,
}

#[derive(Clone, Copy)]
pub(crate) struct UiLayout {
    pub(crate) list_area: Rect,
    pub(crate) detail_area: Rect,
    pub(crate) search_area: Rect,
    pub(crate) results_area: Rect,
    pub(crate) preview_area: Rect,
    pub(crate) git_area: Option<Rect>,
    pub(crate) help_area: Rect,
}

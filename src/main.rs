use ansi_to_tui::IntoText;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use figment::providers::{Format, Toml};
use figment::Figment;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};
use serde::Deserialize;
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    env,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tui_input::backend::crossterm::EventHandler;
use tui_input::{Input, InputRequest};

type AppResult<T> = Result<T, Box<dyn Error>>;

const DATE_WIDTH: usize = 16;
const DATE_PLACEHOLDER: &str = "---- -- -- --:--";

#[derive(Clone)]
struct PreviewData {
    preview: Text<'static>,
    git: Option<Text<'static>>,
}

struct PreviewResult {
    path: String,
    data: PreviewData,
}

#[derive(Clone, Copy, Default)]
struct SortMeta {
    modified_epoch: Option<i64>,
    created_epoch: Option<i64>,
}

struct MetaResult {
    path: String,
    display: Option<String>,
    modified_epoch: Option<i64>,
    created_epoch: Option<i64>,
}

struct TagResult {
    path: String,
    tags: Vec<String>,
}

#[derive(Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    paths: Option<ConfigPaths>,
}

#[derive(Default, Deserialize)]
struct ConfigPaths {
    #[serde(default)]
    index_folders: Vec<String>,
    #[serde(default)]
    static_items: Vec<String>,
}

struct LoadedConfig {
    index_folders: Vec<PathBuf>,
    static_items: Vec<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortMode {
    Match,
    AlphaAsc,
    AlphaDesc,
    CreatedAsc,
    CreatedDesc,
    ModifiedAsc,
    ModifiedDesc,
}

impl SortMode {
    fn next(self) -> Self {
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

    fn label(self) -> &'static str {
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

    fn uses_time(self) -> bool {
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
enum Focus {
    Search,
    Preview,
    Git,
    TagEdit,
}

fn main() -> AppResult<()> {
    ensure_tty_stdin()?;
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args[0] == "navigate" {
        return run_navigate();
    }
    if args[0] == "--help" || args[0] == "-h" {
        print_usage();
        return Ok(());
    }

    eprintln!("Unknown command.");
    print_usage();
    std::process::exit(2);
}

fn ensure_tty_stdin() -> AppResult<()> {
    #[cfg(unix)]
    {
        use std::io::IsTerminal;
        use std::os::unix::io::AsRawFd;

        if io::stdin().is_terminal() {
            return Ok(());
        }

        let tty = fs::File::open("/dev/tty")?;
        let result = unsafe { libc::dup2(tty.as_raw_fd(), libc::STDIN_FILENO) };
        if result == -1 {
            return Err(io::Error::last_os_error().into());
        }
    }
    Ok(())
}

fn print_usage() {
    eprintln!("Usage:\n  navgator [navigate]");
}

fn run_navigate() -> AppResult<()> {
    let items = build_items()?;
    match select_from_list("Navigate", &items)? {
        Some(choice) => write_selection(&choice),
        None => std::process::exit(1),
    }
}

fn write_selection(path: &str) -> AppResult<()> {
    if let Ok(output_path) = env::var("NAVGATOR_OUTPUT") {
        if !output_path.is_empty() {
            fs::write(output_path, path)?;
            return Ok(());
        }
    }
    println!("{}", path);
    Ok(())
}

fn build_items() -> AppResult<Vec<String>> {
    let config = load_config()?;
    let mut items: Vec<PathBuf> = config.static_items;
    let index_folders = config.index_folders;

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
    Ok(out)
}

fn home_dir() -> AppResult<PathBuf> {
    let value = env::var("HOME").map_err(|_| "HOME is not set")?;
    Ok(PathBuf::from(value))
}

fn load_config() -> AppResult<LoadedConfig> {
    let home = home_dir()?;
    let mut index_folders = Vec::new();
    let mut static_items = Vec::new();
    let mut seen_index = HashSet::new();
    let mut seen_static = HashSet::new();
    let mut found_config = false;

    for path in config_paths(&home) {
        if !path.is_file() {
            continue;
        }
        found_config = true;
        let base_dir = path.parent().unwrap_or(&home);
        let config: ConfigFile = Figment::from(Toml::file(&path))
            .extract()
            .map_err(|err| format!("Failed to parse config {}: {}", path.display(), err))?;
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
    }

    if !found_config {
        return Err("No navgator config found. Create one in ~/.config/navgator/config.toml (or set $NAVGATOR_CONFIG).".into());
    }

    Ok(LoadedConfig {
        index_folders,
        static_items,
    })
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

fn is_dir(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_dir())
        .unwrap_or(false)
}

fn select_from_list(_title: &str, items: &[String]) -> AppResult<Option<String>> {
    if items.is_empty() {
        return Ok(None);
    }

    let (mut terminal, _guard) = setup_terminal()?;
    let mut input = Input::default();
    let mut selected = 0usize;
    let mut sort_mode = SortMode::Match;
    let mut focus = Focus::Search;
    let mut meta_cache: HashMap<String, SortMeta> = HashMap::new();
    let mut list_offset = 0usize;
    let accent = Color::Rgb(72, 166, 255);
    let warm = Color::Rgb(255, 181, 92);
    let key_color = Color::Rgb(150, 150, 150);
    let text = Color::Black;
    let muted = text;
    let (preview_tx, preview_rx) = mpsc::channel::<PreviewResult>();
    let (date_tx, date_rx) = mpsc::channel::<MetaResult>();
    let (tag_tx, tag_rx) = mpsc::channel::<TagResult>();
    let mut preview_cache: HashMap<String, PreviewData> = HashMap::new();
    let mut date_cache: HashMap<String, String> = HashMap::new();
    let mut date_in_flight: HashSet<String> = HashSet::new();
    let mut tag_cache: HashMap<String, Vec<String>> = HashMap::new();
    let mut tag_in_flight: HashSet<String> = HashSet::new();
    let mut tag_scan_started = false;
    let mut filtered = filter_and_sort(items, input.value(), sort_mode, &meta_cache, &tag_cache);
    let mut preview_path: Option<String> = None;
    let mut in_flight: Option<String> = None;
    let mut preview_text = build_placeholder_text(None, accent, muted, text, "No selection");
    let mut git_text: Option<Text<'static>> = None;
    let mut preview_scroll = 0usize;
    let mut git_scroll = 0usize;
    let mut preview_max_scroll = 0usize;
    let mut git_max_scroll = 0usize;
    let mut preview_page_step = 5usize;
    let mut git_page_step = 5usize;
    let start_time = Instant::now();
    let mut tag_edit_path: Option<String> = None;
    let mut tag_edit_tags: Vec<String> = Vec::new();
    let mut tag_input = Input::default();
    let mut tag_suggestions: Vec<String> = Vec::new();

    loop {
        let current = current_selection_path(items, &filtered, selected);
        let query_value = input.value();
        let tokens = parse_query_tokens(query_value);

        while let Ok(result) = preview_rx.try_recv() {
            preview_cache.insert(result.path.clone(), result.data.clone());
            if current.as_deref() == Some(result.path.as_str()) {
                preview_text = result.data.preview;
                git_text = result.data.git;
                preview_path = Some(result.path.clone());
            }
            if in_flight.as_deref() == Some(result.path.as_str()) {
                in_flight = None;
            }
        }

        let mut resort_needed = false;
        while let Ok(result) = date_rx.try_recv() {
            let display = result
                .display
                .unwrap_or_else(|| DATE_PLACEHOLDER.to_string());
            date_cache.insert(result.path.clone(), display);
            meta_cache.insert(
                result.path.clone(),
                SortMeta {
                    modified_epoch: result.modified_epoch,
                    created_epoch: result.created_epoch,
                },
            );
            date_in_flight.remove(&result.path);
            if sort_mode.uses_time() {
                resort_needed = true;
            }
        }

        let mut tags_changed = false;
        while let Ok(result) = tag_rx.try_recv() {
            tag_cache.insert(result.path.clone(), result.tags);
            tag_in_flight.remove(&result.path);
            tags_changed = true;
        }

        let query_uses_tags = tokens.needs_tags();
        if query_uses_tags && !tag_scan_started {
            spawn_bulk_tag_fetch(items, &tag_cache, &mut tag_in_flight, &tag_tx);
            tag_scan_started = true;
        }

        if resort_needed {
            let selected_path = current_selection_path(items, &filtered, selected);
            filtered = filter_and_sort(items, input.value(), sort_mode, &meta_cache, &tag_cache);
            selected = match selected_path {
                Some(path) => index_for_path(items, &filtered, &path).unwrap_or(0),
                None => adjust_selected_index(selected, filtered.len()),
            };
        }

        if tags_changed && query_uses_tags {
            let selected_path = current_selection_path(items, &filtered, selected);
            filtered = filter_and_sort(items, input.value(), sort_mode, &meta_cache, &tag_cache);
            selected = match selected_path {
                Some(path) => index_for_path(items, &filtered, &path).unwrap_or(0),
                None => adjust_selected_index(selected, filtered.len()),
            };
        }

        match current.as_deref() {
            None => {
                if preview_path.is_some() || in_flight.is_some() {
                    preview_text =
                        build_placeholder_text(None, accent, muted, text, "No selection");
                    git_text = None;
                    preview_path = None;
                    in_flight = None;
                    preview_scroll = 0;
                    git_scroll = 0;
                }
            }
            Some(path) => {
                if preview_path.as_deref() != Some(path) {
                    preview_scroll = 0;
                    git_scroll = 0;
                    if let Some(data) = preview_cache.get(path) {
                        preview_text = data.preview.clone();
                        git_text = data.git.clone();
                        preview_path = Some(path.to_string());
                    } else if in_flight.as_deref() != Some(path) {
                        preview_text = build_placeholder_text(
                            Some(path),
                            accent,
                            muted,
                            text,
                            "Loading preview...",
                        );
                        git_text = Some(build_placeholder_text(
                            Some(path),
                            accent,
                            muted,
                            text,
                            "Loading git info...",
                        ));
                        preview_path = Some(path.to_string());
                        in_flight = Some(path.to_string());
                        let tx = preview_tx.clone();
                        let path_owned = path.to_string();
                        thread::spawn(move || {
                            let preview = build_preview_text(&path_owned, accent, muted, text);
                            let git = build_git_text(&path_owned, accent, muted, text);
                            let _ = tx.send(PreviewResult {
                                path: path_owned,
                                data: PreviewData { preview, git },
                            });
                        });
                    }
                }
            }
        }

        if focus == Focus::Git && git_text.is_none() {
            focus = Focus::Preview;
        }
        if focus == Focus::TagEdit && tag_edit_path.is_none() {
            focus = Focus::Preview;
        }

        let show_git = git_text.is_some();
        let size = terminal.size()?;
        let ui = compute_ui_layout(size.into(), show_git);

        terminal.draw(|frame| {
            let list_area = ui.list_area;
            let detail_area = ui.detail_area;

            let list_title = format!("Results {}/{}", filtered.len(), items.len());
            let left_title = if focus == Focus::Search {
                format!("* {}", list_title)
            } else {
                list_title
            };
            let left_border_style = if focus == Focus::Search {
                Style::default().fg(accent)
            } else {
                Style::default().fg(muted)
            };
            let left_block = Block::default()
                .borders(Borders::ALL)
                .title(left_title)
                .border_style(left_border_style)
                .border_type(BorderType::Rounded);
            frame.render_widget(left_block, list_area);

            let search_area = ui.search_area;
            let results_area = ui.results_area;

            let search_width = search_area.width.saturating_sub(1) as usize;
            let scroll = if search_width > 0 {
                input.visual_scroll(search_width)
            } else {
                0
            };
            let search = Paragraph::new(input.value())
                .scroll((0, scroll as u16))
                .alignment(Alignment::Left)
                .wrap(Wrap { trim: false });
            frame.render_widget(search, search_area);
            if focus == Focus::Search && search_area.width > 0 && search_area.height > 0 {
                let cursor_x = input.visual_cursor().max(scroll).saturating_sub(scroll);
                frame.set_cursor_position((search_area.x + cursor_x as u16, search_area.y));
            }

            let list_inner_height = results_area.height as usize;
            let total = filtered.len();
            list_offset =
                compute_list_window_offset(selected, list_offset, list_inner_height, total);

            let scrollbar_space = if total > 0 { 1 } else { 0 };
            let list_inner_width = results_area.width.saturating_sub(scrollbar_space) as usize;
            let visible_paths =
                visible_paths_for_window(items, &filtered, list_offset, list_inner_height);
            ensure_dates_for_paths(&visible_paths, &date_cache, &mut date_in_flight, &date_tx);
            ensure_tags_for_paths(&visible_paths, &tag_cache, &mut tag_in_flight, &tag_tx);

            let (list_items, list_selected) = build_visible_list_items(
                items,
                &filtered,
                selected,
                list_offset,
                list_inner_height,
                text,
                muted,
                &date_cache,
                &tag_cache,
                list_inner_width,
                &tokens,
                start_time.elapsed().as_millis() as u64,
            );

            let list = List::new(list_items).highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(warm)
                    .add_modifier(Modifier::BOLD),
            );

            let mut state = ListState::default();
            state.select(list_selected);
            frame.render_stateful_widget(list, results_area, &mut state);

            let preview_height = ui.preview_area.height.saturating_sub(2) as usize;
            let git_height = ui
                .git_area
                .map(|rect| rect.height.saturating_sub(2) as usize)
                .unwrap_or(0);
            preview_page_step = preview_height.max(1);
            git_page_step = git_height.max(1);
            let preview_title = current
                .as_deref()
                .map(entry_name)
                .unwrap_or_else(|| "Preview".to_string());
            let preview_tags = if focus == Focus::TagEdit {
                tag_edit_tags.clone()
            } else {
                current
                    .as_deref()
                    .and_then(|path| tag_cache.get(path))
                    .cloned()
                    .unwrap_or_default()
            };
            let preview_width = ui.preview_area.width.saturating_sub(2) as usize;
            let (preview_combined, tag_cursor) = if focus == Focus::TagEdit {
                compose_preview_text_with_input(
                    &preview_text,
                    &preview_tags,
                    &tag_input,
                    preview_width,
                    text,
                )
            } else {
                (
                    compose_preview_text(&preview_text, &preview_tags, preview_width, text),
                    None,
                )
            };
            preview_max_scroll = text_line_count(&preview_combined).saturating_sub(preview_height);
            git_max_scroll = match git_text.as_ref() {
                Some(git) => text_line_count(git).saturating_sub(git_height),
                None => 0,
            };
            if focus == Focus::TagEdit {
                if let Some((row, _)) = tag_cursor {
                    if row < preview_scroll {
                        preview_scroll = row;
                    } else if row >= preview_scroll + preview_height {
                        preview_scroll = row.saturating_sub(preview_height.saturating_sub(1));
                    }
                }
            }
            preview_scroll = preview_scroll.min(preview_max_scroll);
            git_scroll = git_scroll.min(git_max_scroll);
            render_side_panels(
                frame,
                detail_area,
                &preview_combined,
                git_text.as_ref(),
                &preview_title,
                focus,
                accent,
                text,
                preview_scroll as u16,
                git_scroll as u16,
            );
            if focus == Focus::TagEdit {
                if let Some((row, col)) = tag_cursor {
                    let visible_row = row.saturating_sub(preview_scroll);
                    if visible_row < preview_height {
                        let x = ui.preview_area.x + 1 + col as u16;
                        let y = ui.preview_area.y + 1 + visible_row as u16;
                        frame.set_cursor_position((x, y));
                    }
                }
            }

            let help_line = build_help_line(
                focus,
                sort_mode,
                show_git,
                input_at_end(&input),
                !tag_input.value().trim().is_empty(),
                preview_scroll,
                preview_max_scroll,
                git_scroll,
                text,
                accent,
                key_color,
            );
            let help = Paragraph::new(Text::from(help_line))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Keys")
                        .border_style(Style::default().fg(muted))
                        .border_type(BorderType::Rounded),
                )
                .alignment(Alignment::Left)
                .wrap(Wrap { trim: true });
            frame.render_widget(help, ui.help_area);
        })?;

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.code == KeyCode::Esc {
                        terminal.show_cursor()?;
                        return Ok(None);
                    }
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        terminal.show_cursor()?;
                        return Ok(None);
                    }
                    if key.code == KeyCode::Char('t')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                        && focus != Focus::TagEdit
                    {
                        if let Some(path) = current_selection_path(items, &filtered, selected) {
                            tag_edit_path = Some(path.clone());
                            tag_edit_tags = read_tags_for_path(&path);
                            tag_input.reset();
                            tag_suggestions = collect_tag_suggestions(&tag_cache);
                            focus = Focus::TagEdit;
                            preview_scroll = 0;
                        }
                        continue;
                    }
                    if key.code == KeyCode::Enter && focus != Focus::TagEdit {
                        if let Some(index) = filtered.get(selected) {
                            let value = items[*index].clone();
                            terminal.show_cursor()?;
                            return Ok(Some(value));
                        }
                    }
                    if key.code == KeyCode::Char('s')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        sort_mode = sort_mode.next();
                        filtered = filter_and_sort(
                            items,
                            input.value(),
                            sort_mode,
                            &meta_cache,
                            &tag_cache,
                        );
                        selected = 0;
                        list_offset = 0;
                        if sort_mode.uses_time() {
                            spawn_bulk_metadata_fetch(
                                items,
                                &date_cache,
                                &mut date_in_flight,
                                &date_tx,
                            );
                        }
                        if parse_query_tokens(input.value()).needs_tags() && !tag_scan_started {
                            spawn_bulk_tag_fetch(items, &tag_cache, &mut tag_in_flight, &tag_tx);
                            tag_scan_started = true;
                        }
                        continue;
                    }

                    match focus {
                        Focus::Search => match key.code {
                            KeyCode::Up => {
                                if selected > 0 {
                                    selected -= 1;
                                }
                            }
                            KeyCode::Down => {
                                if selected + 1 < filtered.len() {
                                    selected += 1;
                                }
                            }
                            KeyCode::Right
                                if !key.modifiers.intersects(
                                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                                ) && input_at_end(&input) =>
                            {
                                focus = Focus::Preview;
                            }
                            _ => {
                                let before = input.value().to_string();
                                if key.modifiers.contains(KeyModifiers::SUPER) {
                                    if key.code == KeyCode::Left {
                                        input.handle(InputRequest::GoToStart);
                                    } else if key.code == KeyCode::Right {
                                        input.handle(InputRequest::GoToEnd);
                                    }
                                } else if key.code == KeyCode::Char('u')
                                    && key.modifiers.contains(KeyModifiers::CONTROL)
                                {
                                    input.handle(InputRequest::DeleteLine);
                                } else {
                                    let _ = input.handle_event(&Event::Key(key));
                                }
                                if input.value() != before {
                                    filtered = filter_and_sort(
                                        items,
                                        input.value(),
                                        sort_mode,
                                        &meta_cache,
                                        &tag_cache,
                                    );
                                    selected = 0;
                                    list_offset = 0;
                                }
                            }
                        },
                        Focus::TagEdit => match key.code {
                            KeyCode::Enter => {
                                commit_tag_input(
                                    &mut tag_input,
                                    &mut tag_edit_tags,
                                    &tag_suggestions,
                                );
                                if let Some(path) = tag_edit_path.clone() {
                                    save_tags_for_path(&path, &tag_edit_tags)?;
                                    tag_cache.insert(path.clone(), tag_edit_tags.clone());
                                }
                                focus = Focus::Preview;
                                tag_edit_path = None;
                                tag_edit_tags.clear();
                                tag_input.reset();
                                let selected_path =
                                    current_selection_path(items, &filtered, selected);
                                filtered = filter_and_sort(
                                    items,
                                    input.value(),
                                    sort_mode,
                                    &meta_cache,
                                    &tag_cache,
                                );
                                selected = match selected_path {
                                    Some(value) => {
                                        index_for_path(items, &filtered, &value).unwrap_or(0)
                                    }
                                    None => adjust_selected_index(selected, filtered.len()),
                                };
                            }
                            KeyCode::Tab => {
                                commit_tag_input(
                                    &mut tag_input,
                                    &mut tag_edit_tags,
                                    &tag_suggestions,
                                );
                            }
                            KeyCode::Backspace => {
                                if tag_input.value().is_empty() {
                                    tag_edit_tags.pop();
                                } else {
                                    let _ = tag_input.handle_event(&Event::Key(key));
                                }
                            }
                            _ => {
                                let _ = tag_input.handle_event(&Event::Key(key));
                            }
                        },
                        Focus::Preview => match key.code {
                            KeyCode::Left => {
                                focus = Focus::Search;
                            }
                            KeyCode::Right => {
                                if git_text.is_some() {
                                    focus = Focus::Git;
                                }
                            }
                            KeyCode::Up => {
                                if preview_scroll > 0 {
                                    preview_scroll -= 1;
                                } else if preview_scroll == 0 {
                                    focus = Focus::Search;
                                }
                            }
                            KeyCode::Down => {
                                if preview_scroll < preview_max_scroll {
                                    preview_scroll += 1;
                                } else if preview_scroll >= preview_max_scroll && git_text.is_some()
                                {
                                    focus = Focus::Git;
                                }
                            }
                            KeyCode::PageUp => {
                                preview_scroll = preview_scroll.saturating_sub(preview_page_step);
                            }
                            KeyCode::PageDown => {
                                preview_scroll =
                                    (preview_scroll + preview_page_step).min(preview_max_scroll);
                            }
                            KeyCode::Home => {
                                preview_scroll = 0;
                            }
                            KeyCode::End => {
                                preview_scroll = preview_max_scroll;
                            }
                            _ => {}
                        },
                        Focus::Git => match key.code {
                            KeyCode::Left => {
                                focus = Focus::Search;
                            }
                            KeyCode::Right => {
                                focus = Focus::Preview;
                            }
                            KeyCode::Up => {
                                if git_scroll > 0 {
                                    git_scroll -= 1;
                                } else if git_scroll == 0 {
                                    focus = Focus::Preview;
                                }
                            }
                            KeyCode::Down => {
                                if git_scroll < git_max_scroll {
                                    git_scroll += 1;
                                }
                            }
                            KeyCode::PageUp => {
                                git_scroll = git_scroll.saturating_sub(git_page_step);
                            }
                            KeyCode::PageDown => {
                                git_scroll = (git_scroll + git_page_step).min(git_max_scroll);
                            }
                            KeyCode::Home => {
                                git_scroll = 0;
                            }
                            KeyCode::End => {
                                git_scroll = git_max_scroll;
                            }
                            _ => {}
                        },
                    }
                }
                Event::Mouse(mouse) => {
                    let col = mouse.column;
                    let row = mouse.row;
                    match mouse.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            if rect_contains(ui.list_area, col, row) {
                                focus = Focus::Search;
                            } else if let Some(git_area) = ui.git_area {
                                if rect_contains(git_area, col, row) {
                                    focus = Focus::Git;
                                } else if rect_contains(ui.preview_area, col, row) {
                                    focus = Focus::Preview;
                                }
                            } else if rect_contains(ui.preview_area, col, row) {
                                focus = Focus::Preview;
                            }
                        }
                        MouseEventKind::ScrollUp => {
                            if rect_contains(ui.preview_area, col, row) {
                                preview_scroll = preview_scroll.saturating_sub(1);
                            } else if let Some(git_area) = ui.git_area {
                                if rect_contains(git_area, col, row) {
                                    git_scroll = git_scroll.saturating_sub(1);
                                }
                            } else if rect_contains(ui.results_area, col, row) {
                                if selected > 0 {
                                    selected -= 1;
                                }
                            }
                        }
                        MouseEventKind::ScrollDown => {
                            if rect_contains(ui.preview_area, col, row) {
                                preview_scroll = (preview_scroll + 1).min(preview_max_scroll);
                            } else if let Some(git_area) = ui.git_area {
                                if rect_contains(git_area, col, row) {
                                    git_scroll = (git_scroll + 1).min(git_max_scroll);
                                }
                            } else if rect_contains(ui.results_area, col, row) {
                                if selected + 1 < filtered.len() {
                                    selected += 1;
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
}

fn filter_indices(
    items: &[String],
    query: &str,
    tag_cache: &HashMap<String, Vec<String>>,
) -> Vec<usize> {
    let tokens = parse_query_tokens(query);
    if tokens.is_empty() {
        return (0..items.len()).collect();
    }
    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let tags = tag_cache.get(item).map(Vec::as_slice).unwrap_or(&[]);
            if matches_tokens(item, tags, &tokens) {
                Some(index)
            } else {
                None
            }
        })
        .collect()
}

fn filter_and_sort_by_match(
    items: &[String],
    query: &str,
    tag_cache: &HashMap<String, Vec<String>>,
) -> Vec<usize> {
    let tokens = parse_query_tokens(query);
    if tokens.is_empty() {
        return (0..items.len()).collect();
    }
    let mut scored: Vec<(usize, (usize, usize, usize, usize, usize))> = Vec::new();
    for (index, path) in items.iter().enumerate() {
        let tags = tag_cache.get(path).map(Vec::as_slice).unwrap_or(&[]);
        if !matches_tokens(path, tags, &tokens) {
            continue;
        }
        if let Some(score) = match_score_tokens(&tokens, path, tags) {
            scored.push((index, score));
        }
    }
    scored.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    scored.into_iter().map(|(index, _)| index).collect()
}

#[derive(Default)]
struct QueryTokens {
    folder: Vec<String>,
    tags: Vec<String>,
    any: Vec<String>,
}

impl QueryTokens {
    fn is_empty(&self) -> bool {
        self.folder.is_empty() && self.tags.is_empty() && self.any.is_empty()
    }

    fn needs_tags(&self) -> bool {
        !self.tags.is_empty() || !self.any.is_empty()
    }
}

fn parse_query_tokens(query: &str) -> QueryTokens {
    let mut tokens = QueryTokens::default();
    for raw in query.split_whitespace() {
        if let Some(rest) = raw.strip_prefix('@') {
            if !rest.is_empty() {
                tokens.folder.push(rest.to_string());
            }
        } else if let Some(rest) = raw.strip_prefix('#') {
            if !rest.is_empty() {
                tokens.tags.push(rest.to_string());
            }
        } else if !raw.is_empty() {
            tokens.any.push(raw.to_string());
        }
    }
    tokens
}

fn matches_tokens(path: &str, tags: &[String], tokens: &QueryTokens) -> bool {
    for token in &tokens.folder {
        if !matches_path_token(token, path) {
            return false;
        }
    }

    for token in &tokens.tags {
        if !tags.iter().any(|tag| fuzzy_match(token, tag)) {
            return false;
        }
    }

    for token in &tokens.any {
        let path_match = matches_path_token(token, path);
        let tag_match = tags.iter().any(|tag| fuzzy_match(token, tag));
        if !(path_match || tag_match) {
            return false;
        }
    }

    true
}

fn matches_path_token(token: &str, path: &str) -> bool {
    let entry = entry_name(path);
    fuzzy_match(token, &entry) || fuzzy_match(token, path)
}

fn match_score_tokens(
    tokens: &QueryTokens,
    path: &str,
    tags: &[String],
) -> Option<(usize, usize, usize, usize, usize)> {
    let mut penalty_sum = 0usize;
    let mut span_sum = 0usize;
    let mut gap_sum = 0usize;
    let mut start_sum = 0usize;
    let mut len_sum = 0usize;

    for token in &tokens.folder {
        let score = match_score_for_path(token, path)?;
        penalty_sum = penalty_sum.saturating_add(score.0);
        span_sum = span_sum.saturating_add(score.1);
        gap_sum = gap_sum.saturating_add(score.2);
        start_sum = start_sum.saturating_add(score.3);
        len_sum = len_sum.saturating_add(score.4);
    }

    for token in &tokens.tags {
        let score = best_tag_score(token, tags)?;
        penalty_sum = penalty_sum.saturating_add(score.0);
        span_sum = span_sum.saturating_add(score.1);
        gap_sum = gap_sum.saturating_add(score.2);
        start_sum = start_sum.saturating_add(score.3);
        len_sum = len_sum.saturating_add(score.4);
    }

    for token in &tokens.any {
        let mut best = match_score_for_path(token, path);
        if let Some(tag_score) = best_tag_score(token, tags) {
            best = match best {
                Some(path_score) => Some(path_score.min(tag_score)),
                None => Some(tag_score),
            };
        }
        let score = best?;
        penalty_sum = penalty_sum.saturating_add(score.0);
        span_sum = span_sum.saturating_add(score.1);
        gap_sum = gap_sum.saturating_add(score.2);
        start_sum = start_sum.saturating_add(score.3);
        len_sum = len_sum.saturating_add(score.4);
    }

    Some((penalty_sum, span_sum, gap_sum, start_sum, len_sum))
}

fn best_tag_score(token: &str, tags: &[String]) -> Option<(usize, usize, usize, usize, usize)> {
    let mut best: Option<(usize, usize, usize, usize, usize)> = None;
    for tag in tags {
        if let Some(score) = match_score(token, tag) {
            best = match best {
                Some(current) => Some(current.min(score)),
                None => Some(score),
            };
        }
    }
    best
}

fn match_score_for_path(token: &str, path: &str) -> Option<(usize, usize, usize, usize, usize)> {
    let entry = entry_name(path);
    if let Some(score) = match_score(token, &entry) {
        return Some(score);
    }
    if let Some(score) = match_score(token, path) {
        return Some((
            score.0.saturating_add(2),
            score.1,
            score.2,
            score.3,
            score.4,
        ));
    }
    None
}

fn filter_and_sort(
    items: &[String],
    query: &str,
    sort_mode: SortMode,
    meta_cache: &HashMap<String, SortMeta>,
    tag_cache: &HashMap<String, Vec<String>>,
) -> Vec<usize> {
    if sort_mode == SortMode::Match {
        return filter_and_sort_by_match(items, query, tag_cache);
    }
    let mut indices = filter_indices(items, query, tag_cache);
    sort_indices(&mut indices, items, sort_mode, meta_cache);
    indices
}

fn sort_indices(
    indices: &mut Vec<usize>,
    items: &[String],
    sort_mode: SortMode,
    meta_cache: &HashMap<String, SortMeta>,
) {
    indices.sort_by(|a, b| compare_indices(*a, *b, items, sort_mode, meta_cache));
}

fn compare_indices(
    left: usize,
    right: usize,
    items: &[String],
    sort_mode: SortMode,
    meta_cache: &HashMap<String, SortMeta>,
) -> Ordering {
    let left_path = &items[left];
    let right_path = &items[right];

    match sort_mode {
        SortMode::Match => compare_names(left_path, right_path),
        SortMode::AlphaAsc => compare_names(left_path, right_path),
        SortMode::AlphaDesc => compare_names(right_path, left_path),
        SortMode::CreatedAsc => compare_time(left_path, right_path, meta_cache, TimeField::Created)
            .then_with(|| compare_names(left_path, right_path)),
        SortMode::CreatedDesc => {
            compare_time(right_path, left_path, meta_cache, TimeField::Created)
                .then_with(|| compare_names(left_path, right_path))
        }
        SortMode::ModifiedAsc => {
            compare_time(left_path, right_path, meta_cache, TimeField::Modified)
                .then_with(|| compare_names(left_path, right_path))
        }
        SortMode::ModifiedDesc => {
            compare_time(right_path, left_path, meta_cache, TimeField::Modified)
                .then_with(|| compare_names(left_path, right_path))
        }
    }
}

fn compare_names(left: &str, right: &str) -> Ordering {
    let left_name = entry_name(left).to_lowercase();
    let right_name = entry_name(right).to_lowercase();
    left_name.cmp(&right_name).then_with(|| left.cmp(right))
}

#[derive(Clone, Copy)]
enum TimeField {
    Created,
    Modified,
}

fn compare_time(
    left: &str,
    right: &str,
    meta_cache: &HashMap<String, SortMeta>,
    field: TimeField,
) -> Ordering {
    let left_meta = meta_cache.get(left).copied().unwrap_or_default();
    let right_meta = meta_cache.get(right).copied().unwrap_or_default();
    let left_time = match field {
        TimeField::Created => left_meta.created_epoch,
        TimeField::Modified => left_meta.modified_epoch,
    };
    let right_time = match field {
        TimeField::Created => right_meta.created_epoch,
        TimeField::Modified => right_meta.modified_epoch,
    };

    match (left_time, right_time) {
        (Some(left_value), Some(right_value)) => left_value.cmp(&right_value),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn index_for_path(items: &[String], filtered: &[usize], path: &str) -> Option<usize> {
    filtered.iter().position(|index| {
        items
            .get(*index)
            .map(|candidate| candidate == path)
            .unwrap_or(false)
    })
}

fn build_help_line(
    focus: Focus,
    sort_mode: SortMode,
    show_git: bool,
    cursor_at_end: bool,
    has_tag_input: bool,
    preview_scroll: usize,
    preview_max_scroll: usize,
    git_scroll: usize,
    text: Color,
    accent: Color,
    key_color: Color,
) -> Line<'static> {
    let key_style = Style::default().fg(key_color).add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(accent).add_modifier(Modifier::BOLD);
    let regular_style = Style::default().fg(text);
    let mut spans: Vec<Span> = Vec::new();

    match focus {
        Focus::Search => {
            spans.push(Span::styled("Search", label_style));
            spans.push(Span::styled("  ", regular_style));
            if cursor_at_end {
                spans.push(Span::styled("Right", key_style));
                spans.push(Span::styled(" preview  ", regular_style));
            }
            spans.push(Span::styled("Ctrl+T", key_style));
            spans.push(Span::styled(" tag  ", regular_style));
            spans.push(Span::styled("Ctrl+S", key_style));
            spans.push(Span::styled(
                format!(" {}  ", sort_mode.label()),
                regular_style,
            ));
            spans.push(Span::styled("Ctrl+U", key_style));
            spans.push(Span::styled(" clear", regular_style));
        }
        Focus::Preview => {
            spans.push(Span::styled("Preview", label_style));
            spans.push(Span::styled("  ", regular_style));
            spans.push(Span::styled("Left", key_style));
            spans.push(Span::styled(" search  ", regular_style));
            if show_git {
                spans.push(Span::styled("Right", key_style));
                spans.push(Span::styled(" git  ", regular_style));
            }
            spans.push(Span::styled("Ctrl+T", key_style));
            spans.push(Span::styled(" tag  ", regular_style));
            if preview_scroll == 0 {
                spans.push(Span::styled("Up", key_style));
                spans.push(Span::styled(" search  ", regular_style));
            }
            if show_git && preview_scroll >= preview_max_scroll {
                spans.push(Span::styled("Down", key_style));
                spans.push(Span::styled(" git", regular_style));
            }
        }
        Focus::Git => {
            spans.push(Span::styled("Git", label_style));
            spans.push(Span::styled("  ", regular_style));
            spans.push(Span::styled("Left", key_style));
            spans.push(Span::styled(" search  ", regular_style));
            spans.push(Span::styled("Right", key_style));
            spans.push(Span::styled(" preview  ", regular_style));
            spans.push(Span::styled("Ctrl+T", key_style));
            spans.push(Span::styled(" tag  ", regular_style));
            if git_scroll == 0 {
                spans.push(Span::styled("Up", key_style));
                spans.push(Span::styled(" preview", regular_style));
            }
        }
        Focus::TagEdit => {
            spans.push(Span::styled("Tag", label_style));
            spans.push(Span::styled("  ", regular_style));
            spans.push(Span::styled("Tab", key_style));
            spans.push(Span::styled(" add  ", regular_style));
            spans.push(Span::styled("Enter", key_style));
            if has_tag_input {
                spans.push(Span::styled(" add+done", regular_style));
            } else {
                spans.push(Span::styled(" done", regular_style));
            }
        }
    }

    Line::from(spans)
}

#[derive(Clone, Copy)]
struct UiLayout {
    list_area: Rect,
    detail_area: Rect,
    search_area: Rect,
    results_area: Rect,
    preview_area: Rect,
    git_area: Option<Rect>,
    help_area: Rect,
}

fn compute_ui_layout(size: Rect, show_git: bool) -> UiLayout {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(3)])
        .split(size);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(chunks[0]);

    let list_area = body[0];
    let detail_area = body[1];
    let left_inner = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .inner(list_area);
    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(left_inner);
    let search_area = left_chunks[0];
    let results_area = left_chunks[1];

    let (preview_area, git_area) = if show_git {
        let panels = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(detail_area);
        (panels[0], Some(panels[1]))
    } else {
        (detail_area, None)
    };

    UiLayout {
        list_area,
        detail_area,
        search_area,
        results_area,
        preview_area,
        git_area,
        help_area: chunks[1],
    }
}

fn rect_contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x
        && col < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

fn build_preview_title_line(title: &str, focused: bool, text: Color) -> Line<'static> {
    let label = if focused {
        format!("* {}", title)
    } else {
        title.to_string()
    };
    Line::from(Span::styled(label, Style::default().fg(text)))
}

fn text_line_count(text: &Text) -> usize {
    text.lines.len()
}

fn input_at_end(input: &Input) -> bool {
    input.cursor() >= input.value().chars().count()
}

fn fuzzy_match(query: &str, text: &str) -> bool {
    let mut chars = query.chars().filter(|c| !c.is_whitespace());
    let mut current = chars.next();
    if current.is_none() {
        return true;
    }

    for t in text.chars() {
        if let Some(q) = current {
            if q.eq_ignore_ascii_case(&t) {
                current = chars.next();
                if current.is_none() {
                    return true;
                }
            }
        }
    }
    false
}

fn match_score(query: &str, text: &str) -> Option<(usize, usize, usize, usize, usize)> {
    let qchars: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    if qchars.is_empty() {
        return Some((0, 0, 0, 0, text.chars().count()));
    }

    if let Some(start) = find_case_insensitive(text, query) {
        let span = qchars.len().saturating_sub(1);
        return Some((0, span, 0, start, text.chars().count()));
    }

    let mut positions: Vec<usize> = Vec::with_capacity(qchars.len());
    let mut qi = 0usize;
    for (ti, t) in text.chars().enumerate() {
        if qi >= qchars.len() {
            break;
        }
        if qchars[qi].eq_ignore_ascii_case(&t) {
            positions.push(ti);
            qi += 1;
        }
    }

    if qi < qchars.len() {
        return None;
    }

    let start = *positions.first().unwrap_or(&0);
    let end = *positions.last().unwrap_or(&start);
    let span = end.saturating_sub(start);
    let mut gaps = 0usize;
    for window in positions.windows(2) {
        if let [prev, next] = window {
            gaps = gaps.saturating_add(next.saturating_sub(prev + 1));
        }
    }
    let text_len = text.chars().count();
    Some((1, span, gaps, start, text_len))
}

fn find_case_insensitive(text: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let text_lower = text.to_lowercase();
    let needle_lower = needle.to_lowercase();
    let byte_index = text_lower.find(&needle_lower)?;
    Some(char_index_from_byte(text, byte_index))
}

fn char_index_from_byte(text: &str, byte_index: usize) -> usize {
    text.char_indices()
        .take_while(|(idx, _)| *idx < byte_index)
        .count()
}

fn adjust_selected_index(current: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else if current >= len {
        len - 1
    } else {
        current
    }
}

fn compute_list_window_offset(
    selected: usize,
    current_offset: usize,
    height: usize,
    total: usize,
) -> usize {
    if total == 0 || height == 0 {
        return 0;
    }

    let mut offset = current_offset.min(total.saturating_sub(1));
    if selected < offset {
        offset = selected;
    } else if selected >= offset + height {
        offset = selected + 1 - height;
    }

    let max_offset = total.saturating_sub(height);
    if offset > max_offset {
        offset = max_offset;
    }
    offset
}

fn build_visible_list_items(
    items: &[String],
    filtered: &[usize],
    selected: usize,
    offset: usize,
    height: usize,
    text: Color,
    muted: Color,
    dates: &HashMap<String, String>,
    tags: &HashMap<String, Vec<String>>,
    inner_width: usize,
    tokens: &QueryTokens,
    elapsed_ms: u64,
) -> (Vec<ListItem<'static>>, Option<usize>) {
    if filtered.is_empty() || height == 0 {
        let item = ListItem::new(Line::from(Span::styled(
            "No matches",
            Style::default().fg(muted),
        )));
        return (vec![item], None);
    }

    let end = (offset + height).min(filtered.len());
    let visible = &filtered[offset..end];
    let mut list_items = Vec::with_capacity(visible.len());

    for item_index in visible.iter() {
        let path = &items[*item_index];
        let entry = entry_name(path);
        let date_value = dates
            .get(path)
            .map(String::as_str)
            .unwrap_or(DATE_PLACEHOLDER);
        let date_display = format_date_display(date_value);
        let date_len = date_display.chars().count();
        let tag_list = tags.get(path).map(Vec::as_slice).unwrap_or(&[]);
        let max_entry = inner_width.saturating_sub(date_len + 1);
        let mut entry_display = truncate_with_ellipsis(&entry, max_entry);
        let mut entry_len = entry_display.chars().count();
        if entry_len + date_len + 1 > inner_width {
            let new_len = inner_width.saturating_sub(date_len + 1);
            entry_display = truncate_with_ellipsis(&entry, new_len);
            entry_len = entry_display.chars().count();
        }

        let remaining = inner_width.saturating_sub(entry_len + date_len);
        let tag_space = remaining.saturating_sub(1);
        let (tag_spans, tag_len) = if tag_space > 0 {
            build_tag_spans(tag_list, tokens, tag_space, elapsed_ms, text)
        } else {
            (Vec::new(), 0)
        };
        let tag_block_len = if tag_len > 0 { tag_len + 1 } else { 0 };
        let right_block_len = date_len + tag_block_len;
        let padding = inner_width.saturating_sub(entry_len + right_block_len);
        let mut spans = Vec::new();
        spans.push(Span::styled(entry_display, Style::default().fg(text)));
        spans.push(Span::raw(" ".repeat(padding)));
        if tag_len > 0 {
            spans.push(Span::raw(" "));
            spans.extend(tag_spans);
        }
        spans.push(Span::raw(" "));
        spans.push(Span::styled(date_display, Style::default().fg(muted)));
        let line = Line::from(spans);
        list_items.push(ListItem::new(line));
    }

    let list_selected = selected.checked_sub(offset);
    (list_items, list_selected)
}

fn entry_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|part| part.to_str())
        .unwrap_or(path)
        .to_string()
}

fn current_selection_path(items: &[String], filtered: &[usize], selected: usize) -> Option<String> {
    filtered
        .get(selected)
        .and_then(|index| items.get(*index))
        .cloned()
}

fn visible_paths_for_window(
    items: &[String],
    filtered: &[usize],
    offset: usize,
    height: usize,
) -> Vec<String> {
    if filtered.is_empty() || height == 0 {
        return Vec::new();
    }
    let end = (offset + height).min(filtered.len());
    filtered[offset..end]
        .iter()
        .filter_map(|index| items.get(*index))
        .cloned()
        .collect()
}

fn ensure_dates_for_paths(
    paths: &[String],
    cache: &HashMap<String, String>,
    in_flight: &mut HashSet<String>,
    tx: &mpsc::Sender<MetaResult>,
) {
    for path in paths {
        if cache.contains_key(path) || in_flight.contains(path) {
            continue;
        }
        in_flight.insert(path.clone());
        let path_owned = path.clone();
        let tx = tx.clone();
        thread::spawn(move || {
            let meta = fetch_metadata(&path_owned);
            let _ = tx.send(meta);
        });
    }
}

fn ensure_tags_for_paths(
    paths: &[String],
    cache: &HashMap<String, Vec<String>>,
    in_flight: &mut HashSet<String>,
    tx: &mpsc::Sender<TagResult>,
) {
    for path in paths {
        if cache.contains_key(path) || in_flight.contains(path) {
            continue;
        }
        in_flight.insert(path.clone());
        let path_owned = path.clone();
        let tx = tx.clone();
        thread::spawn(move || {
            let tags = read_tags_for_path(&path_owned);
            let _ = tx.send(TagResult {
                path: path_owned,
                tags,
            });
        });
    }
}

fn spawn_bulk_tag_fetch(
    items: &[String],
    cache: &HashMap<String, Vec<String>>,
    in_flight: &mut HashSet<String>,
    tx: &mpsc::Sender<TagResult>,
) {
    let mut missing = Vec::new();
    for path in items {
        if cache.contains_key(path) || in_flight.contains(path) {
            continue;
        }
        in_flight.insert(path.clone());
        missing.push(path.clone());
    }
    if missing.is_empty() {
        return;
    }
    let tx = tx.clone();
    thread::spawn(move || {
        for path in missing {
            let tags = read_tags_for_path(&path);
            let _ = tx.send(TagResult { path, tags });
        }
    });
}

fn read_tags_for_path(path: &str) -> Vec<String> {
    let dir = Path::new(path);
    let config_path = dir.join(".navgator.toml");
    if !config_path.is_file() {
        return Vec::new();
    }
    let contents = match fs::read_to_string(config_path) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    parse_tags_from_toml(&contents)
}

fn parse_tags_from_toml(contents: &str) -> Vec<String> {
    let mut in_tags = false;
    let mut buffer = String::new();
    for line in contents.lines() {
        let mut cleaned = line;
        if let Some(hash) = cleaned.find('#') {
            cleaned = &cleaned[..hash];
        }
        let trimmed = cleaned.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !in_tags {
            if let Some(eq_pos) = trimmed.find('=') {
                let key = trimmed[..eq_pos].trim();
                if key == "tags" {
                    let value = trimmed[eq_pos + 1..].trim();
                    buffer.push_str(value);
                    buffer.push(' ');
                    if value.contains('[') {
                        in_tags = true;
                    }
                    if value.contains(']') {
                        break;
                    }
                }
            }
        } else {
            buffer.push_str(trimmed);
            buffer.push(' ');
            if trimmed.contains(']') {
                break;
            }
        }
    }

    if buffer.is_empty() {
        return Vec::new();
    }
    extract_quoted_strings(&buffer)
}

fn extract_quoted_strings(value: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '"' {
            let mut text = String::new();
            while let Some(next) = chars.next() {
                if next == '"' {
                    break;
                }
                text.push(next);
            }
            if !text.is_empty() {
                tags.push(text);
            }
        }
    }
    tags
}

fn spawn_bulk_metadata_fetch(
    items: &[String],
    cache: &HashMap<String, String>,
    in_flight: &mut HashSet<String>,
    tx: &mpsc::Sender<MetaResult>,
) {
    let mut missing = Vec::new();
    for path in items {
        if cache.contains_key(path) || in_flight.contains(path) {
            continue;
        }
        in_flight.insert(path.clone());
        missing.push(path.clone());
    }
    if missing.is_empty() {
        return;
    }
    let tx = tx.clone();
    thread::spawn(move || {
        for path in missing {
            let meta = fetch_metadata(&path);
            let _ = tx.send(meta);
        }
    });
}

fn fetch_metadata(path: &str) -> MetaResult {
    let args = vec![
        "-f".to_string(),
        "%m %B %Sm".to_string(),
        "-t".to_string(),
        "%Y-%m-%d %H:%M".to_string(),
        path.to_string(),
    ];
    let output = run_command_output("stat", &args, None);
    let mut display = None;
    let mut modified_epoch = None;
    let mut created_epoch = None;

    if let Some(out) = output {
        let parts: Vec<&str> = out.split_whitespace().collect();
        if parts.len() >= 3 {
            modified_epoch = parse_epoch(parts[0]);
            created_epoch = parse_epoch(parts[1]);
            display = Some(parts[2..].join(" "));
        }
    }

    MetaResult {
        path: path.to_string(),
        display,
        modified_epoch,
        created_epoch,
    }
}

fn parse_epoch(value: &str) -> Option<i64> {
    let parsed = value.trim().parse::<i64>().ok()?;
    if parsed <= 0 {
        None
    } else {
        Some(parsed)
    }
}

fn format_date_display(value: &str) -> String {
    let mut text = value.to_string();
    if text.len() > DATE_WIDTH {
        text.truncate(DATE_WIDTH);
    } else if text.len() < DATE_WIDTH {
        text = format!("{:>width$}", text, width = DATE_WIDTH);
    }
    text
}

fn truncate_with_ellipsis(value: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let count = value.chars().count();
    if count <= max {
        return value.to_string();
    }
    if max <= 3 {
        return value.chars().take(max).collect();
    }
    let trimmed: String = value.chars().take(max - 3).collect();
    format!("{}...", trimmed)
}

fn build_tag_spans(
    tags: &[String],
    tokens: &QueryTokens,
    max_width: usize,
    elapsed_ms: u64,
    text: Color,
) -> (Vec<Span<'static>>, usize) {
    if tags.is_empty() || max_width == 0 {
        return (Vec::new(), 0);
    }

    let mut ordered = Vec::new();
    let mut matching = Vec::new();
    let mut non_matching = Vec::new();
    if tokens.tags.is_empty() {
        ordered.extend_from_slice(tags);
    } else {
        for tag in tags {
            if tokens.tags.iter().any(|token| fuzzy_match(token, tag)) {
                matching.push(tag.clone());
            } else {
                non_matching.push(tag.clone());
            }
        }
        ordered.extend_from_slice(&matching);
        ordered.extend_from_slice(&non_matching);
    }
    let has_tag_query_match = !matching.is_empty();

    let segments = build_tag_segments(&ordered, text);
    let total_len = segments_total_len(&segments);
    let display_width = max_width.max(1);
    let scroll_enabled =
        total_len > display_width && !has_tag_query_match && tokens.tags.is_empty();

    if scroll_enabled && total_len > display_width {
        let max_offset = total_len.saturating_sub(display_width);
        let offset = ((elapsed_ms / 200) as usize) % (max_offset + 1);
        return slice_tag_segments(&segments, offset, display_width);
    }

    let (spans, used) = slice_tag_segments(&segments, 0, display_width.min(total_len));
    if used < total_len {
        let more = "[...]";
        let more_len = more.chars().count();
        let extra = if spans.is_empty() { 0 } else { 1 };
        if used + extra + more_len <= display_width {
            let mut spans = spans;
            if extra > 0 {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(
                more,
                Style::default().fg(text).add_modifier(Modifier::ITALIC),
            ));
            return (spans, used + extra + more_len);
        }
    }

    (spans, used)
}

#[derive(Clone)]
struct TagSegment {
    text: String,
    style: Style,
    len: usize,
}

fn build_tag_segments(tags: &[String], fallback: Color) -> Vec<TagSegment> {
    let mut segments = Vec::new();
    for (index, tag) in tags.iter().enumerate() {
        if index > 0 {
            segments.push(TagSegment {
                text: " ".to_string(),
                style: Style::default().fg(fallback),
                len: 1,
            });
        }
        let pill = format!("[{}]", tag);
        let color = tag_color(tag, fallback);
        let style = Style::default().fg(color).add_modifier(Modifier::ITALIC);
        segments.push(TagSegment {
            text: pill.clone(),
            style,
            len: pill.chars().count(),
        });
    }
    segments
}

fn segments_total_len(segments: &[TagSegment]) -> usize {
    segments.iter().map(|seg| seg.len).sum()
}

fn slice_tag_segments(
    segments: &[TagSegment],
    offset: usize,
    width: usize,
) -> (Vec<Span<'static>>, usize) {
    let mut spans = Vec::new();
    if width == 0 {
        return (spans, 0);
    }
    let mut skipped = 0usize;
    let mut remaining = width;
    for seg in segments {
        if remaining == 0 {
            break;
        }
        if skipped + seg.len <= offset {
            skipped += seg.len;
            continue;
        }
        let start = offset.saturating_sub(skipped);
        let take = remaining.min(seg.len.saturating_sub(start));
        let slice = substring_by_char(&seg.text, start, take);
        spans.push(Span::styled(slice, seg.style));
        remaining = remaining.saturating_sub(take);
        skipped += seg.len;
    }
    (spans, width.saturating_sub(remaining))
}

fn substring_by_char(value: &str, start: usize, len: usize) -> String {
    if len == 0 {
        return String::new();
    }
    let mut result = String::new();
    for (idx, ch) in value.chars().enumerate() {
        if idx < start {
            continue;
        }
        if idx >= start + len {
            break;
        }
        result.push(ch);
    }
    result
}

fn compose_preview_text_with_input(
    base: &Text<'static>,
    tags: &[String],
    input: &Input,
    width: usize,
    text: Color,
) -> (Text<'static>, Option<(usize, usize)>) {
    let tag_lines = build_full_tag_lines(tags, width, text);
    let input_line_index = tag_lines.len();
    let scroll = input.visual_scroll(width.max(1));
    let input_slice = substring_by_char(input.value(), scroll, width.max(1));
    let input_line = Line::from(Span::styled(input_slice, Style::default().fg(text)));
    let cursor_col = input.visual_cursor().max(scroll).saturating_sub(scroll);

    let mut lines = Vec::new();
    lines.extend(tag_lines);
    lines.push(input_line);
    lines.push(Line::from(""));
    lines.extend(base.lines.clone());
    let cursor = Some((input_line_index, cursor_col));
    (Text::from(lines), cursor)
}

fn collect_tag_suggestions(tag_cache: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut set = HashSet::new();
    for tags in tag_cache.values() {
        for tag in tags {
            if tag.starts_with("org/") {
                continue;
            }
            set.insert(tag.clone());
        }
    }
    let mut list: Vec<String> = set.into_iter().collect();
    list.sort();
    list
}

fn commit_tag_input(input: &mut Input, tags: &mut Vec<String>, suggestions: &[String]) {
    let raw = input.value().trim();
    if raw.is_empty() {
        return;
    }
    let mut chosen = raw.to_string();
    let lower = raw.to_lowercase();
    if let Some(match_tag) = suggestions
        .iter()
        .find(|tag| tag.to_lowercase().starts_with(&lower))
    {
        chosen = match_tag.clone();
    }
    if !tags.iter().any(|tag| tag == &chosen) {
        tags.push(chosen);
    }
    input.reset();
}

fn save_tags_for_path(path: &str, tags: &[String]) -> AppResult<()> {
    let dir = Path::new(path);
    let config_path = dir.join(".navgator.toml");
    let contents = if config_path.exists() {
        fs::read_to_string(&config_path)?
    } else {
        String::new()
    };
    let updated = write_tags_into_toml(&contents, tags);
    fs::write(config_path, updated)?;
    Ok(())
}

fn write_tags_into_toml(contents: &str, tags: &[String]) -> String {
    let line = format!("tags = [{}]", format_tags(tags));
    if contents.trim().is_empty() {
        return format!("{}\n", line);
    }

    let mut lines: Vec<String> = contents.lines().map(|line| line.to_string()).collect();
    let mut start = None;
    let mut end = None;
    for (idx, raw) in lines.iter().enumerate() {
        let cleaned = raw.split('#').next().unwrap_or("");
        if start.is_none() {
            if let Some(eq) = cleaned.find('=') {
                let key = cleaned[..eq].trim();
                if key == "tags" {
                    start = Some(idx);
                    if cleaned.contains(']') {
                        end = Some(idx);
                        break;
                    }
                }
            }
        } else if cleaned.contains(']') {
            end = Some(idx);
            break;
        }
    }

    if start.is_none() {
        let mut out = contents.trim_end().to_string();
        out.push('\n');
        out.push_str(&line);
        out.push('\n');
        return out;
    }

    let start = start.unwrap();
    let end = end.unwrap_or(start);
    lines.splice(start..=end, [line.to_string()]);
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn format_tags(tags: &[String]) -> String {
    tags.iter()
        .map(|tag| format!("\"{}\"", tag.replace('"', "\\\"")))
        .collect::<Vec<String>>()
        .join(", ")
}

fn compose_preview_text(
    base: &Text<'static>,
    tags: &[String],
    width: usize,
    text: Color,
) -> Text<'static> {
    if tags.is_empty() {
        return base.clone();
    }

    let tag_lines = build_full_tag_lines(tags, width, text);
    if tag_lines.is_empty() {
        return base.clone();
    }

    let mut lines = Vec::new();
    lines.extend(tag_lines);
    lines.push(Line::from(""));
    lines.extend(base.lines.clone());
    Text::from(lines)
}

fn build_full_tag_lines(tags: &[String], width: usize, text: Color) -> Vec<Line<'static>> {
    if tags.is_empty() || width == 0 {
        return Vec::new();
    }
    let segments = build_tag_segments(tags, text);
    wrap_tag_segments(&segments, width)
}

fn wrap_tag_segments(segments: &[TagSegment], width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut current_len = 0usize;

    for seg in segments {
        if seg.len == 0 {
            continue;
        }
        let mut offset = 0usize;
        while offset < seg.len {
            if current_len == 0 && seg.text.chars().next() == Some(' ') {
                offset = offset.saturating_add(1);
                continue;
            }
            let available = width.saturating_sub(current_len).max(1);
            let remaining = seg.len.saturating_sub(offset);
            let take = remaining.min(available);
            let slice = substring_by_char(&seg.text, offset, take);
            current.push(Span::styled(slice, seg.style));
            current_len = current_len.saturating_add(take);
            offset = offset.saturating_add(take);

            if current_len >= width {
                lines.push(Line::from(current));
                current = Vec::new();
                current_len = 0;
            }
        }
    }

    if !current.is_empty() {
        lines.push(Line::from(current));
    }

    lines
}

fn tag_color(tag: &str, fallback: Color) -> Color {
    let mut hash = 2166136261u32;
    for byte in tag.as_bytes() {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(16777619);
    }
    let hue = (hash % 360) as f32;
    hsl_to_rgb(hue, 0.6, 0.55).unwrap_or(fallback)
}

fn hsl_to_rgb(hue: f32, sat: f32, light: f32) -> Option<Color> {
    if !(0.0..=360.0).contains(&hue) {
        return None;
    }
    let c = (1.0 - (2.0 * light - 1.0).abs()) * sat;
    let h = hue / 60.0;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r1, g1, b1) = if (0.0..1.0).contains(&h) {
        (c, x, 0.0)
    } else if (1.0..2.0).contains(&h) {
        (x, c, 0.0)
    } else if (2.0..3.0).contains(&h) {
        (0.0, c, x)
    } else if (3.0..4.0).contains(&h) {
        (0.0, x, c)
    } else if (4.0..5.0).contains(&h) {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    let m = light - c / 2.0;
    let r = ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    let g = ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    let b = ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Some(Color::Rgb(r, g, b))
}

fn build_placeholder_text(
    path: Option<&str>,
    accent: Color,
    muted: Color,
    text: Color,
    message: &str,
) -> Text<'static> {
    let value = Style::default().fg(text);
    let message_style = if message.starts_with("Loading") {
        Style::default().fg(accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(muted)
    };

    let mut lines = Vec::new();
    if let Some(path) = path {
        lines.extend(build_path_lines(path, value));
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(message.to_string(), message_style)));
    Text::from(lines)
}

fn build_preview_text(path: &str, accent: Color, muted: Color, text: Color) -> Text<'static> {
    let value = Style::default().fg(text);
    let heading = Style::default().fg(accent).add_modifier(Modifier::BOLD);
    let subtle = Style::default().fg(muted);
    let max_lines = 200usize;

    let path_buf = Path::new(path);
    let mut lines = build_path_lines(path, value);
    lines.push(Line::from(""));

    if path_buf.is_dir() {
        lines.push(Line::from(Span::styled("Contents", heading)));
        if let Some(output) = erd_output(path_buf) {
            lines.extend(lines_from_ansi_output(&output, value, max_lines));
        } else {
            lines.push(Line::from(Span::styled("erd output not available", subtle)));
        }
    } else {
        lines.push(Line::from(Span::styled("Not a directory", subtle)));
    }

    Text::from(lines)
}

fn build_git_text(path: &str, accent: Color, _muted: Color, text: Color) -> Option<Text<'static>> {
    let heading = Style::default().fg(accent).add_modifier(Modifier::BOLD);
    let value = Style::default().fg(text);
    let max_lines = 200usize;

    let path_buf = Path::new(path);
    let repo_dir = if path_buf.is_dir() {
        path_buf.to_path_buf()
    } else {
        path_buf.parent()?.to_path_buf()
    };

    let inside = run_git_command_allow_empty(&repo_dir, &["rev-parse", "--is-inside-work-tree"])?;
    if inside.trim() != "true" {
        return None;
    }

    let mut lines = Vec::new();
    if let Some(status_output) = run_git_command_allow_empty(&repo_dir, &["status", "-sb"]) {
        if let Some(first_line) = status_output.lines().next() {
            let branch = first_line.trim_start_matches("## ");
            if !branch.trim().is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("Branch: {}", branch),
                    heading,
                )));
            }
        }
    }

    if let Some(log_output) =
        run_git_command_allow_empty(&repo_dir, &["log", "-3", "--pretty=format:%s (%cr)"])
    {
        if !log_output.trim().is_empty() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled("Recent commits", heading)));
            lines.extend(lines_from_output(&log_output, value, max_lines));
        }
    } else {
        return None;
    }

    if let Some(staged_output) =
        run_git_command_allow_empty(&repo_dir, &["diff", "--stat", "--cached"])
    {
        if !staged_output.trim().is_empty() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled("Staged changes", heading)));
            lines.extend(lines_from_output(&staged_output, value, max_lines));
        }
    }

    if let Some(unstaged_output) = run_git_command_allow_empty(&repo_dir, &["diff", "--stat"]) {
        if !unstaged_output.trim().is_empty() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled("Unstaged changes", heading)));
            lines.extend(lines_from_output(&unstaged_output, value, max_lines));
        }
    }

    if let Some(untracked_output) =
        run_git_command_allow_empty(&repo_dir, &["ls-files", "--others", "--exclude-standard"])
    {
        if !untracked_output.trim().is_empty() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled("Untracked", heading)));
            lines.extend(lines_from_output(&untracked_output, value, max_lines));
        }
    }

    if lines.is_empty() {
        return None;
    }
    Some(Text::from(lines))
}

fn build_path_lines(path: &str, value: Style) -> Vec<Line<'static>> {
    vec![Line::from(Span::styled(path.to_string(), value))]
}

fn erd_output(path: &Path) -> Option<String> {
    let path_str = path.to_string_lossy().to_string();
    let (mut args, used_default) = erd_args();
    args.push(path_str.clone());
    if let Some(output) = run_command_output("erd", &args, None) {
        return Some(output);
    }

    if !used_default {
        let mut fallback = erd_default_args();
        fallback.push(path_str);
        return run_command_output("erd", &fallback, None);
    }
    None
}

fn erd_args() -> (Vec<String>, bool) {
    let mut args = Vec::new();
    let mut used_default = true;
    if let Ok(home) = home_dir() {
        let config_path = home.join(".erdtreerc");
        if let Ok(contents) = fs::read_to_string(config_path) {
            args = parse_erd_config(&contents);
            if !args.is_empty() {
                used_default = false;
            }
        }
    }

    if args.is_empty() {
        args = erd_default_args();
    }
    (args, used_default)
}

fn parse_erd_config(contents: &str) -> Vec<String> {
    let mut args = Vec::new();
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let line = trimmed.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        for token in line.split_whitespace() {
            args.push(token.to_string());
        }
    }
    args
}

fn erd_default_args() -> Vec<String> {
    vec![
        "--dir-order=first".to_string(),
        "--icons".to_string(),
        "--sort=name".to_string(),
        "--level=4".to_string(),
        "--color".to_string(),
        "force".to_string(),
        "--layout=inverted".to_string(),
        "--human".to_string(),
        "--suppress-size".to_string(),
    ]
}

fn lines_from_output(output: &str, style: Style, max_lines: usize) -> Vec<Line<'static>> {
    output
        .lines()
        .take(max_lines)
        .map(|line| Line::from(Span::styled(line.to_string(), style)))
        .collect()
}

fn lines_from_ansi_output(output: &str, style: Style, max_lines: usize) -> Vec<Line<'static>> {
    let text_result = output.as_bytes().to_vec().into_text();
    let Ok(text) = text_result else {
        return lines_from_output(output, style, max_lines);
    };
    text.lines
        .into_iter()
        .take(max_lines)
        .map(|line| line.style(style))
        .collect()
}

fn run_command_output(
    program: &str,
    args: &[String],
    current_dir: Option<&Path>,
) -> Option<String> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(dir) = current_dir {
        cmd.current_dir(dir);
    }
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string();
    if stdout.is_empty() {
        None
    } else {
        Some(stdout)
    }
}

fn run_git_command_allow_empty(repo_dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .arg("-c")
        .arg("color.ui=never")
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string(),
    )
}

fn render_side_panels(
    frame: &mut ratatui::Frame,
    area: Rect,
    preview: &Text<'static>,
    git: Option<&Text<'static>>,
    preview_title: &str,
    focus: Focus,
    accent: Color,
    text: Color,
    preview_scroll: u16,
    git_scroll: u16,
) {
    let preview_focused = matches!(focus, Focus::Preview | Focus::TagEdit);
    let git_focused = focus == Focus::Git;
    let preview_border_style = if preview_focused {
        Style::default().fg(accent)
    } else {
        Style::default().fg(text)
    };
    let git_border_style = if git_focused {
        Style::default().fg(accent)
    } else {
        Style::default().fg(text)
    };
    let preview_title = build_preview_title_line(preview_title, preview_focused, text);

    if let Some(git) = git {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);

        let preview_title = preview_title.clone();
        let preview_paragraph = Paragraph::new(preview.clone())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(preview_title)
                    .border_style(preview_border_style)
                    .border_type(BorderType::Rounded),
            )
            .style(Style::default().fg(text))
            .alignment(Alignment::Left)
            .scroll((preview_scroll, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(preview_paragraph, chunks[0]);

        let git_title = if git_focused { "* Git" } else { "Git" };
        let git_title = Span::styled(git_title, Style::default().fg(text));
        let git_paragraph = Paragraph::new(git.clone())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(git_title)
                    .border_style(git_border_style)
                    .border_type(BorderType::Rounded),
            )
            .style(Style::default().fg(text))
            .alignment(Alignment::Left)
            .scroll((git_scroll, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(git_paragraph, chunks[1]);
    } else {
        let preview_title = preview_title.clone();
        let preview_paragraph = Paragraph::new(preview.clone())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(preview_title)
                    .border_style(preview_border_style)
                    .border_type(BorderType::Rounded),
            )
            .style(Style::default().fg(text))
            .alignment(Alignment::Left)
            .scroll((preview_scroll, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(preview_paragraph, area);
    }
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stderr(), LeaveAlternateScreen, DisableMouseCapture);
    }
}

fn setup_terminal() -> AppResult<(Terminal<CrosstermBackend<io::Stderr>>, TerminalGuard)> {
    enable_raw_mode()?;
    execute!(io::stderr(), EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(io::stderr());
    let terminal = Terminal::new(backend)?;
    Ok((terminal, TerminalGuard))
}

use ansi_to_tui::IntoText;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    env,
    error::Error,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

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
}

fn main() -> AppResult<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args[0] == "navigate" {
        return run_navigate();
    }
    if args[0] == "context" {
        return run_context(&args[1..]);
    }
    if args[0] == "--help" || args[0] == "-h" {
        print_usage();
        return Ok(());
    }

    eprintln!("Unknown command.");
    print_usage();
    std::process::exit(2);
}

fn print_usage() {
    eprintln!(
        "Usage:\n  navgator [navigate]\n  navgator context <name> [--create|--no-create] [--template <template>] [--description <desc>]"
    );
}

fn run_navigate() -> AppResult<()> {
    let items = build_items()?;
    match select_from_list("Navigate", &items)? {
        Some(choice) => {
            println!("{}", choice);
            Ok(())
        }
        None => std::process::exit(1),
    }
}

fn run_context(args: &[String]) -> AppResult<()> {
    let mut name: Option<String> = None;
    let mut create: Option<bool> = None;
    let mut template: Option<String> = None;
    let mut description: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--create" => {
                create = Some(true);
                i += 1;
            }
            "--no-create" => {
                create = Some(false);
                i += 1;
            }
            "--template" => {
                let value = args.get(i + 1).ok_or("Missing value for --template")?;
                template = Some(value.clone());
                i += 2;
            }
            "--description" => {
                let value = args.get(i + 1).ok_or("Missing value for --description")?;
                description = Some(value.clone());
                i += 2;
            }
            "--help" | "-h" => {
                print_usage();
                return Ok(());
            }
            other => {
                if name.is_none() {
                    name = Some(other.to_string());
                    i += 1;
                } else {
                    return Err(format!("Unexpected argument: {}", other).into());
                }
            }
        }
    }

    let name = name.ok_or("Context name is required")?;
    let items = build_items()?;
    if let Some(found) = items.iter().find(|item| item.ends_with(&name)) {
        println!("{}", found);
        return Ok(());
    }

    let create_repo = match create {
        Some(value) => value,
        None => {
            let choices = vec!["Yes!".to_string(), "No".to_string()];
            match select_from_list("Create GH repo?", &choices)? {
                Some(choice) => choice == "Yes!",
                None => std::process::exit(1),
            }
        }
    };

    let home = home_dir()?;
    let base_dir = index_folders(&home)
        .get(0)
        .cloned()
        .ok_or("No index folders configured")?;
    fs::create_dir_all(&base_dir)?;
    let target_dir = base_dir.join(&name);

    if create_repo {
        let template = match template {
            Some(value) => value,
            None => {
                let list = templates();
                match select_from_list("Select a template", &list)? {
                    Some(choice) => choice,
                    None => std::process::exit(1),
                }
            }
        };
        let description = description.unwrap_or_else(|| name.clone());
        run_gh_create(&base_dir, &name, &template, &description)?;
    } else {
        fs::create_dir_all(&target_dir)?;
    }

    if target_dir.join(".envrc").is_file() {
        run_direnv_allow(&target_dir)?;
    }

    println!("{}", target_dir.display());
    Ok(())
}

fn build_items() -> AppResult<Vec<String>> {
    let home = home_dir()?;
    let mut items: Vec<PathBuf> = static_items(&home);
    let index_folders = index_folders(&home);

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

    Ok(items
        .into_iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect())
}

fn home_dir() -> AppResult<PathBuf> {
    let value = env::var("HOME").map_err(|_| "HOME is not set")?;
    Ok(PathBuf::from(value))
}

fn index_folders(home: &Path) -> Vec<PathBuf> {
    vec![home.join("Github"), home.join("Desktop")]
}

fn static_items(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join("Desktop"),
        PathBuf::from("/opt/homebrew"),
        home.join("Downloads"),
        home.join("Library")
            .join("Application Support")
            .join("ModrinthApp")
            .join("profiles")
            .join("Create-Prepare-to-Dye"),
        home.join("Library")
            .join("Application Support")
            .join("ModrinthApp")
            .join("profiles")
            .join("Create ptd 2"),
    ]
}

fn templates() -> Vec<String> {
    vec![
        "yarden-zamir/python-template".to_string(),
        "yarden-zamir/dotnet-template".to_string(),
        "qlik-trial/dotnet-service".to_string(),
        "Computer-Engineering-Major-Ort-Ariel/WebTemplate".to_string(),
    ]
}

fn is_dir(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_dir())
        .unwrap_or(false)
}

fn run_gh_create(base_dir: &Path, name: &str, template: &str, description: &str) -> AppResult<()> {
    let output = Command::new("gh")
        .args([
            "repo",
            "create",
            name,
            "--clone",
            "--description",
            description,
            "--disable-wiki",
            "--public",
            "--template",
            template,
        ])
        .current_dir(base_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let mut stderr = io::stderr();
    stderr.write_all(&output.stdout)?;
    stderr.write_all(&output.stderr)?;
    stderr.flush()?;

    if !output.status.success() {
        return Err("gh repo create failed".into());
    }
    Ok(())
}

fn run_direnv_allow(target_dir: &Path) -> AppResult<()> {
    let output = Command::new("direnv")
        .arg("allow")
        .current_dir(target_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let mut stderr = io::stderr();
    stderr.write_all(&output.stdout)?;
    stderr.write_all(&output.stderr)?;
    stderr.flush()?;

    if !output.status.success() {
        return Err("direnv allow failed".into());
    }
    Ok(())
}

fn select_from_list(_title: &str, items: &[String]) -> AppResult<Option<String>> {
    if items.is_empty() {
        return Ok(None);
    }

    let (mut terminal, _guard) = setup_terminal()?;
    let mut query = String::new();
    let mut cursor = 0usize;
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
    let mut filtered = filter_and_sort(items, &query, sort_mode, &meta_cache, &tag_cache);
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

    loop {
        let current = current_selection_path(items, &filtered, selected);
        let tokens = parse_query_tokens(&query);

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
            filtered = filter_and_sort(items, &query, sort_mode, &meta_cache, &tag_cache);
            selected = match selected_path {
                Some(path) => index_for_path(items, &filtered, &path).unwrap_or(0),
                None => adjust_selected_index(selected, filtered.len()),
            };
        }

        if tags_changed && query_uses_tags {
            let selected_path = current_selection_path(items, &filtered, selected);
            filtered = filter_and_sort(items, &query, sort_mode, &meta_cache, &tag_cache);
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

            let query_label = if focus == Focus::Search {
                build_query_line(&query, cursor, text, accent)
            } else if query.is_empty() {
                Line::from(Span::raw(""))
            } else {
                Line::from(Span::styled(query.as_str(), Style::default().fg(text)))
            };
            let search = Paragraph::new(Text::from(query_label))
                .alignment(Alignment::Left)
                .wrap(Wrap { trim: false });
            frame.render_widget(search, search_area);

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
            let preview_tags = current
                .as_deref()
                .and_then(|path| tag_cache.get(path))
                .cloned()
                .unwrap_or_default();
            let preview_width = ui.preview_area.width.saturating_sub(2) as usize;
            let preview_combined =
                compose_preview_text(&preview_text, &preview_tags, preview_width, text);
            preview_max_scroll = text_line_count(&preview_combined).saturating_sub(preview_height);
            git_max_scroll = match git_text.as_ref() {
                Some(git) => text_line_count(git).saturating_sub(git_height),
                None => 0,
            };
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

            let help_line = build_help_line(
                focus,
                sort_mode,
                show_git,
                cursor >= query_len(&query),
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
                        .border_style(Style::default().fg(accent))
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
                    if key.code == KeyCode::Enter {
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
                        filtered =
                            filter_and_sort(items, &query, sort_mode, &meta_cache, &tag_cache);
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
                        if parse_query_tokens(&query).needs_tags() && !tag_scan_started {
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
                            KeyCode::Left => {
                                if key.modifiers.contains(KeyModifiers::SUPER) {
                                    cursor = 0;
                                } else if key
                                    .modifiers
                                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                                {
                                    move_cursor_word_left(&query, &mut cursor);
                                } else {
                                    move_cursor_left(&mut cursor);
                                }
                            }
                            KeyCode::Right => {
                                if key.modifiers.contains(KeyModifiers::SUPER) {
                                    cursor = query_len(&query);
                                } else if key
                                    .modifiers
                                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                                {
                                    move_cursor_word_right(&query, &mut cursor);
                                } else if cursor >= query_len(&query) {
                                    focus = Focus::Preview;
                                } else {
                                    move_cursor_right(&query, &mut cursor);
                                }
                            }
                            KeyCode::Home => {
                                cursor = 0;
                            }
                            KeyCode::End => {
                                cursor = query_len(&query);
                            }
                            KeyCode::Backspace => {
                                if key
                                    .modifiers
                                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                                {
                                    delete_word_before_cursor(&mut query, &mut cursor);
                                } else {
                                    delete_before_cursor(&mut query, &mut cursor);
                                }
                                filtered = filter_and_sort(
                                    items,
                                    &query,
                                    sort_mode,
                                    &meta_cache,
                                    &tag_cache,
                                );
                                selected = 0;
                                list_offset = 0;
                                cursor = cursor.min(query_len(&query));
                            }
                            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                query.clear();
                                cursor = 0;
                                filtered = filter_and_sort(
                                    items,
                                    &query,
                                    sort_mode,
                                    &meta_cache,
                                    &tag_cache,
                                );
                                selected = 0;
                                list_offset = 0;
                            }
                            KeyCode::Char(ch) => {
                                if !key.modifiers.intersects(
                                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                                ) {
                                    insert_at_cursor(&mut query, &mut cursor, ch);
                                    filtered = filter_and_sort(
                                        items,
                                        &query,
                                        sort_mode,
                                        &meta_cache,
                                        &tag_cache,
                                    );
                                    selected = 0;
                                    list_offset = 0;
                                }
                            }
                            _ => {}
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
    let mut scored: Vec<(usize, (usize, usize, usize, usize))> = Vec::new();
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
        if !fuzzy_match(token, path) {
            return false;
        }
    }

    for token in &tokens.tags {
        if !tags.iter().any(|tag| fuzzy_match(token, tag)) {
            return false;
        }
    }

    for token in &tokens.any {
        let path_match = fuzzy_match(token, path);
        let tag_match = tags.iter().any(|tag| fuzzy_match(token, tag));
        if !(path_match || tag_match) {
            return false;
        }
    }

    true
}

fn match_score_tokens(
    tokens: &QueryTokens,
    path: &str,
    tags: &[String],
) -> Option<(usize, usize, usize, usize)> {
    let mut span_sum = 0usize;
    let mut gap_sum = 0usize;
    let mut start_sum = 0usize;
    let mut len_sum = 0usize;

    for token in &tokens.folder {
        let score = match_score(token, path)?;
        span_sum = span_sum.saturating_add(score.0);
        gap_sum = gap_sum.saturating_add(score.1);
        start_sum = start_sum.saturating_add(score.2);
        len_sum = len_sum.saturating_add(score.3);
    }

    for token in &tokens.tags {
        let score = best_tag_score(token, tags)?;
        span_sum = span_sum.saturating_add(score.0);
        gap_sum = gap_sum.saturating_add(score.1);
        start_sum = start_sum.saturating_add(score.2);
        len_sum = len_sum.saturating_add(score.3);
    }

    for token in &tokens.any {
        let mut best = match_score(token, path);
        if let Some(tag_score) = best_tag_score(token, tags) {
            best = match best {
                Some(path_score) => Some(path_score.min(tag_score)),
                None => Some(tag_score),
            };
        }
        let score = best?;
        span_sum = span_sum.saturating_add(score.0);
        gap_sum = gap_sum.saturating_add(score.1);
        start_sum = start_sum.saturating_add(score.2);
        len_sum = len_sum.saturating_add(score.3);
    }

    Some((span_sum, gap_sum, start_sum, len_sum))
}

fn best_tag_score(token: &str, tags: &[String]) -> Option<(usize, usize, usize, usize)> {
    let mut best: Option<(usize, usize, usize, usize)> = None;
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

fn query_len(query: &str) -> usize {
    query.chars().count()
}

fn byte_index_from_char(query: &str, char_index: usize) -> usize {
    if char_index >= query_len(query) {
        return query.len();
    }
    query
        .char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(query.len())
}

fn insert_at_cursor(query: &mut String, cursor: &mut usize, ch: char) {
    let index = byte_index_from_char(query, *cursor);
    query.insert(index, ch);
    *cursor += 1;
}

fn delete_before_cursor(query: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    let end = byte_index_from_char(query, *cursor);
    let start = byte_index_from_char(query, *cursor - 1);
    query.replace_range(start..end, "");
    *cursor = cursor.saturating_sub(1);
}

fn delete_word_before_cursor(query: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    let chars: Vec<char> = query.chars().collect();
    let mut index = *cursor;
    while index > 0 && chars[index - 1].is_whitespace() {
        index -= 1;
    }
    while index > 0 && is_word_char(chars[index - 1]) {
        index -= 1;
    }
    let start = byte_index_from_char(query, index);
    let end = byte_index_from_char(query, *cursor);
    query.replace_range(start..end, "");
    *cursor = index;
}

fn move_cursor_left(cursor: &mut usize) {
    if *cursor > 0 {
        *cursor -= 1;
    }
}

fn move_cursor_right(query: &str, cursor: &mut usize) {
    let len = query_len(query);
    if *cursor < len {
        *cursor += 1;
    }
}

fn move_cursor_word_left(query: &str, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    let chars: Vec<char> = query.chars().collect();
    let mut index = *cursor;
    while index > 0 && chars[index - 1].is_whitespace() {
        index -= 1;
    }
    while index > 0 && is_word_char(chars[index - 1]) {
        index -= 1;
    }
    *cursor = index;
}

fn move_cursor_word_right(query: &str, cursor: &mut usize) {
    let chars: Vec<char> = query.chars().collect();
    let len = chars.len();
    let mut index = *cursor;
    while index < len && chars[index].is_whitespace() {
        index += 1;
    }
    while index < len && is_word_char(chars[index]) {
        index += 1;
    }
    *cursor = index;
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '-'
}

fn build_query_line(query: &str, cursor: usize, text: Color, accent: Color) -> Line<'static> {
    let mut spans = Vec::new();
    let cursor_style = Style::default().fg(accent).add_modifier(Modifier::BOLD);
    let mut index = 0usize;
    for ch in query.chars() {
        if index == cursor {
            spans.push(Span::styled("|", cursor_style));
        }
        spans.push(Span::styled(ch.to_string(), Style::default().fg(text)));
        index += 1;
    }
    if index == cursor {
        spans.push(Span::styled("|", cursor_style));
    }
    if spans.is_empty() {
        spans.push(Span::styled("|", cursor_style));
    }
    Line::from(spans)
}

fn build_help_line(
    focus: Focus,
    sort_mode: SortMode,
    show_git: bool,
    cursor_at_end: bool,
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
            if git_scroll == 0 {
                spans.push(Span::styled("Up", key_style));
                spans.push(Span::styled(" preview", regular_style));
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

fn match_score(query: &str, text: &str) -> Option<(usize, usize, usize, usize)> {
    let qchars: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    if qchars.is_empty() {
        return Some((0, 0, 0, text.chars().count()));
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
    Some((span, gaps, start, text_len))
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
    let preview_focused = focus == Focus::Preview;
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

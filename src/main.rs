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
    layout::Alignment,
    style::{Color, Modifier, Style},
    text::Text,
    widgets::{Block, BorderType, Borders, List, ListState, Paragraph, Wrap},
    Terminal,
};
use std::{
    collections::{HashMap, HashSet},
    env, fs, io,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tui_input::backend::crossterm::EventHandler;
use tui_input::{Input, InputRequest};

mod commands;
mod config;
mod content;
mod git;
mod github;
mod metadata;
mod model;
mod results;
mod search;
mod tags;
mod ui;

use config::config_schema_json;
use content::{
    apply_git_result, apply_github_readme_result, apply_preview_data, build_placeholder_text,
    build_preview_data, ensure_git_for_preview, preview_tab_visible_indexes, ApplyPreviewData,
};
use github::ensure_github_readme_for_preview;
use metadata::{ensure_dates_for_paths, spawn_bulk_metadata_fetch};
use model::{
    AppResult, DetailTab, Focus, GitResult, GithubReadmeResult, HelpColors, HelpContext,
    MetaResult, PreviewColors, PreviewData, PreviewResult, PreviewSettings, SidePanelRender,
    SortMeta, SortMode, TagResult, VisibleListArgs, DATE_PLACEHOLDER,
};
use results::build_items;
use search::{entry_name, filter_and_sort, index_for_path, parse_query_tokens};
use tags::{
    collect_tag_suggestions, commit_tag_input, ensure_tags_for_paths, read_tags_for_path,
    save_tags_for_path, spawn_bulk_tag_fetch,
};
use ui::{
    build_help_line, build_visible_list_items, compose_preview_text,
    compose_preview_text_with_input, compute_ui_layout, input_at_end, preview_content_area,
    rect_contains, render_side_panels, text_line_count,
};

fn main() -> AppResult<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args[0] == "navigate" {
        ensure_tty_stdin()?;
        return run_navigate();
    }
    if args[0] == "config-schema" || args[0] == "schema" {
        return print_config_schema();
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
    eprintln!("Usage:\n  navgator [navigate|config-schema]");
}

fn print_config_schema() -> AppResult<()> {
    let json = config_schema_json()?;
    println!("{json}");
    Ok(())
}

fn run_navigate() -> AppResult<()> {
    let result = build_items()?;
    match select_from_list("Navigate", &result.items, result.preview_settings)? {
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

fn select_from_list(
    _title: &str,
    items: &[String],
    preview_settings: PreviewSettings,
) -> AppResult<Option<String>> {
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
    let (git_tx, git_rx) = mpsc::channel::<GitResult>();
    let (github_tx, github_rx) = mpsc::channel::<GithubReadmeResult>();
    let (date_tx, date_rx) = mpsc::channel::<MetaResult>();
    let (tag_tx, tag_rx) = mpsc::channel::<TagResult>();
    let mut preview_cache: HashMap<String, PreviewData> = HashMap::new();
    let mut git_in_flight: HashSet<String> = HashSet::new();
    let mut github_in_flight: HashSet<String> = HashSet::new();
    let mut date_cache: HashMap<String, String> = HashMap::new();
    let mut date_in_flight: HashSet<String> = HashSet::new();
    let mut tag_cache: HashMap<String, Vec<String>> = HashMap::new();
    let mut tag_in_flight: HashSet<String> = HashSet::new();
    let mut tag_scan_started = false;
    let mut filtered = filter_and_sort(items, input.value(), sort_mode, &meta_cache, &tag_cache);
    let mut preview_path: Option<String> = None;
    let mut in_flight: Option<String> = None;
    let mut preview_text = build_placeholder_text(None, accent, muted, text, "No selection");
    let mut preview_tab_index = 0usize;
    let mut preview_tab_visible_index = 0usize;
    let mut preview_tab_count = 1usize;
    let mut preview_tab_labels: Vec<String> = Vec::new();
    let mut worktree_filter = Input::default();
    let mut detail_tabs: Vec<DetailTab> = Vec::new();
    let mut detail_tab_index = 0usize;
    let mut preview_scroll = 0usize;
    let mut detail_scroll = 0usize;
    let mut preview_max_scroll = 0usize;
    let mut detail_max_scroll = 0usize;
    let mut preview_page_step = 5usize;
    let mut detail_page_step = 5usize;
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
            if let Some(data) = preview_cache.get(&result.path) {
                ensure_git_for_preview(
                    &result.path,
                    data,
                    &mut git_in_flight,
                    &git_tx,
                    preview_tab_index,
                    PreviewColors {
                        accent,
                        muted,
                        text,
                    },
                );
                ensure_github_readme_for_preview(
                    &result.path,
                    data,
                    &mut github_in_flight,
                    &github_tx,
                    preview_tab_index,
                    PreviewColors {
                        accent,
                        muted,
                        text,
                    },
                );
            }
            if current.as_deref() == Some(result.path.as_str()) {
                apply_preview_data(
                    &result.data,
                    ApplyPreviewData {
                        tab_index: &mut preview_tab_index,
                        tab_visible_index: &mut preview_tab_visible_index,
                        tab_count: &mut preview_tab_count,
                        tab_labels: &mut preview_tab_labels,
                        preview_text: &mut preview_text,
                        detail_tabs: &mut detail_tabs,
                        detail_tab_index: &mut detail_tab_index,
                        worktree_filter: worktree_filter.value(),
                    },
                );
                preview_path = Some(result.path.clone());
            }
            if in_flight.as_deref() == Some(result.path.as_str()) {
                in_flight = None;
            }
        }

        while let Ok(result) = git_rx.try_recv() {
            if result.done {
                git_in_flight.remove(&result.path);
            }
            let mut updated = false;
            if let Some(data) = preview_cache.get_mut(&result.path) {
                apply_git_result(data, result.tab_index, result.git, result.done);
                updated = true;
            }
            if updated && current.as_deref() == Some(result.path.as_str()) {
                if let Some(data) = preview_cache.get(&result.path) {
                    apply_preview_data(
                        data,
                        ApplyPreviewData {
                            tab_index: &mut preview_tab_index,
                            tab_visible_index: &mut preview_tab_visible_index,
                            tab_count: &mut preview_tab_count,
                            tab_labels: &mut preview_tab_labels,
                            preview_text: &mut preview_text,
                            detail_tabs: &mut detail_tabs,
                            detail_tab_index: &mut detail_tab_index,
                            worktree_filter: worktree_filter.value(),
                        },
                    );
                }
            }
        }

        while let Ok(result) = github_rx.try_recv() {
            if result.done {
                github_in_flight.remove(&result.path);
            }
            let mut updated = false;
            if let Some(data) = preview_cache.get_mut(&result.path) {
                apply_github_readme_result(data, result.tab_index, result.readme, result.done);
                updated = true;
            }
            if updated && current.as_deref() == Some(result.path.as_str()) {
                if let Some(data) = preview_cache.get(&result.path) {
                    apply_preview_data(
                        data,
                        ApplyPreviewData {
                            tab_index: &mut preview_tab_index,
                            tab_visible_index: &mut preview_tab_visible_index,
                            tab_count: &mut preview_tab_count,
                            tab_labels: &mut preview_tab_labels,
                            preview_text: &mut preview_text,
                            detail_tabs: &mut detail_tabs,
                            detail_tab_index: &mut detail_tab_index,
                            worktree_filter: worktree_filter.value(),
                        },
                    );
                }
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
                    detail_tabs.clear();
                    preview_path = None;
                    in_flight = None;
                    preview_tab_index = 0;
                    preview_tab_visible_index = 0;
                    detail_tab_index = 0;
                    preview_tab_count = 1;
                    preview_tab_labels.clear();
                    worktree_filter.reset();
                    preview_scroll = 0;
                    detail_scroll = 0;
                }
            }
            Some(path) => {
                if preview_path.as_deref() != Some(path) {
                    preview_tab_index = 0;
                    preview_tab_visible_index = 0;
                    detail_tab_index = 0;
                    preview_tab_count = 1;
                    preview_tab_labels.clear();
                    worktree_filter.reset();
                    preview_scroll = 0;
                    detail_scroll = 0;
                    if let Some(data) = preview_cache.get(path) {
                        apply_preview_data(
                            data,
                            ApplyPreviewData {
                                tab_index: &mut preview_tab_index,
                                tab_visible_index: &mut preview_tab_visible_index,
                                tab_count: &mut preview_tab_count,
                                tab_labels: &mut preview_tab_labels,
                                preview_text: &mut preview_text,
                                detail_tabs: &mut detail_tabs,
                                detail_tab_index: &mut detail_tab_index,
                                worktree_filter: worktree_filter.value(),
                            },
                        );
                        preview_path = Some(path.to_string());
                        ensure_git_for_preview(
                            path,
                            data,
                            &mut git_in_flight,
                            &git_tx,
                            preview_tab_index,
                            PreviewColors {
                                accent,
                                muted,
                                text,
                            },
                        );
                        ensure_github_readme_for_preview(
                            path,
                            data,
                            &mut github_in_flight,
                            &github_tx,
                            preview_tab_index,
                            PreviewColors {
                                accent,
                                muted,
                                text,
                            },
                        );
                    } else if in_flight.as_deref() != Some(path) {
                        preview_text = build_placeholder_text(
                            Some(path),
                            accent,
                            muted,
                            text,
                            "Loading preview...",
                        );
                        detail_tabs.clear();
                        detail_tab_index = 0;
                        preview_tab_labels.clear();
                        preview_path = Some(path.to_string());
                        in_flight = Some(path.to_string());
                        let tx = preview_tx.clone();
                        let path_owned = path.to_string();
                        thread::spawn(move || {
                            let data = build_preview_data(
                                &path_owned,
                                accent,
                                muted,
                                text,
                                preview_settings,
                            );
                            let _ = tx.send(PreviewResult {
                                path: path_owned,
                                data,
                            });
                        });
                    }
                }
            }
        }

        if focus == Focus::Detail && detail_tabs.is_empty() {
            focus = Focus::Preview;
        }
        if focus == Focus::TagEdit && tag_edit_path.is_none() {
            focus = Focus::Preview;
        }

        let show_detail = !detail_tabs.is_empty();
        let size = terminal.size()?;
        let ui = compute_ui_layout(size.into(), show_detail);

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

            let (list_items, list_selected) = build_visible_list_items(VisibleListArgs {
                items,
                filtered: &filtered,
                selected,
                offset: list_offset,
                height: list_inner_height,
                text,
                muted,
                dates: &date_cache,
                tags: &tag_cache,
                inner_width: list_inner_width,
                tokens: &tokens,
                elapsed_ms: start_time.elapsed().as_millis() as u64,
            });

            let list = List::new(list_items).highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(warm)
                    .add_modifier(Modifier::BOLD),
            );

            let mut state = ListState::default();
            state.select(list_selected);
            frame.render_stateful_widget(list, results_area, &mut state);

            let preview_body_area = preview_content_area(ui.preview_area, preview_tab_count);
            let preview_height = preview_body_area.height as usize;
            let detail_height = ui
                .detail_panel_area
                .map(|rect| {
                    let tab_row = if detail_tabs.len() > 1 { 1 } else { 0 };
                    rect.height.saturating_sub(2).saturating_sub(tab_row) as usize
                })
                .unwrap_or(0);
            preview_page_step = preview_height.max(1);
            detail_page_step = detail_height.max(1);
            let preview_title =
                build_preview_panel_title(current.as_deref(), worktree_filter.value());
            let preview_tags = if focus == Focus::TagEdit {
                tag_edit_tags.clone()
            } else {
                current
                    .as_deref()
                    .and_then(|path| tag_cache.get(path))
                    .cloned()
                    .unwrap_or_default()
            };
            let preview_width = preview_body_area.width as usize;
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
            let active_detail = detail_tabs.get(detail_tab_index);
            detail_max_scroll = active_detail
                .map(|tab| text_line_count(&tab.text).saturating_sub(detail_height))
                .unwrap_or(0);
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
            detail_scroll = detail_scroll.min(detail_max_scroll);
            render_side_panels(
                frame,
                SidePanelRender {
                    area: detail_area,
                    preview: &preview_combined,
                    detail_tabs: &detail_tabs,
                    detail_tab_index,
                    preview_title: &preview_title,
                    preview_tab_labels: &preview_tab_labels,
                    preview_tab_index: preview_tab_visible_index,
                    preview_settings,
                    focus,
                    accent,
                    text,
                    preview_scroll: preview_scroll as u16,
                    detail_scroll: detail_scroll as u16,
                },
            );
            if focus == Focus::TagEdit {
                if let Some((row, col)) = tag_cursor {
                    let visible_row = row.saturating_sub(preview_scroll);
                    if visible_row < preview_height {
                        let x = preview_body_area.x + col as u16;
                        let y = preview_body_area.y + visible_row as u16;
                        frame.set_cursor_position((x, y));
                    }
                }
            }

            let help_line = build_help_line(
                HelpContext {
                    focus,
                    sort_mode,
                    show_detail,
                    cursor_at_end: input_at_end(&input),
                    has_tag_input: !tag_input.value().trim().is_empty(),
                    preview_tab_index: preview_tab_visible_index,
                    preview_tab_count,
                    preview_scroll,
                    preview_max_scroll,
                    detail_tab_index,
                    detail_tab_count: detail_tabs.len(),
                    detail_scroll,
                },
                HelpColors {
                    text,
                    accent,
                    key_color,
                },
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
                        let value = enter_selection_path(
                            focus,
                            current.as_deref(),
                            preview_tab_index,
                            &preview_cache,
                        )
                        .or_else(|| filtered.get(selected).map(|index| items[*index].clone()));
                        if let Some(value) = value {
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
                                selected = selected.saturating_sub(1);
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
                            KeyCode::Backspace if tag_input.value().is_empty() => {
                                tag_edit_tags.pop();
                            }
                            KeyCode::Backspace => {
                                let _ = tag_input.handle_event(&Event::Key(key));
                            }
                            _ => {
                                let _ = tag_input.handle_event(&Event::Key(key));
                            }
                        },
                        Focus::Preview => match key.code {
                            KeyCode::Char('u')
                                if key.modifiers.contains(KeyModifiers::CONTROL)
                                    && !worktree_filter.value().is_empty() =>
                            {
                                worktree_filter.reset();
                                preview_tab_visible_index = 0;
                                preview_scroll = 0;
                                detail_tab_index = 0;
                                detail_scroll = 0;
                                if let Some(data) =
                                    current.as_deref().and_then(|path| preview_cache.get(path))
                                {
                                    apply_preview_data(
                                        data,
                                        ApplyPreviewData {
                                            tab_index: &mut preview_tab_index,
                                            tab_visible_index: &mut preview_tab_visible_index,
                                            tab_count: &mut preview_tab_count,
                                            tab_labels: &mut preview_tab_labels,
                                            preview_text: &mut preview_text,
                                            detail_tabs: &mut detail_tabs,
                                            detail_tab_index: &mut detail_tab_index,
                                            worktree_filter: worktree_filter.value(),
                                        },
                                    );
                                }
                            }
                            KeyCode::Char(_)
                                if !key.modifiers.intersects(
                                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                                ) =>
                            {
                                let before = worktree_filter.value().to_string();
                                let _ = worktree_filter.handle_event(&Event::Key(key));
                                if worktree_filter.value() != before {
                                    preview_tab_visible_index = 0;
                                    preview_scroll = 0;
                                    detail_tab_index = 0;
                                    detail_scroll = 0;
                                    if let Some(data) =
                                        current.as_deref().and_then(|path| preview_cache.get(path))
                                    {
                                        apply_preview_data(
                                            data,
                                            ApplyPreviewData {
                                                tab_index: &mut preview_tab_index,
                                                tab_visible_index: &mut preview_tab_visible_index,
                                                tab_count: &mut preview_tab_count,
                                                tab_labels: &mut preview_tab_labels,
                                                preview_text: &mut preview_text,
                                                detail_tabs: &mut detail_tabs,
                                                detail_tab_index: &mut detail_tab_index,
                                                worktree_filter: worktree_filter.value(),
                                            },
                                        );
                                    }
                                }
                            }
                            KeyCode::Backspace if !worktree_filter.value().is_empty() => {
                                let before = worktree_filter.value().to_string();
                                let _ = worktree_filter.handle_event(&Event::Key(key));
                                if worktree_filter.value() != before {
                                    preview_tab_visible_index = 0;
                                    preview_scroll = 0;
                                    detail_tab_index = 0;
                                    detail_scroll = 0;
                                    if let Some(data) =
                                        current.as_deref().and_then(|path| preview_cache.get(path))
                                    {
                                        apply_preview_data(
                                            data,
                                            ApplyPreviewData {
                                                tab_index: &mut preview_tab_index,
                                                tab_visible_index: &mut preview_tab_visible_index,
                                                tab_count: &mut preview_tab_count,
                                                tab_labels: &mut preview_tab_labels,
                                                preview_text: &mut preview_text,
                                                detail_tabs: &mut detail_tabs,
                                                detail_tab_index: &mut detail_tab_index,
                                                worktree_filter: worktree_filter.value(),
                                            },
                                        );
                                    }
                                }
                            }
                            KeyCode::Left => {
                                if preview_tab_visible_index > 0 {
                                    preview_tab_visible_index -= 1;
                                    preview_scroll = 0;
                                    detail_tab_index = 0;
                                    detail_scroll = 0;
                                    if let Some(data) =
                                        current.as_deref().and_then(|path| preview_cache.get(path))
                                    {
                                        let visible_indexes = preview_tab_visible_indexes(
                                            data,
                                            worktree_filter.value(),
                                        );
                                        if let Some(index) =
                                            visible_indexes.get(preview_tab_visible_index)
                                        {
                                            preview_tab_index = *index;
                                        }
                                        apply_preview_data(
                                            data,
                                            ApplyPreviewData {
                                                tab_index: &mut preview_tab_index,
                                                tab_visible_index: &mut preview_tab_visible_index,
                                                tab_count: &mut preview_tab_count,
                                                tab_labels: &mut preview_tab_labels,
                                                preview_text: &mut preview_text,
                                                detail_tabs: &mut detail_tabs,
                                                detail_tab_index: &mut detail_tab_index,
                                                worktree_filter: worktree_filter.value(),
                                            },
                                        );
                                    }
                                } else {
                                    focus = Focus::Search;
                                }
                            }
                            KeyCode::Right => {
                                if preview_tab_visible_index + 1 < preview_tab_count {
                                    preview_tab_visible_index += 1;
                                    preview_scroll = 0;
                                    detail_tab_index = 0;
                                    detail_scroll = 0;
                                    if let Some(data) =
                                        current.as_deref().and_then(|path| preview_cache.get(path))
                                    {
                                        let visible_indexes = preview_tab_visible_indexes(
                                            data,
                                            worktree_filter.value(),
                                        );
                                        if let Some(index) =
                                            visible_indexes.get(preview_tab_visible_index)
                                        {
                                            preview_tab_index = *index;
                                        }
                                        apply_preview_data(
                                            data,
                                            ApplyPreviewData {
                                                tab_index: &mut preview_tab_index,
                                                tab_visible_index: &mut preview_tab_visible_index,
                                                tab_count: &mut preview_tab_count,
                                                tab_labels: &mut preview_tab_labels,
                                                preview_text: &mut preview_text,
                                                detail_tabs: &mut detail_tabs,
                                                detail_tab_index: &mut detail_tab_index,
                                                worktree_filter: worktree_filter.value(),
                                            },
                                        );
                                    }
                                } else if !detail_tabs.is_empty() {
                                    focus = Focus::Detail;
                                }
                            }
                            KeyCode::Up => {
                                if preview_scroll > 0 {
                                    preview_scroll -= 1;
                                } else if preview_tab_visible_index > 0 {
                                    preview_tab_visible_index -= 1;
                                    detail_tab_index = 0;
                                    detail_scroll = 0;
                                    if let Some(data) =
                                        current.as_deref().and_then(|path| preview_cache.get(path))
                                    {
                                        let visible_indexes = preview_tab_visible_indexes(
                                            data,
                                            worktree_filter.value(),
                                        );
                                        if let Some(index) =
                                            visible_indexes.get(preview_tab_visible_index)
                                        {
                                            preview_tab_index = *index;
                                        }
                                        apply_preview_data(
                                            data,
                                            ApplyPreviewData {
                                                tab_index: &mut preview_tab_index,
                                                tab_visible_index: &mut preview_tab_visible_index,
                                                tab_count: &mut preview_tab_count,
                                                tab_labels: &mut preview_tab_labels,
                                                preview_text: &mut preview_text,
                                                detail_tabs: &mut detail_tabs,
                                                detail_tab_index: &mut detail_tab_index,
                                                worktree_filter: worktree_filter.value(),
                                            },
                                        );
                                    }
                                } else if preview_scroll == 0 {
                                    focus = Focus::Search;
                                }
                            }
                            KeyCode::Down => {
                                if preview_scroll < preview_max_scroll {
                                    preview_scroll += 1;
                                } else if preview_tab_visible_index + 1 < preview_tab_count {
                                    preview_tab_visible_index += 1;
                                    preview_scroll = 0;
                                    detail_tab_index = 0;
                                    detail_scroll = 0;
                                    if let Some(data) =
                                        current.as_deref().and_then(|path| preview_cache.get(path))
                                    {
                                        let visible_indexes = preview_tab_visible_indexes(
                                            data,
                                            worktree_filter.value(),
                                        );
                                        if let Some(index) =
                                            visible_indexes.get(preview_tab_visible_index)
                                        {
                                            preview_tab_index = *index;
                                        }
                                        apply_preview_data(
                                            data,
                                            ApplyPreviewData {
                                                tab_index: &mut preview_tab_index,
                                                tab_visible_index: &mut preview_tab_visible_index,
                                                tab_count: &mut preview_tab_count,
                                                tab_labels: &mut preview_tab_labels,
                                                preview_text: &mut preview_text,
                                                detail_tabs: &mut detail_tabs,
                                                detail_tab_index: &mut detail_tab_index,
                                                worktree_filter: worktree_filter.value(),
                                            },
                                        );
                                    }
                                } else if preview_scroll >= preview_max_scroll
                                    && !detail_tabs.is_empty()
                                {
                                    focus = Focus::Detail;
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
                        Focus::Detail => match key.code {
                            KeyCode::Left => {
                                if detail_tab_index > 0 {
                                    detail_tab_index -= 1;
                                    detail_scroll = 0;
                                } else {
                                    preview_tab_visible_index = preview_tab_count.saturating_sub(1);
                                    preview_scroll = 0;
                                    if let Some(data) =
                                        current.as_deref().and_then(|path| preview_cache.get(path))
                                    {
                                        let visible_indexes = preview_tab_visible_indexes(
                                            data,
                                            worktree_filter.value(),
                                        );
                                        if let Some(index) =
                                            visible_indexes.get(preview_tab_visible_index)
                                        {
                                            preview_tab_index = *index;
                                        }
                                        apply_preview_data(
                                            data,
                                            ApplyPreviewData {
                                                tab_index: &mut preview_tab_index,
                                                tab_visible_index: &mut preview_tab_visible_index,
                                                tab_count: &mut preview_tab_count,
                                                tab_labels: &mut preview_tab_labels,
                                                preview_text: &mut preview_text,
                                                detail_tabs: &mut detail_tabs,
                                                detail_tab_index: &mut detail_tab_index,
                                                worktree_filter: worktree_filter.value(),
                                            },
                                        );
                                    }
                                    focus = Focus::Preview;
                                }
                            }
                            KeyCode::Right => {
                                if detail_tab_index + 1 < detail_tabs.len() {
                                    detail_tab_index += 1;
                                    detail_scroll = 0;
                                } else {
                                    focus = Focus::Preview;
                                }
                            }
                            KeyCode::Up => {
                                if detail_scroll > 0 {
                                    detail_scroll -= 1;
                                } else if detail_scroll == 0 {
                                    focus = Focus::Preview;
                                }
                            }
                            KeyCode::Down if detail_scroll < detail_max_scroll => {
                                detail_scroll += 1;
                            }
                            KeyCode::PageUp => {
                                detail_scroll = detail_scroll.saturating_sub(detail_page_step);
                            }
                            KeyCode::PageDown => {
                                detail_scroll =
                                    (detail_scroll + detail_page_step).min(detail_max_scroll);
                            }
                            KeyCode::Home => {
                                detail_scroll = 0;
                            }
                            KeyCode::End => {
                                detail_scroll = detail_max_scroll;
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
                            } else if let Some(detail_panel_area) = ui.detail_panel_area {
                                if rect_contains(detail_panel_area, col, row) {
                                    focus = Focus::Detail;
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
                            } else if let Some(detail_panel_area) = ui.detail_panel_area {
                                if rect_contains(detail_panel_area, col, row) {
                                    detail_scroll = detail_scroll.saturating_sub(1);
                                }
                            } else if rect_contains(ui.results_area, col, row) {
                                selected = selected.saturating_sub(1);
                            }
                        }
                        MouseEventKind::ScrollDown => {
                            if rect_contains(ui.preview_area, col, row) {
                                preview_scroll = (preview_scroll + 1).min(preview_max_scroll);
                            } else if let Some(detail_panel_area) = ui.detail_panel_area {
                                if rect_contains(detail_panel_area, col, row) {
                                    detail_scroll = (detail_scroll + 1).min(detail_max_scroll);
                                }
                            } else if rect_contains(ui.results_area, col, row) {
                                selected = selected
                                    .saturating_add(1)
                                    .min(filtered.len().saturating_sub(1));
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

fn current_selection_path(items: &[String], filtered: &[usize], selected: usize) -> Option<String> {
    filtered
        .get(selected)
        .and_then(|index| items.get(*index))
        .cloned()
}

fn enter_selection_path(
    focus: Focus,
    current_path: Option<&str>,
    preview_tab_index: usize,
    preview_cache: &HashMap<String, PreviewData>,
) -> Option<String> {
    if !matches!(focus, Focus::Preview | Focus::Detail) {
        return None;
    }
    let current_path = current_path?;
    preview_cache
        .get(current_path)
        .and_then(|data| data.previews.get(preview_tab_index))
        .map(|tab| tab.path.clone())
}

fn build_preview_panel_title(path: Option<&str>, worktree_filter: &str) -> String {
    let title = path
        .map(entry_name)
        .unwrap_or_else(|| "Preview".to_string());
    let filter = worktree_filter.trim();
    if filter.is_empty() {
        title
    } else {
        format!("{} / {}", title, filter)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::default_preview_settings;
    use ratatui::text::Line;

    #[test]
    fn parses_git_worktree_porcelain_with_bare_and_branch_entries() {
        let output = "worktree /repos/example.git\nbare\n\nworktree /repos/example\nHEAD 123456\nbranch refs/heads/main\n\nworktree /repos/example-feature\nHEAD abcdef\nbranch refs/heads/feature/worktree\n";

        let worktrees = git::parse_git_worktree_list(output);

        assert_eq!(worktrees.len(), 3);
        assert_eq!(worktrees[0].path, "/repos/example.git");
        assert!(worktrees[0].bare);
        assert_eq!(worktrees[1].path, "/repos/example");
        assert_eq!(worktrees[1].branch.as_deref(), Some("main"));
        assert_eq!(worktrees[2].branch.as_deref(), Some("feature/worktree"));
    }

    #[test]
    fn labels_detached_worktree_when_no_branch_is_reported() {
        let output = "worktree /repos/example-detached\nHEAD abcdef\ndetached\n";

        let worktrees = git::parse_git_worktree_list(output);

        assert_eq!(worktrees.len(), 1);
        assert!(worktrees[0].detached);
        assert_eq!(git::git_worktree_label(&worktrees[0], true), "detached");
    }

    #[test]
    fn shortens_worktree_tab_label_after_last_slash() {
        assert_eq!(
            git::worktree_tab_label("feat/yarden/potato", true),
            "potato"
        );
        assert_eq!(
            git::worktree_tab_label("feat/yarden/potato", false),
            "feat/yarden/potato"
        );
    }

    #[test]
    fn pseudo_scrolls_tabs_from_previous_label() {
        let labels = vec![
            "main".to_string(),
            "feature-1".to_string(),
            "feature-2".to_string(),
            "feature-3".to_string(),
            "feature-4".to_string(),
        ];

        let settings = default_preview_settings();
        let (first_visible, first_selected) = ui::visible_tab_window(&labels, 0, 35, settings);
        let (middle_visible, middle_selected) = ui::visible_tab_window(&labels, 2, 40, settings);

        assert_eq!(first_selected, 0);
        assert_eq!(
            first_visible,
            vec!["main", "feature-1", "feature-2", "f..."]
        );
        assert_eq!(middle_selected, 1);
        assert_eq!(
            middle_visible,
            vec!["feature-1", "feature-2", "feature-3", "f..."]
        );
    }

    #[test]
    fn truncates_long_tab_labels_with_ellipsis() {
        assert_eq!(
            ui::truncate_tab_label("very-long-feature", 10),
            "very-lo..."
        );
        assert_eq!(ui::truncate_tab_label("abc", 10), "abc");
        assert_eq!(ui::truncate_tab_label("abcdef", 3), "...");
    }

    #[test]
    fn keeps_more_selected_tab_chars_before_ellipsis() {
        let labels = vec![
            "previous-worktree".to_string(),
            "selected-worktree".to_string(),
            "next-worktree".to_string(),
        ];
        let settings = PreviewSettings {
            shorten_worktree_tab_labels: true,
            worktree_tab_min_chars: 6,
            selected_worktree_tab_min_chars: 10,
        };

        let (visible, selected) = ui::visible_tab_window(&labels, 1, 25, settings);

        assert_eq!(selected, 1);
        assert_eq!(visible, vec!["previo...", "selected-w..."]);
    }

    #[test]
    fn enter_from_preview_returns_active_worktree_path() {
        let mut cache = HashMap::new();
        cache.insert(
            "/repos/project.git".to_string(),
            PreviewData {
                previews: vec![
                    model::PreviewTab {
                        path: "/repos/project".to_string(),
                        label: "main".to_string(),
                        text: Text::default(),
                        git: None,
                        github_readme: None,
                    },
                    model::PreviewTab {
                        path: "/repos/project-feature".to_string(),
                        label: "feature".to_string(),
                        text: Text::default(),
                        git: None,
                        github_readme: None,
                    },
                ],
                selected_repo_is_bare: false,
                git_loaded: false,
                github_readme_loaded: false,
            },
        );

        let value = enter_selection_path(Focus::Preview, Some("/repos/project.git"), 1, &cache);

        assert_eq!(value.as_deref(), Some("/repos/project-feature"));
        assert!(
            enter_selection_path(Focus::Search, Some("/repos/project.git"), 1, &cache).is_none()
        );
    }

    #[test]
    fn applies_git_result_to_one_preview_tab() {
        let mut data = PreviewData {
            previews: vec![
                model::PreviewTab {
                    path: "/repos/project".to_string(),
                    label: "main".to_string(),
                    text: Text::default(),
                    git: None,
                    github_readme: None,
                },
                model::PreviewTab {
                    path: "/repos/project-feature".to_string(),
                    label: "feature".to_string(),
                    text: Text::default(),
                    git: None,
                    github_readme: None,
                },
            ],
            selected_repo_is_bare: true,
            git_loaded: false,
            github_readme_loaded: false,
        };

        apply_git_result(
            &mut data,
            1,
            Some(Text::from(Line::from("Branch: feature"))),
            false,
        );

        assert!(data.previews[0].git.is_none());
        assert!(data.previews[1].git.is_some());
        assert!(!data.git_loaded);
    }

    #[test]
    fn orders_github_detail_before_git_detail() {
        let tab = model::PreviewTab {
            path: "/repos/project".to_string(),
            label: "main".to_string(),
            text: Text::default(),
            git: Some(Text::from(Line::from("Branch: main"))),
            github_readme: Some(Text::from(Line::from("# Project"))),
        };

        let detail_tabs = content::detail_tabs_for_preview(&tab);

        assert_eq!(detail_tabs.len(), 2);
        assert_eq!(detail_tabs[0].label, "GitHub");
        assert_eq!(detail_tabs[1].label, "Git");
    }

    #[test]
    fn filters_preview_worktree_tabs_by_label_or_path() {
        let data = PreviewData {
            previews: vec![
                model::PreviewTab {
                    path: "/repos/project".to_string(),
                    label: "main".to_string(),
                    text: Text::default(),
                    git: None,
                    github_readme: None,
                },
                model::PreviewTab {
                    path: "/repos/project-feature".to_string(),
                    label: "feature".to_string(),
                    text: Text::default(),
                    git: None,
                    github_readme: None,
                },
                model::PreviewTab {
                    path: "/repos/project-hotfix".to_string(),
                    label: "hotfix".to_string(),
                    text: Text::default(),
                    git: None,
                    github_readme: None,
                },
            ],
            selected_repo_is_bare: false,
            git_loaded: false,
            github_readme_loaded: false,
        };

        assert_eq!(content::preview_tab_visible_indexes(&data, "feat"), vec![1]);
        assert_eq!(content::preview_tab_visible_indexes(&data, "hot"), vec![2]);
        assert_eq!(
            content::preview_tab_visible_indexes(&data, "missing"),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn sorts_default_branch_worktree_first() {
        let trunk = model::GitWorktree {
            path: "/repos/project-trunk".to_string(),
            branch: Some("trunk".to_string()),
            detached: false,
            bare: false,
        };
        let feature = model::GitWorktree {
            path: "/repos/project-feature".to_string(),
            branch: Some("feature".to_string()),
            detached: false,
            bare: false,
        };
        let mut worktrees = vec![&feature, &trunk];

        content::sort_worktrees_default_first(&mut worktrees, Some("trunk"));

        assert_eq!(worktrees[0].branch.as_deref(), Some("trunk"));
        assert_eq!(worktrees[1].branch.as_deref(), Some("feature"));
    }

    #[test]
    fn displays_home_paths_with_tilde() {
        assert_eq!(
            content::display_path_with_home("/Users/kcw", "/Users/kcw"),
            "~"
        );
        assert_eq!(
            content::display_path_with_home("/Users/kcw/Github/navgator", "/Users/kcw"),
            "~/Github/navgator"
        );
        assert_eq!(
            content::display_path_with_home("/Users/kcw-other/Github", "/Users/kcw"),
            "/Users/kcw-other/Github"
        );
    }

    #[test]
    fn resolves_dot_bare_worktree_container() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let root = env::temp_dir().join(format!("navgator-dot-bare-test-{unique}"));
        let dot_bare = root.join(".bare");
        fs::create_dir_all(&root).expect("test root should be created");

        let status = std::process::Command::new("git")
            .arg("init")
            .arg("--bare")
            .arg(&dot_bare)
            .status()
            .expect("git should be available");
        assert!(status.success());

        let resolved = git::git_command_dir_for_path(&root);
        let _ = fs::remove_dir_all(&root);

        assert_eq!(resolved.as_deref(), Some(dot_bare.as_path()));
    }
}

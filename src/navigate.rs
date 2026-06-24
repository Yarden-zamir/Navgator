use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
        MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::Alignment,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};
use std::{
    collections::{HashMap, HashSet},
    io,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use tui_input::backend::crossterm::EventHandler;
use tui_input::{Input, InputRequest};

use crate::preview::{
    build_git_text, build_placeholder_text, build_preview_text, build_tag_spans,
    compose_preview_text, compose_preview_text_with_input, ensure_dates_for_paths,
    format_date_display, spawn_bulk_metadata_fetch, truncate_with_ellipsis, MetaResult,
    DATE_PLACEHOLDER,
};
use crate::search::{
    entry_name, filter_and_sort, index_for_path, parse_query_tokens, QueryTokens, SortMeta,
    SortMode,
};
use crate::tags::{
    collect_tag_suggestions, commit_tag_input, read_tags_for_path, save_tags_for_path,
};
use crate::ui::{
    self, build_help_line, compute_ui_layout, input_at_end, rect_contains, render_side_panels,
    text_line_count, Focus,
};
use crate::AppResult;

#[derive(Clone)]
struct PreviewData {
    preview: Text<'static>,
    git: Option<Text<'static>>,
}

struct PreviewResult {
    path: String,
    data: PreviewData,
}

struct TagResult {
    path: String,
    tags: Vec<String>,
}

struct NavigatorState {
    input: Input,
    selected: usize,
    sort_mode: SortMode,
    focus: Focus,
    meta_cache: HashMap<String, SortMeta>,
    list_offset: usize,
    preview_cache: HashMap<String, PreviewData>,
    date_cache: HashMap<String, String>,
    date_in_flight: HashSet<String>,
    tag_cache: HashMap<String, Vec<String>>,
    tag_in_flight: HashSet<String>,
    tag_scan_started: bool,
    filtered: Vec<usize>,
    preview_path: Option<String>,
    preview_in_flight: Option<String>,
    preview_text: Text<'static>,
    git_text: Option<Text<'static>>,
    preview_scroll: usize,
    git_scroll: usize,
    preview_max_scroll: usize,
    git_max_scroll: usize,
    preview_page_step: usize,
    git_page_step: usize,
    start_time: Instant,
    tag_edit_path: Option<String>,
    tag_edit_tags: Vec<String>,
    tag_input: Input,
    tag_suggestions: Vec<String>,
    accent: Color,
    warm: Color,
    key_color: Color,
    text: Color,
    muted: Color,
}

enum LoopAction {
    Continue,
    Quit,
    Select(String),
}

impl NavigatorState {
    fn new(items: &[String]) -> Self {
        let accent = Color::Rgb(72, 166, 255);
        let warm = Color::Rgb(255, 181, 92);
        let key_color = Color::Rgb(150, 150, 150);
        let text = Color::Black;
        let muted = text;
        let input = Input::default();
        let meta_cache = HashMap::new();
        let tag_cache = HashMap::new();
        let filtered = filter_and_sort(
            items,
            input.value(),
            SortMode::Match,
            &meta_cache,
            &tag_cache,
        );

        Self {
            input,
            selected: 0,
            sort_mode: SortMode::Match,
            focus: Focus::Search,
            meta_cache,
            list_offset: 0,
            preview_cache: HashMap::new(),
            date_cache: HashMap::new(),
            date_in_flight: HashSet::new(),
            tag_cache,
            tag_in_flight: HashSet::new(),
            tag_scan_started: false,
            filtered,
            preview_path: None,
            preview_in_flight: None,
            preview_text: build_placeholder_text(None, accent, muted, text, "No selection"),
            git_text: None,
            preview_scroll: 0,
            git_scroll: 0,
            preview_max_scroll: 0,
            git_max_scroll: 0,
            preview_page_step: 5,
            git_page_step: 5,
            start_time: Instant::now(),
            tag_edit_path: None,
            tag_edit_tags: Vec::new(),
            tag_input: Input::default(),
            tag_suggestions: Vec::new(),
            accent,
            warm,
            key_color,
            text,
            muted,
        }
    }

    fn current_path(&self, items: &[String]) -> Option<String> {
        current_selection_path(items, &self.filtered, self.selected)
    }

    fn refilter(&mut self, items: &[String]) {
        self.filtered = filter_and_sort(
            items,
            self.input.value(),
            self.sort_mode,
            &self.meta_cache,
            &self.tag_cache,
        );
    }

    fn refilter_preserving_selection(&mut self, items: &[String], selected_path: Option<String>) {
        self.refilter(items);
        self.selected = match selected_path {
            Some(path) => index_for_path(items, &self.filtered, &path).unwrap_or(0),
            None => adjust_selected_index(self.selected, self.filtered.len()),
        };
    }
}

pub(crate) fn select_from_list(_title: &str, items: &[String]) -> AppResult<Option<String>> {
    if items.is_empty() {
        return Ok(None);
    }

    let (mut terminal, _guard) = setup_terminal()?;
    let mut state = NavigatorState::new(items);
    let (preview_tx, preview_rx) = mpsc::channel::<PreviewResult>();
    let (date_tx, date_rx) = mpsc::channel::<MetaResult>();
    let (tag_tx, tag_rx) = mpsc::channel::<TagResult>();

    loop {
        process_worker_results(&mut state, items, &preview_rx, &date_rx, &tag_rx, &tag_tx);
        sync_preview_for_current(&mut state, items, &preview_tx);

        let ui = draw_frame(&mut terminal, &mut state, items, &date_tx, &tag_tx)?;

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    match handle_key_event(&mut state, key, items, &date_tx, &tag_tx)? {
                        LoopAction::Continue => {}
                        LoopAction::Quit => {
                            terminal.show_cursor()?;
                            return Ok(None);
                        }
                        LoopAction::Select(value) => {
                            terminal.show_cursor()?;
                            return Ok(Some(value));
                        }
                    }
                }
                Event::Mouse(mouse) => {
                    handle_mouse_event(&mut state, mouse, ui);
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
}

fn process_worker_results(
    state: &mut NavigatorState,
    items: &[String],
    preview_rx: &mpsc::Receiver<PreviewResult>,
    date_rx: &mpsc::Receiver<MetaResult>,
    tag_rx: &mpsc::Receiver<TagResult>,
    tag_tx: &mpsc::Sender<TagResult>,
) {
    let current = state.current_path(items);

    while let Ok(result) = preview_rx.try_recv() {
        state
            .preview_cache
            .insert(result.path.clone(), result.data.clone());
        if current.as_deref() == Some(result.path.as_str()) {
            state.preview_text = result.data.preview;
            state.git_text = result.data.git;
            state.preview_path = Some(result.path.clone());
        }
        if state.preview_in_flight.as_deref() == Some(result.path.as_str()) {
            state.preview_in_flight = None;
        }
    }

    let mut resort_needed = false;
    while let Ok(result) = date_rx.try_recv() {
        let display = result
            .display
            .unwrap_or_else(|| DATE_PLACEHOLDER.to_string());
        state.date_cache.insert(result.path.clone(), display);
        state.meta_cache.insert(
            result.path.clone(),
            SortMeta {
                modified_epoch: result.modified_epoch,
                created_epoch: result.created_epoch,
            },
        );
        state.date_in_flight.remove(&result.path);
        if state.sort_mode.uses_time() {
            resort_needed = true;
        }
    }

    let mut tags_changed = false;
    while let Ok(result) = tag_rx.try_recv() {
        state.tag_cache.insert(result.path.clone(), result.tags);
        state.tag_in_flight.remove(&result.path);
        tags_changed = true;
    }

    let query_uses_tags = parse_query_tokens(state.input.value()).needs_tags();
    if query_uses_tags && !state.tag_scan_started {
        spawn_bulk_tag_fetch(items, &state.tag_cache, &mut state.tag_in_flight, tag_tx);
        state.tag_scan_started = true;
    }

    if resort_needed {
        state.refilter_preserving_selection(items, current.clone());
    }
    if tags_changed && query_uses_tags {
        state.refilter_preserving_selection(items, current);
    }
}

fn sync_preview_for_current(
    state: &mut NavigatorState,
    items: &[String],
    preview_tx: &mpsc::Sender<PreviewResult>,
) {
    let current = state.current_path(items);
    match current.as_deref() {
        None => {
            if state.preview_path.is_some() || state.preview_in_flight.is_some() {
                state.preview_text = build_placeholder_text(
                    None,
                    state.accent,
                    state.muted,
                    state.text,
                    "No selection",
                );
                state.git_text = None;
                state.preview_path = None;
                state.preview_in_flight = None;
                state.preview_scroll = 0;
                state.git_scroll = 0;
            }
        }
        Some(path) => {
            if state.preview_path.as_deref() != Some(path) {
                state.preview_scroll = 0;
                state.git_scroll = 0;
                if let Some(data) = state.preview_cache.get(path) {
                    state.preview_text = data.preview.clone();
                    state.git_text = data.git.clone();
                    state.preview_path = Some(path.to_string());
                } else if state.preview_in_flight.as_deref() != Some(path) {
                    state.preview_text = build_placeholder_text(
                        Some(path),
                        state.accent,
                        state.muted,
                        state.text,
                        "Loading preview...",
                    );
                    state.git_text = Some(build_placeholder_text(
                        Some(path),
                        state.accent,
                        state.muted,
                        state.text,
                        "Loading git info...",
                    ));
                    state.preview_path = Some(path.to_string());
                    state.preview_in_flight = Some(path.to_string());
                    let tx = preview_tx.clone();
                    let path_owned = path.to_string();
                    let accent = state.accent;
                    let muted = state.muted;
                    let text = state.text;
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

    if state.focus == Focus::Git && state.git_text.is_none() {
        state.focus = Focus::Preview;
    }
    if state.focus == Focus::TagEdit && state.tag_edit_path.is_none() {
        state.focus = Focus::Preview;
    }
}

fn draw_frame(
    terminal: &mut Terminal<CrosstermBackend<io::Stderr>>,
    state: &mut NavigatorState,
    items: &[String],
    date_tx: &mpsc::Sender<MetaResult>,
    tag_tx: &mpsc::Sender<TagResult>,
) -> AppResult<ui::UiLayout> {
    let current = state.current_path(items);
    let tokens = parse_query_tokens(state.input.value());
    let show_git = state.git_text.is_some();
    let size = terminal.size()?;
    let ui = compute_ui_layout(size.into(), show_git);

    let list_inner_height = ui.results_area.height as usize;
    let total = state.filtered.len();
    state.list_offset =
        compute_list_window_offset(state.selected, state.list_offset, list_inner_height, total);

    let scrollbar_space = if total > 0 { 1 } else { 0 };
    let list_inner_width = ui.results_area.width.saturating_sub(scrollbar_space) as usize;
    let visible_paths =
        visible_paths_for_window(items, &state.filtered, state.list_offset, list_inner_height);
    ensure_dates_for_paths(
        &visible_paths,
        &state.date_cache,
        &mut state.date_in_flight,
        date_tx,
    );
    ensure_tags_for_paths(
        &visible_paths,
        &state.tag_cache,
        &mut state.tag_in_flight,
        tag_tx,
    );

    let (list_items, list_selected) = build_visible_list_items(
        items,
        &state.filtered,
        state.selected,
        state.list_offset,
        list_inner_height,
        state.text,
        state.muted,
        &state.date_cache,
        &state.tag_cache,
        list_inner_width,
        &tokens,
        state.start_time.elapsed().as_millis() as u64,
    );

    let list = List::new(list_items).highlight_style(
        Style::default()
            .fg(Color::Black)
            .bg(state.warm)
            .add_modifier(Modifier::BOLD),
    );
    let mut list_state = ListState::default();
    list_state.select(list_selected);

    let preview_height = ui.preview_area.height.saturating_sub(2) as usize;
    let git_height = ui
        .git_area
        .map(|rect| rect.height.saturating_sub(2) as usize)
        .unwrap_or(0);
    state.preview_page_step = preview_height.max(1);
    state.git_page_step = git_height.max(1);

    let preview_title = current
        .as_deref()
        .map(entry_name)
        .unwrap_or_else(|| "Preview".to_string());
    let preview_tags = if state.focus == Focus::TagEdit {
        state.tag_edit_tags.clone()
    } else {
        current
            .as_deref()
            .and_then(|path| state.tag_cache.get(path))
            .cloned()
            .unwrap_or_default()
    };
    let preview_width = ui.preview_area.width.saturating_sub(2) as usize;
    let (preview_combined, tag_cursor) = if state.focus == Focus::TagEdit {
        compose_preview_text_with_input(
            &state.preview_text,
            &preview_tags,
            &state.tag_input,
            preview_width,
            state.text,
        )
    } else {
        (
            compose_preview_text(
                &state.preview_text,
                &preview_tags,
                preview_width,
                state.text,
            ),
            None,
        )
    };

    state.preview_max_scroll = text_line_count(&preview_combined).saturating_sub(preview_height);
    state.git_max_scroll = match state.git_text.as_ref() {
        Some(git) => text_line_count(git).saturating_sub(git_height),
        None => 0,
    };
    if state.focus == Focus::TagEdit {
        if let Some((row, _)) = tag_cursor {
            if row < state.preview_scroll {
                state.preview_scroll = row;
            } else if row >= state.preview_scroll + preview_height {
                state.preview_scroll = row.saturating_sub(preview_height.saturating_sub(1));
            }
        }
    }
    state.preview_scroll = state.preview_scroll.min(state.preview_max_scroll);
    state.git_scroll = state.git_scroll.min(state.git_max_scroll);

    let list_title = format!("Results {}/{}", state.filtered.len(), items.len());
    let left_title = if state.focus == Focus::Search {
        format!("* {}", list_title)
    } else {
        list_title
    };
    let left_border_style = if state.focus == Focus::Search {
        Style::default().fg(state.accent)
    } else {
        Style::default().fg(state.muted)
    };
    let search_width = ui.search_area.width.saturating_sub(1) as usize;
    let search_scroll = if search_width > 0 {
        state.input.visual_scroll(search_width)
    } else {
        0
    };
    let search_value = state.input.value().to_string();
    let search_cursor = state
        .input
        .visual_cursor()
        .max(search_scroll)
        .saturating_sub(search_scroll);
    let help_line = build_help_line(
        state.focus,
        state.sort_mode,
        show_git,
        input_at_end(&state.input),
        !state.tag_input.value().trim().is_empty(),
        state.preview_scroll,
        state.preview_max_scroll,
        state.git_scroll,
        state.text,
        state.accent,
        state.key_color,
    );
    let focus = state.focus;
    let git_text = state.git_text.clone();
    let preview_scroll = state.preview_scroll;
    let git_scroll = state.git_scroll;
    let muted = state.muted;
    let text = state.text;
    let accent = state.accent;

    terminal.draw(|frame| {
        let left_block = Block::default()
            .borders(Borders::ALL)
            .title(left_title)
            .border_style(left_border_style)
            .border_type(BorderType::Rounded);
        frame.render_widget(left_block, ui.list_area);

        let search = Paragraph::new(search_value.as_str())
            .scroll((0, search_scroll as u16))
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false });
        frame.render_widget(search, ui.search_area);
        if focus == Focus::Search && ui.search_area.width > 0 && ui.search_area.height > 0 {
            frame.set_cursor_position((ui.search_area.x + search_cursor as u16, ui.search_area.y));
        }

        frame.render_stateful_widget(list, ui.results_area, &mut list_state);

        render_side_panels(
            frame,
            ui.detail_area,
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

    Ok(ui)
}

fn handle_key_event(
    state: &mut NavigatorState,
    key: KeyEvent,
    items: &[String],
    date_tx: &mpsc::Sender<MetaResult>,
    tag_tx: &mpsc::Sender<TagResult>,
) -> AppResult<LoopAction> {
    if key.code == KeyCode::Esc
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
    {
        return Ok(LoopAction::Quit);
    }

    if key.code == KeyCode::Char('t')
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && state.focus != Focus::TagEdit
    {
        if let Some(path) = state.current_path(items) {
            state.tag_edit_path = Some(path.clone());
            state.tag_edit_tags = read_tags_for_path(&path);
            state.tag_input.reset();
            state.tag_suggestions = collect_tag_suggestions(&state.tag_cache);
            state.focus = Focus::TagEdit;
            state.preview_scroll = 0;
        }
        return Ok(LoopAction::Continue);
    }

    if key.code == KeyCode::Enter && state.focus != Focus::TagEdit {
        if let Some(index) = state.filtered.get(state.selected) {
            return Ok(LoopAction::Select(items[*index].clone()));
        }
    }

    if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
        state.sort_mode = state.sort_mode.next();
        state.refilter(items);
        state.selected = 0;
        state.list_offset = 0;
        if state.sort_mode.uses_time() {
            spawn_bulk_metadata_fetch(items, &state.date_cache, &mut state.date_in_flight, date_tx);
        }
        if parse_query_tokens(state.input.value()).needs_tags() && !state.tag_scan_started {
            spawn_bulk_tag_fetch(items, &state.tag_cache, &mut state.tag_in_flight, tag_tx);
            state.tag_scan_started = true;
        }
        return Ok(LoopAction::Continue);
    }

    match state.focus {
        Focus::Search => match key.code {
            KeyCode::Up => {
                state.selected = state.selected.saturating_sub(1);
            }
            KeyCode::Down => {
                if state.selected + 1 < state.filtered.len() {
                    state.selected += 1;
                }
            }
            KeyCode::Right
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) && input_at_end(&state.input) =>
            {
                state.focus = Focus::Preview;
            }
            _ => {
                let before = state.input.value().to_string();
                if key.modifiers.contains(KeyModifiers::SUPER) {
                    if key.code == KeyCode::Left {
                        state.input.handle(InputRequest::GoToStart);
                    } else if key.code == KeyCode::Right {
                        state.input.handle(InputRequest::GoToEnd);
                    }
                } else if key.code == KeyCode::Char('u')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    state.input.handle(InputRequest::DeleteLine);
                } else {
                    let _ = state.input.handle_event(&Event::Key(key));
                }
                if state.input.value() != before {
                    state.refilter(items);
                    state.selected = 0;
                    state.list_offset = 0;
                }
            }
        },
        Focus::TagEdit => match key.code {
            KeyCode::Enter => {
                commit_tag_input(
                    &mut state.tag_input,
                    &mut state.tag_edit_tags,
                    &state.tag_suggestions,
                );
                if let Some(path) = state.tag_edit_path.clone() {
                    save_tags_for_path(&path, &state.tag_edit_tags)?;
                    state.tag_cache.insert(path, state.tag_edit_tags.clone());
                }
                state.focus = Focus::Preview;
                state.tag_edit_path = None;
                state.tag_edit_tags.clear();
                state.tag_input.reset();
                let selected_path = state.current_path(items);
                state.refilter_preserving_selection(items, selected_path);
            }
            KeyCode::Tab => {
                commit_tag_input(
                    &mut state.tag_input,
                    &mut state.tag_edit_tags,
                    &state.tag_suggestions,
                );
            }
            KeyCode::Backspace => {
                if state.tag_input.value().is_empty() {
                    state.tag_edit_tags.pop();
                } else {
                    let _ = state.tag_input.handle_event(&Event::Key(key));
                }
            }
            _ => {
                let _ = state.tag_input.handle_event(&Event::Key(key));
            }
        },
        Focus::Preview => match key.code {
            KeyCode::Left => {
                state.focus = Focus::Search;
            }
            KeyCode::Right => {
                if state.git_text.is_some() {
                    state.focus = Focus::Git;
                }
            }
            KeyCode::Up => {
                if state.preview_scroll > 0 {
                    state.preview_scroll -= 1;
                } else {
                    state.focus = Focus::Search;
                }
            }
            KeyCode::Down => {
                if state.preview_scroll < state.preview_max_scroll {
                    state.preview_scroll += 1;
                } else if state.git_text.is_some() {
                    state.focus = Focus::Git;
                }
            }
            KeyCode::PageUp => {
                state.preview_scroll = state.preview_scroll.saturating_sub(state.preview_page_step);
            }
            KeyCode::PageDown => {
                state.preview_scroll =
                    (state.preview_scroll + state.preview_page_step).min(state.preview_max_scroll);
            }
            KeyCode::Home => {
                state.preview_scroll = 0;
            }
            KeyCode::End => {
                state.preview_scroll = state.preview_max_scroll;
            }
            _ => {}
        },
        Focus::Git => match key.code {
            KeyCode::Left => {
                state.focus = Focus::Search;
            }
            KeyCode::Right => {
                state.focus = Focus::Preview;
            }
            KeyCode::Up => {
                if state.git_scroll > 0 {
                    state.git_scroll -= 1;
                } else {
                    state.focus = Focus::Preview;
                }
            }
            KeyCode::Down => {
                if state.git_scroll < state.git_max_scroll {
                    state.git_scroll += 1;
                }
            }
            KeyCode::PageUp => {
                state.git_scroll = state.git_scroll.saturating_sub(state.git_page_step);
            }
            KeyCode::PageDown => {
                state.git_scroll =
                    (state.git_scroll + state.git_page_step).min(state.git_max_scroll);
            }
            KeyCode::Home => {
                state.git_scroll = 0;
            }
            KeyCode::End => {
                state.git_scroll = state.git_max_scroll;
            }
            _ => {}
        },
    }

    Ok(LoopAction::Continue)
}

fn handle_mouse_event(state: &mut NavigatorState, mouse: MouseEvent, ui: ui::UiLayout) {
    let col = mouse.column;
    let row = mouse.row;

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if rect_contains(ui.list_area, col, row) {
                state.focus = Focus::Search;
            } else if let Some(git_area) = ui.git_area {
                if rect_contains(git_area, col, row) {
                    state.focus = Focus::Git;
                } else if rect_contains(ui.preview_area, col, row) {
                    state.focus = Focus::Preview;
                }
            } else if rect_contains(ui.preview_area, col, row) {
                state.focus = Focus::Preview;
            }
        }
        MouseEventKind::ScrollUp => {
            if rect_contains(ui.preview_area, col, row) {
                state.preview_scroll = state.preview_scroll.saturating_sub(1);
            } else if let Some(git_area) = ui.git_area {
                if rect_contains(git_area, col, row) {
                    state.git_scroll = state.git_scroll.saturating_sub(1);
                }
            } else if rect_contains(ui.results_area, col, row) {
                state.selected = state.selected.saturating_sub(1);
            }
        }
        MouseEventKind::ScrollDown => {
            if rect_contains(ui.preview_area, col, row) {
                state.preview_scroll = (state.preview_scroll + 1).min(state.preview_max_scroll);
            } else if let Some(git_area) = ui.git_area {
                if rect_contains(git_area, col, row) {
                    state.git_scroll = (state.git_scroll + 1).min(state.git_max_scroll);
                }
            } else if rect_contains(ui.results_area, col, row)
                && state.selected + 1 < state.filtered.len()
            {
                state.selected += 1;
            }
        }
        _ => {}
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

    for item_index in visible {
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

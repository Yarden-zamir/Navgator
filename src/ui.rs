use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
};
use tui_input::Input;

use crate::search::SortMode;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Search,
    Preview,
    Git,
    TagEdit,
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

pub(crate) fn compute_ui_layout(size: Rect, show_git: bool) -> UiLayout {
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

pub(crate) fn rect_contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x
        && col < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

pub(crate) fn text_line_count(text: &Text) -> usize {
    text.lines.len()
}

pub(crate) fn input_at_end(input: &Input) -> bool {
    input.cursor() >= input.value().chars().count()
}

pub(crate) fn build_help_line(
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

pub(crate) fn render_side_panels(
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

fn build_preview_title_line(title: &str, focused: bool, text: Color) -> Line<'static> {
    let label = if focused {
        format!("* {}", title)
    } else {
        title.to_string()
    };
    Line::from(Span::styled(label, Style::default().fg(text)))
}

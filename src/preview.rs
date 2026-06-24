use ansi_to_tui::IntoText;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
};
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};
use tui_input::Input;

use crate::commands::{run_command_output, run_git_command_allow_empty};
use crate::search::{fuzzy_match, QueryTokens};

const DATE_WIDTH: usize = 16;
pub(crate) const DATE_PLACEHOLDER: &str = "---- -- -- --:--";

pub(crate) struct MetaResult {
    pub(crate) path: String,
    pub(crate) display: Option<String>,
    pub(crate) modified_epoch: Option<i64>,
    pub(crate) created_epoch: Option<i64>,
}

pub(crate) fn ensure_dates_for_paths(
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

pub(crate) fn spawn_bulk_metadata_fetch(
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

pub(crate) fn format_date_display(value: &str) -> String {
    let mut text = value.to_string();
    if text.len() > DATE_WIDTH {
        text.truncate(DATE_WIDTH);
    } else if text.len() < DATE_WIDTH {
        text = format!("{:>width$}", text, width = DATE_WIDTH);
    }
    text
}

pub(crate) fn truncate_with_ellipsis(value: &str, max: usize) -> String {
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

pub(crate) fn build_tag_spans(
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

pub(crate) fn compose_preview_text_with_input(
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

pub(crate) fn compose_preview_text(
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

pub(crate) fn build_placeholder_text(
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

pub(crate) fn build_preview_text(
    path: &str,
    accent: Color,
    muted: Color,
    text: Color,
) -> Text<'static> {
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

pub(crate) fn build_git_text(
    path: &str,
    accent: Color,
    _muted: Color,
    text: Color,
) -> Option<Text<'static>> {
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
            if current_len == 0 && seg.text.starts_with(' ') {
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
    if let Ok(home) = env::var("HOME") {
        let config_path = PathBuf::from(home).join(".erdtreerc");
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

use crate::model::{MatchScore, SortMeta, SortMode};
use std::{cmp::Ordering, collections::HashMap, path::Path};

#[derive(Default)]
pub(crate) struct QueryTokens {
    pub(crate) folder: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) any: Vec<String>,
}

impl QueryTokens {
    pub(crate) fn is_empty(&self) -> bool {
        self.folder.is_empty() && self.tags.is_empty() && self.any.is_empty()
    }

    pub(crate) fn needs_tags(&self) -> bool {
        !self.tags.is_empty() || !self.any.is_empty()
    }
}

pub(crate) fn parse_query_tokens(query: &str) -> QueryTokens {
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

pub(crate) fn filter_and_sort(
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

pub(crate) fn index_for_path(items: &[String], filtered: &[usize], path: &str) -> Option<usize> {
    filtered.iter().position(|index| {
        items
            .get(*index)
            .map(|candidate| candidate == path)
            .unwrap_or(false)
    })
}

pub(crate) fn entry_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|part| part.to_str())
        .unwrap_or(path)
        .to_string()
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
        .filter_map(|(index, path)| {
            let tags = tag_cache.get(path).map(Vec::as_slice).unwrap_or(&[]);
            if matches_tokens(path, tags, &tokens) {
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
    let mut scored: Vec<(usize, MatchScore)> = Vec::new();
    for (index, path) in items.iter().enumerate() {
        let tags = tag_cache.get(path).map(Vec::as_slice).unwrap_or(&[]);
        if !matches_tokens(path, tags, &tokens) {
            continue;
        }
        if let Some(score) = match_score_tokens(&tokens, path, tags) {
            scored.push((index, score));
        }
    }
    scored.sort_by(|(left_idx, left), (right_idx, right)| {
        left.cmp(right).then_with(|| {
            compare_names(&items[*left_idx], &items[*right_idx])
                .then_with(|| left_idx.cmp(right_idx))
        })
    });
    scored.into_iter().map(|(index, _)| index).collect()
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
        if !path_match && !tag_match {
            return false;
        }
    }
    true
}

fn matches_path_token(token: &str, path: &str) -> bool {
    let entry = entry_name(path);
    fuzzy_match(token, &entry) || fuzzy_match(token, path)
}

fn match_score_tokens(tokens: &QueryTokens, path: &str, tags: &[String]) -> Option<MatchScore> {
    let mut penalty_sum = 0usize;
    let mut span_sum = 0usize;
    let mut gap_sum = 0usize;
    let mut start_sum = 0usize;
    let mut len_sum = 0usize;

    for token in &tokens.folder {
        let score = match_score_for_path(token, path)?;
        penalty_sum += score.0;
        span_sum += score.1;
        gap_sum += score.2;
        start_sum += score.3;
        len_sum += score.4;
    }
    for token in &tokens.tags {
        let score = best_tag_score(token, tags)?;
        penalty_sum += score.0;
        span_sum += score.1;
        gap_sum += score.2;
        start_sum += score.3;
        len_sum += score.4;
    }
    for token in &tokens.any {
        let path_score = match_score_for_path(token, path);
        let tag_score = best_tag_score(token, tags);
        let score = match (path_score, tag_score) {
            (Some(path), Some(tag)) => path.min(tag),
            (Some(path), None) => path,
            (None, Some(tag)) => tag,
            (None, None) => return None,
        };
        penalty_sum += score.0;
        span_sum += score.1;
        gap_sum += score.2;
        start_sum += score.3;
        len_sum += score.4;
    }

    Some((penalty_sum, span_sum, gap_sum, start_sum, len_sum))
}

fn best_tag_score(token: &str, tags: &[String]) -> Option<MatchScore> {
    let mut best: Option<MatchScore> = None;
    for tag in tags {
        if let Some(score) = match_score(token, tag) {
            best = match best {
                Some(existing) => Some(existing.min(score)),
                None => Some(score),
            };
        }
    }
    best
}

fn match_score_for_path(token: &str, path: &str) -> Option<MatchScore> {
    let entry = entry_name(path);
    if let Some(score) = match_score(token, &entry) {
        return Some(score);
    }
    let mut score = match_score(token, path)?;
    score.0 += 1;
    Some(score)
}

fn sort_indices(
    indices: &mut [usize],
    items: &[String],
    sort_mode: SortMode,
    meta_cache: &HashMap<String, SortMeta>,
) {
    indices.sort_by(|left, right| compare_indices(*left, *right, items, sort_mode, meta_cache));
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
        SortMode::Match => Ordering::Equal,
        SortMode::AlphaAsc => compare_names(left_path, right_path).then_with(|| left.cmp(&right)),
        SortMode::AlphaDesc => compare_names(right_path, left_path).then_with(|| left.cmp(&right)),
        SortMode::CreatedAsc => {
            compare_time(left_path, right_path, meta_cache, TimeField::Created, false)
                .then_with(|| compare_names(left_path, right_path))
        }
        SortMode::CreatedDesc => {
            compare_time(left_path, right_path, meta_cache, TimeField::Created, true)
                .then_with(|| compare_names(left_path, right_path))
        }
        SortMode::ModifiedAsc => compare_time(
            left_path,
            right_path,
            meta_cache,
            TimeField::Modified,
            false,
        )
        .then_with(|| compare_names(left_path, right_path)),
        SortMode::ModifiedDesc => {
            compare_time(left_path, right_path, meta_cache, TimeField::Modified, true)
                .then_with(|| compare_names(left_path, right_path))
        }
    }
}

fn compare_names(left: &str, right: &str) -> Ordering {
    entry_name(left)
        .to_lowercase()
        .cmp(&entry_name(right).to_lowercase())
}

enum TimeField {
    Created,
    Modified,
}

fn compare_time(
    left_path: &str,
    right_path: &str,
    meta_cache: &HashMap<String, SortMeta>,
    field: TimeField,
    descending: bool,
) -> Ordering {
    let value = |path: &str| -> Option<i64> {
        let meta = meta_cache.get(path)?;
        match field {
            TimeField::Created => meta.created_epoch,
            TimeField::Modified => meta.modified_epoch,
        }
    };

    let left_value = value(left_path);
    let right_value = value(right_path);
    let ordering = match (left_value, right_value) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    };
    if descending {
        ordering.reverse()
    } else {
        ordering
    }
}

pub(crate) fn fuzzy_match(query: &str, text: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let mut query_chars = query.chars().filter(|c| !c.is_whitespace());
    let mut current = query_chars.next();
    if current.is_none() {
        return true;
    }
    for ch in text.chars() {
        if let Some(expected) = current {
            if expected.eq_ignore_ascii_case(&ch) {
                current = query_chars.next();
                if current.is_none() {
                    return true;
                }
            }
        }
    }
    false
}

fn match_score(query: &str, text: &str) -> Option<MatchScore> {
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

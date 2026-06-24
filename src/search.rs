use std::{cmp::Ordering, collections::HashMap, path::Path};

#[derive(Clone, Copy, Default)]
pub(crate) struct SortMeta {
    pub(crate) modified_epoch: Option<i64>,
    pub(crate) created_epoch: Option<i64>,
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

pub(crate) fn fuzzy_match(query: &str, text: &str) -> bool {
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

fn sort_indices(
    indices: &mut [usize],
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

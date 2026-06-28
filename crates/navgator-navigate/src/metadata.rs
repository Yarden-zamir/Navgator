use crate::commands::run_command_output;
use crate::model::{MetaResult, DATE_WIDTH};
use std::{
    collections::{HashMap, HashSet},
    sync::mpsc,
    thread,
};

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

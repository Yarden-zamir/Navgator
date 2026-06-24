use crate::commands::{git_command_succeeds, run_git_command_allow_empty};
use crate::model::GitWorktree;
use crate::search::entry_name;
use std::path::{Path, PathBuf};

pub(crate) fn git_command_dir_for_path(path: &Path) -> Option<PathBuf> {
    let dir = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(Path::to_path_buf)?
    };

    if git_command_succeeds(&dir, &["rev-parse", "--git-dir"]) {
        return Some(dir);
    }

    let dot_bare = dir.join(".bare");
    if dot_bare.is_dir() && git_is_bare_repository(&dot_bare) {
        return Some(dot_bare);
    }

    Some(dir)
}

pub(crate) fn git_is_inside_work_tree(repo_dir: &Path) -> bool {
    run_git_command_allow_empty(repo_dir, &["rev-parse", "--is-inside-work-tree"])
        .map(|value| value.trim() == "true")
        .unwrap_or(false)
}

pub(crate) fn git_is_bare_repository(repo_dir: &Path) -> bool {
    run_git_command_allow_empty(repo_dir, &["rev-parse", "--is-bare-repository"])
        .map(|value| value.trim() == "true")
        .unwrap_or(false)
}

pub(crate) fn git_worktrees_for_path(path: &Path) -> Vec<GitWorktree> {
    let Some(repo_dir) = git_command_dir_for_path(path) else {
        return Vec::new();
    };
    let Some(output) = run_git_command_allow_empty(&repo_dir, &["worktree", "list", "--porcelain"])
    else {
        return Vec::new();
    };
    parse_git_worktree_list(&output)
}

pub(crate) fn parse_git_worktree_list(output: &str) -> Vec<GitWorktree> {
    let mut worktrees = Vec::new();
    let mut current: Option<GitWorktree> = None;

    for raw_line in output.lines() {
        let line = raw_line.trim_end();
        if line.is_empty() {
            if let Some(worktree) = current.take() {
                worktrees.push(worktree);
            }
            continue;
        }

        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(worktree) = current.take() {
                worktrees.push(worktree);
            }
            current = Some(GitWorktree {
                path: path.to_string(),
                branch: None,
                detached: false,
                bare: false,
            });
        } else if let Some(worktree) = current.as_mut() {
            if let Some(branch) = line.strip_prefix("branch ") {
                worktree.branch = Some(git_branch_label(branch));
            } else if line == "detached" {
                worktree.detached = true;
            } else if line == "bare" {
                worktree.bare = true;
            }
        }
    }

    if let Some(worktree) = current {
        worktrees.push(worktree);
    }

    worktrees
}

pub(crate) fn git_worktree_label(worktree: &GitWorktree, shorten_after_slash: bool) -> String {
    if let Some(branch) = worktree.branch.as_ref() {
        if !branch.trim().is_empty() {
            return worktree_tab_label(branch, shorten_after_slash);
        }
    }
    if worktree.detached {
        return "detached".to_string();
    }
    let name = entry_name(&worktree.path);
    if name.trim().is_empty() {
        "worktree".to_string()
    } else {
        worktree_tab_label(&name, shorten_after_slash)
    }
}

pub(crate) fn worktree_tab_label(label: &str, shorten_after_slash: bool) -> String {
    if !shorten_after_slash {
        return label.to_string();
    }
    label
        .rsplit('/')
        .find(|segment| !segment.trim().is_empty())
        .unwrap_or(label)
        .to_string()
}

fn git_branch_label(branch: &str) -> String {
    if let Some(value) = branch.strip_prefix("refs/heads/") {
        return value.to_string();
    }
    if let Some(value) = branch.strip_prefix("refs/remotes/") {
        return value.to_string();
    }
    branch.to_string()
}

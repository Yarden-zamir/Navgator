# Git Worktree Preview Tabs

## Goal

Support browsing git repositories that have worktrees, including bare repositories that manage worktrees, without changing normal directory preview behavior.

## Navigation Contract

- Normal folders and repos keep the current flow: browser -> preview -> git.
- When the selected path has multiple git worktrees, preview becomes a tabbed sequence: browser -> preview(worktree 1) -> preview(worktree 2) -> ... -> git.
- `Right` from search enters preview tab 1.
- `Right` from a preview tab advances to the next preview tab; from the last preview tab it moves to Git when Git content exists.
- `Left` reverses that chain: Git -> last preview tab -> previous preview tab -> search.
- `PageUp`, `PageDown`, `Home`, and `End` scroll only the active preview or Git panel.
- `Up` and `Down` scroll the active panel first; at the top/bottom boundary they follow the same search -> preview tabs -> git focus chain.

## Discovery Rules

- Use `git worktree list --porcelain` as the source of truth for worktree paths.
- Treat a selected non-bare worktree as worktree-capable when the command returns more than one `worktree` entry.
- Treat a selected bare repository as worktree-capable when `git -C <path> rev-parse --is-bare-repository` returns `true` and worktree entries exist.
- Treat a selected worktree container with a direct `.bare/` child as worktree-capable when `git -C <path>/.bare rev-parse --is-bare-repository` returns `true`.
- Do not render the bare repository's own `bare` entry as a content-preview tab when non-bare worktrees are available.
- Preserve Git's worktree order; this normally places the main worktree first.
- If discovery fails or only one worktree is reported, fall back to the selected path as the only preview tab.

## Rendering

- Keep one preview panel; render a Ratatui `Tabs` row inside that panel only when multiple preview tabs exist.
- Use worktree labels as tab titles and keep the selected tab in sync with the active preview index.
- Pseudo-scroll the rendered tab titles as a contiguous slice that starts at the previous tab when possible, so previous/current/next remain visible on ordinary widths.
- Adaptively truncate long tab labels before dropping tabs; defaults keep 6 label characters before `...` for non-selected tabs and 10 before `...` for the selected tab.
- `[preview].worktree_tab_min_chars` and `[preview].selected_worktree_tab_min_chars` configure those truncation thresholds.
- By default, shorten worktree tab labels to the segment after the last slash; `[preview].shorten_worktree_tab_labels = false` keeps full labels.
- Git panel remains a single panel for the selected repo/worktree group.
- For bare repos and `.bare/` worktree containers, the Git panel notes `Bare repository` but branch/commit/diff/untracked details are sourced from the active worktree tab.
- Preview tab folder contents should render before Git details are available; Git details load asynchronously after preview tabs are cached.
- Git details stream per tab, starting with the active tab, so the visible worktree's Git panel can appear before every worktree has finished loading.

## Implementation Notes

- Keep the change in `src/main.rs`; the split `src/*.rs` modules are not currently wired into the binary.
- Keep external command execution through `Command` with explicit args and the existing no-color git behavior.
- Cache all preview tabs under the selected browser path, and reset the active tab/scroll when selection changes.
- Compute preview scroll height and tag-edit cursor placement from the tab-aware preview body rect, not from the outer panel height.

# Possible Improvements

## Recommended Next Work

- Refresh worktree cache after create/delete.
  - Update `worktrees.json` when a remote branch creates a worktree.
  - Remove deleted worktrees from cache after `Ctrl+D` succeeds.
  - Keep the next launch consistent without waiting for the background scanner.

- Add explicit remote refresh.
  - Keep `Ctrl+O` fast by showing cached and local remote branches immediately.
  - Add `Ctrl+R` in remote mode to force a network refresh.
  - Surface refresh state in the existing remote toggle label/status.

- Add inline worktree health indicators.
  - Show `dirty`, `ahead N`, or `behind N` for visible worktree rows.
  - Load only for visible rows to avoid startup cost.
  - Reuse the delete safety checks where possible.

## Useful UI Improvements

- Add a contextual action menu.
  - Use one key for actions instead of adding many shortcuts.
  - Project actions: open, copy path, refresh worktrees.
  - Worktree actions: open, delete, copy path, copy branch.
  - Remote branch actions: create worktree, copy branch, refresh remote.

- Add delete confirmation.
  - Show `Enter` to confirm and `Esc` to cancel before safety checks run.
  - Keep current safety checks unchanged.
  - Present safety failures as a checklist when deletion is blocked.

- Improve remote branch preview.
  - Show branch age, author, last commit subject, and short SHA when locally available.
  - Avoid network work during preview unless explicitly refreshed.
  - Preserve the current fast `Ctrl+O` behavior.

## Performance Improvements

- Add session preview cache.
  - Cache tree preview and Git detail per path during the session.
  - Avoid rerunning `erd` and repeated Git status/log commands when moving between rows.

- Defer non-visible metadata.
  - Continue loading modified dates for sorting, but prioritize current and visible rows first.
  - Keep background scans bounded and cancel stale batches when possible.

- Make Git detail tab loading more selective.
  - Load the active preview tab first.
  - Avoid loading all worktree tabs until the user visits them.

## Search Improvements

- Add structured search prefixes.
  - `branch:term` for branch names only.
  - `author:name` for remote branch authors.
  - `repo:name` for project/repository names.

- Show why a row matched.
  - If search matched author/context/tag rather than the visible branch name, show the matching field in the right-side context.

## Lower Priority Ideas

- Open selected project/worktree in an editor.
  - Support `$EDITOR` first.
  - Optional support for common GUI editors can come later.

- Add configurable keybindings.
  - Keep defaults simple.
  - Allow advanced users to customize without changing code.

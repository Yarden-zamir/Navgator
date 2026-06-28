# navgator

Rust TUI tools for project navigation and GitHub issue exploration. Shared generic helpers live in `navgator-core`; each TUI is its own binary crate.

![navgator screenshot](Screenshot.png)

## Build

```
cargo build --release --workspace
```

## Run

```
./target/release/navgator-navigate
```

Explore GitHub issues for the repo in the current folder:

```
./target/release/navgator-issues
```

## Zsh wrapper

```
source /Users/kcw/GitHub/navgator/scripts/navgator.zsh
```

Bind example:

```
bindkey '^T' navigate
```

## Search

- Default: fuzzy match against folder paths and tags.
- `@term`: match folder path only.
- `#term`: match tags only.

Examples:

```
@create #mods
```

## Tags

Place a `.navgator.toml` in a folder:

```
tags = ["mods", "minecraft", "create"]
```

Tags render as colored pills in results and in the preview. Colors are deterministic per tag name.

## Sorting

Cycle with `Ctrl+S`:

- `Match` (default)
- `A->Z` / `Z->A`
- `Created ^` / `Created v`
- `Modified ^` / `Modified v`

Sorting by time triggers background metadata scans.

## Panels and navigation

- Search panel (left) edits query.
- Preview panel (right top) shows path and `erd` tree.
- Details panel (right bottom) tabs GitHub README/repo summary and Git details when available.
- Right/Left switch focus between panels; Up/Down scroll within panels.
- Mouse click focuses a panel; mouse wheel scrolls.

## GitHub issues

`navgator-issues` uses `gh` and the current repo's `origin` remote to show issues in a separate TUI.

- Type to filter by title, body, number, or labels.
- `#term` filters issue numbers and labels.
- `@term` filters authors and assignees.
- `Tab` cycles open, closed, and all issues.
- `r` refreshes from GitHub.
- `Enter` prints the selected issue URL.

## Preview tooling

- Uses `erd` with `~/.erdtreerc` if present; otherwise defaults:
  `--dir-order=first --icons --sort=name --level=4 --color force --layout=inverted --human --suppress-size`
- Git panel is hidden when not in a git repo.

## Config

No defaults are used. If no config is found, the app will exit and ask you to create one.

Config file search order (merge all found):

- `$NAVGATOR_CONFIG`
- `/etc/navgator/config.toml`
- `$XDG_CONFIG_HOME/navgator/config.toml`
- `~/.config/navgator/config.toml`
- `~/.navgator.toml`
- `./.navgator.toml`
- `./.navgator/config.toml`

Config format (TOML):

```
"$schema" = "https://raw.githubusercontent.com/Yarden-zamir/Navgator/main/config-schema.json"

[paths]
index_folders = ["/Users/kcw/Github", "/Users/kcw/Desktop"]
static_items = ["/opt/homebrew", "/Users/kcw/Downloads"]

[preview]
shorten_worktree_tab_labels = true
worktree_tab_min_chars = 6
selected_worktree_tab_min_chars = 10
```

If an existing config contains `[paths]` but no `"$schema"`, navgator prepends this line automatically when loading that config.

`shorten_worktree_tab_labels` defaults to `true`; worktree branch labels like `feat/yarden/potato` render as `potato` in preview tabs. Set it to `false` to show full labels.
`worktree_tab_min_chars` defaults to `6`; `selected_worktree_tab_min_chars` defaults to `10`. These control how many label characters are kept before `...` when worktree preview tabs must shrink.

Schema file is generated from the Rust config structs:

```
cargo run -p navgator-navigate -- config-schema > config-schema.json
```

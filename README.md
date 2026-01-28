# navgator

Rust TUI directory picker with search, tags, and previews. Prints the selected path to stdout.

## Build

```
cargo build --release
```

## Run

```
./target/release/navgator navigate
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
- Git panel (right bottom) shows branch, recent commits, diff stats, and untracked files.
- Right/Left switch focus between panels; Up/Down scroll within panels.
- Mouse click focuses a panel; mouse wheel scrolls.

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
[paths]
index_folders = ["/Users/kcw/Github", "/Users/kcw/Desktop"]
static_items = ["/opt/homebrew", "/Users/kcw/Downloads"]
```

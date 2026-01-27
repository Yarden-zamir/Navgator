# navgator

Rust TUI directory picker with search, tags, and previews. Prints the selected path to stdout.

## Build

```
cargo build --release
```

## Run

```
./target/release/navgator navigate
./target/release/navgator context <name> [--create|--no-create] [--template <template>] [--description <desc>]
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

Tags render as colored pills in results and in the preview title. Colors are deterministic per tag name.

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

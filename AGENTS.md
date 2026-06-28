# AGENTS.md

## Project Shape

- Cargo workspace with one small shared library and two implementation crates.
- `crates/navgator-core`: generic helpers only (`AppResult`, selection output, tty setup, generic command output, fuzzy match). Do not put Git, GitHub, content-provider, config, tag, or TUI-compositor behavior here.
- `crates/navgator-navigate`: project navigator implementation. Git, GitHub README, folder/worktree content, config, tags, metadata, search ranking/explanations, compositor, and UI are implementation-specific here.
- `crates/navgator-issues`: GitHub issue explorer implementation. It owns its own Git/GitHub helpers and issue-specific streaming/compositor/UI behavior, even where that duplicates navigate behavior.
- Runtime entrypoints: `navgator-navigate` runs the project navigator TUI; `navgator-issues` runs a GitHub issue explorer for the current repo; `navgator-navigate config-schema`/`schema` prints JSON schema.

## Commands

- Fast compile check: `cargo check --workspace`
- Release build used by the wrapper fallback: `cargo build --release --workspace`
- Format check: `cargo fmt -- --check`
- Apply formatting: `cargo fmt`
- Strict lint: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Tests: `cargo test --workspace`; focused tests use `cargo test -p <crate> <test_name>`
- Generate schema after config struct changes: `cargo run -p navgator-navigate -- config-schema > config-schema.json`

## Running Locally

- TUI run: `cargo run -p navgator-navigate`; issue explorer run: `cargo run -p navgator-issues`
- Release binary run: `./target/release/navgator-navigate` or `./target/release/navgator-issues`
- Zsh wrapper: `source /Users/kcw/GitHub/navgator/scripts/navgator.zsh`, then bind/use `navigate`.
- Wrapper binary lookup order is `$NAVGATOR_BIN`, `navgator-navigate` on `PATH`, `target/release/navgator-navigate`, then `target/debug/navgator-navigate`.
- Wrapper passes the selected path via `NAVGATOR_OUTPUT`; otherwise the binary prints the selected path to stdout.

## Config Behavior

- Config is required; no built-in folder defaults exist.
- Config files are merged in this order, with first-seen path dedupe per list: `$NAVGATOR_CONFIG`, `/etc/navgator/config.toml`, `$XDG_CONFIG_HOME/navgator/config.toml`, `~/.config/navgator/config.toml`, `~/.navgator.toml`, `./.navgator.toml`, `./.navgator/config.toml`.
- Supported TOML shape includes `[paths]` with `index_folders = [...]` and `static_items = [...]`, plus `[preview] shorten_worktree_tab_labels = true|false`, `worktree_tab_min_chars = 6`, and `selected_worktree_tab_min_chars = 10`.
- Relative config paths are resolved against the config file directory; `~/` and `$HOME` are expanded; non-existent paths are ignored.
- Loading a config with `[paths]` but no `"$schema"` may rewrite that config to prepend the schema URL.

## Runtime Gotchas

- Preview uses external `erd`; if `~/.erdtreerc` exists, its non-comment whitespace tokens are used as args, otherwise built-in defaults are used.
- Git preview commands are best-effort and hidden/omitted on failure or outside git repos.
- Background preview, metadata, and tag work uses `std::thread` plus `mpsc`; keep the draw/event loop non-blocking and drain channels before rendering.
- External commands should use `Command` with explicit args; existing git calls force `NO_COLOR=1`/`color.ui=never`.

## Scripts

- Python helpers live in `scripts/`; run with uv: `uv run python scripts/update_org_tags.py` and `uv run python scripts/update_language_tags.py`.

## Agent Workflow

- Keep behavior-changing edits in the active Cargo-built code path unless you first wire modules into `src/main.rs`.
- Update `README.md` and regenerate `config-schema.json` when config CLI/schema behavior changes.

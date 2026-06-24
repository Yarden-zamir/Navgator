# AGENTS.md

## Project Shape

- Single Rust binary crate with active modules wired from `src/main.rs`; do not add duplicate inactive split modules.
- Current split: `config.rs` (config/schema), `results.rs` (project results), `metadata.rs` (date metadata), `tags.rs` (local tag metadata/actions), `git.rs` (Git/worktree service), `content.rs` (folder/worktree/Git content providers), `ui.rs` (layout/rendering/tab composition), `search.rs` (query/filter/sort), `commands.rs` (external command helpers), and `model.rs` (shared types/provider-model scaffolding).
- Runtime entrypoints: `navgator` and `navgator navigate` run the TUI; `navgator config-schema`/`schema` prints JSON schema.

## Commands

- Fast compile check: `cargo check`
- Release build used by the wrapper fallback: `cargo build --release`
- Format check: `cargo fmt -- --check`
- Apply formatting: `cargo fmt`
- Strict lint: `cargo clippy --all-targets --all-features -- -D warnings`
- Tests: `cargo test`; focused tests use `cargo test <test_name>`
- Generate schema after config struct changes: `cargo run -- config-schema > config-schema.json`

## Running Locally

- TUI run: `cargo run -- navigate`
- Release binary run: `./target/release/navgator navigate`
- Zsh wrapper: `source /Users/kcw/GitHub/navgator/scripts/navgator.zsh`, then bind/use `navigate`.
- Wrapper binary lookup order is `$NAVGATOR_BIN`, `navgator` on `PATH`, `target/release/navgator`, then `target/debug/navgator`.
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

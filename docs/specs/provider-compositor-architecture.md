# Provider/Compositor Architecture

## Goals

- Turn navgator into a generic search and content engine, with project navigation as one implementation on top.
- Keep the UI responsive: never block the draw/event loop on filesystem, Git, metadata, or external-command work that can complete in the background.
- Make contracts explicit with Rust types so providers are easy to compose, test, and extend.
- Preserve current behavior while changing representation only.
- Support future domains such as GitHub issues, todos, cluster explorers, repo stats, or other non-file-backed sources without putting file-specific assumptions into core model types.

## Non-Goals For First Refactor

- No dynamic plugin loading.
- No external ABI or scripting interface.
- No behavior change to search, tags, preview, worktree tabs, Git panels, config, or keybindings.
- No provider configuration language yet; the first compositor can be hardcoded to match current behavior.

## Current Behavior To Preserve

- Config-driven project results from `[paths].index_folders` and `[paths].static_items`.
- Search supports normal fuzzy tokens, `@folder` tokens, and `#tag` tokens.
- Sort modes cycle through match, alpha, created, and modified ordering.
- Result rows show entry name, date, and tags.
- Tags are read/written from `.navgator.toml`, with `Ctrl+T` tag editing and suggestions.
- Preview content uses `erd` for folder contents and falls back gracefully when unavailable.
- Display paths under `$HOME` as `~`, but return absolute paths when selecting.
- Git worktree containers are detected for normal worktrees, bare repos, and direct `.bare/` child layouts.
- Worktree content is shown as tabs in the primary content panel.
- Worktree tab labels support slash-shortening, pseudo-scrolling, and configurable truncation thresholds.
- `Enter` from a worktree content tab returns that worktree path, not the parent container.
- Git panel notes `Bare repository` for bare/container selections but shows Git status for the active worktree tab.
- Folder content renders before Git details; Git details load asynchronously and stream active-tab-first.

## Conceptual Model

The core model should not know that an item is a folder. It should know that providers produce selectable results, metadata, content targets, content blocks, and actions.

### Results Provider

A results provider owns a searchable collection.

Examples:

- Project results from local config paths.
- GitHub issues from a repo or organization.
- Todo items from a file or service.
- Kubernetes clusters/namespaces/resources.

Responsibilities:

- Build or refresh its result set.
- Interpret query input for its own domain.
- Filter and sort results, possibly using metadata supplied by metadata providers.
- Emit stable result IDs and display labels.
- Define which metadata keys are useful for result-row rendering and sorting.
- Register result-scope keybindings when needed.

### Metadata Provider

A metadata provider supplies facts about result IDs or content target IDs.

Examples:

- File created/modified dates.
- Local project tags.
- Language tags from repo analysis.
- GitHub labels/assignees/state.
- Todo priority/due date.
- Cluster health/status.

Responsibilities:

- Load metadata asynchronously when possible.
- Emit typed metadata values keyed by metadata key.
- Optionally provide sortable values separately from display values.
- Optionally register metadata-editing actions such as tag editing.
- Avoid UI layout decisions.

### Content Provider

A content provider produces renderable content for a selected result or content target.

Examples:

- Folder tree content.
- Git worktree content targets and tabs.
- Git status content.
- Repo stats content.
- GitHub issue body/comments.
- Todo details.

Responsibilities:

- Declare whether it supports a given input target.
- Load content asynchronously unless it is already available and cheap.
- Emit one or more content targets when a selected item expands into sub-targets.
- Emit semantic content blocks, not raw UI widgets when possible.
- Optionally override the active target for downstream providers.

### Compositor

The compositor owns layout and provider placement.

Responsibilities:

- Define panels, tabbed panels, focus order, sizes, and provider priority.
- Decide which providers feed each panel.
- Render semantic content into Ratatui widgets.
- Decide fallback behavior when a higher-priority provider has not loaded yet.
- Manage tabs and active content target propagation.
- Route keybindings to providers or built-in focus/navigation behavior.
- Keep the draw/event loop non-blocking by consuming provider updates from channels.

## Core Types

These are conceptual names; exact names can change during implementation.

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ProviderId(String);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ResultId(String);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ContentId(String);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct MetadataKey(String);
```

IDs are strings so they can represent paths, issue IDs, cluster object IDs, or compound provider IDs without forcing filesystem concepts into the core.

```rust
struct ResultEntry {
    id: ResultId,
    provider_id: ProviderId,
    display: String,
    metadata: MetadataMap,
}

struct ContentTarget {
    id: ContentId,
    source_result_id: ResultId,
    provider_id: ProviderId,
    display: String,
    metadata: MetadataMap,
    selection_value: SelectionValue,
}
```

`selection_value` is what `Enter` returns for the active target. For project navigation it is an absolute path. For GitHub issues it might be a URL or issue key.

```rust
enum SelectionValue {
    Path(PathBuf),
    Url(String),
    Text(String),
    ProviderSpecific { provider_id: ProviderId, value: String },
}
```

Metadata values should be typed instead of stringly typed.

```rust
enum MetadataValue {
    Text(String),
    Number(i64),
    Decimal(f64),
    Bool(bool),
    DateTime(i64),
    Tags(Vec<String>),
    List(Vec<MetadataValue>),
}

struct MetadataEntry {
    key: MetadataKey,
    value: MetadataValue,
    display: Option<String>,
    sort_value: Option<MetadataValue>,
}

type MetadataMap = BTreeMap<MetadataKey, MetadataEntry>;
```

Panel content should also be semantic.

```rust
enum ContentBlock {
    Text { lines: Vec<StyledLine> },
    List { items: Vec<ListRow> },
    Tree { lines: Vec<StyledLine> },
    Table { columns: Vec<TableColumn>, rows: Vec<TableRow> },
    Empty { message: String },
    Loading { message: String },
    Error { message: String },
}
```

Ratatui-specific conversion should live near the compositor/UI renderer, not in providers that do not need styling control.

## Provider Traits

Traits should be small and concrete enough to use without advanced indirection.

```rust
trait ResultsProvider {
    fn id(&self) -> ProviderId;
    fn refresh(&self, ctx: &ProviderContext, sink: ResultsSink);
    fn search(&self, query: &SearchQuery, store: &ProviderStore) -> Vec<ResultEntry>;
    fn row_spec(&self) -> ResultRowSpec;
    fn keybindings(&self) -> Vec<KeyBinding>;
}
```

`refresh` may spawn background work and send updates. `search` should use already-known data and stay fast enough for each keypress.

```rust
trait MetadataProvider {
    fn id(&self) -> ProviderId;
    fn supports_result(&self, result: &ResultEntry) -> bool;
    fn supports_content(&self, target: &ContentTarget) -> bool;
    fn load_for_result(&self, result: &ResultEntry, ctx: &ProviderContext, sink: MetadataSink);
    fn load_for_content(&self, target: &ContentTarget, ctx: &ProviderContext, sink: MetadataSink);
    fn keybindings(&self) -> Vec<KeyBinding>;
}
```

```rust
trait ContentProvider {
    fn id(&self) -> ProviderId;
    fn priority(&self) -> i32;
    fn supports(&self, input: &ContentInput, store: &ProviderStore) -> bool;
    fn load(&self, input: ContentInput, ctx: &ProviderContext, sink: ContentSink);
    fn keybindings(&self) -> Vec<KeyBinding>;
}
```

Content providers emit updates.

```rust
struct ContentUpdate {
    provider_id: ProviderId,
    panel_id: PanelId,
    source_input_id: ContentId,
    targets: Vec<ContentTarget>,
    active_target: Option<ContentId>,
    blocks: Vec<ContentBlock>,
    status: LoadStatus,
}

enum LoadStatus {
    Loading,
    Partial,
    Ready,
    Failed(String),
}
```

## Compositor Model

The compositor should be explicit and typed. It can be hardcoded first.

```rust
struct CompositorSpec {
    results_panel: ResultsPanelSpec,
    panels: Vec<PanelSpec>,
    focus_order: Vec<PanelId>,
}

struct ResultsPanelSpec {
    provider_id: ProviderId,
    row: ResultRowSpec,
}

struct PanelSpec {
    id: PanelId,
    title: String,
    layout: PanelLayout,
    providers: Vec<PanelProviderSpec>,
    children: Vec<PanelSpec>,
}

enum PanelLayout {
    Single,
    VerticalSplit { percentages: Vec<u16> },
    HorizontalSplit { percentages: Vec<u16> },
    Tabs { tab_policy: TabPolicy },
}

struct PanelProviderSpec {
    provider_id: ProviderId,
    priority: i32,
    fallback: FallbackPolicy,
}
```

Tabbed panels must be able to contain other panels.

Example future layout:

```text
Right side
  Top panel: active content tabs
    Folder/worktree content

  Bottom tabbed panel
    Tab 1: Git status panel
    Tab 2: GitHub repo stats panel
    Tab 3: CI status panel
```

Spec shape:

```rust
PanelSpec {
    id: PanelId("details"),
    layout: PanelLayout::Tabs { tab_policy },
    children: vec![
        PanelSpec { id: PanelId("git"), providers: vec![git_status], .. },
        PanelSpec { id: PanelId("github-stats"), providers: vec![repo_stats], .. },
    ],
    ..
}
```

The compositor controls tab rendering, active tab state, truncation, focus transitions, and provider selection inside each tab.

## Provider Priority And Fallback

Multiple content providers may support the same selected item.

Current panel 1 behavior should be represented as:

```text
Primary content panel providers:
  100 GitWorktreeContentProvider
   10 FolderContentProvider
```

Rules:

- Cheap fallback content can render immediately.
- Higher-priority providers may replace lower-priority content when ready.
- Higher-priority providers may emit content targets that override the active target for downstream panels.
- Lower-priority providers should not block waiting for higher-priority providers.

For `qdi-db-commands`:

```text
Initial selection:
  ResultEntry id = project:/Users/kcw/Github/qdi-db-commands

Fast fallback:
  FolderContentProvider shows folder contents for the container.

Worktree provider ready:
  GitWorktreeContentProvider replaces primary content with tabs:
    main
    oauth-scopes
    t
    ...
  Active ContentTarget becomes selected worktree path.

Downstream details:
  GitStatusContentProvider receives the active worktree ContentTarget.
```

## Async And Responsiveness Rules

- The event loop must never run external commands directly.
- Search should use cached/indexed data and stay responsive for each keypress.
- Metadata providers must track in-flight work per item/key to avoid duplicate loads.
- Content providers must emit `Loading` or fallback content quickly when work is slow.
- Expensive provider work should stream partial updates when possible.
- Active-target work should be prioritized before offscreen or inactive targets.
- Stale worker results must be safe: apply only if they still match a known result/content ID.

Current implementation examples to preserve:

- `erd` preview is background work.
- Git status is background work.
- Git details stream per worktree tab, active tab first.
- Metadata/date/tag loading is background work.

## Keybindings

Providers can register keybindings, but the compositor owns conflict resolution and display.

```rust
struct KeyBinding {
    provider_id: ProviderId,
    scope: KeyScope,
    key: KeyChord,
    label: String,
    action: ActionId,
}

enum KeyScope {
    Global,
    ResultsPanel,
    Panel(PanelId),
    ActiveContent,
}
```

Current examples:

- Tag metadata provider registers `Ctrl+T` for tag editing.
- Results compositor owns `Ctrl+S` for sort mode and `Ctrl+U` for clearing input.
- Panel compositor owns arrow navigation and tab switching.

Provider actions should receive typed context.

```rust
struct ActionContext {
    selected_result: Option<ResultEntry>,
    active_content_target: Option<ContentTarget>,
    metadata: MetadataStore,
}
```

## Current Behavior Mapping

### ProjectResultsProvider

- Owns config path expansion and result IDs.
- Uses path strings only internally; emits generic `ResultEntry` values.
- Supports fuzzy search, `@folder`, and metadata-backed `#tag` matching.
- Defines row metadata slots: tags and date.

### ProjectMetadataProvider

This can be split into two providers during implementation if cleaner.

- Date metadata: created/modified display and sort values.
- Tag metadata: local project tags and tag editing action.

### FolderContentProvider

- Supports content targets with a path selection value that is a directory.
- Emits folder tree content from `erd`.
- Converts displayed paths under `$HOME` to `~`.

### GitWorktreeContentProvider

- Supports content targets that represent Git repos, bare repos, `.bare/` containers, or worktrees.
- Uses `git worktree list --porcelain` as source of truth.
- Emits one content target per non-bare worktree.
- Supplies tab labels from branch/worktree labels.
- Overrides active target to the selected worktree target.

### GitStatusContentProvider

- Supports active content targets that are Git worktrees/repos.
- Emits branch, recent commits, staged/unstaged/untracked summaries.
- For bare/container-origin content, includes `Bare repository` note while showing active worktree status.

## Suggested Module Layout

First implementation should keep modules simple and concrete.

```text
src/main.rs          CLI dispatch only
src/model.rs         IDs, entries, targets, metadata, content blocks, settings
src/config.rs        config loading/schema/path normalization
src/commands.rs      process helpers and no-color Git command helpers
src/search.rs        query parsing, fuzzy search, sorting
src/providers/
  mod.rs             provider traits and registry structs
  projects.rs        ProjectResultsProvider
  metadata.rs        date/tag metadata providers
  folder.rs          FolderContentProvider
  git.rs             Git service, worktree parser, Git content providers
src/compositor.rs    panel specs, provider priority, state propagation
src/ui.rs            Ratatui rendering, layout, tab rendering/truncation
src/navigate.rs      event loop, worker channels, input/focus orchestration
```

## Implementation Phases

### Phase 1: Safe Module Cleanup

- Keep behavior in `src/main.rs` until each module is wired.
- Extract pure helpers first: commands, display paths, search, tags.
- Add `mod` declarations only when the module is used by the active binary.
- Delete duplicate inactive code immediately.
- Run full verification after each extraction batch.

### Phase 2: Shared Model Types

- Introduce `ResultEntry`, `ContentTarget`, `SelectionValue`, `MetadataValue`, `ContentBlock`, and settings structs.
- Adapt current path-based behavior to these types without adding dynamic dispatch.
- Keep exact UI behavior.

### Phase 3: Concrete Providers

- Implement project results, metadata, folder content, Git worktree content, and Git status as concrete structs/functions.
- Use typed update messages instead of ad-hoc preview/Git result structs.
- Keep the compositor hardcoded to current layout.

### Phase 4: Provider Traits And Registry

- Introduce traits once concrete boundaries are stable.
- Add provider ordering, priority, and fallback rules.
- Keep registry compile-time and in-process.

### Phase 5: Configurable Composition

- Only after traits are stable, consider config-driven panel/provider composition.
- Tabbed panels containing panels should be part of the compositor model before this phase.

## Verification Requirements

- `cargo fmt -- --check`
- `cargo check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test`
- `cargo build --release`

Behavior-sensitive tests to preserve or add:

- Worktree porcelain parsing.
- `.bare/` container detection.
- Worktree tab label shortening and truncation.
- Active worktree `Enter` selection.
- Home path `~` display.
- Per-tab Git update application.
- Search token matching and tag matching.
- Config merge/default behavior.

## Design Guardrails

- Core model types must not assume files or directories.
- Providers can know their own domain; the compositor should not know Git internals.
- UI should render semantic content and metadata, not execute provider logic.
- Background workers must be idempotent and safe to apply late.
- Prefer explicit structs over large tuples or stringly typed maps.
- Avoid dynamic plugin loading until the internal provider contracts prove stable.

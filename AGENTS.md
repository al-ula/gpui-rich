# `gpui-rich` Agent Rules

## Architecture Invariants

This workspace implements a non-paginated, Google Docs-style rich text renderer on GPUI designed to grow cleanly into a full editor. Preserve these crate boundaries:

* `crates/rich-model` (pure Rust, zero GPUI dependency): Hierarchical `BlockNode` tree with stable `BlockId` and `version`, `InlineContent` with normalized contiguous `StyleSpan`s and `\u{FFFC}` sentinels for inline atomic objects, `DocPos` + `Affinity` addressing, `DocumentSelection`, `RichCommand` / `CommandOutcome`, and `Transaction` / `Step` mutations with `History` undo/redo.
* `crates/rich-layout` (pure Rust, zero GPUI dependency): Generic over `GlyphBackend` and `TextShaper` so headless tests run via `MockShaper` without a GPU window. Owns mixed-baseline line composition, UAX #14 wrapping, glyph slicing, tab stops, justification, recursive table/blockquote layout, incremental `LayoutCache`, Fenwick tree `HeightIndex`, collapsible headings, `HorizontalLayout::Fill` vs infinite-length `Paper(PaperSpec)`, and `(BlockId, local_y_offset)` scroll anchoring.
* `crates/rich-gpui` (`gpui-kit 0.10`, `default-features = false`, optional `gpui-fast` feature): `GpuiShaper` over `WindowTextSystem::shape_line`, unified `RichDocumentElement` painting 6 strict z-order layers, and command-driven `RichDocumentView` (all interactions and Phase 2 editing operations route through `RichCommand` + `dispatch_command` and `gpui_kit::actions!` scoped to `"RichDocument"`).
* `examples/docs_viewer`: Interactive showcase application.

### Dependency Note (Linux + `gpui-kit`)

On Linux, `gpui-kit` transitively uses `yeslogic-fontconfig-sys`, which queries `pkg-config` for `fontconfig.pc` unless `dlopen` is enabled. `.cargo/config.toml` sets `RUST_FONTCONFIG_DLOPEN = "1"` so builds dynamically load `libfontconfig.so.1` without requiring `fontconfig-devel` headers.

## Rust Coding Guidelines

* Comments explain "why" for tricky or non-obvious code only. Avoid comments that summarize what code does.
* Prefer extending existing files over creating new ones. Add a new file only for a new logical component, and avoid many small files.
* Avoid `unwrap()` and `expect()` in library and application code. Propagate errors with `?` or handle them explicitly. Avoid panicking indexing or slicing by using `.get()` and checked access whenever bounds are uncertain.
* Never silently discard errors with `let _ =` on fallible operations:
  * Propagate with `?` when the caller should handle the error.
  * Log errors when intentional ignoring needs visibility.
  * Use `match` or `if let Err(...)` for explicit recovery.
* Never create `mod.rs` or `<module>_<submodule>.rs`. Use `src/<module>.rs` + `src/<module>/<submodule>.rs` naming with descriptive module paths.
* Implement strictly what is requested. Avoid speculative features or unrequested additions.
* Use full words for variable names without abbreviations (for example, `queue`, `index`, `context`), except framework-idiomatic names such as GPUI's `cx`.
* Use `cargo add` to add crate dependencies.
* Prefer rustdocs (`///` and `//!`) for documenting current code over separate standalone documentation files. Plans and progress trackers (such as `ARCHITECTURE.md`) are outside this rule and belong in the repository.

## Testing & Build Strategy

* Always limit parallel Cargo jobs (for example, `cargo check --workspace -j 4`, `cargo build -j 4`, `cargo test -p rich-model -p rich-layout -j 4`).
* Be conservative when writing tests: create tests only for critical architectural invariants or essential safety contracts. Avoid trivial, redundant, or fixture-heavy tests.

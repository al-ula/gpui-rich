# `gpui-rich` Agent Rules

## Architecture Invariants

This crate implements a non-paginated, Google Docs-style rich text renderer on GPUI designed to grow cleanly into a full editor. Preserve these module and feature boundaries inside `gpui-rich`:

* `src/model.rs` + `src/model/` (pure Rust, zero GPUI dependency): Hierarchical `BlockNode` tree with stable `BlockId` and `version`, `InlineContent` with normalized contiguous `StyleSpan`s and `\u{FFFC}` sentinels for inline atomic objects, `DocPos` + `Affinity` addressing, `DocumentSelection`, `RichCommand` / `CommandOutcome`, `Transaction` / `Step` mutations with `History` undo/redo, clipboard/HTML codecs, and structural/table edits (`src/model/edit/{structure,table}.rs`).
* `src/layout.rs` + `src/layout/` (pure Rust, zero GPUI dependency): Generic over `GlyphBackend` and `TextShaper` so headless tests run via `MockShaper` without a GPU window (`cargo test --no-default-features -j 4`). Owns mixed-baseline line composition, UAX #14 wrapping, glyph slicing, tab stops, justification, recursive table/blockquote layout, incremental `LayoutCache`, Fenwick tree `HeightIndex`, collapsible headings, `HorizontalLayout::Fill` vs infinite-length `Paper(PaperSpec)`, and `(BlockId, local_y_offset)` scroll anchoring.
* `src/gpui.rs` + `src/gpui/` (gated on the default-on `gpui` feature using `gpui-kit 0.10`, `default-features = false`, optional `gpui-fast` feature): `GpuiShaper` over `WindowTextSystem::shape_line`, unified `RichDocumentElement` (`src/gpui/element.rs` + `src/gpui/element/paint.rs`) painting 6 strict z-order layers, and command-driven `RichDocumentView` (`src/gpui/view.rs` + `src/gpui/view/*.rs`, routing all interactions and editing operations through `RichCommand` + `dispatch_command` and `gpui_kit::actions!` scoped to `"RichDocument"`).
* `examples/docs_viewer.rs`: Interactive showcase application (`cargo run --example docs_viewer`).

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

* Always limit parallel Cargo jobs (for example, `cargo test --no-default-features -j 4`, `cargo check --all-targets -j 4`, `cargo check --all-targets --features gpui-fast -j 4`, `cargo build -j 4`).
* Keep `src/model` and `src/layout` strictly free of `gpui-kit` imports so `cargo test --no-default-features -j 4` compiles and passes without GPUI.
* Be conservative when writing tests: create tests only for critical architectural invariants or essential safety contracts. Avoid trivial, redundant, or fixture-heavy tests.

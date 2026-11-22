# `gpui-rich` Agent Rules

## Architecture Invariants

This workspace implements a non-paginated, Google Docs-style rich text renderer on GPUI designed to grow cleanly into a full editor. Preserve these crate boundaries:

* `crates/rich-model` (pure Rust, zero GPUI dependency): Hierarchical `BlockNode` tree with stable `BlockId` and `version`, `InlineContent` with normalized contiguous `StyleSpan`s and `\u{FFFC}` sentinels for inline atomic objects, `DocPos` + `Affinity` addressing, `DocumentSelection`, and `Transaction` / `Step` mutations with `History` undo/redo.
* `crates/rich-layout` (pure Rust, zero GPUI dependency): Generic over `GlyphBackend` and `TextShaper` so headless tests run via `MockShaper` without a GPU window. Owns mixed-baseline line composition, UAX #14 wrapping, glyph slicing, tab stops, justification, recursive table/blockquote layout, incremental `LayoutCache`, Fenwick tree `HeightIndex`, collapsible headings, and `(BlockId, local_y_offset)` scroll anchoring.
* `crates/rich-gpui` (`gpui 0.2.2`): `GpuiShaper` over `WindowTextSystem::shape_line`, unified `RichDocumentElement` painting 6 strict z-order layers, and `RichDocumentView` (scrolling, anchored zoom, selection, copy, and Phase 2 `EntityInputHandler` host).
* `examples/docs_viewer`: Interactive showcase application.

### Dependency Note (Linux + `gpui 0.2.2`)

`gpui 0.2.2` transitively pulls `xattr 0.2.3`, which references `libc::ENOATTR` on Linux. `Cargo.lock` pins `libc 0.2.178` for compatibility. Avoid running an unconstrained `cargo update` that bumps `libc` past `0.2.178` without verifying `cargo check -p xattr -j 4`.

## Rust Coding Guidelines

* Comments explain "why" for tricky or non-obvious code only. Avoid comments that summarize what code does.
* Prefer extending existing files over creating new ones. Add a new file only for a new logical component, and avoid many small files.
* Avoid `unwrap()` and `expect()` in library and application code. Propagate errors with `?` or handle them explicitly. Avoid panicking indexing or slicing by using `.get()` and checked access whenever bounds are uncertain.
* Never silently discard errors with `let _ =` on fallible operations:
  * Propagate with `?` when the caller should handle the error.
  * Log errors when intentional ignoring needs visibility.
  * Use `match` or `if let Err(...)` for explicit recovery.
* Never create `mod.rs`. Use `src/<module>.rs` naming with flat, descriptive module paths.
* Implement strictly what is requested. Avoid speculative features or unrequested additions.
* Use full words for variable names without abbreviations (for example, `queue`, `index`, `context`), except framework-idiomatic names such as GPUI's `cx`.
* Use `cargo add` to add crate dependencies.
* Prefer rustdocs (`///` and `//!`) over separate standalone documentation files.

## Testing & Build Strategy

* Always limit parallel Cargo jobs (for example, `cargo check --workspace -j 4`, `cargo build -j 4`, `cargo test -p rich-model -p rich-layout -j 4`).
* Be conservative when writing tests: create tests only for critical architectural invariants or essential safety contracts. Avoid trivial, redundant, or fixture-heavy tests.

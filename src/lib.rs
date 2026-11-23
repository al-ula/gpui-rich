//! Non-paginated, Google Docs-style rich text model, layout engine, and GPUI renderer.

#[cfg(feature = "gpui")]
pub mod gpui;
pub mod layout;
pub mod model;

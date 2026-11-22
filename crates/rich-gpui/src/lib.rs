//! GPUI rendering and interaction layer for non-paginated `gpui-rich` documents.

pub mod element;
pub mod shaper;
pub mod view;

pub use element::RichDocumentElement;
pub use shaper::{GpuiGlyphBackend, GpuiShaper, rgba_to_hsla, to_gpui_font};
pub use view::RichDocumentView;

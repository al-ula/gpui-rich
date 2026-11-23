//! Pure Rust text shaping traits, mixed-baseline line composition, and virtualized flow layout.

pub mod block;
pub mod flow;
pub mod geometry;
pub mod line;
pub mod shaper;

pub use block::*;
pub use flow::*;
pub use geometry::*;
pub use line::*;
pub use shaper::*;

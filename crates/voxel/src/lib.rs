//! Воксельный мир: хранение, занятость, связность.
//!
//! Крейт-основание мира. От него зависят `sim-core`, `worldgen`, `nav`
//! и `meshing`; сам он зависит только от `fx`.
//!
//! - [`store`] — раскладка чанка. Палитра на чанк (ADR-081).
//! - [`topo`] — связность породы: отделение кусков и связность пустот
//!   (ADR-055, ADR-057).

#![forbid(unsafe_code)]

pub mod bodies;
pub mod store;
pub mod topo;
pub mod world;

pub use topo::{Chunk, Components, Run};
pub use bodies::{Bodies, Body, BodyStats};
pub use world::{Edit, FlushStats, Generator, World};

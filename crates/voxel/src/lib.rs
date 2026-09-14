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
pub mod collapse;
pub mod fleet;
pub mod impact;
pub mod los;
pub mod roster;
pub mod shape;
pub mod store;
pub mod topo;
pub mod unit;
pub mod world;

pub use bodies::{Bodies, Body, BodyStats};
pub use fleet::{Fleet, FleetStats, UnitId};
pub use los::{line_of_sight, LosResult};
pub use topo::{Chunk, Components, Run};
pub use unit::{Part, Status, UnitBody};
pub use world::{Edit, FlushStats, Generator, World, NOBODY};

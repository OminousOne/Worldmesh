//! What every Worldmesh process shares: the messages (`protocol`), the bus and its encodings
//! (`bus`, `codec`, `subjects`), sure delivery over it (`reliable`), the world's shape (`config`), the clock (`time`), the numbers
//! each process reports (`stats`), and view merging (`views`).
//!
//! Games use these through the `worldmesh` crate.

pub mod bus;
pub mod codec;
pub mod config;
pub mod protocol;
pub mod reliable;
pub mod stats;
pub mod subjects;
pub mod time;
pub mod views;

pub use config::{ViewShape, WorldConfig};
pub use protocol::{EntityId, GroupId, SPAWNED_ID_START};

//! Worldmesh splits a large multiplayer world across many servers while players see one
//! seamless world.
pub mod bus;
pub mod codec;
pub mod config;
pub mod game;
pub mod group;
pub mod protocol;
pub mod snapshot;
pub mod stats;
pub mod subjects;
pub mod time;

pub use config::{ViewShape, WorldConfig};
pub use game::{Entity, Game, Incoming, Me, Nearby, Sensed, Spawn, Timed, To};
pub use protocol::{EntityId, GroupId};

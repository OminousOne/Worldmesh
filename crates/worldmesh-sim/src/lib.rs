//! Server Entity Groups: the game model and the processes that simulate entities. The only
//! place game code runs.
//!
//! A game is a [`World`] assembled from [`Plugin`]s: components (an entity's state),
//! messages (commands, actions, events) with their handlers, systems that run every tick,
//! and kinds of entity. Game code runs through [`Me`]: the entity being simulated.

pub mod component;
pub mod entity;
pub mod group;
pub mod me;
pub mod world;

#[cfg(any(test, feature = "test-game"))]
pub mod test_game;

pub use component::{AnyComponent, Bundle, Component, Message, Wire};
pub use entity::{Entity, Spawn};
pub use me::{Incoming, Me, Nearby, Query, Timed, To};
pub use world::{Category, Plugin, Rules, Stage, World};

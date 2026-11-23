//! An example game on Worldmesh: an isometric world with a walled town, swords, bows,
//! fireballs and bots that fight back.
//!
//! - [`game`]: the [`worldmesh::Game`] implementation, and the types players and entities
//!   exchange.
//! - [`bots`]: bot AI. It sees the world only through what Worldmesh gives it (its own group's
//!   entities exactly, everyone else from "what's near me?" answers).
//! - [`movement`], [`combat`], [`map`]: the rules, as plain functions. The client runs the same
//!   movement rules to predict its own player.

pub mod bots;
pub mod combat;
pub mod game;
pub mod map;
pub mod movement;

pub use game::Medieval;

/// Seed for the generated map. Every process generates the same map from it.
pub const WORLD_SEED: u64 = 0x5EED_CAFE;
pub const MAP_WIDTH: u32 = 64;
pub const MAP_HEIGHT: u32 = 64;

#[cfg(test)]
mod tests;

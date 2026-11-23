//! An example game on Worldmesh: an isometric world with a walled town, swords, bows,
//! fireballs and bots that fight back. Everything here is game code, built on the library as
//! three plugins:
//!
//! - [`movement::Movement`]: walking on the map. The client runs the same rules to predict
//!   its own player.
//! - [`combat::Combat`]: health, abilities, death and respawning.
//! - [`bots::Bots`]: bot AI. It sees the world only through what Worldmesh gives it (its own
//!   group's entities exactly, everyone else from "what's near me?" answers).
//!
//! [`world`] puts them together with the map and two kinds of unit, "hero" (players) and
//! "bot".

use std::sync::Arc;

use worldmesh::{Spawn, ViewShape, World, WorldConfig};

pub mod bots;
pub mod combat;
pub mod map;
pub mod movement;

/// Seed for the generated map. Every process generates the same map from it.
pub const WORLD_SEED: u64 = 0x5EED_CAFE;
pub const MAP_WIDTH: u32 = 64;
pub const MAP_HEIGHT: u32 = 64;

pub fn config() -> WorldConfig {
    WorldConfig {
        width: MAP_WIDTH,
        height: MAP_HEIGHT,
        chunk_size: 8,
        // A 2:1 isometric screen of about 2048 x 1152 px around the player.
        view: ViewShape::Iso {
            half_w: 32.0,
            half_h: 36.0,
        },
        input_substeps: movement::SUBSTEPS,
    }
}

/// The map every process generates.
pub fn map() -> Arc<map::MapData> {
    Arc::new(map::generate(WORLD_SEED, MAP_WIDTH, MAP_HEIGHT))
}

/// The whole game: the plugins, where players appear, and the map for every client.
pub fn world() -> World {
    let map = map();
    let mut world = World::new(config());
    world
        .plugin(movement::Movement { map: map.clone() })
        .plugin(combat::Combat { map: map.clone() })
        .plugin(bots::Bots { map: map.clone() })
        .welcome("map", &*map);
    world.join(move |player| {
        let (x, y) = map.spawn_point(player);
        Spawn::new("hero", x, y)
    });
    world
}

#[cfg(test)]
mod tests;

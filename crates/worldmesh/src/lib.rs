//! Worldmesh splits the work of a large multiplayer world across many servers while players
//! see one seamless world.
//!
//! A game is a [`World`] built from [`Plugin`]s. Each plugin registers:
//!
//! - **components**: an entity's state, one serde struct each ([`World::component`], or
//!   [`World::visible`] for those players and other entities can see),
//! - **messages**: commands (from a player to their own entity), actions (from one entity to
//!   another, an area or everyone) and events (for players' screens), each with any number
//!   of handlers,
//! - **systems**: code that runs for every entity every tick, in stages,
//! - **kinds**: named bundles of components to spawn entities from.
//!
//! Game code always runs as one entity, through [`Me`]: it changes only that entity, and
//! reaches others by sending actions and asking what's near. Worldmesh runs everything else:
//!
//! - **Server Entity Groups** ([`group`]) simulate entities. Each entity lives in exactly one
//!   group, and groups hand entities to each other live. This is the only place game code runs.
//! - **Spatial Layer Cells** ([`spatial`]) each own an area of the map. They answer "what's
//!   near me?", route actions aimed at an area, and build what each player can see.
//! - **The orchestrator** ([`orchestrator`]) decides which groups and cells are in use, who
//!   owns which area, and which group each new entity goes to.
//! - **Gateways** ([`gateway`]) hold players' WebSockets and merge their views.
//!
//! Every process talks over NATS. A game's binary runs any of the four roles:
//!
//! ```ignore
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let mut world = World::new(WorldConfig::default());
//!     world.plugin(Movement).plugin(Combat);
//!     worldmesh::run(world).await
//! }
//! ```
//!
//! See the guides at the repository's `docs/` site, and `examples/medieval` for a full game.

pub mod cli;

pub use worldmesh_core::{bus, codec, config, protocol, stats, subjects, time, views};
pub use worldmesh_gateway as gateway;
pub use worldmesh_orchestrator as orchestrator;
pub use worldmesh_sim::{component, entity, group, me, world};
pub use worldmesh_spatial as spatial;
pub use worldmesh_testing as testing;

pub use cli::{run, run_with, Cli, Role};
pub use config::{ViewShape, WorldConfig};
pub use gateway::GatewayOptions;
pub use protocol::{EntityId, GroupId, SPAWNED_ID_START};
pub use worldmesh_sim::{
    AnyComponent, Bundle, Category, Component, Entity, Incoming, Me, Message, Nearby, Plugin, Query, Rules, Spawn, Stage,
    Timed, To, Wire, World,
};

/// `#[derive(Component)]` and `#[derive(Message)]`. Name one with
/// `#[worldmesh(name = "...")]`; by default it's the type's name in snake_case.
pub use worldmesh_macros::{Component, Message};

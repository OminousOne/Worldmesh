//! Worldmesh splits the work of a large multiplayer world across many servers while players
//! see one seamless world.
//!
//! A game implements [`Game`]: what an entity holds, what players send, what entities do to
//! each other, and how one game tick goes for one entity. Worldmesh runs everything else:
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
//!     worldmesh::run(MyGame::new()).await
//! }
//! ```
//!
//! See the guides at the repository's `docs/` site, and `examples/medieval` for a full game.

pub mod bus;
pub mod cli;
pub mod codec;
pub mod config;
pub mod game;
pub mod gateway;
pub mod group;
pub mod orchestrator;
pub mod protocol;
pub mod snapshot;
pub mod spatial;
pub mod stats;
pub mod subjects;
pub mod testing;
pub mod time;

pub use cli::{run, run_with, Cli, Role};
pub use config::{ViewShape, WorldConfig};
pub use game::{Entity, Game, Incoming, Me, Nearby, Sensed, Spawn, Timed, To, Wire};
pub use gateway::GatewayOptions;
pub use protocol::{EntityId, GroupId, SPAWNED_ID_START};

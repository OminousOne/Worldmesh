//! One binary, four roles. A game's `main` hands its [`Game`] to [`run`], and the same binary
//! then runs as whichever process the command line asks for:
//!
//! ```text
//! my-game orchestrator --groups 2 --spatial 2
//! my-game group
//! my-game spatial
//! my-game gateway --listen 0.0.0.0:8080
//! ```
//!
//! Every option can also come from an environment variable (shown in `--help`), which suits
//! containers: run any number of identical `group` or `spatial` containers and they pick their
//! own ids.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::game::Game;
use crate::gateway::GatewayOptions;
use crate::protocol::GroupId;
use crate::{gateway, group, orchestrator, spatial, time};

/// The command line every Worldmesh process takes.
#[derive(Debug, Clone, Parser)]
#[command(about = "A Worldmesh world process")]
pub struct Cli {
    /// NATS bus address.
    #[arg(long, env = "NATS_URL", default_value = "nats://127.0.0.1:4222")]
    pub nats_url: String,

    /// Simulation ticks per second. Every process in the world must use the same value.
    #[arg(long, env = "TICK_HZ", default_value_t = time::DEFAULT_TICK_HZ)]
    pub tick_hz: u32,

    #[command(subcommand)]
    pub role: Role,
}

/// Which process to run.
#[derive(Debug, Clone, Subcommand)]
pub enum Role {
    /// Run Server Entity Groups: simulate the entities the orchestrator gives them (the only
    /// role that runs the game's simulation).
    Group {
        /// Id of the first group this process runs; by default it picks a random one.
        #[arg(long, env = "GROUP_ID")]
        group_id: Option<GroupId>,
    },
    /// Run a Spatial Layer Cell: answers "what's near here?" for the chunks it's given.
    Spatial {
        /// Fixed id; by default each process picks a random one.
        #[arg(long, env = "SPATIAL_ID")]
        spatial_id: Option<GroupId>,
    },
    /// Run a gateway: holds player WebSockets and puts each player's view together.
    Gateway {
        #[arg(long, env = "LISTEN", default_value = "0.0.0.0:8080")]
        listen: String,
        /// NATS monitoring endpoint (host:port) to include NATS in the debug graphs.
        #[arg(long, env = "NATS_MONITOR")]
        nats_monitor: Option<String>,
        /// Accept debug panel commands (scaling, fake lag) from players. Development only.
        #[arg(long, env = "ALLOW_DEBUG")]
        allow_debug: bool,
    },
    /// Run the orchestrator: picks the groups and Spatial Layer Cells in use and places entities.
    Orchestrator {
        /// How many Server Entity Groups to use (more are started on group processes as needed).
        #[arg(long, env = "GROUPS", default_value_t = 2)]
        groups: u32,
        /// How many Spatial Layer Cells to use (capped at how many are running).
        #[arg(long, env = "SPATIAL", default_value_t = 2)]
        spatial: u32,
    },
}

/// Run the process the command line asks for, with this game. Sets up logging (`RUST_LOG`,
/// default `info`) unless the program already has a tracing subscriber.
pub async fn run<G: Game>(game: G) -> Result<()> {
    run_with(game, Cli::parse()).await
}

/// Like [`run`], with a command line you parsed yourself (say, as part of a bigger CLI).
pub async fn run_with<G: Game>(game: G, cli: Cli) -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .try_init();

    let tick_ms = time::set_tick_hz(cli.tick_hz);
    info!(tick_hz = cli.tick_hz, tick_ms, "tick rate");
    let nats = connect_with_retry(&cli.nats_url).await;
    let config = game.config();
    let game = Arc::new(game);

    match cli.role {
        Role::Group { group_id } => group::run(nats, game, group_id).await,
        Role::Spatial { spatial_id } => spatial::run(nats, config, spatial_id).await,
        Role::Gateway { listen, nats_monitor, allow_debug } => {
            gateway::run(nats, game, GatewayOptions { listen, nats_monitor, allow_debug }).await
        }
        Role::Orchestrator { groups, spatial } => orchestrator::run(nats, config, groups, spatial).await,
    }
}

/// Connect to NATS, retrying every second: the bus may still be starting (docker compose
/// doesn't wait for readiness).
pub async fn connect_with_retry(url: &str) -> async_nats::Client {
    loop {
        match async_nats::connect(url).await {
            Ok(client) => {
                info!(url, "connected to bus");
                return client;
            }
            Err(err) => {
                warn!(url, %err, "bus not reachable yet, retrying");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_parses_every_role() {
        let cli = Cli::try_parse_from(["game", "--tick-hz", "20", "orchestrator", "--groups", "3"]).unwrap();
        assert_eq!(cli.tick_hz, 20);
        assert!(matches!(cli.role, Role::Orchestrator { groups: 3, spatial: 2 }));
        let cli = Cli::try_parse_from(["game", "gateway", "--allow-debug"]).unwrap();
        assert!(matches!(cli.role, Role::Gateway { allow_debug: true, .. }));
        assert!(Cli::try_parse_from(["game", "group", "--group-id", "7"]).is_ok());
        assert!(Cli::try_parse_from(["game", "spatial"]).is_ok());
        assert!(Cli::try_parse_from(["game"]).is_err(), "a role is required");
    }
}

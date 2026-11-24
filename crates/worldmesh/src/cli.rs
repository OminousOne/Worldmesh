//! One binary, four roles. A game's `main` hands its [`World`] to [`run`], and the same binary
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

use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use worldmesh_core::protocol::GroupId;
use worldmesh_core::time;
use worldmesh_gateway::GatewayOptions;
use worldmesh_sim::group;
use worldmesh_sim::world::World;
use {
    worldmesh_gateway as gateway, worldmesh_orchestrator as orchestrator,
    worldmesh_spatial as spatial,
};

/// The command line every Worldmesh process takes.
#[derive(Debug, Clone, Parser)]
#[command(about = "A Worldmesh world process")]
pub struct Cli {
    /// NATS bus address. For a NATS cluster, list several, separated by commas: the process
    /// connects to one of them at random (spreading processes over the cluster), and moves to
    /// another if its server stops.
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
    /// Run a Spatial Layer Cell: tracks every entity in the chunks it's given, builds players'
    /// views of them, routes area actions and answers spatial queries.
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

/// Run the process the command line asks for, with this world. Sets up logging (`RUST_LOG`,
/// default `info`) unless the program already has a tracing subscriber.
pub async fn run(world: World) -> Result<()> {
    run_with(world, Cli::parse()).await
}

/// Like [`run`], with a command line you parsed yourself (say, as part of a bigger CLI).
pub async fn run_with(world: World, cli: Cli) -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .try_init();

    let tick_ms = time::set_tick_hz(cli.tick_hz);
    info!(tick_hz = cli.tick_hz, tick_ms, "tick rate");
    let nats = connect_with_retry(&cli.nats_url).await;
    // Checked (and frozen) the same way in every role, so a mistake shows up everywhere.
    let rules = world.build();
    let config = rules.config;

    match cli.role {
        Role::Group { group_id } => group::run(nats, rules, group_id).await,
        Role::Spatial { spatial_id } => spatial::run(nats, config, spatial_id).await,
        Role::Gateway {
            listen,
            nats_monitor,
            allow_debug,
        } => {
            gateway::run(
                nats,
                rules,
                GatewayOptions {
                    listen,
                    nats_monitor,
                    allow_debug,
                },
            )
            .await
        }
        Role::Orchestrator { groups, spatial } => {
            orchestrator::run(nats, config, groups, spatial).await
        }
    }
}

/// Connect to NATS, retrying every second: the bus may still be starting (docker compose
/// doesn't wait for readiness). `url` may list several servers of a cluster, separated by
/// commas; the client also learns the cluster's other servers from the one it reaches.
pub async fn connect_with_retry(url: &str) -> async_nats::Client {
    let servers = match parse_servers(url) {
        Ok(servers) => servers,
        Err(err) => {
            warn!(url, %err, "can't read the NATS address; using it as it is");
            vec![]
        }
    };
    loop {
        let attempt = if servers.is_empty() {
            async_nats::connect(url).await
        } else {
            async_nats::connect(servers.clone()).await
        };
        match attempt {
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

/// `nats://a:4222, nats://b:4222` as a list of servers.
fn parse_servers(url: &str) -> Result<Vec<async_nats::ServerAddr>, String> {
    url.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().map_err(|e: std::io::Error| format!("{s}: {e}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_parses_every_role() {
        let cli = Cli::try_parse_from(["game", "--tick-hz", "20", "orchestrator", "--groups", "3"])
            .unwrap();
        assert_eq!(cli.tick_hz, 20);
        assert!(matches!(
            cli.role,
            Role::Orchestrator {
                groups: 3,
                spatial: 2
            }
        ));
        let cli = Cli::try_parse_from(["game", "gateway", "--allow-debug"]).unwrap();
        assert!(matches!(
            cli.role,
            Role::Gateway {
                allow_debug: true,
                ..
            }
        ));
        assert!(Cli::try_parse_from(["game", "group", "--group-id", "7"]).is_ok());
        assert!(Cli::try_parse_from(["game", "spatial"]).is_ok());
        assert!(Cli::try_parse_from(["game"]).is_err(), "a role is required");
    }

    #[test]
    fn a_nats_address_can_list_a_clusters_servers() {
        assert_eq!(parse_servers("nats://127.0.0.1:4222").unwrap().len(), 1);
        let servers = parse_servers("nats://a:4222, nats://b:4222,nats://c:4222").unwrap();
        let hosts: Vec<&str> = servers.iter().map(|s| s.host()).collect();
        assert_eq!(hosts, vec!["a", "b", "c"]);
    }
}

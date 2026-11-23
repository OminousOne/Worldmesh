//! One binary for every process of the world: `medieval orchestrator`, `medieval group`,
//! `medieval spatial`, `medieval gateway` (see `--help`).

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    worldmesh::run(medieval::world()).await
}

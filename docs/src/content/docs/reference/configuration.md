---
title: Configuration and CLI
description: WorldConfig, the command line every process takes, and the environment variables behind it.
---

## `WorldConfig`

Passed to `World::new`. It must be the same in every process, which it is, since it's your code.

```rust
pub struct WorldConfig {
    /// World size in tiles. Positions are f32 in 0..width and 0..height.
    pub width: u32,
    pub height: u32,
    /// Chunk side in tiles: the unit of area a Spatial Layer Cell owns.
    pub chunk_size: u32,
    /// What a player can see around them.
    pub view: ViewShape,
    /// How finely an input's timing within a tick is kept.
    pub input_substeps: u8,
}

pub enum ViewShape {
    /// |dx| <= half_w and |dy| <= half_h.
    Rect { half_w: f32, half_h: f32 },
    /// A screen rectangle in 2:1 isometric:
    /// |dx - dy| <= half_w and |dx + dy| <= half_h.
    Iso { half_w: f32, half_h: f32 },
}
```

`WorldConfig::default()` is 64 × 64 tiles, 8-tile chunks, a 20 × 12 rectangle view and 16 sub-steps.

## Running a process

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    worldmesh::run(my_world()).await
}
```

`worldmesh::run`:

1. parses the command line,
2. sets up logging (`RUST_LOG`, default `info`), unless you set up `tracing` yourself,
3. builds the world, so a mistake in it, like two components with the same name, stops every role at start,
4. connects to NATS, retrying every second until it's there,
5. runs the role.

`worldmesh::run_with(world, cli)` takes a `worldmesh::Cli` you parsed yourself, say as a subcommand of a bigger CLI.

## Command line

```text
my-game [--nats-url URL] [--tick-hz N] <ROLE> [OPTIONS]
```

| Option | Environment | Default | What it sets |
| --- | --- | --- | --- |
| `--nats-url` | `NATS_URL` | `nats://127.0.0.1:4222` | The bus |
| `--tick-hz` | `TICK_HZ` | `10` | Ticks per second; the same in every process |

### `orchestrator`

| Option | Environment | Default | What it sets |
| --- | --- | --- | --- |
| `--groups` | `GROUPS` | `2` | Server Entity Groups in use at start |
| `--spatial` | `SPATIAL` | `2` | Spatial Layer Cells in use at start, up to how many run |

### `group`

| Option | Environment | Default | What it sets |
| --- | --- | --- | --- |
| `--group-id` | `GROUP_ID` | random | Id of the first group this process runs |

### `spatial`

| Option | Environment | Default | What it sets |
| --- | --- | --- | --- |
| `--spatial-id` | `SPATIAL_ID` | random | This cell's id |

### `gateway`

| Option | Environment | Default | What it sets |
| --- | --- | --- | --- |
| `--listen` | `LISTEN` | `0.0.0.0:8080` | Players connect to `/ws`; `/health` answers `ok` |
| `--nats-monitor` | `NATS_MONITOR` | none | NATS monitoring address (`host:8222`), to show NATS in the metrics |
| `--allow-debug` | `ALLOW_DEBUG` | off | Accept debug panel commands (scaling, fake lag) from players. Development only. |

## Limits

| Limit | Value |
| --- | --- |
| Groups in use | 1,000 |
| Fake lag per group | 5,000 ms |
| Input scheduled ahead | 5 s. Later ones are pulled in. |
| Forwarding after a handoff | 1 s |
| `sensed()` answers kept | 1 s |
| `position_at` trail | 20 ticks |
| Entities per answers message | 4,000. More are split into several messages. |
| A process is dead after | 3 s without heartbeats |

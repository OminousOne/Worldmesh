---
title: Configuration and CLI
description: WorldConfig, the command line every process takes, and the environment variables behind it.
---

## `WorldConfig`

Returned by `Game::config`. Must be the same in every process.

```rust
pub struct WorldConfig {
    pub width: u32,          // world size in tiles; positions are f32 in 0..width, 0..height
    pub height: u32,
    pub chunk_size: u32,     // chunk side in tiles: the unit of area a Spatial Layer Cell owns
    pub view: ViewShape,     // what a player can see around them
    pub input_substeps: u8,  // how finely an input's timing within a tick is kept
}

pub enum ViewShape {
    Rect { half_w: f32, half_h: f32 }, // |dx| <= half_w, |dy| <= half_h
    Iso { half_w: f32, half_h: f32 },  // a screen rectangle in 2:1 isometric: |dx - dy| <= half_w, |dx + dy| <= half_h
}
```

The default is 64 × 64 tiles, 8-tile chunks, a 20 × 12 rectangle view and 16 sub-steps.

## Running a process

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    worldmesh::run(MyGame::new()).await
}
```

`worldmesh::run` parses the command line, sets up logging (`RUST_LOG`, default `info`, unless you set up `tracing` yourself), connects to NATS (retrying every second until it's there) and runs the role. `worldmesh::run_with(game, cli)` takes a `worldmesh::Cli` you parsed yourself, say as a subcommand of a bigger CLI.

## Command line

```text
my-game [--nats-url URL] [--tick-hz N] <ROLE> [OPTIONS]
```

| Option | Environment | Default | |
| --- | --- | --- | --- |
| `--nats-url` | `NATS_URL` | `nats://127.0.0.1:4222` | The bus |
| `--tick-hz` | `TICK_HZ` | `10` | Ticks per second; the same in every process |

### `orchestrator`

| Option | Environment | Default | |
| --- | --- | --- | --- |
| `--groups` | `GROUPS` | `2` | Server Entity Groups in use at start |
| `--spatial` | `SPATIAL` | `2` | Spatial Layer Cells in use at start (capped at how many run) |

### `group`

| Option | Environment | Default | |
| --- | --- | --- | --- |
| `--group-id` | `GROUP_ID` | random | Id of the first group this process runs |

### `spatial`

| Option | Environment | Default | |
| --- | --- | --- | --- |
| `--spatial-id` | `SPATIAL_ID` | random | This cell's id |

### `gateway`

| Option | Environment | Default | |
| --- | --- | --- | --- |
| `--listen` | `LISTEN` | `0.0.0.0:8080` | Players connect to `/ws`; `/health` answers `ok` |
| `--nats-monitor` | `NATS_MONITOR` | none | NATS monitoring address (`host:8222`), to show NATS in the metrics |
| `--allow-debug` | `ALLOW_DEBUG` | off | Accept debug panel commands (scaling, fake lag) from players. Development only. |

## Limits

| | |
| --- | --- |
| Groups in use | 1,000 |
| Fake lag per group | 5,000 ms |
| Input scheduled ahead | 5 s (later ones are pulled in) |
| Forwarding after a handoff | 1 s |
| `sensed()` answers kept | 1 s |
| `position_at` trail | 20 ticks |
| Entities per answers message | 4,000 (more are split into several) |
| A process is dead after | 3 s without heartbeats |

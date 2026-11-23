---
title: Running a world
description: The processes to run, how they find each other, and what to watch.
---

A world is NATS plus four kinds of process, all from your game's one binary.

| Process | How many | Notes |
| --- | --- | --- |
| NATS | 1 (or a cluster) | Port 4222. Monitoring on 8222 is worth turning on (`-m 8222`). |
| Orchestrator | Exactly 1 | Starts with `--groups` and `--spatial` in use. |
| Group processes | Any | Each runs as many Server Entity Groups as the orchestrator asks for. |
| Spatial Layer Cells | Any | One cell per process. Run a few more than you use, to scale into. |
| Gateways | 1 or more | Each holds its players' WebSockets; put them behind a load balancer. |

Group and cell processes pick their own ids, so you can run any number of identical containers with no configuration. Every process needs the same `NATS_URL` and `TICK_HZ`.

```sh
export NATS_URL=nats://nats:4222 TICK_HZ=10
my-game orchestrator --groups 4 --spatial 4
my-game group            # as many as you like
my-game spatial          # as many as you like
my-game gateway --listen 0.0.0.0:8080 --nats-monitor nats:8222
```

`examples/medieval/docker-compose.yml` is a complete setup to start from, and `examples/medieval/Dockerfile` builds one image for every role. All options are in [Configuration and CLI](/Worldmesh/reference/configuration/).

## Production notes

- **Don't pass `--allow-debug`** to public gateways: it lets any connected player rescale the world and add fake lag.
- **Clocks**: every process derives the current tick from its system clock. Keep hosts in sync with NTP; a host a few ms off just delivers a few ms early or late, one a whole tick off makes its groups lag.
- **Logs**: `RUST_LOG=info` (the default) logs registrations, assignments, handoffs and warnings; `RUST_LOG=worldmesh=debug` adds per-tick detail.
- **Health**: gateways answer `GET /health` with `ok`.
- **Network**: groups and cells only talk to NATS; only gateways need to be reachable from outside.

## Watching it

Every process publishes its numbers once a second (CPU, memory, bus and WebSocket traffic, entities, tick time and load, actions, handoffs, questions, views), and gateways pass them on to players with the debug panel open. The metrics view shows them over the last minute. The number to watch is each group's and cell's **load**: the share of each tick it spends working. Past about 50%, add groups (or cells).

## What's not there yet

No authentication (any WebSocket client can join), no persistence (entities live in memory), and scaling is manual. See the repository README for the roadmap.

---
title: Running a world
description: The processes to run, how they find each other, and what to watch.
---

A world is NATS plus four kinds of process, all from your game's one binary:

| Process | How many | Notes |
| --- | --- | --- |
| NATS | 1, or a cluster of any size | Port 4222. Turn on monitoring (`-m 8222`): the gateway can show it in the metrics. See [Scaling the bus](#scaling-the-bus). |
| Orchestrator | Exactly 1 | Starts with `--groups` and `--spatial` in use |
| Group processes | Any number | Each runs as many Server Entity Groups as the orchestrator asks for |
| Spatial Layer Cells | Any number | One cell per process. Run a few more than you use, to scale into. |
| Gateways | Any number, down to one per player | Each holds its players' WebSockets. Put them behind a load balancer. |

Group processes, cells and gateways pick their own ids, so you can run any number of identical containers with no configuration. Every process needs the same `NATS_URL` and `TICK_HZ`.

```sh
export NATS_URL=nats://nats:4222 TICK_HZ=10
my-game orchestrator --groups 4 --spatial 4
my-game group      # as many as you like
my-game spatial    # as many as you like
my-game gateway --listen 0.0.0.0:8080 --nats-monitor nats:8222
```

`examples/medieval/docker-compose.yml` is a complete setup to start from, and `examples/medieval/Dockerfile` builds one image for every role. Every option is in [Configuration and CLI](/Worldmesh/reference/configuration/).

## Scaling the bus

The bus scales horizontally. Run NATS as a [cluster](https://docs.nats.io/running-a-nats-service/configuration/clustering) of as many servers as you need, and give every process the cluster's addresses:

```sh
export NATS_URL=nats://nats-1:4222,nats://nats-2:4222,nats://nats-3:4222
```

- **Processes spread out.** Each connects to one of the listed servers at random, and learns the rest of the cluster from it.
- **A server can stop.** Its processes move to another server and carry on. We test this by stopping one of three servers mid-game: every process moves over and the game carries on.
- **Traffic is split by subject.** Almost everything Worldmesh sends is addressed to one process: one entity, one group, one cell or one gateway. NATS carries each message only to the servers with a subscriber for it, so each server handles only its share.
- **Regions.** For a world served from several places, NATS [superclusters](https://docs.nats.io/running-a-nats-service/configuration/gateways) join clusters, and send across only what the other side subscribes to.

A few subjects reach every process of a role: assignments, where players are looking, and actions sent to everyone. They're small, or rare, or both.

## In production

:::danger
Never pass `--allow-debug` to a public gateway. It lets any connected player rescale the world and add fake lag.
:::

- **Clocks**: every process works out the current tick from its system clock, so keep hosts in sync with NTP. A host a few ms off delivers a few ms early or late; one a whole tick off makes its groups lag.
- **Network**: groups and cells only talk to NATS. Only gateways need to be reachable from outside.
- **Health**: gateways answer `GET /health` with `ok`.
- **Logs**: `RUST_LOG=info`, the default, logs registrations, assignments, handoffs and warnings. `RUST_LOG=worldmesh=debug` adds per-tick detail.

## What to watch

Every process publishes its numbers once a second: CPU, memory, bus and WebSocket traffic, entities, tick time and load, actions, handoffs, questions and views. Gateways pass them on to players with the debug panel open, and its metrics view shows the last minute.

The number to watch is each group's and cell's **load**: the share of each tick it spends working. Past about 50%, add groups or cells.

## Not there yet

There's no authentication (any WebSocket client can join), no persistence (entities live in memory), and scaling is manual. The [issues](https://github.com/OminousOne/Worldmesh/issues) track what's next.

---
title: Scaling and handoffs
description: Adding and removing groups and cells while the world runs, and how entities move between groups.
---

## Two dials

- **Groups in use**: how many Server Entity Groups share the entities. This is the simulation's capacity. Group processes start as many groups as they're asked for, so this has no fixed limit (a safety cap of 1,000).
- **Cells in use**: how many Spatial Layer Cells share the map. This is the capacity for questions, area actions and views. Capped at how many cell processes are running.

Today both are set by hand: the orchestrator's `--groups` and `--spatial` at start, then live from the [debug panel](/Worldmesh/guides/debug-panel/). Automatic scaling (from the load numbers each process already reports) is on the roadmap.

## Changing the number of groups

Raising it starts new groups on the group processes running the fewest. Once all are up, the orchestrator re-splits every entity (by count, neighbours together, as few moving as possible: adding a fourth group to three moves about a quarter of the entities) and tells each group which entities to release.

Lowering it hands every entity of the groups taken out of use to the rest, and stops those groups once they've been empty for a few seconds.

## A handoff

When a group releases an entity:

1. It finishes the tick (the entity is in that tick's report, so it never vanishes from view).
2. It sends the entity's full record to the new group's inbox: your `Data`, plus Worldmesh's bookkeeping (the last tick it was simulated, its recent positions for `position_at`, which actions it already got, and any of its player's inputs not yet due).
3. For a second, it keeps listening on the entity's subject and forwards anything that reaches it to the new group.

The new group starts simulating it from the next tick (never the same tick twice: it skips any tick the old group already ran). The player sees nothing: no pause, no lost input, no action applied twice.

## Changing the number of cells

Cells are re-split along the Hilbert curve of chunks. Nothing is handed off: groups send their next reports to the new owners, and cells rebuild everything from those reports every tick anyway.

## When a process dies

- A **group** that stops sending heartbeats for 3 seconds is dropped. Its entities are gone with it; players are re-joined by the gateway (a player missing from their own view for 3 seconds is placed again).
- A **cell** that stops is dropped and its area re-split among the rest.
- **Gateways** are stateless apart from their connections: players reconnect.

There's no persistence yet, so a dead group's bots and monsters don't come back. That's on the roadmap too.

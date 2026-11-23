---
title: Wire protocol
description: The messages between browsers and gateways, as JSON and as compact binary snapshots.
---

`@worldmesh/client` speaks this for you. It's documented for writing clients in other languages, and for reading the network tab.

Players connect to the gateway's `/ws`. The browser sends JSON text messages; the gateway sends binary messages that are either JSON (starting with `{`) or a compact snapshot (starting with byte 1 or 2).

## Browser to gateway

Each message has a `t` field.

```json
{ "t": "input", "tick": 1234, "sub": 5, "input": { "dx": 1, "dy": 0 } }
{ "t": "command", "command": { "k": "use", "ability": "bow", "target": { "entity": 7 } } }
{ "t": "settings", "compact": true, "debug": false }
{ "t": "debug", "set_groups": 4, "set_spatial": 3, "regroup": true, "binary_bus": true, "set_lag": [12345, 200] }
```

- `input`: your game's `Input`, to take effect at sub-step `sub` of tick `tick` (tick 0: as soon as possible). Dropped unless it parses as your `Input`.
- `command`: your game's `Command`, for your own entity. Dropped unless it parses as your `Command`.
- `settings`: compact binary snapshots or JSON; whether to include debug traffic. Until the first `settings`, snapshots are JSON without debug traffic.
- `debug`: every field optional. Refused unless the gateway runs with `--allow-debug`.

## Gateway to browser

```json
{ "t": "welcome", "id": 3, "tick_ms": 100, "pos_scale": 1023.0,
  "world": { "width": 64, "height": 64, "chunk_size": 8, "view": { "shape": "iso", "half_w": 32, "half_h": 36 }, "input_substeps": 16 },
  "game": { "...": "your Welcome" } }
{ "t": "snapshot", "tick": 1234, "entities": [ { "id": 3, "x": 31.5, "y": 30.5, "group": 5, "spatial": 32769, "player": true, "view": { "...": "your View" } } ],
  "events": [ { "...": "your Event" } ], "senses": [], "bus": [], "streams": [] }
{ "t": "ownership", "version": 4, "groups": [5, 9], "spatial": [32769, 32770], "owners": [32769, "..."], "chunk_size": 8, "chunks_x": 8, "chunks_y": 8,
  "spatial_running": 3, "groups_running": 2, "group_hosts": 2, "binary_bus": true, "group_lag": [] }
{ "t": "input_ack", "tick": 1234, "sub": 5, "early_ms": 42 }
{ "t": "stats", "processes": [ { "role": "group", "id": 5, "players": 750, "load_pct": 9.4, "...": "..." } ] }
```

- `snapshot`: once per tick, what you can see. Your entity is always in it.
- `ownership`: on connect and whenever the assignment changes.
- `input_ack`: for each input: how many ms before its moment it reached the gateway (negative: late). Drives the prediction lead.
- `stats`: once a second, every process's numbers.

Your types appear as serde writes them to JSON.

## Compact snapshots

Binary, little endian, and only what changed since the last snapshot sent to you. The decoder keeps every entity in view and applies each snapshot to them.

```text
u8  kind                 1 = snapshot, 2 = first after a reset (forget everything first)
u64 tick
u16 entered, then per entity: u32 id · u8 flags (bit 0: player) · fields (all of them)
u16 updated, then per entity: u32 id · fields (what changed)
u16 left,    then per entity: u32 id
u16 events,  then per event:  u32 length · MessagePack (your Event)
u32 debug length · JSON {"senses":[…],"bus":[…],"streams":[…]} (empty parts left out)

fields: u8 mask, then in this order whatever the mask has:
  1 POS      your own entity: f32 x · f32 y; others: u16 x · u16 y (tiles × pos_scale)
  2 GROUP    u16
  4 SPATIAL  u16
  8 VIEW     u32 length · MessagePack (your View, with named fields)
```

An entity out of view for 5 ticks is listed in `left` (so one that misses a tick mid-handoff doesn't flicker). Your own position is exact because prediction compares it bit for bit; everyone else's is rounded to `1 / pos_scale` tiles.

The format is checked from both ends: a Rust test writes snapshots to `packages/client/test/fixtures/snapshots.json`, and the client's tests decode them.

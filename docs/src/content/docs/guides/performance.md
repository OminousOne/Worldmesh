---
title: Performance
description: Where the traffic goes, what it costs, and the knobs that change it.
---

Measured with the medieval example: 3,000 aggressive bots fighting on a 64 × 64 map, 4 groups, 3 cells, 10 ticks per second, release builds, every process on one 2-core machine.

| | |
| --- | --- |
| Entities per group | 750 / 750 / 750 / 751 |
| Busiest group | about 13% of each tick |
| Busiest cell | about 25% of each tick |
| Snapshots | 10 per second, about 1,800 entities in view |
| Bus, total | about 45 MB/s |
| Of which "what's near me?" answers | about 41 MB/s |
| Reports (groups to cells) | about 2.0 MB/s |
| Views (cells to gateway) | about 1.3 MB/s |
| Actions, inputs, handoffs | about 0.1 MB/s |

## Where the bytes go

Almost all of it is answers to "what's near me?". Every aggressive bot asks every 200 ms, and an answer lists everyone in the circle. In a crowd of thousands that's thousands of entities per answer.

The fix is in your game, because only your game knows what an entity needs to know:

- Ask only for the components you'll read: `Query::within(r).with::<Dead>()`. The medieval bots only need to know who's dead; asking for that alone (instead of every visible component) took the bus from about 74 to 45 MB/s in the fight above.
- Ask about fewer entities: `.nearest(n)` gets only the closest few, `.target(id)` always includes the one you care about.
- Ask less often, or only when something changed (a bot that was just hit already knows where the attacker is).
- Make visible only what others need. Visible components are in every answer that asks for them and every player's view. Field names count: components are MessagePack with named fields, so `{"until":1234}` is 6 bytes of name for 3 of value.

## Players

What a player receives depends on how much is in their view and how much of it changes. A compact snapshot sends an entity standing still for nothing, a moving one for about 9 bytes, and a component only when it changes. A player in the middle of the 3,000-bot fight above, with about 1,800 entities in view and nearly all of them moving and fighting, receives about 330 KB/s (2.9 MB/s as JSON). With a few dozen in view it's 10 to 25 KB/s.

## Knobs

| Knob | Effect |
| --- | --- |
| More groups | Splits the simulation; each group's load drops roughly in proportion. Adds a little report traffic. |
| More cells | Splits questions and view building. Questions near a border go to two cells. |
| `chunk_size` | Smaller chunks split areas more evenly but cost more bookkeeping. 8 tiles is a good start. |
| `TICK_HZ` | Higher is smoother and more responsive, and multiplies every per-tick cost. |
| View size | Bigger views mean bigger snapshots and more cells per view. |
| Compact snapshots | On by default; JSON is about 9 times bigger in a busy fight. |
| MessagePack bus | On by default; JSON is about 3 times bigger (134 MB/s instead of 44 in the fight above). |

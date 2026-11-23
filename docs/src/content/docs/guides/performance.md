---
title: Performance
description: Where the traffic goes, what it costs, and the knobs that change it.
---

Measured with the medieval example: 3,000 aggressive bots fighting on a 64 × 64 map, 4 groups, 3 cells, 10 ticks per second, release builds, all on one machine.

| | |
| --- | --- |
| Entities per group | 750 / 750 / 751 / 750 |
| Busiest group | 9% of each tick |
| Busiest cell | 15% of each tick |
| Snapshots | 10 per second, about 1,800 entities in view |
| Bus, total | about 73 MB/s |
| Of which "what's near me?" answers | about 70 MB/s |
| Reports (groups to cells) | about 2.3 MB/s |
| Views (cells to gateway) | about 1.5 MB/s |
| Actions, inputs, handoffs | about 0.1 MB/s |

## Where the bytes go

Almost all of it is answers to "what's near me?". Every aggressive bot asks every 200 ms, and an answer lists everyone in the circle with their view. In a crowd of thousands that's thousands of entities per answer.

The fix is in your game, because only your game knows what an entity needs to know:

- Ask with `query_with(r, nearest, target)` to get only the nearest few plus a target.
- Ask less often, or only when something changed (a bot that was just hit already knows where the attacker is).
- Keep `View` small. It's in every answer and every player's view. Field names count: views are MessagePack with named fields, so `{"hp":100}` is 4 bytes of name for 1 of value.

## Players

What a player receives depends on how much is in their view and how much of it changes. A compact snapshot sends an entity standing still for nothing, a moving one for about 9 bytes, and its view only when it changes. A player in the middle of the 3,000-bot fight above, with about 1,800 entities in view and nearly all of them moving and fighting, receives about 430 KB/s (3.1 MB/s as JSON). With a few dozen in view it's 10 to 25 KB/s.

## Knobs

| Knob | Effect |
| --- | --- |
| More groups | Splits the simulation; each group's load drops roughly in proportion. Adds a little report traffic. |
| More cells | Splits questions and view building. Questions near a border go to two cells. |
| `chunk_size` | Smaller chunks split areas more evenly but cost more bookkeeping. 8 tiles is a good start. |
| `TICK_HZ` | Higher is smoother and more responsive, and multiplies every per-tick cost. |
| View size | Bigger views mean bigger snapshots and more cells per view. |
| Compact snapshots | On by default; JSON is about 7 times bigger in a busy fight. |
| MessagePack bus | On by default; JSON is about 2.5 times bigger (185 MB/s instead of 72 in the fight above). |

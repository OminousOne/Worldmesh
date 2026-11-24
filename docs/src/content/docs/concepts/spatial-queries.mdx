---
title: '"What''s near me?"'
description: How entities see entities in other groups, and how to keep answers small.
---

An entity knows the other entities in its own group exactly: `me.local()` reads them directly. Entities in other groups it only knows by asking the spatial layer. Either way it can read only their visible components.

## Asking

```rust
me.query(6.0); // who's within 6 tiles of me, in any group?
```

The question goes with the group's report to every Spatial Layer Cell whose area the circle touches. Each answers before the group's next tick, so on the next tick:

```rust
for s in me.sensed() {
    // s.id, s.x, s.y, s.group, s.player, s.kind(), s.get::<Health>()
}
```

Answers list other groups' entities only (your own group's you already know exactly), and are dropped after a second. Ask again whenever you need fresher ones; the medieval example's bots ask every 200 ms, staggered by id so they don't all ask on the same tick.

## Using what you know

Two helpers combine both sources:

```rust
let near = me.nearby(6.0);     // own group's (exact) and others' (from the last answer), within 6 tiles
let target = me.find(id);      // what this entity knows of one entity, if anything
```

Each `Nearby` says whether it's `local` (exact and current) or from an answer (a tick or two old).

## Keeping answers small

By default an answer lists everyone in the circle, with every visible component. In a crowd that's a lot: fighting bots asking every 200 ms are most of the bus traffic. A `Query` narrows it, both in who and in what:

```rust
me.query(Query::within(12.0).nearest(5).target(foe).with::<Dead>());
// the 5 nearest, plus my foe if it's in range, and of each only whether it's dead
```

Ask only for the components you'll read. The medieval example's bots only need to know who's dead, so they ask for `Dead` alone; with 1,000 bots that cut the answers on the bus by 40% compared with sending every visible component. Prune who you ask about too as soon as your gameplay says what an entity actually needs (nearest few, enemies only, the weakest). Very large answers are split into several messages automatically (at most 4,000 entities each), so nothing ever hits the bus's 1 MB message limit.

## Who asks

Only ask from entities that need it. A wandering bot that never fights never asks. A bot that was just hit doesn't need to ask either: the hit itself says who and where the attacker was (`Incoming::from`). In the medieval example, Combat keeps that in a `LastHit` component and Bots reads it.

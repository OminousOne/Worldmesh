---
title: Testing game rules
description: Run a whole world in one process with TestWorld, tick by tick, and assert on what happened.
---

`worldmesh::testing::TestWorld` runs real Server Entity Groups and Spatial Layer Cells in one process, without NATS. It passes their messages exactly as the bus would: actions to entities in any group, area actions through the cells, handoffs, spawns, spatial query results and players' views. One `step()` is one tick of the whole world.

## A first test

```rust
use worldmesh::testing::TestWorld;

#[test]
fn a_sword_hit_lands_on_the_next_tick_in_another_group() {
    // 2 groups, 2 cells.
    let mut w = TestWorld::new(my_world(), 2, 2);
    // Where the world's join says, in the first group.
    let alice = w.join(1);
    // In group 2.
    let bob = w.join_in(2, 2);
    w.step();

    w.command(alice, Attack { target: bob });
    // Alice's command handler runs and sends the hit.
    w.step();
    assert_eq!(w.entity(bob).unwrap().get::<Health>(), Some(&Health(100)));

    // The hit lands in Bob's group.
    w.step();
    assert_eq!(w.entity(bob).unwrap().get::<Health>(), Some(&Health(80)));
    assert_eq!(w.event_names(), vec!["hurt"]);
}
```

## Driving the world

| Method | What it does |
| --- | --- |
| `TestWorld::new(world, groups, cells)` | Groups get ids 1, 2, and so on. Cells split the map as the orchestrator does. |
| `join(player)`, `join_in(group, player)` | A player connects (through the world's `join`), in the first group or a chosen one |
| `spawn_in(group, Spawn::new(kind, x, y))` | An entity no player controls. Returns its id. |
| `leave(player)` | A player disconnects |
| `input(player, sub, input)` | An input for sub-step `sub` of the next tick |
| `input_at(player, tick, sub, input)` | An input for any tick |
| `command(player, command)` | A command, handled on the next step. Panics if its type isn't registered as a command. |
| `hand_off(id, group)` | Moves an entity to another group, at the end of the next step |
| `step()`, `run(n)`, `run_ms(ms)` | Runs one tick, `n` ticks, or about `ms` of world time |
| `run_until(max, \|w\| ...)` | Runs until a condition holds. False if it didn't within `max` ticks. |

Entities your game spawns with `me.spawn` get ids from 2³¹ up and stay in the group that spawned them.

## Looking at it

| Method | What you get |
| --- | --- |
| `entity(id)` | The entity, wherever it is: `get::<T>()`, `has::<T>()`, `x`, `y` |
| `group_of(id)` | The group simulating it |
| `entities()` | Every entity, with its group |
| `events::<E>()` | The events of type `E` from the last step, decoded |
| `event_names()` | The names of every event from the last step, in order |
| `view(player)` | What that player could see after the last step: `Seen`s, with `kind` and `get::<T>()` for visible components |
| `events_seen::<E>(player)` | The events of type `E` that player got |
| `rules()` | The built world: its schema, and `command_from_json` to check what a client sends |
| `tick()` | The last tick simulated |

## What to test

- **Both sides of every check**: in reach and out of reach, in sight and blocked, cooldown ready and not.
- **Across groups**: put the two entities in different groups with `join_in` or `spawn_in`. It should make no difference, and now you know it doesn't.
- **During a handoff**: `hand_off` the target in the same step you send it something. It should land exactly once.
- **What players see**: `view` and `events_seen` catch a component you forgot to make visible (or one you didn't mean to), and an event placed where nobody can see it.
- **The client's JSON**: `w.rules().command_from_json("use", json)` proves the gateway reads what your client sends, under the name it sends it.

The medieval example's `tests.rs` does all of these.

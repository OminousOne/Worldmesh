---
title: Testing game rules
description: Run a whole world in one process with TestWorld, tick by tick, and assert on what happened.
---

`worldmesh::testing::TestWorld` runs real Server Entity Groups and Spatial Layer Cells in one process, without NATS, and passes their messages between them exactly as the bus would: actions to entities in any group, area actions through the cells, handoffs, spawns, "what's near me?" answers and players' views. One `step()` is one tick of the whole world.

## A first test

```rust
use worldmesh::testing::TestWorld;

#[test]
fn a_sword_hit_lands_on_the_next_tick_in_another_group() {
    let mut w = TestWorld::new(MyGame::default(), 2, 2); // 2 groups, 2 cells
    let alice = w.join(1);          // where Game::join says, in the first group
    let bob = w.join_in(2, 2);      // in group 2
    w.step();

    w.command(alice, Command::Attack { target: bob });
    w.step();                        // Alice's on_command runs and sends the hit
    assert_eq!(w.entity(bob).unwrap().data.hp, 100);
    w.step();                        // the hit lands in Bob's group
    assert_eq!(w.entity(bob).unwrap().data.hp, 80);
}
```

## Driving the world

| Method | What it does |
| --- | --- |
| `TestWorld::new(game, groups, cells)` | Groups get ids 1, 2, ...; cells split the map like the orchestrator does |
| `join(player)` / `join_in(group, player)` | A player connects (`Game::join`), in the first or a chosen group |
| `spawn_in(group, x, y, data)` | An entity no player controls; returns its id |
| `leave(player)` | A player disconnects |
| `input(player, sub, input)` | An input for sub-step `sub` of the next tick |
| `input_at(player, tick, sub, input)` | An input for any tick |
| `command(player, command)` | A command, handled on the next step |
| `hand_off(id, group)` | Move an entity to another group (at the end of the next step) |
| `step()`, `run(n)`, `run_ms(ms)` | Run one tick, `n` ticks, or about `ms` of world time |
| `run_until(max, \|w\| ...)` | Run until a condition holds; false if it never did within `max` ticks |

Entities your game spawns with `me.spawn` get ids from 2³¹ up and stay in the group that spawned them.

## Looking at it

| Method | What you get |
| --- | --- |
| `entity(id)` | The entity, wherever it is |
| `group_of(id)` | The group simulating it |
| `entities()` | Every entity, with its group |
| `events()` | Your game's events from the last step, decoded |
| `view(player)` | What that player could see after the last step (`Seen` with your `View`) |
| `events_seen(player)` | The events that player got |
| `tick()` | The last tick simulated |

## Things worth testing

- **Both sides of every check**: in reach and out of reach, in sight and blocked, cooldown ready and not.
- **Across groups**: put the two entities in different groups (`join_in`, `spawn_in`). It should make no difference, and now you know.
- **During a handoff**: `hand_off` the target in the same step you send it something. It should land exactly once.
- **What players see**: `view` and `events_seen` catch a `View` that forgot a field, or an event placed where nobody can see it.
- **The client's JSON**: `serde_json::from_str::<Command>(r#"{"k":"use",...}"#)` proves your types read what your client sends.

The medieval example's `tests.rs` does all of these.

---
title: Entities and actions
description: What an entity is, how entities affect each other across servers, and what you can trust when an action lands.
---

## Entities

An entity is what Worldmesh needs (an id, a position, whether a player controls it, a kind) plus the components your game gives it.

```rust
pub struct Entity {
    pub id: EntityId,
    pub x: f32,      // in tiles; where it stands decides who can see it
    pub y: f32,
    pub player: bool,
    // its kind, and its components: get::<T>(), get_mut::<T>(), has::<T>()
}
```

- **Players' entities** have the player's id (1 to 2³¹ - 1, given by the gateway). The world's `join` function says where they appear and as what kind when the player connects; they're removed when the player disconnects.
- **Spawned entities** (bots, monsters, projectiles) come from `me.spawn(Spawn::new("bot", x, y))`. The orchestrator gives them ids from 2³¹ up and picks their group. `me.despawn()` removes one.

An entity's components travel with it whenever it moves to another group, so keep everything the entity needs in them: HP, cooldowns, AI state, inventory.

## What others see

Visible components (registered with `world.visible::<T>()`) are what others see of an entity: the browsers that can see it, and other entities asking "what's near me?". Each is sent to a browser only when its bytes change, and on its own, so a health change doesn't resend anything else. Keep the rest hidden: it never leaves the entity's server except in a handoff.

## Actions

Entities affect each other only through actions. `me.send(to, action)` sends one:

| `To` | Who gets it |
| --- | --- |
| `To::Entity(id)` | That entity, wherever it is |
| `To::Area { x, y, r }` | Every entity within `r` tiles of (x, y), except the sender |
| `To::Everyone` | Every entity in the world, except the sender (use sparingly) |

The target handles it with every handler registered for that action (`world.action(f)`), in its own group, in the order they were registered.

### Always the next tick

Every action is delivered on the tick after it was sent, wherever the target is: in the same group, in another group on another machine, or in the middle of moving between groups. An action never lands on the same tick (even within one group), and it never takes longer when everything keeps up. Game rules can count on it: an arrow takes one tick to land, every time.

### Exactly once

Each action is numbered by its sender. If the target was just handed to another group, the old group forwards what reaches it, and the new group may also hear it directly; receivers drop duplicates. An entity mid-handoff gets each action exactly once.

### What you can trust

An action handler gets an `Incoming`:

```rust
pub struct Incoming {
    pub from: Facts,                      // sender id, its group, its x and y, the tick it sent this
    pub area: Option<(f32, f32, f32)>,    // set if it was sent to an area this entity stands in
    pub everyone: bool,                   // sent to everyone
    pub forwarded: bool,                  // passed on by this entity's previous group
    pub trace: u64,                       // ties everything one action led to together
}
```

`from` is stamped by the sender's group from the entity it simulates. Clients never fill it in. So a range check like this can't be fooled by a hacked client:

```rust
let (x, y) = me.position_at(from.from.tick); // where *I* stood when it was sent
let reach = ((x - from.from.x).powi(2) + (y - from.from.y).powi(2)).sqrt();
if reach > SWORD_RANGE { return; } // out of reach: nothing happens
```

`me.position_at(tick)` is the entity's position at the end of that tick (from the last couple of seconds). Judge by the moment the action was sent, not when it lands: otherwise a target that runs away would dodge everything that was fairly in reach.

### Two sides of every ability

The medieval example splits each ability in two, and so can your game:

1. **The user's side** (a command handler or a system, in the user's group): alive? cooldown ready? a point within reach? Then `me.send` it.
2. **The target's side** (an action handler, in the target's group): was I in reach (and in sight) when it was sent? Then apply it.

Each side checks what only it knows exactly. Neither needs the other's state.

## Commands

A command is a player's request, sent from the browser by name (`client.sendCommand('use', { ability: 'bow' })`), checked by the gateway against the type registered under that name, and delivered to the player's own entity on the next tick, to every handler registered for it. A command for someone else's entity is impossible: the gateway only addresses commands to the sender's own entity. So is sending an action from a browser: the gateway accepts only names registered as commands.

## Events

`me.emit(event)` shows something to the players who can see where it happened (and the entity's own player). `me.emit_at(x, y, event)` places it elsewhere, and `me.emit_involving(event, &[ids])` also sends it to those players wherever they are, so an archer sees "hit" even when the target was off their screen. Events are for showing things; they change nothing. Browsers get each as `{ name, data }`, and `client.on('tagged', fn)` listens for one.

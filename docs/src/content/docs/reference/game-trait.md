---
title: Game trait and Me
description: Everything a game implements, and everything its code can do from an entity.
---

## `Game`

```rust
pub trait Game: Sized + Send + Sync + 'static {
    type Data: Wire;     // an entity's state; travels with it between groups
    type View: Wire;     // what others see of an entity
    type Input: Wire;    // a player's held input, timed to a sub-step
    type Command: Wire;  // a player's request, for their own entity
    type Action: Wire;   // what entities do to each other
    type Event: Wire;    // what players see happen
    type Welcome: Serialize + Send + Sync + 'static; // sent to every player on connect

    fn config(&self) -> WorldConfig;
    fn welcome(&self) -> Self::Welcome;
    fn join(&self, player: EntityId) -> Spawn<Self::Data>;
    fn view(&self, entity: &Entity<Self::Data>) -> Self::View;
    fn tick(&self, me: &mut Me<'_, Self>, inputs: &[Timed<Self::Input>]);
    fn on_command(&self, me: &mut Me<'_, Self>, command: Self::Command) {}
    fn on_action(&self, me: &mut Me<'_, Self>, action: Self::Action, from: &Incoming) {}
}
```

`Wire` is `Serialize + DeserializeOwned + Clone + Send + Sync + 'static`: anything serde can carry.

| Method | Called by | When |
| --- | --- | --- |
| `config` | Every process | At start. Must be the same everywhere. |
| `welcome` | Gateway | Once at start; sent to each player as they connect |
| `join` | Gateway | A player connects (and if they need re-joining) |
| `view` | Groups | Every entity, every tick, for reports and answers |
| `tick` | Groups | Every entity, once per tick |
| `on_command` | Groups | The entity's own player sent a command (next tick) |
| `on_action` | Groups | An action reached the entity (the tick after it was sent) |

Only `config`, `welcome` and `join` run outside groups, so they mustn't depend on world state.

## `Me`

`Me<'_, G>` is the entity being simulated and everything the game can do from it. It dereferences to `Entity<G::Data>`, so `me.id`, `me.x`, `me.y`, `me.player` and `me.data` work directly (and `me.x = ...` moves it).

### Time and identity

| Method | Returns |
| --- | --- |
| `tick()` | The tick being simulated |
| `group()` | The Server Entity Group simulating this entity right now |
| `trace()` | The trace of what's being handled (the incoming command's or action's), or 0 in `tick` |
| `position_at(tick)` | Where this entity stood at the end of `tick` (its current position if older than its trail of about 2 seconds) |

### Doing things

| Method | What it does |
| --- | --- |
| `send(to, action) -> u64` | Send an action, delivered on the next tick wherever the target is. Returns its trace. |
| `emit(event)` | Show an event to players who can see this entity (and its own player) |
| `emit_at(x, y, event)` | Show an event to players who can see (x, y) |
| `emit_involving(event, &[ids])` | Like `emit`, and to these players wherever they are |
| `spawn(x, y, data)` | Create an entity; the orchestrator gives it an id and a group; it ticks within a tick or two |
| `despawn()` | Remove this entity after this callback (a player's comes back: the gateway re-joins them) |

### Knowing things

| Method | Returns |
| --- | --- |
| `local()` | The other entities in this group: exact and current |
| `local_entity(id)` | One of them |
| `query(r)` | Asks who's within `r` tiles, in any group; answered by the next tick |
| `query_with(r, nearest, target)` | Same, only the `nearest` closest (0: all), always including `target` if in range |
| `sensed()` | Other groups' entities from the latest answer (`Sensed<View>`); dropped after a second |
| `nearby(r)` | Everything this entity knows of within `r` tiles: own group's and from the answer (`Nearby<View>`, with `local`) |
| `find(id)` | What this entity knows of one entity, if anything (`Nearby<View>`) |

## Types

```rust
pub struct Entity<D> { pub id: EntityId, pub x: f32, pub y: f32, pub player: bool, pub data: D }
pub struct Spawn<D> { pub x: f32, pub y: f32, pub data: D }
pub struct Timed<I> { pub sub: u8, pub input: I }

pub enum To {
    Entity(EntityId),
    Area { x: f32, y: f32, r: f32 }, // everyone within r of (x, y) but the sender
    Everyone,                        // everyone but the sender
}

pub struct Incoming {
    pub from: Facts,                   // stamped by the sender's group
    pub area: Option<(f32, f32, f32)>, // sent to an area this entity stands in
    pub everyone: bool,
    pub forwarded: bool,               // passed on by this entity's previous group
    pub trace: u64,
}
pub struct Facts { pub id: EntityId, pub group: GroupId, pub x: f32, pub y: f32, pub tick: u64 }

pub struct Sensed<V> { pub id: EntityId, pub x: f32, pub y: f32, pub group: GroupId, pub player: bool, pub view: V }
pub struct Nearby<V> { /* the same as Sensed */ pub local: bool }
```

`EntityId` and `GroupId` are `u32`. Players' ids run from 1; spawned entities' from `SPAWNED_ID_START` (2³¹).

## Rules of thumb

- **Keep entity state in `Data`.** The game value (`&self`) is shared and immutable; anything per entity must travel with the entity.
- **Never assume where another entity is simulated.** It might be in this group now and another next tick. Use actions for anything that changes another entity.
- **Judge actions by `from` and `position_at(from.from.tick)`,** not by what the sender claims in the action.
- **Seed randomness from (entity, tick)** so an entity decides the same thing whichever group simulates it.

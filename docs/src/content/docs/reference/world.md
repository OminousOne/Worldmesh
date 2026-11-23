---
title: World, plugins and Me
description: Everything a game registers, and everything its code can do from an entity.
---

## `World`

A game is a `World`. Build it, then hand it to `worldmesh::run`, which checks it, freezes it and runs whichever role the command line asks for.

```rust
let mut world = World::new(WorldConfig { width: 64, height: 64, chunk_size: 8, view, input_substeps: 16 });
world.plugin(Movement).plugin(Combat);
world.join(|player| Spawn::new("hero", 30.5, 32.5));
worldmesh::run(world).await
```

| Method | What it registers |
| --- | --- |
| `plugin(p)` | Calls `p.build(&mut world)`. Any `Fn(&mut World)` is a plugin too. |
| `component::<T>()` | A component only the entity's own server sees |
| `visible::<T>()` | A component players and other entities can see too (registering it as hidden first is fine) |
| `input::<I>()` | The world's input type (one per world) |
| `command(f)` | A handler for command `C`: `Fn(&mut Me, &C)` |
| `action(f)` | A handler for action `A`: `Fn(&mut Me, &A, &Incoming)` |
| `event::<E>()` | An event type |
| `system(stage, f)` | `Fn(&mut Me)`, for every entity, every tick |
| `system_for::<T>(stage, f)` | The same, only for entities with a `T` |
| `kind(name, bundle)` | A kind of entity and its starting components; again with the same name adds to it |
| `join(f)` | Where and as what a player appears: `Fn(EntityId) -> Spawn` (required) |
| `welcome(key, value)` | Something every player gets on connect, under `key` |

Registering two components or messages with the same name, or one message as two of command, action and event, panics when the world is built. So does a world without `join`. Components a kind uses are registered (hidden) if they aren't yet.

## `Plugin`

```rust
pub trait Plugin {
    fn build(&self, world: &mut World);
}
```

A plugin holds whatever its handlers share (the map, say: wrap it in an `Arc` and clone it into each closure). Handlers and systems must be `Send + Sync + 'static`; they run on many threads and many servers, so keep per-entity state in components, not in the plugin.

## Components and messages

```rust
pub trait Component: Wire { const NAME: &'static str; }
pub trait Message: Wire { const NAME: &'static str; }
```

`Wire` is `Serialize + DeserializeOwned + Clone + Send + Sync + 'static`. Derive both:

```rust
#[derive(Component, Clone, Serialize, Deserialize)]
struct Health(u16);                     // "health"

#[derive(Message, Clone, Serialize, Deserialize)]
#[worldmesh(name = "use")]
struct UseAbility { ability: Ability }  // "use" instead of "use_ability"
```

## `Stage`

| Stage | Runs |
| --- | --- |
| `Stage::First` | First, across every entity |
| `Stage::Update` | Then |
| `Stage::Last` | Last |

Within a stage, systems run in the order they were registered. Commands and actions that arrived are handled before `First`.

## `Me`

`Me` is the entity being simulated and everything game code can do from it. It dereferences to `Entity`, so `me.id`, `me.x`, `me.y`, `me.player`, `me.get::<T>()`, `me.get_mut::<T>()` and `me.has::<T>()` work directly, and `me.x = ...` moves it.

### Time and identity

| Method | Returns |
| --- | --- |
| `tick()` | The tick being simulated |
| `group()` | The Server Entity Group simulating this entity right now |
| `kind()` | This entity's kind |
| `config()` | The world's config |
| `trace()` | The trace of what's being handled (the command's or action's), or 0 in a system |
| `position_at(tick)` | Where this entity stood at the end of `tick` (its current position if older than its trail of about 2 seconds) |
| `inputs::<I>()` | Its player's inputs due this tick, each with its sub-step (`Timed<I>`); empty outside systems |

### Changing itself

| Method | What it does |
| --- | --- |
| `insert(c)` | Add a component, or replace the one of its type |
| `remove::<T>()` | Remove a component, returning it |
| `get_mut::<T>()` | Change a component in place |
| `despawn()` | Remove this entity after this callback (a player's comes back: the gateway re-joins them) |

### Doing things

| Method | What it does |
| --- | --- |
| `send(to, action) -> u64` | Send an action, delivered on the next tick wherever the target is. Returns its trace. |
| `emit(event)` | Show an event to players who can see this entity (and its own player) |
| `emit_at(x, y, event)` | Show an event to players who can see (x, y) |
| `emit_involving(event, &[ids])` | Like `emit`, and to these players wherever they are |
| `spawn(Spawn)` | Create an entity; the orchestrator gives it an id and a group; it ticks within a tick or two |

### Knowing things

| Method | Returns |
| --- | --- |
| `local()` | The other entities in this group: exact and current |
| `query(q)` | Asks who's near, in any group; answered by the next tick. `q` is a radius or a `Query`. |
| `sensed()` | Other groups' entities from the latest answer; dropped after a second |
| `nearby(r)` | Everything this entity knows of within `r` tiles, from both |
| `find(id)` | What this entity knows of one entity, if anything |

All of them give `Nearby`s, which read only visible components:

```rust
pub struct Nearby<'a> { pub id: EntityId, pub x: f32, pub y: f32, pub group: GroupId, pub player: bool, pub local: bool, /* ... */ }
impl Nearby<'_> {
    fn kind(&self) -> &str;
    fn get<T: Component>(&self) -> Option<T>; // None if T isn't visible, or wasn't asked for
    fn has<T: Component>(&self) -> bool;
    fn distance_to(&self, x: f32, y: f32) -> f32;
}
```

### `Query`

```rust
me.query(6.0);                                                    // everyone within 6 tiles, every visible component
me.query(Query::within(12.0).nearest(5).target(foe).with::<Dead>()); // the 5 nearest plus `foe`, and only `Dead`
```

| Method | Narrows the answer to |
| --- | --- |
| `Query::within(r)` | Entities within `r` tiles |
| `.nearest(n)` | The `n` closest (0: all) |
| `.target(id)` | Always include `id` if it's in range |
| `.with::<T>()` | Only these visible components (call again for more) |

## Types

```rust
pub struct Entity { pub id: EntityId, pub x: f32, pub y: f32, pub player: bool, /* kind, components */ }
pub struct Spawn { pub kind: String, pub x: f32, pub y: f32, /* extra components */ }
impl Spawn { fn new(kind: &str, x: f32, y: f32) -> Spawn; fn with<T: Component>(self, c: T) -> Spawn; }
pub struct Bundle { /* components */ }
impl Bundle { fn new() -> Bundle; fn with<T: Component>(self, c: T) -> Bundle; }
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
```

`EntityId` and `GroupId` are `u32`. Players' ids run from 1; spawned entities' from `SPAWNED_ID_START` (2³¹).

## Rules of thumb

- **Keep entity state in components.** Plugins are shared and immutable; anything per entity must travel with the entity.
- **Never assume where another entity is simulated.** It might be in this group now and another next tick. Use actions for anything that changes another entity.
- **Judge actions by `from` and `position_at(from.from.tick)`,** not by what the sender claims in the action.
- **Seed randomness from (entity, tick)** so an entity decides the same thing whichever group simulates it.
- **Ask only for what you'll read.** `Query::with` keeps answers small; answers are the biggest thing on the bus in a crowd.

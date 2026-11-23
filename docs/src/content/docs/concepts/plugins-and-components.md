---
title: Plugins, components and messages
description: What Worldmesh does and what your game adds, and how a game is put together from plugins.
---

## Worldmesh does the networking, your game does the game

Worldmesh ships no game rules. No health, no movement speed, no inventory, no AI. It gives every entity only what the networking needs, and runs everything around it:

| Worldmesh | Your game |
| --- | --- |
| Every entity's id, position (`x`, `y`), whether a connected player controls it, and its kind | Everything else an entity is: components you define |
| Which server simulates each entity, and moving it between servers while it keeps moving | What happens on a tick: your systems |
| Getting a player's inputs and commands to their entity, wherever it is | What an input or command does: your handlers |
| Delivering actions between entities, exactly once, on the next tick | What an action does to its target: your handlers |
| "What's near me?" across servers | What an entity does with the answer |
| Building each player's view, sending only what changed | Which components players can see |
| Showing events to the right players | What your events mean on screen |

Even walking is yours. The position is Worldmesh's because it decides who can see an entity and which Spatial Layer Cell holds it, but how an input moves it is a system you write. The `examples/` folder has two games built this way: `hello` (tag) and `medieval` (movement, combat and bots as three plugins).

## The pieces

A game is a `World` built from plugins:

```rust
let mut world = World::new(config);
world.plugin(Movement).plugin(Combat).plugin(Bots);
worldmesh::run(world).await
```

A plugin registers any of these:

| Piece | What it is | Registered with |
| --- | --- | --- |
| **Component** | A piece of an entity's state, one serde type each | `component::<T>()` (only the server sees it) or `visible::<T>()` (players and other entities can see it too) |
| **Kind** | A named set of components to start an entity with | `kind("hero", Bundle::new().with(Health(100)))` |
| **Input** | What a player holds down, timed to a sub-step | `input::<Move>()` (one per world) |
| **Command** | A player's request, for their own entity | `command(handler)` |
| **Action** | What one entity does to another, an area or everyone | `action(handler)` |
| **Event** | Something players see happen | `event::<E>()` |
| **System** | Code that runs for every entity, every tick | `system(stage, f)` or `system_for::<T>(stage, f)` |

A world also says where players appear (`join`) and what every player gets when they connect (`welcome`).

```rust
struct Combat;

impl Plugin for Combat {
    fn build(&self, world: &mut World) {
        world
            .visible::<Health>()
            .component::<Cooldowns>()
            .command(use_ability)          // fn(&mut Me, &UseAbility)
            .action(take_hit)              // fn(&mut Me, &Hit, &Incoming)
            .event::<Effect>()
            .system_for::<Dead>(Stage::Last, respawn)
            .kind("hero", Bundle::new().with(Health(100)).with(Cooldowns::default()));
    }
}
```

## Components

A component is any serde type with a name. Derive it:

```rust
#[derive(Component, Clone, Serialize, Deserialize)]
struct Health(u16);

#[derive(Component, Clone, Default, Serialize, Deserialize)]
#[worldmesh(name = "combat.cooldowns")] // the default would be "cooldowns"
struct Cooldowns(Vec<(Ability, u64)>);
```

Names default to the type's name in snake_case. They're what clients see, and two plugins can't use the same one (`#[worldmesh(name = "...")]` picks another).

An entity's components travel with it whenever it moves to another server, so keep everything an entity needs in them.

**Visible or not.** A visible component is part of what others see of the entity: players whose screen it's on, and other entities asking "what's near me?". Each is sent and diffed on its own, so a health change sends only the new health. Keep hidden what only the entity's own server needs (cooldowns, AI state): it never leaves that server except in a handoff.

**Presence means something.** Components can come and go: `me.insert(Dead { until })` and `me.remove::<Dead>()`. A client sees `dead` appear and disappear. Markers with no data (`struct Immobile;`) are often all you need.

## Kinds

A kind names the components an entity starts with:

```rust
world.kind("bot", Bundle::new().with(Health(100)).with(Brain::default()));
me.spawn(Spawn::new("bot", x, y).with(Brain::new(BotMode::Aggressive))); // replaces the default Brain
```

Calling `kind` again with the same name adds to it, so each plugin gives a shared kind its own components. In the medieval example, Movement adds a `Heading` to "hero" and "bot", Combat adds `Health` and `Cooldowns`, and Bots adds a `Brain` to "bot" only. Clients get each entity's kind name with it.

## Messages

Commands, actions and events are messages: serde types with a name.

```rust
#[derive(Message, Clone, Serialize, Deserialize)]
#[worldmesh(name = "use")]
struct UseAbility { ability: Ability, target: Target }

#[derive(Message, Clone, Serialize, Deserialize)]
struct Hit { ability: Ability }
```

A message is one of the three, never two: a player can send a command, but never an action, so a client can't fake what one entity does to another.

**Any number of handlers.** Register a command or action handler in as many plugins as need it. They run in the order they were registered, on the receiving entity, and stop early if one despawns it. A quest plugin can listen for `Hit` without the combat plugin knowing it exists.

**Events** go to clients by name: `{ "name": "effect", "data": { ... } }`. In the browser, `client.on('effect', fn)`.

## Systems and stages

A system runs once per tick for every entity (or only those with a component, with `system_for::<T>`). Each tick has three stages, and every system of a stage runs across all entities before the next stage starts:

| Stage | For |
| --- | --- |
| `First` | Deciding: AI, reading what arrived |
| `Update` | Doing: movement, most rules |
| `Last` | Cleaning up: respawning, asking "what's near me?" for next tick |

Commands and actions are handled before the systems run, so a hit that landed this tick is already applied when your systems see the entity.

## Plugins meet through components

Plugins don't call each other's internals; they agree on components. In the medieval example:

- Combat inserts `Immobile` (from Movement) when a unit dies, and Movement doesn't walk anything immobile.
- Combat records the last hit a unit survived in a `LastHit` component; Bots reads it to fight back.
- Bots steer by setting their `Heading`, the same component a player's input sets.
- Bots fight through Combat's public `use_ability`, so a bot can't do anything a player can't.

## Only change yourself

Game code always runs as one entity, through `Me`, and changes only that entity. Everything else it reads (`me.nearby`, `me.find`) or asks for (`me.send`). This is what lets any entity run on any server: nothing ever reaches into another entity's memory, so it doesn't matter where that entity is. See [Entities and actions](/Worldmesh/concepts/entities-and-actions/).

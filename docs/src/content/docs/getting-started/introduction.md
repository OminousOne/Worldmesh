---
title: Introduction
description: What Worldmesh is, the problem it solves, and how it's put together.
---

Worldmesh is a networking library, written in Rust, that lets a game split the work of a large multiplayer world across many servers while players see one seamless world.

You write your game as if it ran in one place. Worldmesh decides which server simulates each entity, moves entities between servers while they keep moving, delivers what entities do to each other wherever they are, and builds each player's view from every server that holds a part of it.

Worldmesh is only the networking. It ships no game rules: no health, no movement, no inventory. Every entity gets an id, a position and a kind from Worldmesh; everything else about it is components your game defines, and everything it does is code your game registers.

## The problem

A single server runs out of room long before a big world does. The usual ways around that are visible to players:

- **Shards**: many copies of the world, and your friends are in another one.
- **Zones**: the map is cut into pieces with loading screens between them, and a crowd in one zone overloads its server while the others idle.
- **Instances**: separate copies of busy places.

Worldmesh keeps one world with no seams, and splits it two ways at once:

- **By entity**: each entity (a player, a bot, a monster) is simulated by exactly one **Server Entity Group**, wherever it stands. Groups are balanced by how many entities they hold, so a crowd in one town spreads over as many groups as it takes.
- **By area**: each **Spatial Layer Cell** owns an area of the map and answers "what's near here?" from copies of the entities standing in it. It never simulates anything, so it's cheap to move area between cells.

## How it fits together

| Piece | What it does | Runs your code? |
| --- | --- | --- |
| Server Entity Group | Simulates its entities every tick, delivers actions to them, hands entities to other groups | Yes: your systems and handlers |
| Spatial Layer Cell | Holds copies of the entities in its area; answers "what's near me?"; routes actions aimed at an area; builds players' views | No |
| Orchestrator | Decides which groups and cells are in use, who owns which area, where new entities go | No |
| Gateway | Holds players' WebSockets; checks their messages against your types; merges their views | Only your `join` function |
| NATS | The bus every process talks over | |

Every role is the same binary: your game's `main` calls `worldmesh::run(world)`, and the command line picks the role.

## What a game looks like

A game is a `World` built from plugins. Each plugin registers components (an entity's state), messages (commands, actions, events) with their handlers, systems that run every tick, and kinds of entity:

```rust
#[derive(Component, Clone, Serialize, Deserialize)]
struct Health(u16);

#[derive(Message, Clone, Serialize, Deserialize)]
struct Hit { damage: u16 }

struct Combat;

impl Plugin for Combat {
    fn build(&self, world: &mut World) {
        world
            .visible::<Health>()                                    // players can see it
            .action(|me, hit: &Hit, _from| {                        // lands on the next tick, wherever `me` is
                if let Some(h) = me.get_mut::<Health>() {
                    h.0 = h.0.saturating_sub(hit.damage);
                }
            })
            .kind("hero", Bundle::new().with(Health(100)));
    }
}

let mut world = World::new(config);
world.plugin(Movement).plugin(Combat).join(|player| Spawn::new("hero", 30.5, 32.5));
worldmesh::run(world).await
```

Game code always runs as one entity, through `Me`: it changes only that entity, and reaches others with `me.send(To::Entity(id), Hit { damage: 20 })`, `me.query(6.0)`, `me.nearby(6.0)`, `me.spawn(...)` and `me.emit(event)`. See [Plugins, components and messages](/Worldmesh/concepts/plugins-and-components/) and the [World reference](/Worldmesh/reference/world/).

## What's in the repository

- `crates/`: the library, one crate per role (`worldmesh-sim`, `worldmesh-spatial`, `worldmesh-orchestrator`, `worldmesh-gateway`), the shared `worldmesh-core`, `worldmesh-testing`, the derive macros, and `worldmesh`, which puts them together.
- `packages/client` (`@worldmesh/client`): the browser side, with snapshots, interpolation and prediction.
- `packages/debug` (`@worldmesh/debug`): the debug panel.
- `examples/hello`: tag, the smallest complete game.
- `examples/medieval`: an isometric world with swords, bows, fireballs and bots, server (three plugins) and client.
- `skills/`: agent skills for building on Worldmesh.

## Status

Worldmesh is young (0.1). The core works end to end and is tested, including thousands of fighting bots across groups. Scaling is manual (from the debug panel) for now, and there's no persistence or authentication yet. The repository README lists what's next.

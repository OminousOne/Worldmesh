---
title: Introduction
description: What Worldmesh is, the problem it solves, and how it's put together.
---

Worldmesh is a networking library, written in Rust, that lets a game split the work of a large multiplayer world across many servers while players see one seamless world.

You write your game as if it ran in one place. Worldmesh decides which server simulates each entity, moves entities between servers while they keep moving, delivers what entities do to each other wherever they are, and builds each player's view from every server that holds a part of it.

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
| Server Entity Group | Simulates its entities every tick, delivers actions to them, hands entities to other groups | Yes: `Game::tick`, `on_command`, `on_action` |
| Spatial Layer Cell | Holds copies of the entities in its area; answers "what's near me?"; routes actions aimed at an area; builds players' views | No |
| Orchestrator | Decides which groups and cells are in use, who owns which area, where new entities go | No |
| Gateway | Holds players' WebSockets; checks their messages against your types; merges their views | Only `welcome` and `join` |
| NATS | The bus every process talks over | |

Every role is the same binary: your game's `main` calls `worldmesh::run(MyGame)`, and the command line picks the role.

## What a game looks like

```rust
impl Game for MyGame {
    type Data = Unit;       // what an entity holds
    type View = Look;       // what others see of it
    type Input = Move;      // what a player holds down
    type Command = Command; // what a player asks for
    type Action = Action;   // what entities do to each other
    type Event = Event;     // what players see happen
    type Welcome = Welcome; // what every player gets on connect

    fn config(&self) -> WorldConfig { /* world size, chunks, view */ }
    fn welcome(&self) -> Welcome { /* the map, say */ }
    fn join(&self, player: EntityId) -> Spawn<Unit> { /* where they appear */ }
    fn view(&self, e: &Entity<Unit>) -> Look { /* what others see */ }
    fn tick(&self, me: &mut Me<'_, Self>, inputs: &[Timed<Move>]) { /* one tick of one entity */ }
    fn on_command(&self, me: &mut Me<'_, Self>, command: Command) { /* a player's request */ }
    fn on_action(&self, me: &mut Me<'_, Self>, action: Action, from: &Incoming) { /* something landed */ }
}
```

`Me` is the entity being simulated plus everything it can do: `me.send(To::Entity(id), action)`, `me.query(6.0)`, `me.nearby(6.0)`, `me.spawn(...)`, `me.emit(event)`. See the [Game trait reference](/Worldmesh/reference/game-trait/).

## What's in the repository

- `crates/worldmesh`: the library.
- `packages/client` (`@worldmesh/client`): the browser side, with snapshots, interpolation and prediction.
- `packages/debug` (`@worldmesh/debug`): the debug panel.
- `examples/hello`: tag, the smallest complete game.
- `examples/medieval`: an isometric world with swords, bows, fireballs and bots, server and client.
- `skills/`: agent skills for building on Worldmesh.

## Status

Worldmesh is young (0.1). The core works end to end and is tested, including thousands of fighting bots across groups. Scaling is manual (from the debug panel) for now, and there's no persistence or authentication yet. The repository README lists what's next.

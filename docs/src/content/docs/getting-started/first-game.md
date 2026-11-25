---
title: Your first game
description: Build tag on Worldmesh as a plugin, step by step, test it without servers, and run it.
---

This guide builds tag: players walk around, and whoever is "it" can tag another player within reach. It's small, but it uses everything a real game does: components, timed inputs, a system, a command, actions in both directions, an event, and a check that can't be cheated.

Worldmesh brings none of the game, not even walking. It gives each entity an id, a position and a kind, and does the networking. Everything below is your code. The finished game is [`examples/hello`](https://github.com/OminousOne/Worldmesh/tree/main/examples/hello).

## 1. Create the crate

Worldmesh isn't on crates.io yet, so depend on the repository:

```toml title="Cargo.toml"
[dependencies]
worldmesh = { git = "https://github.com/OminousOne/Worldmesh" }
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
anyhow = "1"
```

## 2. Components: what an entity holds

```rust title="src/main.rs"
use serde::{Deserialize, Serialize};
use worldmesh::{
    time, Bundle, Component, EntityId, Incoming, Me, Message, Plugin, Spawn,
    Stage, To, ViewShape, World, WorldConfig,
};

/// How far a tag reaches, in tiles.
const REACH: f32 = 1.5;
/// Walking speed, in tiles per second.
const SPEED: f32 = 4.0;

/// Whether a player is it. Visible: everyone can see who's it.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
struct It(bool);

/// The direction a player walks in. Only their own server needs it.
#[derive(Component, Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct Heading {
    dx: f32,
    dy: f32,
}
```

Components move with their entity from server to server. `It` will be visible: players who can see the entity get it as `"it": true`. `Heading` stays on the server.

## 3. Messages: what players and entities send

```rust
/// A player's input: the direction to walk in, from now on.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Walk {
    dx: f32,
    dy: f32,
}

/// A player's command: tag someone. Sent as `sendCommand('tag', { target: 2 })`.
#[derive(Message, Debug, Clone, Serialize, Deserialize)]
struct Tag {
    target: EntityId,
}

/// An action from whoever is it: "you're it now", if they really reached you.
#[derive(Message, Debug, Clone, Serialize, Deserialize)]
struct YouAreIt;

/// An action back to the tagger: the tag landed, they're not it any more.
#[derive(Message, Debug, Clone, Serialize, Deserialize)]
struct NotIt;

/// An event for players' screens.
#[derive(Message, Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Tagged {
    by: EntityId,
    who: EntityId,
}
```

Names come from the type: `Tag` is `"tag"` and `YouAreIt` is `"you_are_it"`. To pick another, add `#[worldmesh(name = "...")]` to the type.

## 4. The plugin

A plugin registers everything with the world:

```rust
struct TagGame;

impl Plugin for TagGame {
    fn build(&self, world: &mut World) {
        world
            .visible::<It>()
            .component::<Heading>()
            .input::<Walk>()
            .command(tag)
            .action(you_are_it)
            .action(|me, _: &NotIt, _| me.insert(It(false)))
            .event::<Tagged>()
            .system_for::<Heading>(Stage::Update, walk)
            .kind(
                "player",
                Bundle::new().with(It(false)).with(Heading::default()),
            )
            // Player 1 starts out it.
            .join(|player| {
                let x = 30.5 + (player % 4) as f32;
                Spawn::new("player", x, 32.5).with(It(player == 1))
            });
    }
}
```

`kind` names the components an entity starts with. `join` says where a connecting player appears, and as what. It runs in the gateway; the orchestrator then picks a group for the new entity.

## 5. A system: walking

A system runs every tick for every entity (here, every entity with a `Heading`), in whichever group simulates it. Each input comes with the sub-step of the tick it takes effect at, so a key released halfway through a tick stops you halfway, on the server and in the client's prediction alike.

```rust
fn walk(me: &mut Me<'_>) {
    let substeps = me.config().input_substeps;
    let per_step = SPEED * time::tick_ms() as f32 / 1000.0 / f32::from(substeps);
    let Some(mut heading) = me.get::<Heading>().copied() else {
        return;
    };
    let mut at = 0;
    for t in me.inputs::<Walk>() {
        step(me, heading, per_step * f32::from(t.sub - at));
        at = t.sub;
        // Never faster diagonally.
        let len = t.input.dx.hypot(t.input.dy);
        heading = if len > 1.0 {
            Heading { dx: t.input.dx / len, dy: t.input.dy / len }
        } else {
            Heading { dx: t.input.dx, dy: t.input.dy }
        };
    }
    step(me, heading, per_step * f32::from(substeps - at));
    me.insert(heading);
}

fn step(me: &mut Me<'_>, h: Heading, distance: f32) {
    me.x = (me.x + h.dx * distance).clamp(0.0, 63.99);
    me.y = (me.y + h.dy * distance).clamp(0.0, 63.99);
}
```

`me` dereferences to the entity, so `me.x`, `me.y` and `me.get::<T>()` are right there. Move it freely: where it stands decides who can see it.

## 6. Commands and actions

A command comes from the entity's own player. This one sends an action to the target, wherever the target is:

```rust
fn tag(me: &mut Me<'_>, &Tag { target }: &Tag) {
    if me.get::<It>() == Some(&It(true)) {
        me.send_reliable(To::Entity(target), YouAreIt);
    }
}
```

The action arrives on the next tick, in the target's group: on another machine, or even while the target is moving between groups. A lost tag would leave nobody "it", so it's sent with `send_reliable`: sure to land, exactly once. (Plain `send` is for things the next tick replaces anyway.) The target decides what it does:

```rust
/// Judged by where we both stood on the tick the tag was sent. The sender's position
/// comes from its own group, never from a client.
fn you_are_it(me: &mut Me<'_>, _: &YouAreIt, from: &Incoming) {
    let (x, y) = me.position_at(from.from.tick);
    let in_reach = (x - from.from.x).hypot(y - from.from.y) <= REACH;
    if in_reach && me.get::<It>() != Some(&It(true)) {
        me.insert(It(true));
        me.send_reliable(To::Entity(from.from.id), NotIt);
        let who = me.id;
        me.emit_involving(Tagged { by: from.from.id, who }, &[from.from.id]);
    }
}
```

Two things make this check fair:

- **`from.from` comes from the server.** The sender's id, position and tick are stamped by the sender's group, never by a client, so a hacked client can't claim to be closer than it is.
- **`me.position_at(tick)` looks back.** It's where the target stood when the tag was sent, not a tick later when it lands, so running away doesn't dodge a tag that was in reach.

`you_are_it` only changes the entity it runs for. It never reaches into the tagger; it sends `NotIt` back instead. That's what lets the two players be on different servers. See [Entities and actions](/Worldmesh/concepts/entities-and-actions/).

## 7. Test it without servers

`TestWorld` runs real groups and Spatial Layer Cells in one process and passes their messages as the bus would, one tick at a time:

```rust
fn world() -> World {
    let mut world = World::new(WorldConfig {
        width: 64,
        height: 64,
        chunk_size: 8,
        view: ViewShape::Rect { half_w: 20.0, half_h: 12.0 },
        input_substeps: 16,
    });
    world.plugin(TagGame);
    world
}

#[cfg(test)]
mod tests {
    use super::*;
    use worldmesh::testing::TestWorld;

    #[test]
    fn a_tag_in_reach_passes_it_on_even_across_groups() {
        // 2 groups, 2 cells.
        let mut w = TestWorld::new(world(), 2, 2);
        // Alice is it. Bob is in the other group, a tile away.
        let alice = w.join_in(1, 1);
        let bob = w.join_in(2, 2);
        w.step();

        w.command(alice, Tag { target: bob });
        // The command sends the tag; it lands on the next tick.
        w.run(2);
        assert_eq!(w.entity(bob).unwrap().get::<It>(), Some(&It(true)));
        assert_eq!(w.events::<Tagged>(), vec![Tagged { by: alice, who: bob }]);

        // The reply reaches Alice.
        w.step();
        assert_eq!(w.entity(alice).unwrap().get::<It>(), Some(&It(false)));
    }
}
```

Run it with `cargo test`. More in [Testing game rules](/Worldmesh/guides/testing/).

## 8. Run it

`main` hands the world to Worldmesh, and the command line picks the role:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    worldmesh::run(world()).await
}
```

Start a [NATS server](https://docs.nats.io/running-a-nats-service/introduction/installation) on port 4222, then:

```sh
hello orchestrator --groups 2 --spatial 2 &
hello group & hello group &
hello spatial & hello spatial &
hello gateway --listen 0.0.0.0:8080 --allow-debug &
```

Players connect to `ws://localhost:8080/ws`. [The browser client](/Worldmesh/guides/client/) shows how to draw them.

## Next

- [Plugins, components and messages](/Worldmesh/concepts/plugins-and-components/): split a bigger game into plugins that work together.
- `examples/medieval`: movement, combat and bots as three plugins, with a browser client.

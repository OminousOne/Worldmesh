---
title: Your first game
description: Build tag on Worldmesh as a plugin, step by step, test it without servers, and run it.
---

This guide builds tag: players walk around, and whoever is "it" can tag another player within reach. It's small, but it uses everything a real game does: components, timed inputs, a system, a command, actions in both directions, an event, and a check that can't be cheated. The finished code is `examples/hello` in the repository.

Worldmesh brings none of the game: not even walking. It gives each entity an id, a position and a kind, and does the networking. Everything below is your code.

## 1. Create the crate

```toml
# Cargo.toml
[dependencies]
worldmesh = "0.1"
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
anyhow = "1"
```

## 2. Components: what an entity holds

```rust
use serde::{Deserialize, Serialize};
use worldmesh::{
    time, Bundle, Component, EntityId, Incoming, Me, Message, Plugin, Spawn, Stage, To, ViewShape, World, WorldConfig,
};

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

Components move with the entity between servers. `It` will be visible (sent to players who can see the entity, as `"it": true`); `Heading` stays on the server.

## 3. Messages: what players and entities send

```rust
/// A player's input: the direction to walk in, from now on.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Walk {
    dx: f32,
    dy: f32,
}

/// A command: tag someone. Players send it as `sendCommand('tag', { target: 2 })`.
#[derive(Message, Debug, Clone, Serialize, Deserialize)]
struct Tag {
    target: EntityId,
}

/// Actions, from one entity to another.
#[derive(Message, Debug, Clone, Serialize, Deserialize)]
struct YouAreIt;

#[derive(Message, Debug, Clone, Serialize, Deserialize)]
struct NotIt;

/// An event, for players' screens.
#[derive(Message, Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Tagged {
    by: EntityId,
    who: EntityId,
}
```

Names come from the type: `Tag` is `"tag"`, `YouAreIt` is `"you_are_it"`. Pick another with `#[worldmesh(name = "...")]`.

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
            .kind("player", Bundle::new().with(It(false)).with(Heading::default()))
            // Player 1 starts out it.
            .join(|player| Spawn::new("player", 30.5 + (player % 4) as f32, 32.5).with(It(player == 1)));
    }
}
```

`kind` names the components an entity starts with; `join` says where a connecting player appears, and as what. It runs in the gateway; the orchestrator then picks a group for the new entity.

## 5. A system: walking

A system runs every tick for every entity (here, every entity with a `Heading`), in whichever group simulates it. A player's inputs come with the sub-step of the tick they take effect at, so a key released half way through a tick stops you half way, on the server and in the client's prediction alike.

```rust
fn walk(me: &mut Me<'_>) {
    let substeps = me.config().input_substeps;
    let per_step = 4.0 * time::tick_ms() as f32 / 1000.0 / f32::from(substeps);
    let Some(mut heading) = me.get::<Heading>().copied() else { return };
    let mut at = 0;
    for t in me.inputs::<Walk>() {
        step(me, heading, per_step * f32::from(t.sub - at));
        at = t.sub;
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

A command comes from the entity's own player. Here it sends an action to the target, wherever the target is:

```rust
fn tag(me: &mut Me<'_>, &Tag { target }: &Tag) {
    if me.get::<It>() == Some(&It(true)) {
        me.send(To::Entity(target), YouAreIt);
    }
}
```

The action is delivered on the next tick, in the target's group, even if that's another machine, even if the target is moving between groups right then. The target decides what it does:

```rust
fn you_are_it(me: &mut Me<'_>, _: &YouAreIt, from: &Incoming) {
    let (x, y) = me.position_at(from.from.tick);
    let in_reach = (x - from.from.x).hypot(y - from.from.y) <= 1.5;
    if in_reach && me.get::<It>() != Some(&It(true)) {
        me.insert(It(true));
        me.send(To::Entity(from.from.id), NotIt);
        let who = me.id;
        me.emit_involving(Tagged { by: from.from.id, who }, &[from.from.id]);
    }
}
```

Two things make this check fair:

- `from.from` (the sender's id, position and tick) is stamped by the sender's group, never by a client, so a hacked client can't claim to be closer than it is.
- `me.position_at(tick)` is where the target stood when the tag was sent, not a tick later when it lands, so a target running away doesn't dodge a tag that was in reach.

Notice that `you_are_it` only changes the entity it runs for. It never reaches into the tagger; it sends `NotIt` back instead. That's what lets the two players be on different servers. See [Entities and actions](/Worldmesh/concepts/entities-and-actions/).

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
        let mut w = TestWorld::new(world(), 2, 2); // 2 groups, 2 cells
        let alice = w.join_in(1, 1);              // it
        let bob = w.join_in(2, 2);                // in the other group, a tile away
        w.step();
        w.command(alice, Tag { target: bob });
        w.run(2); // the command sends the tag; it lands on the next tick
        assert_eq!(w.entity(bob).unwrap().get::<It>(), Some(&It(true)));
        assert_eq!(w.events::<Tagged>(), vec![Tagged { by: alice, who: bob }]);
        w.step(); // the reply reaches Alice
        assert_eq!(w.entity(alice).unwrap().get::<It>(), Some(&It(false)));
    }
}
```

More in [Testing game rules](/Worldmesh/guides/testing/).

## 8. Run it

`main` hands the world to Worldmesh, and the command line picks the role:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    worldmesh::run(world()).await
}
```

With NATS running on port 4222:

```sh
hello orchestrator --groups 2 --spatial 2 &
hello group & hello group &
hello spatial & hello spatial &
hello gateway --listen 0.0.0.0:8080 --allow-debug &
```

Players connect to `ws://localhost:8080/ws`. For the browser side, see [The browser client](/Worldmesh/guides/client/).

## Where to go next

- [Plugins, components and messages](/Worldmesh/concepts/plugins-and-components/): splitting a bigger game into plugins that work together.
- `examples/medieval`: movement, combat and bots as three plugins, with a browser client.

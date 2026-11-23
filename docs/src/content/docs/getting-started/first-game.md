---
title: Your first game
description: Build tag on Worldmesh, step by step, test it without servers, and run it.
---

This guide builds tag: players walk around, and whoever is "it" can tag another player within reach. It's small, but it uses everything a real game does: timed inputs, a command, actions in both directions, an event, and a check that can't be cheated. The finished code is `examples/hello` in the repository.

## 1. Create the crate

```toml
# Cargo.toml
[dependencies]
worldmesh = "0.1"
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
anyhow = "1"
```

## 2. Say what travels

A game is a handful of types. Each one crosses the network at some point, so each derives `Serialize`, `Deserialize` and `Clone`.

```rust
use serde::{Deserialize, Serialize};
use worldmesh::{time, Entity, EntityId, Game, Incoming, Me, Spawn, Timed, To, ViewShape, WorldConfig};

struct Tag;

/// What each player's entity holds. Moves with it between servers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Player {
    dir: (f32, f32),
    it: bool,
}

/// What everyone else sees of a player. Keep it small: it goes to every client that can see it.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Look {
    it: bool,
}

/// A player's input: the direction to walk in, from now on.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Walk {
    dx: f32,
    dy: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
enum Command {
    Tag { target: EntityId },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
enum Action {
    YouAreIt,
    NotIt,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "k", rename_all = "snake_case")]
enum Event {
    Tagged { by: EntityId, who: EntityId },
}
```

The `tag = "k"` style makes each one read naturally as JSON in the browser: `{"k":"tag","target":2}`.

## 3. Describe the world

```rust
impl Game for Tag {
    type Data = Player;
    type View = Look;
    type Input = Walk;
    type Command = Command;
    type Action = Action;
    type Event = Event;
    type Welcome = ();

    fn config(&self) -> WorldConfig {
        WorldConfig {
            width: 64,
            height: 64,
            chunk_size: 8,
            view: ViewShape::Rect { half_w: 20.0, half_h: 12.0 },
            input_substeps: 16,
        }
    }

    fn welcome(&self) {}

    fn join(&self, player: EntityId) -> Spawn<Player> {
        // Player 1 starts out it.
        Spawn { x: 30.5 + (player % 4) as f32, y: 32.5, data: Player { it: player == 1, ..Player::default() } }
    }

    fn view(&self, e: &Entity<Player>) -> Look {
        Look { it: e.data.it }
    }
```

`config` must be the same in every process (it's your code, so it is). `join` runs in the gateway when a player connects; the orchestrator then picks a group for the new entity.

## 4. One tick of one entity

`tick` runs once per tick for every entity, in whichever group simulates it. A player's inputs come with the sub-step of the tick they take effect at, so a key released half way through a tick stops you half way, on the server and in the client's prediction alike.

```rust
    fn tick(&self, me: &mut Me<'_, Self>, inputs: &[Timed<Walk>]) {
        let substeps = self.config().input_substeps;
        let per_step = 4.0 * time::tick_ms() as f32 / 1000.0 / f32::from(substeps);
        let mut at = 0;
        for t in inputs {
            walk(me, per_step * f32::from(t.sub - at));
            at = t.sub;
            let len = t.input.dx.hypot(t.input.dy);
            me.data.dir = if len > 1.0 { (t.input.dx / len, t.input.dy / len) } else { (t.input.dx, t.input.dy) };
        }
        walk(me, per_step * f32::from(substeps - at));
    }
```

`me` dereferences to the entity, so `me.x`, `me.y` and `me.data` are right there. Move it freely: where it stands decides who can see it.

## 5. Commands and actions

A command comes from the entity's own player. Here it sends an action to the target, wherever the target is:

```rust
    fn on_command(&self, me: &mut Me<'_, Self>, command: Command) {
        let Command::Tag { target } = command;
        if me.data.it {
            me.send(To::Entity(target), Action::YouAreIt);
        }
    }
```

The action is delivered on the next tick, in the target's group, even if that's another machine, even if the target is moving between groups right then. The target decides what it does:

```rust
    fn on_action(&self, me: &mut Me<'_, Self>, action: Action, from: &Incoming) {
        match action {
            Action::YouAreIt => {
                let (x, y) = me.position_at(from.from.tick);
                if (x - from.from.x).hypot(y - from.from.y) <= 1.5 && !me.data.it {
                    me.data.it = true;
                    me.send(To::Entity(from.from.id), Action::NotIt);
                    let who = me.id;
                    me.emit_involving(Event::Tagged { by: from.from.id, who }, &[from.from.id]);
                }
            }
            Action::NotIt => me.data.it = false,
        }
    }
}
```

Two things make this check fair:

- `from.from` (the sender's id, position and tick) is stamped by the sender's group, never by a client, so a hacked client can't claim to be closer than it is.
- `me.position_at(tick)` is where the target stood when the tag was sent, not a tick later when it lands, so a target running away doesn't dodge a tag that was in reach.

See [Entities and actions](/Worldmesh/concepts/entities-and-actions/) for more.

## 6. Test it without servers

`TestWorld` runs real groups and Spatial Layer Cells in one process and passes their messages as the bus would, one tick at a time:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use worldmesh::testing::TestWorld;

    #[test]
    fn a_tag_in_reach_passes_it_on_even_across_groups() {
        let mut w = TestWorld::new(Tag, 2, 2); // 2 groups, 2 cells
        let alice = w.join_in(1, 1);           // it
        let bob = w.join_in(2, 2);             // in the other group, a tile away
        w.step();
        w.command(alice, Command::Tag { target: bob });
        w.run(2); // the command sends the tag; it lands on the next tick
        assert!(w.entity(bob).unwrap().data.it);
        w.step(); // the reply reaches Alice
        assert!(!w.entity(alice).unwrap().data.it);
    }
}
```

More in [Testing game rules](/Worldmesh/guides/testing/).

## 7. Run it

`main` hands the game to Worldmesh, and the command line picks the role:

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    worldmesh::run(Tag).await
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

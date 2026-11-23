//! The smallest Worldmesh game: tag. Players walk around; whoever is "it" can tag another
//! player within reach, wherever on the servers that player is simulated.
//!
//! It's the code from the "Your first game" guide. Run every role from this one binary:
//! `hello orchestrator`, `hello group`, `hello spatial`, `hello gateway` (see `--help`).

use serde::{Deserialize, Serialize};
use worldmesh::{
    time, Bundle, Component, EntityId, Incoming, Me, Message, Plugin, Spawn, Stage, To, ViewShape,
    World, WorldConfig,
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

/// A player's input: the direction to walk in, from now on.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Walk {
    dx: f32,
    dy: f32,
}

/// A player's command: tag someone.
#[derive(Message, Debug, Clone, Serialize, Deserialize)]
struct Tag {
    target: EntityId,
}

/// An action from whoever is it: "you're it now" (if they really reached you).
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

/// The whole game, as one plugin.
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
                Spawn::new("player", 30.5 + (player % 4) as f32, 32.5).with(It(player == 1))
            });
    }
}

fn tag(me: &mut Me<'_>, &Tag { target }: &Tag) {
    if me.get::<It>() == Some(&It(true)) {
        me.send(To::Entity(target), YouAreIt);
    }
}

/// Judged here, by where we stood on the tick the tag was sent. The sender's position comes
/// from its own group, never from a client.
fn you_are_it(me: &mut Me<'_>, _: &YouAreIt, from: &Incoming) {
    let (x, y) = me.position_at(from.from.tick);
    let in_reach = (x - from.from.x).hypot(y - from.from.y) <= REACH;
    if in_reach && me.get::<It>() != Some(&It(true)) {
        me.insert(It(true));
        me.send(To::Entity(from.from.id), NotIt);
        let who = me.id;
        me.emit_involving(
            Tagged {
                by: from.from.id,
                who,
            },
            &[from.from.id],
        );
    }
}

/// Walk, changing direction at the exact sub-step each input asked for.
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
        let len = t.input.dx.hypot(t.input.dy);
        heading = if len > 1.0 {
            Heading {
                dx: t.input.dx / len,
                dy: t.input.dy / len,
            }
        } else {
            Heading {
                dx: t.input.dx,
                dy: t.input.dy,
            }
        };
    }
    step(me, heading, per_step * f32::from(substeps - at));
    me.insert(heading);
}

fn step(me: &mut Me<'_>, h: Heading, distance: f32) {
    me.x = (me.x + h.dx * distance).clamp(0.0, 63.99);
    me.y = (me.y + h.dy * distance).clamp(0.0, 63.99);
}

fn world() -> World {
    let mut world = World::new(WorldConfig {
        width: 64,
        height: 64,
        chunk_size: 8,
        view: ViewShape::Rect {
            half_w: 20.0,
            half_h: 12.0,
        },
        input_substeps: 16,
    });
    world.plugin(TagGame);
    world
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    worldmesh::run(world()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use worldmesh::testing::TestWorld;

    fn it(w: &TestWorld, id: EntityId) -> bool {
        w.entity(id).unwrap().get::<It>() == Some(&It(true))
    }

    #[test]
    fn a_tag_in_reach_passes_it_on_even_across_groups() {
        let mut w = TestWorld::new(world(), 2, 2);
        let alice = w.join_in(1, 1); // it, at x 31.5
        let bob = w.join_in(2, 2); // in another group, a tile away
        w.step();
        w.command(alice, Tag { target: bob });
        w.run(2); // the command sends the tag; it lands on the next tick
        assert!(it(&w, bob));
        assert_eq!(
            w.events::<Tagged>(),
            vec![Tagged {
                by: alice,
                who: bob
            }]
        );
        w.step(); // the reply reaches Alice
        assert!(!it(&w, alice));
        assert!(
            w.view(alice)
                .iter()
                .any(|s| s.id == bob && s.get::<It>() == Some(It(true))),
            "Alice can see Bob is it"
        );
    }

    #[test]
    fn a_tag_out_of_reach_does_nothing() {
        let mut w = TestWorld::new(world(), 1, 1);
        let (alice, bob) = (w.join(1), w.join(2));
        w.input(bob, 0, Walk { dx: 1.0, dy: 0.0 });
        w.run(10); // Bob walks 4 tiles away
        w.command(alice, Tag { target: bob });
        w.run(3);
        assert!(it(&w, alice) && !it(&w, bob));
    }
}

//! The smallest Worldmesh game: tag. Players walk around; whoever is "it" can tag another
//! player within reach, wherever on the servers that player is simulated.
//!
//! It's the code from the "Your first game" guide. Run every role from this one binary:
//! `hello orchestrator`, `hello group`, `hello spatial`, `hello gateway` (see `--help`).

use serde::{Deserialize, Serialize};
use worldmesh::{time, Entity, EntityId, Game, Incoming, Me, Spawn, Timed, To, ViewShape, WorldConfig};

/// How far a tag reaches, in tiles.
const REACH: f32 = 1.5;
/// Walking speed, in tiles per second.
const SPEED: f32 = 4.0;

struct Tag;

/// What each player's entity holds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Player {
    /// The direction they're walking in.
    dir: (f32, f32),
    it: bool,
}

/// What everyone else sees of a player.
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
    /// From whoever is it: "you're it now" (if they really reached you).
    YouAreIt,
    /// Back to the tagger: the tag landed, they're not it any more.
    NotIt,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "k", rename_all = "snake_case")]
enum Event {
    Tagged { by: EntityId, who: EntityId },
}

impl Game for Tag {
    type Data = Player;
    type View = Look;
    type Input = Walk;
    type Command = Command;
    type Action = Action;
    type Event = Event;
    type Welcome = ();

    fn config(&self) -> WorldConfig {
        WorldConfig { width: 64, height: 64, chunk_size: 8, view: ViewShape::Rect { half_w: 20.0, half_h: 12.0 }, input_substeps: 16 }
    }

    fn welcome(&self) {}

    fn join(&self, player: EntityId) -> Spawn<Player> {
        // Player 1 starts out it.
        Spawn { x: 30.5 + (player % 4) as f32, y: 32.5, data: Player { it: player == 1, ..Player::default() } }
    }

    fn view(&self, e: &Entity<Player>) -> Look {
        Look { it: e.data.it }
    }

    /// Walk, changing direction at the exact sub-step each input asked for.
    fn tick(&self, me: &mut Me<'_, Self>, inputs: &[Timed<Walk>]) {
        let substeps = self.config().input_substeps;
        let per_step = SPEED * time::tick_ms() as f32 / 1000.0 / f32::from(substeps);
        let mut at = 0;
        for t in inputs {
            walk(me, per_step * f32::from(t.sub - at));
            at = t.sub;
            let len = t.input.dx.hypot(t.input.dy);
            me.data.dir = if len > 1.0 { (t.input.dx / len, t.input.dy / len) } else { (t.input.dx, t.input.dy) };
        }
        walk(me, per_step * f32::from(substeps - at));
    }

    fn on_command(&self, me: &mut Me<'_, Self>, command: Command) {
        let Command::Tag { target } = command;
        if me.data.it {
            me.send(To::Entity(target), Action::YouAreIt);
        }
    }

    fn on_action(&self, me: &mut Me<'_, Self>, action: Action, from: &Incoming) {
        match action {
            Action::YouAreIt => {
                // Judged here, by where we stood on the tick the tag was sent. The sender's
                // position comes from its own group, never from a client.
                let (x, y) = me.position_at(from.from.tick);
                if (x - from.from.x).hypot(y - from.from.y) <= REACH && !me.data.it {
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

fn walk(me: &mut Me<'_, Tag>, distance: f32) {
    let (dx, dy) = me.data.dir;
    me.x = (me.x + dx * distance).clamp(0.0, 63.99);
    me.y = (me.y + dy * distance).clamp(0.0, 63.99);
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    worldmesh::run(Tag).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use worldmesh::testing::TestWorld;

    #[test]
    fn a_tag_in_reach_passes_it_on_even_across_groups() {
        let mut w = TestWorld::new(Tag, 2, 2);
        let alice = w.join_in(1, 1); // it, at x 31.5
        let bob = w.join_in(2, 2); // in another group, a tile away
        w.step();
        w.command(alice, Command::Tag { target: bob });
        w.run(2); // the command sends the tag; it lands on the next tick
        assert!(w.entity(bob).unwrap().data.it);
        assert_eq!(w.events(), &[Event::Tagged { by: alice, who: bob }]);
        w.step(); // the reply reaches Alice
        assert!(!w.entity(alice).unwrap().data.it);
    }

    #[test]
    fn a_tag_out_of_reach_does_nothing() {
        let mut w = TestWorld::new(Tag, 1, 1);
        let (alice, bob) = (w.join(1), w.join(2));
        w.input(bob, 0, Walk { dx: 1.0, dy: 0.0 });
        w.run(10); // Bob walks 4 tiles away
        w.command(alice, Command::Tag { target: bob });
        w.run(3);
        assert!(w.entity(alice).unwrap().data.it && !w.entity(bob).unwrap().data.it);
    }
}

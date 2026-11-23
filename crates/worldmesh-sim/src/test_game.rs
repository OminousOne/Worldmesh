//! A tiny game used by the library's own tests: entities walk one tile per tick, and players
//! can hit, blast, shout, spawn and vanish. Shared with `worldmesh-testing`'s tests through the
//! `test-game` feature.

use serde::{Deserialize, Serialize};
use worldmesh_core::config::{ViewShape, WorldConfig};
use worldmesh_core::protocol::EntityId;

use crate::game::{Entity, Game, Incoming, Me, Spawn, Timed, To};

// ---- A tiny game: entities walk at one tile per tick and take damage ----

pub struct TestGame;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Data {
    pub hp: i32,
    pub dir: (f32, f32),
    /// What happened to it: "dmg <n> from <sender> on <tick>[ fwd][ area][ all]".
    pub log: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct View {
    pub hp: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Input {
    pub dx: f32,
    pub dy: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    Hit { target: u32 },
    Blast { x: f32, y: f32, r: f32 },
    Shout,
    Spawn,
    Vanish,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Action {
    Damage(i32),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "e", rename_all = "snake_case")]
pub enum Event {
    Hurt { id: u32, hp: i32 },
}

const SUB: f32 = 16.0;

impl Game for TestGame {
    type Data = Data;
    type View = View;
    type Input = Input;
    type Command = Command;
    type Action = Action;
    type Event = Event;
    type Welcome = ();

    fn config(&self) -> WorldConfig {
        WorldConfig { width: 64, height: 64, chunk_size: 8, view: ViewShape::Rect { half_w: 20.0, half_h: 12.0 }, input_substeps: 16 }
    }

    fn welcome(&self) {}

    fn join(&self, _player: EntityId) -> Spawn<Data> {
        Spawn { x: 10.5, y: 10.5, data: Data { hp: 100, ..Data::default() } }
    }

    fn view(&self, e: &Entity<Data>) -> View {
        View { hp: e.data.hp }
    }

    /// One tile per tick in the held direction, changing direction at each input's sub-step.
    fn tick(&self, me: &mut Me<'_, Self>, inputs: &[Timed<Input>]) {
        let mut from = 0.0;
        for t in inputs {
            let at = f32::from(t.sub);
            let (dx, dy) = me.data.dir;
            me.x += dx * (at - from) / SUB;
            me.y += dy * (at - from) / SUB;
            from = at;
            me.data.dir = (t.input.dx, t.input.dy);
        }
        let (dx, dy) = me.data.dir;
        me.x += dx * (SUB - from) / SUB;
        me.y += dy * (SUB - from) / SUB;
    }

    fn on_command(&self, me: &mut Me<'_, Self>, command: Command) {
        match command {
            Command::Hit { target } => {
                me.send(To::Entity(target), Action::Damage(10));
            }
            Command::Blast { x, y, r } => {
                me.send(To::Area { x, y, r }, Action::Damage(5));
            }
            Command::Shout => {
                me.send(To::Everyone, Action::Damage(1));
            }
            Command::Spawn => {
                let (x, y) = (me.x + 1.0, me.y);
                me.spawn(x, y, Data { hp: 7, ..Data::default() });
            }
            Command::Vanish => me.despawn(),
        }
    }

    fn on_action(&self, me: &mut Me<'_, Self>, action: Action, from: &Incoming) {
        let Action::Damage(n) = action;
        me.data.hp -= n;
        let mut line = format!("dmg {n} from {} on {}", from.from.id, me.tick());
        if from.forwarded {
            line += " fwd";
        }
        if from.area.is_some() {
            line += " area";
        }
        if from.everyone {
            line += " all";
        }
        me.data.log.push(line);
        let (id, hp) = (me.id, me.data.hp);
        me.emit(Event::Hurt { id, hp });
    }
}

// ---- Helpers ----

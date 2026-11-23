//! A tiny game used by the library's own tests: units walk one tile per tick, and players can
//! hit, blast, shout, spawn and vanish. Shared with `worldmesh-testing`'s tests through the
//! `test-game` feature.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use worldmesh_core::config::{ViewShape, WorldConfig};

use crate::component::{Bundle, Component, Message};
use crate::entity::Spawn;
use crate::me::{Incoming, Me, To};
use crate::world::{Rules, Stage, World};

/// How much health a unit has. Visible.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Health(pub i32);
impl Component for Health {
    const NAME: &'static str = "health";
}

/// The direction a unit walks in.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Walker {
    pub dir: (f32, f32),
}
impl Component for Walker {
    const NAME: &'static str = "walker";
}

/// What happened to a unit: "dmg <n> from <sender> on <tick>[ fwd][ area][ all]".
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Log(pub Vec<String>);
impl Component for Log {
    const NAME: &'static str = "log";
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
impl Message for Command {
    const NAME: &'static str = "command";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Damage(pub i32);
impl Message for Damage {
    const NAME: &'static str = "damage";
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Hurt {
    pub id: u32,
    pub hp: i32,
}
impl Message for Hurt {
    const NAME: &'static str = "hurt";
}

const SUB: f32 = 16.0;

/// One tile per tick in the held direction, changing direction at each input's sub-step.
fn walk(me: &mut Me<'_>) {
    let inputs = me.inputs::<Input>();
    let Some(mut dir) = me.get::<Walker>().map(|w| w.dir) else { return };
    let mut from = 0.0;
    for t in inputs {
        let at = f32::from(t.sub);
        me.x += dir.0 * (at - from) / SUB;
        me.y += dir.1 * (at - from) / SUB;
        from = at;
        dir = (t.input.dx, t.input.dy);
    }
    me.x += dir.0 * (SUB - from) / SUB;
    me.y += dir.1 * (SUB - from) / SUB;
    if let Some(w) = me.get_mut::<Walker>() {
        w.dir = dir;
    }
}

fn command(me: &mut Me<'_>, command: &Command) {
    match *command {
        Command::Hit { target } => {
            me.send(To::Entity(target), Damage(10));
        }
        Command::Blast { x, y, r } => {
            me.send(To::Area { x, y, r }, Damage(5));
        }
        Command::Shout => {
            me.send(To::Everyone, Damage(1));
        }
        Command::Spawn => {
            let (x, y) = (me.x + 1.0, me.y);
            me.spawn(Spawn::new("unit", x, y).with(Health(7)));
        }
        Command::Vanish => me.despawn(),
    }
}

fn damage(me: &mut Me<'_>, &Damage(n): &Damage, from: &Incoming) {
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
    let Some(hp) = me.get_mut::<Health>().map(|h| {
        h.0 -= n;
        h.0
    }) else {
        return;
    };
    if let Some(log) = me.get_mut::<Log>() {
        log.0.push(line);
    }
    let id = me.id;
    me.emit(Hurt { id, hp });
}

pub fn config() -> WorldConfig {
    WorldConfig { width: 64, height: 64, chunk_size: 8, view: ViewShape::Rect { half_w: 20.0, half_h: 12.0 }, input_substeps: 16 }
}

/// The test game's world.
pub fn world() -> World {
    let mut w = World::new(config());
    w.visible::<Health>()
        .component::<Walker>()
        .component::<Log>()
        .input::<Input>()
        .command(command)
        .action(damage)
        .event::<Hurt>()
        .system_for::<Walker>(Stage::Update, walk)
        .kind("unit", Bundle::new().with(Health(100)).with(Walker::default()).with(Log::default()))
        .join(|_player| Spawn::new("unit", 10.5, 10.5));
    w
}

pub fn rules() -> Arc<Rules> {
    world().build()
}

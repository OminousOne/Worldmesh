//! The game: what a unit holds, what players send, what units do to each other, and one tick
//! of one unit. Worldmesh runs this in whichever Server Entity Group simulates each unit.

use serde::{Deserialize, Serialize};
use worldmesh::{time, Entity, EntityId, Game, Incoming, Me, Spawn, Timed, To, ViewShape, WorldConfig};

use crate::bots::{self, BotMode, Brain};
use crate::combat::{self, Ability, Aim, MAX_HP, RANGE_SLACK, RESPAWN_MS};
use crate::map::{self, splitmix64, MapData};
use crate::movement::{self, SUBSTEPS};
use crate::{MAP_HEIGHT, MAP_WIDTH, WORLD_SEED};

/// Most bots one "spawn bots" command adds.
pub const MAX_BOTS_PER_SPAWN: u32 = 200;

pub struct Medieval {
    pub map: MapData,
}

impl Medieval {
    pub fn new() -> Self {
        Self { map: map::generate(WORLD_SEED, MAP_WIDTH, MAP_HEIGHT) }
    }
}

impl Default for Medieval {
    fn default() -> Self {
        Self::new()
    }
}

/// What every unit (player or bot) holds. Travels with it between groups.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Unit {
    pub hp: u16,
    /// Direction held at the end of the last tick.
    pub dir: (f32, f32),
    /// Dead until this tick, then it respawns.
    pub dead_until: Option<u64>,
    /// The tick each ability is ready again.
    pub cooldowns: Vec<(Ability, u64)>,
    /// Who it's fighting (bots).
    pub target: Option<EntityId>,
    /// Bots only.
    pub bot: Option<Brain>,
}

impl Unit {
    pub fn player() -> Self {
        Self { hp: MAX_HP, ..Self::default() }
    }

    pub fn bot(mode: BotMode) -> Self {
        Self { hp: MAX_HP, bot: Some(Brain::new(mode)), ..Self::default() }
    }
}

/// What others see of a unit (clients, and other units' "what's near me?").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Look {
    pub hp: u16,
    pub dead: bool,
    pub bot: bool,
    pub target: Option<EntityId>,
}

/// A player's movement: the direction to walk in (length at most 1; zero to stop).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Move {
    pub dx: f32,
    pub dy: f32,
}

/// What an ability is aimed at.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Entity(EntityId),
    Point(f32, f32),
}

/// A player's requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum Command {
    Use { ability: Ability, target: Target },
    /// Add bots at random places (a testing tool, exposed in the example's debug panel).
    SpawnBots { count: u32, mode: BotMode },
    /// Remove every bot in the world.
    ClearBots,
}

/// What units do to each other.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum Action {
    /// An ability landing: on its target (which checks the reach), or on everyone in its circle.
    Hit { ability: Ability },
    /// Bots leave the world.
    Vanish,
}

/// What players see happen.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum Event {
    /// The user's side accepted a use and sent it (an arrow flies, a fireball launches).
    Used { tick: u64, actor: EntityId, ability: Ability, target: Target, x: f32, y: f32, group: u32, trace: u64 },
    /// The target's side checked it and applied it (or not) to one unit.
    Effect {
        tick: u64,
        actor: EntityId,
        target: EntityId,
        ability: Ability,
        damage: u16,
        /// The target's HP afterwards.
        hp: u16,
        outcome: Outcome,
        /// The group that sent it and the one that applied it.
        from_group: u32,
        to_group: u32,
        /// Ticks between the use and the effect (always at least 1, wherever the target is).
        delay_ticks: u64,
        /// It reached the target's previous group first and was forwarded after a handoff.
        forwarded: bool,
        trace: u64,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Hit,
    Killed,
    /// Out of reach where the target stood when it was used (it moved, or the client asked
    /// for too much).
    OutOfRange,
    /// A wall in the way.
    Blocked,
}

/// Sent to every player on connect.
#[derive(Debug, Clone, Serialize)]
pub struct Welcome {
    pub map: MapData,
}

impl Game for Medieval {
    type Data = Unit;
    type View = Look;
    type Input = Move;
    type Command = Command;
    type Action = Action;
    type Event = Event;
    type Welcome = Welcome;

    fn config(&self) -> WorldConfig {
        WorldConfig {
            width: MAP_WIDTH,
            height: MAP_HEIGHT,
            chunk_size: 8,
            // A 2:1 isometric screen of about 2048 x 1152 px around the player.
            view: ViewShape::Iso { half_w: 32.0, half_h: 36.0 },
            input_substeps: SUBSTEPS,
        }
    }

    fn welcome(&self) -> Welcome {
        Welcome { map: self.map.clone() }
    }

    fn join(&self, player: EntityId) -> Spawn<Unit> {
        let (x, y) = self.map.spawn_point(player);
        Spawn { x, y, data: Unit::player() }
    }

    fn view(&self, e: &Entity<Unit>) -> Look {
        Look { hp: e.data.hp, dead: e.data.dead_until.is_some(), bot: e.data.bot.is_some(), target: e.data.target }
    }

    /// Bots decide, everyone moves (switching direction at the exact sub-step each input
    /// asked for), the dead come back, bots ask what's around them for next tick.
    fn tick(&self, me: &mut Me<'_, Self>, inputs: &[Timed<Move>]) {
        if me.data.bot.is_some() {
            bots::think(self, me);
        }
        let changes: Vec<movement::Change> =
            inputs.iter().map(|t| (t.sub, movement::sanitize_dir(t.input.dx, t.input.dy))).collect();
        let start = me.data.dir;
        if me.data.dead_until.is_none() {
            let (x, y, end) = movement::simulate_tick(&self.map, me.x, me.y, start, &changes);
            (me.x, me.y, me.data.dir) = (x, y, end);
        } else {
            me.data.dir = changes.last().map(|c| c.1).unwrap_or(start); // the dead don't move
        }
        self.respawn(me);
        if me.data.bot.is_some() {
            bots::ask(me);
        }
    }

    fn on_command(&self, me: &mut Me<'_, Self>, command: Command) {
        match command {
            Command::Use { ability, target } => self.use_ability(me, ability, target),
            Command::SpawnBots { count, mode } => {
                let seed = splitmix64(me.tick() ^ (u64::from(me.id) << 40) ^ me.trace());
                for i in 0..count.min(MAX_BOTS_PER_SPAWN) {
                    let (x, y) = self.map.random_point(splitmix64(seed ^ u64::from(i)));
                    me.spawn(x, y, Unit::bot(mode));
                }
            }
            Command::ClearBots => {
                me.send(To::Everyone, Action::Vanish);
            }
        }
    }

    fn on_action(&self, me: &mut Me<'_, Self>, action: Action, from: &Incoming) {
        match action {
            Action::Hit { ability } => self.take_hit(me, ability, from),
            Action::Vanish => {
                if me.data.bot.is_some() {
                    me.despawn();
                }
            }
        }
    }
}

impl Medieval {
    /// The user's side, where the user is simulated: alive, cooldown ready, a point in reach.
    /// Then the hit goes to its target's address, wherever that is.
    pub fn use_ability(&self, me: &mut Me<'_, Self>, ability: Ability, target: Target) {
        let stats = ability.stats();
        let tick = me.tick();
        let ready = me.data.cooldowns.iter().find(|(a, _)| *a == ability).is_none_or(|&(_, at)| tick >= at);
        if me.data.dead_until.is_some() || !ready {
            return;
        }
        let to = match (stats.aim, target) {
            (Aim::Entity, Target::Entity(t)) if t != me.id => To::Entity(t),
            // A point is just a place: the user's side can check the reach to it itself.
            (Aim::Point, Target::Point(x, y)) if combat::reaches(&self.map, ability, me.x, me.y, x, y, 0.0) => {
                To::Area { x, y, r: stats.radius }
            }
            _ => return,
        };
        me.data.cooldowns.retain(|(a, _)| *a != ability);
        me.data.cooldowns.push((ability, tick + time::ticks_in(stats.cooldown_ms)));
        let trace = me.send(to, Action::Hit { ability });
        let (actor, x, y, group) = (me.id, me.x, me.y, me.group());
        me.emit(Event::Used { tick, actor, ability, target, x, y, group, trace });
    }

    /// The target's side, where the target is simulated. For a single target: was it in
    /// reach (and in sight) where it stood on the tick the hit was sent? Area hits already
    /// landed where they were aimed.
    fn take_hit(&self, me: &mut Me<'_, Self>, ability: Ability, from: &Incoming) {
        if me.data.dead_until.is_some() {
            return;
        }
        let stats = ability.stats();
        let sender = from.from;
        let tick = me.tick();
        let (tx, ty) = me.position_at(sender.tick);
        let mut outcome = if from.area.is_some() {
            Outcome::Hit
        } else if combat::distance(sender.x, sender.y, tx, ty) > stats.range + RANGE_SLACK {
            Outcome::OutOfRange
        } else if stats.needs_line_of_sight && !combat::line_of_sight(&self.map, sender.x, sender.y, tx, ty) {
            Outcome::Blocked
        } else {
            Outcome::Hit
        };
        let mut damage = 0;
        if outcome == Outcome::Hit {
            damage = stats.damage.min(me.data.hp);
            me.data.hp -= damage;
            if me.data.hp == 0 {
                outcome = Outcome::Killed;
                me.data.dead_until = Some(tick + time::ticks_in(RESPAWN_MS));
                me.data.target = None;
            } else {
                bots::felt(me, &sender);
            }
        }
        let event = Event::Effect {
            tick,
            actor: sender.id,
            target: me.id,
            ability,
            damage,
            hp: me.data.hp,
            outcome,
            from_group: sender.group,
            to_group: me.group(),
            delay_ticks: tick.saturating_sub(sender.tick),
            forwarded: from.forwarded,
            trace: from.trace,
        };
        // The attacker's player sees the result even if it landed off their screen.
        me.emit_involving(event, &[sender.id]);
    }

    /// The dead come back with full HP: players in town, bots anywhere.
    fn respawn(&self, me: &mut Me<'_, Self>) {
        let tick = me.tick();
        if me.data.dead_until.is_none_or(|until| tick < until) {
            return;
        }
        let id = me.id;
        let (x, y) =
            if me.data.bot.is_some() { self.map.random_point(splitmix64(u64::from(id) ^ (tick << 20))) } else { self.map.spawn_point(id) };
        (me.x, me.y) = (x, y);
        (me.data.hp, me.data.dead_until, me.data.target) = (MAX_HP, None, None);
        if let Some(brain) = &mut me.data.bot {
            brain.wander = None;
            brain.felt = None;
        }
    }
}

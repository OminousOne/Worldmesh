//! Bots: the [`Bots`] plugin. Bot AI runs in whichever group simulates the bot. A bot knows
//! its own group's units exactly and everyone else from its latest spatial query results
//! (a tick or two old), and fights exactly like a player does: through
//! [`combat::use_ability`]. Its randomness is seeded by (world, bot, tick), so moving to
//! another group never changes what it decides.
//!
//! It works with the other plugins only through their components: it steers by setting its
//! [`Heading`], reads [`Dead`] and [`LastHit`], and sets [`Fighting`].

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use worldmesh::{
    time, Bundle, Component, EntityId, Me, Message, Plugin, Query, Spawn, Stage, To, World,
};

use crate::combat::{self, distance, Ability, Dead, Fighting, LastHit, Target};
use crate::map::{splitmix64, MapData};
use crate::movement::{self, Heading};
use crate::WORLD_SEED;

/// How far an idle bot looks for a fight. Has nothing to do with ability ranges.
pub const SENSE_RANGE: f32 = 6.0;
/// A bot keeps chasing a target this far.
pub const CHASE_RANGE: f32 = 12.0;
/// How many units a bot's question asks for, nearest first (plus its target). 0: everyone in
/// the circle. Everyone for now; this is the hook for pruning the list once gameplay says what
/// a bot actually needs (nearest N, weakest N, players only, ...).
pub const SENSE_NEAREST: u16 = 0;
/// How often a bot that cares asks what's around it.
const SENSE_EVERY_MS: u64 = 200;
/// How long a bot remembers where whoever hit it stood (until its senses say more).
const FELT_FOR_MS: u64 = 1000;

/// How a bot behaves.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum BotMode {
    /// Picks fights with whoever comes near.
    Aggressive,
    /// Wanders; fights back when hit.
    #[default]
    FightBack,
    /// Never fights, not even back: just keeps running around.
    Wander,
}

/// A bot's state of mind. A component, so it travels with the bot between groups.
#[derive(Component, Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Brain {
    pub mode: BotMode,
    /// Where it's wandering to, and when it gives up.
    pub wander: Option<(f32, f32)>,
    pub wander_until: u64,
    pub idle_until: u64,
    /// Where it stood when it last thought (to notice it's stuck on a wall).
    pub last_pos: (f32, f32),
    /// Whoever last hit it: (id, x, y, tick), so it can fight back before it asks around.
    pub felt: Option<(EntityId, f32, f32, u64)>,
}

impl Brain {
    pub fn new(mode: BotMode) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }

    fn fights(&self) -> bool {
        self.mode != BotMode::Wander
    }
}

/// A player's command: add bots at random places (a testing tool, in the example's debug panel).
#[derive(Message, Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct SpawnBots {
    pub count: u32,
    pub mode: BotMode,
}

/// A player's command: remove every bot in the world.
#[derive(Message, Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ClearBots;

/// Sent to everyone by [`ClearBots`]: bots leave the world.
#[derive(Message, Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Vanish;

/// Most bots one [`SpawnBots`] adds.
pub const MAX_BOTS_PER_SPAWN: u32 = 200;

/// Bots that wander, pick fights and fight back.
pub struct Bots {
    pub map: Arc<MapData>,
}

impl Plugin for Bots {
    fn build(&self, world: &mut World) {
        let (think_map, spawn_map) = (self.map.clone(), self.map.clone());
        world
            .component::<Brain>()
            .command(move |me, s: &SpawnBots| spawn_bots(&spawn_map, me, s))
            .command(|me, _: &ClearBots| {
                me.send(To::Everyone, Vanish);
            })
            .action(|me, _: &Vanish, _| {
                if me.has::<Brain>() {
                    me.despawn();
                }
            })
            .system_for::<Brain>(Stage::First, move |me| think(&think_map, me))
            .system_for::<Brain>(Stage::Last, ask)
            .kind("bot", Bundle::new().with(Brain::default()));
    }
}

fn spawn_bots(map: &MapData, me: &mut Me<'_>, &SpawnBots { count, mode }: &SpawnBots) {
    let seed = splitmix64(me.tick() ^ (u64::from(me.id) << 40) ^ me.trace());
    for i in 0..count.min(MAX_BOTS_PER_SPAWN) {
        let (x, y) = map.random_point(splitmix64(seed ^ u64::from(i)));
        me.spawn(Spawn::new("bot", x, y).with(Brain::new(mode)));
    }
}

/// What `me` knows of unit `id`: (x, y, dead). Exact if it's in this group, otherwise from its
/// senses, or where it stood when it hit us.
fn sense(me: &Me<'_>, brain: &Brain, id: EntityId) -> Option<(f32, f32, bool)> {
    if let Some(n) = me.find(id) {
        return Some((n.x, n.y, n.has::<Dead>()));
    }
    let felt = brain.felt?;
    (felt.0 == id && felt.3 + time::ticks_in(FELT_FOR_MS) >= me.tick())
        .then_some((felt.1, felt.2, false))
}

/// The nearest living unit `me` knows of within `radius`.
fn nearest_other(me: &Me<'_>, radius: f32) -> Option<EntityId> {
    let (x, y) = (me.x, me.y);
    me.nearby(radius)
        .into_iter()
        .filter(|n| !n.has::<Dead>())
        .min_by(|a, b| {
            distance(x, y, a.x, a.y)
                .total_cmp(&distance(x, y, b.x, b.y))
                .then(a.id.cmp(&b.id))
        })
        .map(|n| n.id)
}

/// Decide where to go and what to use this tick.
fn think(map: &MapData, me: &mut Me<'_>) {
    let Some(mut brain) = me.get::<Brain>().cloned() else {
        return;
    };
    if me.has::<Dead>() {
        // Nothing to do; it comes back somewhere else with a fresh mind.
        (brain.wander, brain.felt) = (None, None);
        me.insert(brain);
        return;
    }
    // Hit since we last thought: remember where from, and fight back (unless we never fight).
    if let Some(hit) = me.remove::<LastHit>() {
        if brain.fights() {
            brain.felt = Some((hit.by, hit.x, hit.y, hit.tick));
            if !me.has::<Fighting>() {
                me.insert(Fighting(hit.by));
            }
        }
    }
    let tick = me.tick();
    let (id, x, y) = (me.id, me.x, me.y);
    let rng = splitmix64(WORLD_SEED ^ (u64::from(id) << 32) ^ tick);

    // Keep a target only while it can be sensed, alive and close; aggressive bots look for one.
    let mut target = me.get::<Fighting>().map(|f| f.0).filter(|&t| {
        sense(me, &brain, t)
            .is_some_and(|(tx, ty, dead)| !dead && distance(x, y, tx, ty) <= CHASE_RANGE)
    });
    if !brain.fights() {
        target = None;
    }
    if target.is_none() && brain.mode == BotMode::Aggressive && rng.is_multiple_of(8) {
        target = nearest_other(me, SENSE_RANGE);
    }

    let mut dir = (0.0, 0.0);
    let mut action = None;
    if let Some((t, (tx, ty, _))) = target.and_then(|t| sense(me, &brain, t).map(|s| (t, s))) {
        let dist = distance(x, y, tx, ty);
        // Sword up close; otherwise now and then a fireball, else the bow.
        action = Some(if dist <= Ability::Sword.stats().range {
            (Ability::Sword, Target::Entity(t))
        } else if rng % 16 == 1 && dist <= Ability::Fireball.stats().range {
            (Ability::Fireball, Target::Point(tx, ty))
        } else {
            (Ability::Bow, Target::Entity(t))
        });
        if dist > 1.2 {
            dir = ((tx - x) / dist, (ty - y) / dist);
        }
        brain.wander = None;
    } else if let Some((wx, wy)) = brain.wander {
        let dist = distance(x, y, wx, wy);
        // Bots walk straight at their spot; one stuck on a wall picks another right away.
        let moving = me
            .get::<Heading>()
            .is_some_and(|h| (h.dx, h.dy) != (0.0, 0.0));
        let stuck = moving && brain.last_pos == (x, y);
        if stuck {
            brain.wander = None;
            brain.idle_until = tick;
        } else if dist < 0.5 || tick >= brain.wander_until {
            brain.wander = None;
            // Wanderers barely stop; the others rest 0.5 to 3 s.
            let rest = if brain.mode == BotMode::Wander {
                100 + (rng >> 16) % 400
            } else {
                500 + (rng >> 16) % 2500
            };
            brain.idle_until = tick + time::ticks_in(rest);
        } else {
            dir = ((wx - x) / dist, (wy - y) / dist);
        }
    } else if tick >= brain.idle_until {
        // Wander somewhere within 10 tiles (wanderers: 16).
        let reach: u64 = if brain.mode == BotMode::Wander {
            16
        } else {
            10
        };
        let ox = ((rng >> 8) % (2 * reach + 1)) as i32 - reach as i32;
        let oy = ((rng >> 24) % (2 * reach + 1)) as i32 - reach as i32;
        let (tx, ty) = map.nearest_walkable(x as i32 + ox, y as i32 + oy);
        brain.wander = Some(movement::tile_center(tx, ty));
        brain.wander_until = tick + time::ticks_in(8000);
    }

    brain.last_pos = (x, y);
    let (dx, dy) = movement::sanitize_dir(dir.0, dir.1);
    me.insert(Heading { dx, dy });
    match target {
        Some(t) => me.insert(Fighting(t)),
        None => {
            me.remove::<Fighting>();
        }
    }
    me.insert(brain);
    if let Some((ability, aim)) = action {
        combat::use_ability(map, me, ability, aim);
    }
}

/// Bots that care what's around them (aggressive, or after someone) ask, from where they
/// ended up this tick; wanderers never do. Staggered, so not every bot asks on the same tick.
/// They only need to know who's dead, so that's the only component they ask for.
fn ask(me: &mut Me<'_>) {
    let Some(mode) = me.get::<Brain>().map(|b| b.mode) else {
        return;
    };
    let every = time::ticks_in(SENSE_EVERY_MS).max(1);
    let due = (me.tick() + u64::from(me.id)).is_multiple_of(every);
    let target = me.get::<Fighting>().map(|f| f.0);
    let cares = mode == BotMode::Aggressive || target.is_some();
    if !due || me.has::<Dead>() || !cares {
        return;
    }
    let mut query = Query::within(if target.is_some() {
        CHASE_RANGE
    } else {
        SENSE_RANGE
    })
    .nearest(SENSE_NEAREST)
    .with::<Dead>();
    if let Some(t) = target {
        query = query.target(t);
    }
    me.query(query);
}

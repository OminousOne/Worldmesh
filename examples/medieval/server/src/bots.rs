//! Bot AI, run by whichever group simulates the bot. A bot knows its own group's units
//! exactly and everyone else from its latest "what's near me?" answer (a tick or two old),
//! and acts exactly like a player does: through [`Medieval::use_ability`]. Its randomness is
//! seeded by (world, bot, tick), so moving to another group never changes what it decides.

use serde::{Deserialize, Serialize};
use worldmesh::protocol::Facts;
use worldmesh::{time, EntityId, Me};

use crate::combat::{distance, Ability};
use crate::game::{Medieval, Target};
use crate::map::splitmix64;
use crate::movement;
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

/// A bot's state of mind. Part of its unit's data, so it travels with it between groups.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
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
        Self { mode, ..Self::default() }
    }

    fn fights(&self) -> bool {
        self.mode != BotMode::Wander
    }
}

/// What bot `me` knows of unit `id`: (x, y, dead). Exact if it's in this group, otherwise from
/// its senses, or where it stood when it hit us.
fn sense(me: &Me<'_, Medieval>, id: EntityId) -> Option<(f32, f32, bool)> {
    if let Some(n) = me.find(id) {
        return Some((n.x, n.y, n.view.dead));
    }
    let felt = me.data.bot.as_ref()?.felt?;
    (felt.0 == id && felt.3 + time::ticks_in(FELT_FOR_MS) >= me.tick()).then_some((felt.1, felt.2, false))
}

/// The nearest living unit bot `me` knows of within `radius`.
fn nearest_other(me: &Me<'_, Medieval>, radius: f32) -> Option<EntityId> {
    let (x, y) = (me.x, me.y);
    me.nearby(radius)
        .into_iter()
        .filter(|n| !n.view.dead)
        .min_by(|a, b| distance(x, y, a.x, a.y).total_cmp(&distance(x, y, b.x, b.y)).then(a.id.cmp(&b.id)))
        .map(|n| n.id)
}

/// A bot was hit: it knows where the attacker stood, and fights back unless it never fights.
pub fn felt(me: &mut Me<'_, Medieval>, attacker: &Facts) {
    let Some(brain) = &mut me.data.bot else { return };
    if !brain.fights() {
        return;
    }
    brain.felt = Some((attacker.id, attacker.x, attacker.y, attacker.tick));
    if me.data.target.is_none() {
        me.data.target = Some(attacker.id);
    }
}

/// Decide where to go and what to use this tick.
pub fn think(game: &Medieval, me: &mut Me<'_, Medieval>) {
    if me.data.dead_until.is_some() {
        return;
    }
    let Some(mut brain) = me.data.bot.clone() else { return };
    let tick = me.tick();
    let (id, x, y) = (me.id, me.x, me.y);
    let rng = splitmix64(WORLD_SEED ^ (u64::from(id) << 32) ^ tick);

    // Keep a target only while it can be sensed, alive and close; aggressive bots look for one.
    let mut target = me
        .data
        .target
        .filter(|&t| sense(me, t).is_some_and(|(tx, ty, dead)| !dead && distance(x, y, tx, ty) <= CHASE_RANGE));
    if !brain.fights() {
        target = None;
    }
    if target.is_none() && brain.mode == BotMode::Aggressive && rng.is_multiple_of(8) {
        target = nearest_other(me, SENSE_RANGE);
    }

    let mut dir = (0.0, 0.0);
    let mut action = None;
    if let Some((t, (tx, ty, _))) = target.and_then(|t| sense(me, t).map(|s| (t, s))) {
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
        let stuck = me.data.dir != (0.0, 0.0) && brain.last_pos == (x, y);
        if stuck {
            brain.wander = None;
            brain.idle_until = tick;
        } else if dist < 0.5 || tick >= brain.wander_until {
            brain.wander = None;
            // Wanderers barely stop; the others rest 0.5 to 3 s.
            let rest = if brain.mode == BotMode::Wander { 100 + (rng >> 16) % 400 } else { 500 + (rng >> 16) % 2500 };
            brain.idle_until = tick + time::ticks_in(rest);
        } else {
            dir = ((wx - x) / dist, (wy - y) / dist);
        }
    } else if tick >= brain.idle_until {
        // Wander somewhere within 10 tiles (wanderers: 16).
        let reach: u64 = if brain.mode == BotMode::Wander { 16 } else { 10 };
        let ox = ((rng >> 8) % (2 * reach + 1)) as i32 - reach as i32;
        let oy = ((rng >> 24) % (2 * reach + 1)) as i32 - reach as i32;
        let (tx, ty) = game.map.nearest_walkable(x as i32 + ox, y as i32 + oy);
        brain.wander = Some(movement::tile_center(tx, ty));
        brain.wander_until = tick + time::ticks_in(8000);
    }

    brain.last_pos = (x, y);
    me.data.dir = movement::sanitize_dir(dir.0, dir.1);
    (me.data.target, me.data.bot) = (target, Some(brain));
    if let Some((ability, aim)) = action {
        game.use_ability(me, ability, aim);
    }
}

/// Bots that care what's around them (aggressive, or after someone) ask, from where they
/// ended up this tick; wanderers never do. Staggered, so not every bot asks on the same tick.
pub fn ask(me: &mut Me<'_, Medieval>) {
    let Some(brain) = &me.data.bot else { return };
    let every = time::ticks_in(SENSE_EVERY_MS).max(1);
    let due = (me.tick() + u64::from(me.id)).is_multiple_of(every);
    let cares = brain.mode == BotMode::Aggressive || me.data.target.is_some();
    if !due || me.data.dead_until.is_some() || !cares {
        return;
    }
    let target = me.data.target;
    let r = if target.is_some() { CHASE_RANGE } else { SENSE_RANGE };
    me.query_with(r, SENSE_NEAREST, target);
}

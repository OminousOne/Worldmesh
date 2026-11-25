//! Combat: the [`Combat`] plugin, and abilities as data with the rules for checking them.
//!
//! Adding an ability means adding a row to `Ability::stats`; nothing about Worldmesh changes.
//! Its reach can be anything: the user's side is checked where the user is simulated, the
//! target's side where the target is, wherever each one is.
//!
//! What other plugins can use: [`use_ability`] (bots fight exactly like players), the
//! [`Fighting`] and [`LastHit`] components, and [`Dead`] (which also makes a unit
//! [`Immobile`] until it respawns).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use worldmesh::{
    time, Bundle, Component, EntityId, Incoming, Me, Message, Plugin, Stage, To, World,
};

use crate::map::{splitmix64, MapData, Tile};
use crate::movement::Immobile;

pub const MAX_HP: u16 = 100;
/// Dead entities come back (full HP, at a spawn point) after this long.
pub const RESPAWN_MS: u64 = 3000;
/// The target's group accepts a range this much over the limit (positions are a tick apart).
pub const RANGE_SLACK: f32 = 0.3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ability {
    #[default]
    Sword,
    Bow,
    Fireball,
}

/// What an ability is aimed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aim {
    Entity,
    Point,
}

pub struct AbilityStats {
    pub aim: Aim,
    /// Reach in tiles from the user, centre to centre (to the target, or to the point).
    pub range: f32,
    /// 0 for a single target; otherwise everything within this many tiles of the point.
    pub radius: f32,
    pub damage: u16,
    pub cooldown_ms: u64,
    /// Blocked by walls between the user and the target (or the point).
    pub needs_line_of_sight: bool,
}

impl Ability {
    pub const ALL: [Ability; 3] = [Ability::Sword, Ability::Bow, Ability::Fireball];

    pub fn stats(self) -> AbilityStats {
        match self {
            Ability::Sword => AbilityStats {
                aim: Aim::Entity,
                range: 1.5,
                radius: 0.0,
                damage: 20,
                cooldown_ms: 800,
                needs_line_of_sight: false,
            },
            Ability::Bow => AbilityStats {
                aim: Aim::Entity,
                range: 12.0,
                radius: 0.0,
                damage: 12,
                cooldown_ms: 1200,
                needs_line_of_sight: true,
            },
            Ability::Fireball => AbilityStats {
                aim: Aim::Point,
                range: 10.0,
                radius: 2.5,
                damage: 25,
                cooldown_ms: 3000,
                needs_line_of_sight: true,
            },
        }
    }
}

// ---- Components ----

/// Hit points. Visible: health bars.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health(pub u16);

/// Dead until this tick, then it respawns. Visible.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dead {
    pub until: u64,
}

/// Who it's fighting. Visible (the debug panel shows it).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fighting(pub EntityId);

/// The tick each ability is ready again.
#[derive(Component, Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cooldowns(pub Vec<(Ability, u64)>);

/// The last hit it took and lived: who, and where they stood on which tick.
#[derive(Component, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LastHit {
    pub by: EntityId,
    pub x: f32,
    pub y: f32,
    pub tick: u64,
}

/// Respawns anywhere on the map instead of in town.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RespawnAnywhere;

// ---- Messages ----

/// What an ability is aimed at.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Entity(EntityId),
    Point(f32, f32),
}

/// A player's command: use an ability.
#[derive(Message, Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[worldmesh(name = "use")]
pub struct UseAbility {
    pub ability: Ability,
    pub target: Target,
}

/// An ability landing: on its target (which checks the reach), or on everyone in its circle.
#[derive(Message, Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Hit {
    pub ability: Ability,
}

/// Event: the user's side accepted a use and sent it (an arrow flies, a fireball launches).
#[derive(Message, Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Used {
    pub tick: u64,
    pub actor: EntityId,
    pub ability: Ability,
    pub target: Target,
    pub x: f32,
    pub y: f32,
    pub group: u32,
    pub trace: u64,
}

/// Event: the target's side checked a hit and applied it (or not) to one unit.
#[derive(Message, Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Effect {
    pub tick: u64,
    pub actor: EntityId,
    pub target: EntityId,
    pub ability: Ability,
    pub damage: u16,
    /// The target's HP afterwards.
    pub hp: u16,
    pub outcome: Outcome,
    /// The group that sent it and the one that applied it.
    pub from_group: u32,
    pub to_group: u32,
    /// Ticks between the use and the effect (always at least 1, wherever the target is).
    pub delay_ticks: u64,
    /// It reached the target's previous group first and was forwarded after a handoff.
    pub forwarded: bool,
    pub trace: u64,
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

// ---- The plugin ----

/// Health, abilities, death and respawning, for heroes and bots.
pub struct Combat {
    pub map: Arc<MapData>,
}

impl Plugin for Combat {
    fn build(&self, world: &mut World) {
        let (use_map, hit_map, respawn_map) =
            (self.map.clone(), self.map.clone(), self.map.clone());
        let unit = Bundle::new()
            .with(Health(MAX_HP))
            .with(Cooldowns::default());
        world
            .visible::<Health>()
            .visible::<Dead>()
            .visible::<Fighting>()
            .component::<Cooldowns>()
            .component::<LastHit>()
            .component::<RespawnAnywhere>()
            .command(move |me, u: &UseAbility| use_ability(&use_map, me, u.ability, u.target))
            .action(move |me, hit: &Hit, from| take_hit(&hit_map, me, hit.ability, from))
            .event::<Used>()
            .event::<Effect>()
            .system_for::<Dead>(Stage::Last, move |me| respawn(&respawn_map, me))
            .kind("hero", unit.clone())
            .kind("bot", unit.with(RespawnAnywhere));
    }
}

/// The user's side, where the user is simulated: alive, cooldown ready, a point in reach.
/// Then the hit goes to its target's address, wherever that is.
pub fn use_ability(map: &MapData, me: &mut Me<'_>, ability: Ability, target: Target) {
    let stats = ability.stats();
    let tick = me.tick();
    let ready = me.get::<Cooldowns>().is_some_and(|c| {
        c.0.iter()
            .find(|(a, _)| *a == ability)
            .is_none_or(|&(_, at)| tick >= at)
    });
    if me.has::<Dead>() || !ready {
        return;
    }
    let to = match (stats.aim, target) {
        (Aim::Entity, Target::Entity(t)) if t != me.id => To::Entity(t),
        // A point is just a place: the user's side can check the reach to it itself.
        (Aim::Point, Target::Point(x, y)) if reaches(map, ability, me.x, me.y, x, y, 0.0) => {
            To::Area {
                x,
                y,
                r: stats.radius,
            }
        }
        _ => return,
    };
    if let Some(c) = me.get_mut::<Cooldowns>() {
        c.0.retain(|(a, _)| *a != ability);
        c.0.push((ability, tick + time::ticks_in(stats.cooldown_ms)));
    }
    let trace = me.send_reliable(to, Hit { ability });
    let (actor, x, y, group) = (me.id, me.x, me.y, me.group());
    me.emit(Used {
        tick,
        actor,
        ability,
        target,
        x,
        y,
        group,
        trace,
    });
}

/// The target's side, where the target is simulated. For a single target: was it in reach
/// (and in sight) where it stood on the tick the hit was sent? Area hits already landed
/// where they were aimed.
fn take_hit(map: &MapData, me: &mut Me<'_>, ability: Ability, from: &Incoming) {
    let Some(Health(hp)) = me.get::<Health>().copied() else {
        return;
    };
    if me.has::<Dead>() {
        return;
    }
    let stats = ability.stats();
    let sender = from.from;
    let tick = me.tick();
    let (tx, ty) = me.position_at(sender.tick);
    let mut outcome = if from.area.is_some() {
        Outcome::Hit
    } else if distance(sender.x, sender.y, tx, ty) > stats.range + RANGE_SLACK {
        Outcome::OutOfRange
    } else if stats.needs_line_of_sight && !line_of_sight(map, sender.x, sender.y, tx, ty) {
        Outcome::Blocked
    } else {
        Outcome::Hit
    };
    let mut damage = 0;
    let mut hp_after = hp;
    if outcome == Outcome::Hit {
        damage = stats.damage.min(hp);
        hp_after = hp - damage;
        me.insert(Health(hp_after));
        if hp_after == 0 {
            outcome = Outcome::Killed;
            me.insert(Dead {
                until: tick + time::ticks_in(RESPAWN_MS),
            });
            me.insert(Immobile);
            me.remove::<Fighting>();
        } else {
            me.insert(LastHit {
                by: sender.id,
                x: sender.x,
                y: sender.y,
                tick: sender.tick,
            });
        }
    }
    let effect = Effect {
        tick,
        actor: sender.id,
        target: me.id,
        ability,
        damage,
        hp: hp_after,
        outcome,
        from_group: sender.group,
        to_group: me.group(),
        delay_ticks: tick.saturating_sub(sender.tick),
        forwarded: from.forwarded,
        trace: from.trace,
    };
    // The attacker's player sees the result even if it landed off their screen.
    me.emit_involving(effect, &[sender.id]);
}

/// The dead come back with full HP: heroes in town, the rest anywhere.
fn respawn(map: &MapData, me: &mut Me<'_>) {
    let tick = me.tick();
    if me.get::<Dead>().is_some_and(|d| tick < d.until) {
        return;
    }
    let id = me.id;
    (me.x, me.y) = if me.has::<RespawnAnywhere>() {
        map.random_point(splitmix64(u64::from(id) ^ (tick << 20)))
    } else {
        map.spawn_point(id)
    };
    me.insert(Health(MAX_HP));
    me.remove::<Dead>();
    me.remove::<Immobile>();
    me.remove::<Fighting>();
    me.remove::<LastHit>();
}

pub fn distance(ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    ((ax - bx) * (ax - bx) + (ay - by) * (ay - by)).sqrt()
}

/// No wall on the straight line between the two points (checked every quarter tile).
pub fn line_of_sight(map: &MapData, ax: f32, ay: f32, bx: f32, by: f32) -> bool {
    let steps = (distance(ax, ay, bx, by) * 4.0).ceil().max(1.0) as u32;
    (0..=steps).all(|i| {
        let t = i as f32 / steps as f32;
        let (x, y) = (ax + (bx - ax) * t, ay + (by - ay) * t);
        map.tile(x.floor() as i32, y.floor() as i32) != Tile::Wall
    })
}

/// Can `ability`, used from (ax, ay), reach (bx, by)? `slack` widens the range a little for
/// the target's side, which compares positions from neighbouring ticks.
pub fn reaches(
    map: &MapData,
    ability: Ability,
    ax: f32,
    ay: f32,
    bx: f32,
    by: f32,
    slack: f32,
) -> bool {
    let stats = ability.stats();
    distance(ax, ay, bx, by) <= stats.range + slack
        && (!stats.needs_line_of_sight || line_of_sight(map, ax, ay, bx, by))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::generate;
    use crate::{MAP_HEIGHT, MAP_WIDTH, WORLD_SEED};

    #[test]
    fn ranges() {
        let map = generate(WORLD_SEED, MAP_WIDTH, MAP_HEIGHT);
        let (cx, cy) = map.center();
        let (x, y) = (cx as f32 + 0.5, cy as f32 + 0.5);
        assert!(reaches(&map, Ability::Sword, x, y, x + 1.4, y, 0.0));
        assert!(!reaches(&map, Ability::Sword, x, y, x + 2.0, y, 0.0));
        assert!(Ability::Bow.stats().range > crate::bots::SENSE_RANGE);
    }

    #[test]
    fn walls_block_arrows() {
        let map = generate(WORLD_SEED, MAP_WIDTH, MAP_HEIGHT);
        let (wx, wy) = (1..MAP_WIDTH as i32 - 1)
            .flat_map(|x| (1..MAP_HEIGHT as i32 - 1).map(move |y| (x, y)))
            .find(|&(x, y)| {
                map.tile(x, y) == Tile::Wall && map.walkable(x - 1, y) && map.walkable(x + 1, y)
            })
            .expect("map has a wall between two walkable tiles");
        let (ax, bx, y) = (wx as f32 - 0.5, wx as f32 + 1.5, wy as f32 + 0.5);
        assert!(!line_of_sight(&map, ax, y, bx, y));
        assert!(!reaches(&map, Ability::Bow, ax, y, bx, y, 0.0));
    }
}

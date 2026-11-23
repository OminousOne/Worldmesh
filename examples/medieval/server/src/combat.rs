//! Abilities as data, plus the rules for checking them.
//!
//! Adding an ability means adding a row to `Ability::stats`; nothing about Worldmesh changes.
//! Its reach can be anything: the user's side is checked where the user is simulated, the
//! target's side where the target is, wherever each one is.

use serde::{Deserialize, Serialize};

use crate::map::{MapData, Tile};

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
            Ability::Sword => AbilityStats { aim: Aim::Entity, range: 1.5, radius: 0.0, damage: 20, cooldown_ms: 800, needs_line_of_sight: false },
            Ability::Bow => AbilityStats { aim: Aim::Entity, range: 12.0, radius: 0.0, damage: 12, cooldown_ms: 1200, needs_line_of_sight: true },
            Ability::Fireball => AbilityStats { aim: Aim::Point, range: 10.0, radius: 2.5, damage: 25, cooldown_ms: 3000, needs_line_of_sight: true },
        }
    }
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
pub fn reaches(map: &MapData, ability: Ability, ax: f32, ay: f32, bx: f32, by: f32, slack: f32) -> bool {
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
            .find(|&(x, y)| map.tile(x, y) == Tile::Wall && map.walkable(x - 1, y) && map.walkable(x + 1, y))
            .expect("map has a wall between two walkable tiles");
        let (ax, bx, y) = (wx as f32 - 0.5, wx as f32 + 1.5, wy as f32 + 0.5);
        assert!(!line_of_sight(&map, ax, y, bx, y));
        assert!(!reaches(&map, Ability::Bow, ax, y, bx, y, 0.0));
    }
}

//! Movement rules. The server runs these authoritatively and the client runs the same rules
//! (ported to TypeScript) to predict its own player. Positions are in tiles as f32:
//! tile (x, y) covers [x, x + 1) by [y, y + 1).

use worldmesh::time;

use crate::map::MapData;

pub const SPEED_TILES_PER_SEC: f32 = 5.0;
/// Distance covered in one tick, which depends on the tick rate.
pub fn speed_per_tick() -> f32 {
    SPEED_TILES_PER_SEC * time::tick_ms() as f32 / 1000.0
}
/// Half the width of a player's collision box.
pub const PLAYER_HALF_SIZE: f32 = 0.3;
/// A tick is split into this many sub-steps; a direction change can land on any of them,
/// so letting go of a key mid-tick stops you there instead of at a tick boundary.
pub const SUBSTEPS: u8 = 16;

/// A direction change during a tick: from sub-step `.0` on, move in direction `.1`.
pub type Change = (u8, (f32, f32));

/// Centre of tile (tx, ty).
pub fn tile_center(tx: i32, ty: i32) -> (f32, f32) {
    (tx as f32 + 0.5, ty as f32 + 0.5)
}

/// Clamp a requested direction to length <= 1. Garbage (NaN, infinity) means stand still.
pub fn sanitize_dir(dx: f32, dy: f32) -> (f32, f32) {
    if !dx.is_finite() || !dy.is_finite() {
        return (0.0, 0.0);
    }
    let len = (dx * dx + dy * dy).sqrt();
    if len > 1.0 {
        (dx / len, dy / len)
    } else {
        (dx, dy)
    }
}

/// True when a box centred on (x, y) overlaps only walkable tiles.
pub fn box_is_clear(map: &MapData, x: f32, y: f32, half: f32) -> bool {
    let min_tx = (x - half).floor() as i32;
    let max_tx = (x + half).ceil() as i32 - 1;
    let min_ty = (y - half).floor() as i32;
    let max_ty = (y + half).ceil() as i32 - 1;
    (min_ty..=max_ty).all(|ty| (min_tx..=max_tx).all(|tx| map.walkable(tx, ty)))
}

/// Advance one whole tick in direction (dx, dy).
pub fn step(map: &MapData, x: f32, y: f32, dx: f32, dy: f32) -> (f32, f32) {
    step_part(map, x, y, dx, dy, 1.0)
}

/// Advance `part` of a tick (0 to 1) in direction (dx, dy). Each axis is resolved
/// separately, so players slide along walls instead of sticking to them.
pub fn step_part(map: &MapData, x: f32, y: f32, dx: f32, dy: f32, part: f32) -> (f32, f32) {
    let (dx, dy) = sanitize_dir(dx, dy);
    let dist = speed_per_tick() * part;
    let (vx, vy) = (dx * dist, dy * dist);
    let x = x + furthest_clear(vx, |d| box_is_clear(map, x + d, y, PLAYER_HALF_SIZE));
    let y = y + furthest_clear(vy, |d| box_is_clear(map, x, y + d, PLAYER_HALF_SIZE));
    (x, y)
}

/// One whole tick for a player who starts it moving in `dir` and changes direction at
/// each sub-step in `changes` (sorted by sub-step). Returns the new position and the
/// direction held at the end of the tick. Server and client both run exactly this.
pub fn simulate_tick(map: &MapData, x: f32, y: f32, dir: (f32, f32), changes: &[Change]) -> (f32, f32, (f32, f32)) {
    let (mut x, mut y, mut dir) = (x, y, dir);
    let mut from = 0u8;
    for &(at, next) in changes {
        let at = at.min(SUBSTEPS);
        if at > from {
            (x, y) = segment(map, x, y, dir, from, at);
            from = at;
        }
        dir = next;
    }
    (x, y) = segment(map, x, y, dir, from, SUBSTEPS);
    (x, y, dir)
}

/// Move in `dir` from sub-step `from` to sub-step `to` of a tick.
fn segment(map: &MapData, x: f32, y: f32, dir: (f32, f32), from: u8, to: u8) -> (f32, f32) {
    if to <= from || (dir.0 == 0.0 && dir.1 == 0.0) {
        return (x, y);
    }
    step_part(map, x, y, dir.0, dir.1, (to - from) as f32 / SUBSTEPS as f32)
}

/// Largest part of `delta` that can be moved while `clear` holds. A tick's move is
/// shorter than a tile, so once a move is blocked every longer one is too.
fn furthest_clear(delta: f32, clear: impl Fn(f32) -> bool) -> f32 {
    if delta == 0.0 || clear(delta) {
        return delta;
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32); // fractions of delta: lo is clear, hi is blocked
    for _ in 0..20 {
        let mid = (lo + hi) * 0.5;
        if clear(delta * mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    delta * lo
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::Tile;

    const HALF: f32 = PLAYER_HALF_SIZE;

    /// 10x10 open map with a wall column at x = 6 (outside the map counts as wall).
    fn map_with_wall_column() -> MapData {
        let mut tiles = vec![Tile::Path as u8; 100];
        for y in 0..10 {
            tiles[y * 10 + 6] = Tile::Wall as u8;
        }
        MapData { width: 10, height: 10, tiles }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn straight_move_covers_speed_per_tick() {
        let map = map_with_wall_column();
        let (x, y) = tile_center(2, 5);
        let (nx, ny) = step(&map, x, y, 1.0, 0.0);
        assert!(close(nx, x + speed_per_tick()) && close(ny, y));
    }

    #[test]
    fn any_angle_moves_at_the_same_speed() {
        let map = map_with_wall_column();
        let (x, y) = tile_center(3, 4);
        for (dx, dy) in [(1.0, 1.0), (0.0, -3.0), (-2.0, 5.0), (0.6, 0.8)] {
            let (nx, ny) = step(&map, x, y, dx, dy);
            let dist = ((nx - x).powi(2) + (ny - y).powi(2)).sqrt();
            assert!(close(dist, speed_per_tick()), "dir ({dx}, {dy}) moved {dist}");
        }
    }

    #[test]
    fn garbage_direction_means_stand_still() {
        let map = map_with_wall_column();
        let (x, y) = tile_center(3, 4);
        assert_eq!(step(&map, x, y, f32::NAN, 1.0), (x, y));
    }

    #[test]
    fn stops_flush_against_wall() {
        let map = map_with_wall_column();
        let (mut x, mut y) = tile_center(3, 5);
        for _ in 0..20 {
            (x, y) = step(&map, x, y, 1.0, 0.0);
        }
        assert!(close(x + HALF, 6.0) && x + HALF <= 6.0, "x = {x}");
        assert!(box_is_clear(&map, x, y, HALF));
    }

    #[test]
    fn slides_along_wall() {
        let map = map_with_wall_column();
        let (sx, sy) = tile_center(5, 1);
        let (mut x, mut y) = (sx, sy);
        for _ in 0..10 {
            (x, y) = step(&map, x, y, 1.0, 1.0);
        }
        assert!(close(x + HALF, 6.0), "pinned against the wall");
        assert!(y > sy + 1.0, "kept moving along it");
    }

    #[test]
    fn map_edge_blocks() {
        let map = map_with_wall_column();
        let (mut x, mut y) = tile_center(1, 1);
        for _ in 0..20 {
            (x, y) = step(&map, x, y, -1.0, -1.0);
        }
        assert!(close(x - HALF, 0.0) && close(y - HALF, 0.0));
    }

    #[test]
    fn a_tick_without_changes_is_a_plain_step() {
        let map = map_with_wall_column();
        let (x, y) = tile_center(2, 2);
        let (nx, ny, dir) = simulate_tick(&map, x, y, (0.6, 0.8), &[]);
        assert_eq!((nx, ny), step(&map, x, y, 0.6, 0.8));
        assert_eq!(dir, (0.6, 0.8));
    }

    #[test]
    fn stopping_mid_tick_covers_only_part_of_the_tick() {
        let map = map_with_wall_column();
        let (x, y) = tile_center(2, 2);
        // Walking east, let go after 4 of 16 sub-steps: a quarter of a tick's distance.
        let (nx, ny, dir) = simulate_tick(&map, x, y, (1.0, 0.0), &[(4, (0.0, 0.0))]);
        assert!(close(nx - x, speed_per_tick() / 4.0), "moved {}", nx - x);
        assert_eq!((ny, dir), (y, (0.0, 0.0)));
    }

    #[test]
    fn a_change_at_sub_step_zero_covers_the_whole_tick() {
        let map = map_with_wall_column();
        let (x, y) = tile_center(2, 2);
        let (nx, ny, _) = simulate_tick(&map, x, y, (0.0, 0.0), &[(0, (0.0, 1.0))]);
        assert_eq!((nx, ny), step(&map, x, y, 0.0, 1.0));
    }
}

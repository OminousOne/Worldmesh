//! The map: generated from a seed, so every process (and every client, from the welcome
//! message) has the same one. Grass, paths, water, and a walled town in the middle.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Tile {
    Grass = 0,
    Path = 1,
    Water = 2,
    Wall = 3,
}

impl Tile {
    pub fn walkable(self) -> bool {
        matches!(self, Tile::Grass | Tile::Path)
    }

    fn from_u8(v: u8) -> Tile {
        match v {
            0 => Tile::Grass,
            1 => Tile::Path,
            2 => Tile::Water,
            _ => Tile::Wall,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapData {
    pub width: u32,
    pub height: u32,
    /// Row-major tile ids (see `Tile`).
    pub tiles: Vec<u8>,
}

impl MapData {
    pub fn tile(&self, x: i32, y: i32) -> Tile {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return Tile::Wall;
        }
        Tile::from_u8(self.tiles[(y as u32 * self.width + x as u32) as usize])
    }

    pub fn walkable(&self, x: i32, y: i32) -> bool {
        self.tile(x, y).walkable()
    }

    fn set(&mut self, x: i32, y: i32, t: Tile) {
        if x >= 0 && y >= 0 && x < self.width as i32 && y < self.height as i32 {
            let i = (y as u32 * self.width + x as u32) as usize;
            self.tiles[i] = t as u8;
        }
    }

    pub fn center(&self) -> (i32, i32) {
        (self.width as i32 / 2, self.height as i32 / 2)
    }

    /// Where player `id` (re)appears: around the town centre, spread over a 5 x 5 patch.
    pub fn spawn_point(&self, id: u32) -> (f32, f32) {
        let (cx, cy) = self.center();
        let (tx, ty) = self.nearest_walkable(cx + (id % 5) as i32 - 2, cy + ((id / 5) % 5) as i32 - 2);
        crate::movement::tile_center(tx, ty)
    }

    /// A walkable tile anywhere on the map, picked by `hash` (bots spawn and respawn here).
    pub fn random_point(&self, hash: u64) -> (f32, f32) {
        let x = (hash % self.width as u64) as i32;
        let y = ((hash >> 32) % self.height as u64) as i32;
        let (tx, ty) = self.nearest_walkable(x, y);
        crate::movement::tile_center(tx, ty)
    }

    /// Nearest walkable tile to `(x, y)`, searching outward in square rings.
    pub fn nearest_walkable(&self, x: i32, y: i32) -> (i32, i32) {
        let max_r = self.width.max(self.height) as i32;
        for r in 0..max_r {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs() != r && dy.abs() != r {
                        continue;
                    }
                    if self.walkable(x + dx, y + dy) {
                        return (x + dx, y + dy);
                    }
                }
            }
        }
        self.center()
    }
}

pub use worldmesh::codec::splitmix64;

/// Value noise in [0, 1) on an integer lattice, smoothed between lattice points.
fn noise(seed: u64, x: i32, y: i32, scale: i32) -> f64 {
    let lattice = |lx: i32, ly: i32| -> f64 {
        let h = splitmix64(seed ^ (lx as u64).wrapping_mul(0x9E37_79B1) ^ (ly as u64).wrapping_mul(0x85EB_CA77));
        (h >> 11) as f64 / (1u64 << 53) as f64
    };
    let (gx, gy) = (x.div_euclid(scale), y.div_euclid(scale));
    let fx = x.rem_euclid(scale) as f64 / scale as f64;
    let fy = y.rem_euclid(scale) as f64 / scale as f64;
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let top = lattice(gx, gy) * (1.0 - sx) + lattice(gx + 1, gy) * sx;
    let bottom = lattice(gx, gy + 1) * (1.0 - sx) + lattice(gx + 1, gy + 1) * sx;
    top * (1.0 - sy) + bottom * sy
}

pub fn generate(seed: u64, width: u32, height: u32) -> MapData {
    let mut map = MapData { width, height, tiles: vec![Tile::Grass as u8; (width * height) as usize] };
    let (w, h) = (width as i32, height as i32);

    // Terrain: lakes from low noise, sandy paths around their shores.
    for y in 0..h {
        for x in 0..w {
            let n = noise(seed, x, y, 9);
            let t = if n < 0.22 {
                Tile::Water
            } else if n < 0.27 {
                Tile::Path
            } else {
                Tile::Grass
            };
            map.set(x, y, t);
        }
    }

    // World border.
    for i in 0..w {
        map.set(i, 0, Tile::Wall);
        map.set(i, h - 1, Tile::Wall);
    }
    for i in 0..h {
        map.set(0, i, Tile::Wall);
        map.set(w - 1, i, Tile::Wall);
    }

    // A walled town in the middle with a gate on each side and roads out.
    let (cx, cy) = map.center();
    let (hw, hh) = (7, 6);
    for x in cx - hw..=cx + hw {
        for y in cy - hh..=cy + hh {
            let edge = x == cx - hw || x == cx + hw || y == cy - hh || y == cy + hh;
            map.set(x, y, if edge { Tile::Wall } else { Tile::Path });
        }
    }
    for i in -1..=1 {
        map.set(cx + i, cy - hh, Tile::Path);
        map.set(cx + i, cy + hh, Tile::Path);
        map.set(cx - hw, cy + i, Tile::Path);
        map.set(cx + hw, cy + i, Tile::Path);
    }
    for d in hw..w / 2 - 1 {
        map.set(cx + d, cy, Tile::Path);
        map.set(cx - d, cy, Tile::Path);
    }
    for d in hh..h / 2 - 1 {
        map.set(cx, cy + d, Tile::Path);
        map.set(cx, cy - d, Tile::Path);
    }

    // A few pillars inside the town so walls are easy to test.
    for (px, py) in [(-4, -3), (4, -3), (-4, 3), (4, 3)] {
        map.set(cx + px, cy + py, Tile::Wall);
    }

    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_is_deterministic() {
        let a = generate(42, 64, 64);
        let b = generate(42, 64, 64);
        assert_eq!(a.tiles, b.tiles);
    }

    #[test]
    fn town_center_is_walkable_and_border_is_not() {
        let m = generate(42, 64, 64);
        let (cx, cy) = m.center();
        assert!(m.walkable(cx, cy));
        assert!(!m.walkable(0, 0));
        assert!(!m.walkable(-5, 10));
    }
}

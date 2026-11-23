//! The shape of the world: its size, how it's cut into chunks, and what a player can see.
//!
//! Chunks are fixed blocks of tiles. A chunk's index never changes; only which Spatial Layer
//! Cell owns it does. The orchestrator splits chunks between cells along a Hilbert curve, and
//! sorts entities along the same curve to keep neighbours in the same Server Entity Group.

use serde::{Deserialize, Serialize};

/// The world every process agrees on. The game returns it from
/// `Game::config`; every role must run with the same one.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct WorldConfig {
    /// World size in tiles. Positions are `f32` tile coordinates in `0..width`, `0..height`.
    pub width: u32,
    pub height: u32,
    /// Chunk side in tiles: the unit of area a Spatial Layer Cell owns. Smaller chunks split
    /// areas more evenly but cost more bookkeeping.
    pub chunk_size: u32,
    /// What a player can see around them. Spatial Layer Cells send each player only this.
    pub view: ViewShape,
    /// How finely an input's timing within a tick is kept (the client and the server apply an
    /// input at the same sub-step, so movement predicted on the client matches exactly).
    pub input_substeps: u8,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self { width: 64, height: 64, chunk_size: 8, view: ViewShape::Rect { half_w: 20.0, half_h: 12.0 }, input_substeps: 16 }
    }
}

/// The area around a player that they can see, in tiles.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum ViewShape {
    /// An axis-aligned rectangle: |dx| <= half_w and |dy| <= half_h.
    Rect { half_w: f32, half_h: f32 },
    /// A rectangle on a 2:1 isometric screen. Screen x follows (dx - dy) and screen y follows
    /// (dx + dy), so it's |dx - dy| <= half_w and |dx + dy| <= half_h.
    Iso { half_w: f32, half_h: f32 },
}

impl ViewShape {
    /// Is (x, y) visible to a player at (vx, vy)? `margin` widens the view (in tiles).
    pub fn contains(&self, vx: f32, vy: f32, x: f32, y: f32, margin: f32) -> bool {
        let (dx, dy) = (x - vx, y - vy);
        match *self {
            ViewShape::Rect { half_w, half_h } => dx.abs() <= half_w + margin && dy.abs() <= half_h + margin,
            ViewShape::Iso { half_w, half_h } => (dx - dy).abs() <= half_w + margin && (dx + dy).abs() <= half_h + margin,
        }
    }

    /// Could any of the box [x0, x1] x [y0, y1] be visible to a player at (vx, vy)?
    pub fn overlaps_box(&self, vx: f32, vy: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
        match *self {
            ViewShape::Rect { half_w, half_h } => x1 - vx >= -half_w && x0 - vx <= half_w && y1 - vy >= -half_h && y0 - vy <= half_h,
            ViewShape::Iso { half_w, half_h } => {
                // Both shapes are convex: they overlap unless one of the view's axes
                // (u = dx - dy, v = dx + dy) or the box's (x, y) separates them.
                let corners = [(x0, y0), (x1, y0), (x0, y1), (x1, y1)].map(|(x, y)| (x - vx, y - vy));
                let (umin, umax) = minmax(corners.map(|(dx, dy)| dx - dy));
                let (vmin, vmax) = minmax(corners.map(|(dx, dy)| dx + dy));
                let half = (half_w + half_h) / 2.0; // the view's extent along x and y
                umax >= -half_w
                    && umin <= half_w
                    && vmax >= -half_h
                    && vmin <= half_h
                    && x1 - vx >= -half
                    && x0 - vx <= half
                    && y1 - vy >= -half
                    && y0 - vy <= half
            }
        }
    }
}

fn minmax<const N: usize>(values: [f32; N]) -> (f32, f32) {
    values.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| (lo.min(v), hi.max(v)))
}

impl WorldConfig {
    pub fn chunks_x(&self) -> u32 {
        self.width.div_ceil(self.chunk_size)
    }

    pub fn chunks_y(&self) -> u32 {
        self.height.div_ceil(self.chunk_size)
    }

    pub fn chunk_count(&self) -> usize {
        (self.chunks_x() * self.chunks_y()) as usize
    }

    /// Index (row-major) of the chunk containing (x, y). Positions off the world clamp to the
    /// nearest edge chunk.
    pub fn chunk_at(&self, x: f32, y: f32) -> usize {
        let cs = self.chunk_size as f32;
        let cx = ((x / cs).floor() as i64).clamp(0, i64::from(self.chunks_x()) - 1) as u32;
        let cy = ((y / cs).floor() as i64).clamp(0, i64::from(self.chunks_y()) - 1) as u32;
        (cy * self.chunks_x() + cx) as usize
    }

    /// (x0, y0, x1, y1) in tiles of chunk `i`.
    pub fn chunk_bounds(&self, i: usize) -> (f32, f32, f32, f32) {
        let cs = self.chunk_size as f32;
        let (cx, cy) = ((i as u32 % self.chunks_x()) as f32, (i as u32 / self.chunks_x()) as f32);
        (cx * cs, cy * cs, (cx + 1.0) * cs, (cy + 1.0) * cs)
    }

    /// Every chunk the circle around (x, y) with radius `r` touches.
    pub fn chunks_in_circle(&self, x: f32, y: f32, r: f32) -> Vec<usize> {
        let cs = self.chunk_size as f32;
        let (cx0, cy0) = self.chunk_xy(self.chunk_at(x - r, y - r));
        let (cx1, cy1) = self.chunk_xy(self.chunk_at(x + r, y + r));
        let mut out = Vec::new();
        for cy in cy0..=cy1 {
            for cx in cx0..=cx1 {
                let (x0, y0) = (cx as f32 * cs, cy as f32 * cs);
                let (nx, ny) = (x.clamp(x0, x0 + cs), y.clamp(y0, y0 + cs));
                if ((x - nx).powi(2) + (y - ny).powi(2)).sqrt() <= r {
                    out.push((cy * self.chunks_x() + cx) as usize);
                }
            }
        }
        out
    }

    fn chunk_xy(&self, i: usize) -> (u32, u32) {
        (i as u32 % self.chunks_x(), i as u32 / self.chunks_x())
    }

    /// Side of the smallest power-of-two square covering the chunk grid (the Hilbert curve's).
    pub fn hilbert_side(&self) -> u32 {
        self.chunks_x().max(self.chunks_y()).next_power_of_two()
    }

    /// Chunk indices along a Hilbert curve: neighbours in this list are neighbours on the
    /// map, so any run of it is a compact region.
    pub fn chunks_in_hilbert_order(&self) -> Vec<usize> {
        let n = self.hilbert_side();
        (0..n * n)
            .map(|d| hilbert_point(n, d))
            .filter(|&(x, y)| x < self.chunks_x() && y < self.chunks_y())
            .map(|(x, y)| (y * self.chunks_x() + x) as usize)
            .collect()
    }

    /// Position along a Hilbert curve covering the world tile by tile. Entities with close
    /// indices stand close together, so cutting entities sorted by it into runs gives each
    /// run a compact patch of the world.
    pub fn tile_hilbert_index(&self, x: f32, y: f32) -> u64 {
        let n = self.width.max(self.height).next_power_of_two();
        let tx = (x.floor() as i64).clamp(0, i64::from(n) - 1) as u32;
        let ty = (y.floor() as i64).clamp(0, i64::from(n) - 1) as u32;
        hilbert_index(n, tx, ty)
    }

    /// Quantization step for other entities' positions in optimized snapshots: positions are
    /// sent as `u16` steps of `1 / pos_scale` tiles.
    pub fn pos_scale(&self) -> f32 {
        (65535.0 / self.width.max(self.height).max(1) as f32).floor().clamp(1.0, 4096.0)
    }
}

/// Step along an n x n Hilbert curve that visits (x, y). `n` must be a power of two.
pub fn hilbert_index(n: u32, x: u32, y: u32) -> u64 {
    let (mut x, mut y, mut d) = (x, y, 0u64);
    let mut s = n / 2;
    while s > 0 {
        let rx = u32::from(x & s > 0);
        let ry = u32::from(y & s > 0);
        d += u64::from(s) * u64::from(s) * u64::from((3 * rx) ^ ry);
        // Rotate so the curve inside this quadrant has the standard orientation.
        if ry == 0 {
            if rx == 1 {
                x = n - 1 - x;
                y = n - 1 - y;
            }
            std::mem::swap(&mut x, &mut y);
        }
        s /= 2;
    }
    d
}

/// Position of step `d` along an n x n Hilbert curve.
fn hilbert_point(n: u32, d: u32) -> (u32, u32) {
    let (mut x, mut y, mut t) = (0u32, 0u32, d);
    let mut s = 1;
    while s < n {
        let rx = 1 & (t / 2);
        let ry = 1 & (t ^ rx);
        if ry == 0 {
            if rx == 1 {
                x = s - 1 - x;
                y = s - 1 - y;
            }
            std::mem::swap(&mut x, &mut y);
        }
        x += s * rx;
        y += s * ry;
        t /= 4;
        s *= 2;
    }
    (x, y)
}

/// Split chunks among `cells`: walk the Hilbert order and cut it into one contiguous run per
/// cell with roughly equal total weight. Every cell gets at least one chunk. Returns the
/// owning cell per chunk (row-major).
pub fn partition(order: &[usize], weights: &[f32], cells: &[u32]) -> Vec<u32> {
    let mut owners = vec![0u32; weights.len()];
    if cells.is_empty() {
        return owners;
    }
    let k = cells.len().min(order.len());
    let total: f32 = order.iter().map(|&c| weights[c]).sum();
    let (mut cell, mut count, mut acc) = (0usize, 0usize, 0.0f32);
    for (pos, &chunk) in order.iter().enumerate() {
        let w = weights[chunk];
        let target = (cell + 1) as f32 * total / k as f32;
        let cells_after = k - cell - 1;
        let chunks_left = order.len() - pos;
        let must_move_on = chunks_left == cells_after; // leave one chunk for each remaining cell
        let share_reached = acc + w / 2.0 > target;
        if cell + 1 < k && count > 0 && (must_move_on || share_reached) {
            cell += 1;
            count = 0;
        }
        owners[chunk] = cells[cell];
        acc += w;
        count += 1;
    }
    owners
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> WorldConfig {
        WorldConfig { width: 64, height: 64, chunk_size: 8, ..WorldConfig::default() }
    }

    #[test]
    fn chunk_lookup_and_bounds_agree() {
        let w = world();
        assert_eq!(w.chunk_count(), 64);
        let i = w.chunk_at(17.5, 9.2);
        assert_eq!(i, 8 + 2);
        let (x0, y0, x1, y1) = w.chunk_bounds(i);
        assert!((x0..x1).contains(&17.5) && (y0..y1).contains(&9.2));
        assert_eq!(w.chunk_at(-5.0, 1000.0), 56, "off the map clamps to the edge");
    }

    #[test]
    fn circles_touch_the_chunks_they_overlap() {
        let w = world();
        assert_eq!(w.chunks_in_circle(4.0, 4.0, 1.0), vec![0]);
        let mut both = w.chunks_in_circle(8.0, 4.0, 1.0);
        both.sort();
        assert_eq!(both, vec![0, 1]);
    }

    #[test]
    fn hilbert_order_visits_every_chunk_once_and_neighbours_touch() {
        let w = world();
        let order = w.chunks_in_hilbert_order();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(sorted, (0..64).collect::<Vec<_>>());
        for pair in order.windows(2) {
            let (a, b) = (w.chunk_xy(pair[0]), w.chunk_xy(pair[1]));
            assert_eq!(a.0.abs_diff(b.0) + a.1.abs_diff(b.1), 1);
        }
    }

    #[test]
    fn hilbert_order_skips_chunks_outside_a_non_square_world() {
        let w = WorldConfig { width: 48, height: 16, chunk_size: 8, ..WorldConfig::default() };
        let order = w.chunks_in_hilbert_order();
        assert_eq!(order.len(), 12);
        assert!(order.iter().all(|&c| c < 12));
    }

    #[test]
    fn hilbert_index_round_trips() {
        for d in 0..256 {
            let (x, y) = hilbert_point(16, d);
            assert_eq!(hilbert_index(16, x, y), u64::from(d));
        }
    }

    #[test]
    fn partition_gives_every_cell_an_even_contiguous_share() {
        let w = world();
        let order = w.chunks_in_hilbert_order();
        let owners = partition(&order, &[1.0; 64], &[7, 8, 9, 10]);
        for cell in [7, 8, 9, 10] {
            assert_eq!(owners.iter().filter(|&&o| o == cell).count(), 16);
        }
        let runs = order.windows(2).filter(|p| owners[p[0]] != owners[p[1]]).count();
        assert_eq!(runs, 3, "each cell owns one run of the curve");
    }

    #[test]
    fn views_see_what_they_should() {
        let rect = ViewShape::Rect { half_w: 10.0, half_h: 5.0 };
        assert!(rect.contains(0.0, 0.0, 9.0, -4.0, 0.0));
        assert!(!rect.contains(0.0, 0.0, 0.0, 6.0, 0.0));
        assert!(rect.overlaps_box(0.0, 0.0, 9.0, 4.0, 20.0, 20.0));
        let iso = ViewShape::Iso { half_w: 32.0, half_h: 36.0 };
        assert!(iso.contains(32.0, 32.0, 50.0, 50.0, 0.0), "down the screen");
        assert!(!iso.contains(32.0, 32.0, 52.0, 52.0, 0.0));
        assert!(iso.overlaps_box(32.0, 32.0, 48.0, 48.0, 56.0, 56.0));
        assert!(!iso.overlaps_box(32.0, 32.0, 60.0, 0.0, 64.0, 4.0), "far corner");
    }
}

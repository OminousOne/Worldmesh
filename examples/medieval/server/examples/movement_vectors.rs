//! Prints a long random walk on the real map as JSON (floats as raw f32 bits), so the
//! client's TypeScript port of the movement rules can be checked against this code
//! bit for bit. Run it through `npm run parity` in the example client. Set TICK_HZ to check
//! another tick rate.
//!
//! Each tick: [x, y, dx, dy, nx, ny, ndx, ndy, (sub, cdx, cdy)...]: the start position and
//! direction, the result of `simulate_tick`, then the mid-tick direction changes.

use medieval::map::{generate, splitmix64};
use medieval::movement::{simulate_tick, tile_center, Change, SUBSTEPS};
use medieval::{MAP_HEIGHT, MAP_WIDTH, WORLD_SEED};
use worldmesh::time;

fn random_dir(seed: &mut u64) -> (f32, f32) {
    *seed = splitmix64(*seed);
    let angle = (*seed % 3600) as f32 / 3600.0 * std::f32::consts::TAU;
    // Any angle, sometimes stop, sometimes shorter, sometimes longer than 1 (gets normalized).
    let len = match *seed % 13 {
        0 | 1 => 0.0,
        2 => 0.5,
        3 => 1.7,
        _ => 0.9999,
    };
    (angle.cos() * len, angle.sin() * len)
}

fn main() {
    let hz = std::env::var("TICK_HZ")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(time::DEFAULT_TICK_HZ);
    let tick_ms = time::set_tick_hz(hz);
    let map = generate(WORLD_SEED, MAP_WIDTH, MAP_HEIGHT);
    let (cx, cy) = map.center();
    let (mut x, mut y) = tile_center(cx, cy);
    let mut dir = (0.0f32, 0.0f32);
    let mut seed = 42u64;
    let mut ticks = Vec::new();

    for _ in 0..20_000 {
        // Most ticks have no change; some have one to three at random sub-steps.
        seed = splitmix64(seed);
        let count = match seed % 10 {
            0..=5 => 0,
            6 | 7 => 1,
            8 => 2,
            _ => 3,
        };
        let mut changes: Vec<Change> = (0..count)
            .map(|_| {
                seed = splitmix64(seed);
                ((seed % SUBSTEPS as u64) as u8, random_dir(&mut seed))
            })
            .collect();
        changes.sort_by_key(|c| c.0);

        let (nx, ny, ndir) = simulate_tick(&map, x, y, dir, &changes);
        let mut row: Vec<String> = [x, y, dir.0, dir.1, nx, ny, ndir.0, ndir.1]
            .map(|v| v.to_bits().to_string())
            .to_vec();
        for (sub, (cdx, cdy)) in &changes {
            row.extend([
                sub.to_string(),
                cdx.to_bits().to_string(),
                cdy.to_bits().to_string(),
            ]);
        }
        ticks.push(format!("[{}]", row.join(",")));
        (x, y, dir) = (nx, ny, ndir);
    }

    println!(
        "{{\"tick_ms\":{},\"width\":{},\"height\":{},\"tiles\":{:?},\"ticks\":[{}]}}",
        tick_ms,
        map.width,
        map.height,
        map.tiles,
        ticks.join(",")
    );
}

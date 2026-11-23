//! The shared clock. Every machine derives the same tick number from its (synced) wall
//! clock, so "tick N" means the same moment everywhere.

use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Ticks per second unless the TICK_HZ setting says otherwise.
pub const DEFAULT_TICK_HZ: u32 = 10;

static TICK_MS: OnceLock<u64> = OnceLock::new();

/// Set the tick rate once, at startup. Every process in the world must use the same one.
/// Ticks are a whole number of ms, so 30 Hz means 33 ms ticks. Returns the tick length in ms.
pub fn set_tick_hz(hz: u32) -> u64 {
    let ms = 1000 / u64::from(hz.clamp(1, 100));
    *TICK_MS.get_or_init(|| ms)
}

/// Length of one tick in ms.
pub fn tick_ms() -> u64 {
    *TICK_MS.get_or_init(|| 1000 / u64::from(DEFAULT_TICK_HZ))
}

/// How many ticks fit in `ms` (at least 1).
pub fn ticks_in(ms: u64) -> u64 {
    (ms / tick_ms()).max(1)
}

/// Fixed world start time (2026-01-01T00:00:00Z) in Unix milliseconds.
pub const WORLD_EPOCH_MS: u64 = 1_767_225_600_000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before 1970")
        .as_millis() as u64
}

/// Milliseconds since the world epoch.
pub fn world_ms() -> u64 {
    now_ms().saturating_sub(WORLD_EPOCH_MS)
}

pub fn current_tick() -> u64 {
    world_ms() / tick_ms()
}

/// Time left until tick `tick` starts (zero if it already has). Loops that wake up for other
/// reasons too should aim at a tick number like this rather than at "the next tick", which
/// changes under them as a boundary passes.
pub fn until_tick(tick: u64) -> Duration {
    Duration::from_millis((tick * tick_ms()).saturating_sub(world_ms()))
}

/// Time left until the next tick boundary, so loops line up with the shared grid.
pub fn until_next_tick() -> Duration {
    let into_tick = world_ms() % tick_ms();
    Duration::from_millis(tick_ms() - into_tick)
}

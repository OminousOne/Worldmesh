// TypeScript port of ../server/src/movement.rs, bit for bit: every operation is
// rounded to f32 with Math.fround exactly where the Rust code does f32 math, so the
// client's predictions match the server's results exactly.
// Keep the two files in sync. `npm run parity` checks them against each other.

export type Grid = { width: number; height: number; tiles: number[] };

const f = Math.fround;

let speedPerTick = 0;
/** Use the server's tick length (from Welcome). Same arithmetic as the Rust `speed_per_tick`. */
export function setTickMs(ms: number) {
  speedPerTick = f(f(f(5.0) * f(ms)) / f(1000));
}
setTickMs(100);
/** Half the width of a player's collision box. */
export const PLAYER_HALF_SIZE = f(0.3);
/** A tick is split into this many sub-steps; a direction change can land on any of them. */
export const SUBSTEPS = 16;

/** A direction change during a tick: from sub-step `sub` on, move in (dx, dy). */
export type Change = { sub: number; dx: number; dy: number };

const GRASS = 0;
const PATH = 1;

function walkable(g: Grid, x: number, y: number): boolean {
  if (x < 0 || y < 0 || x >= g.width || y >= g.height) return false;
  const t = g.tiles[y * g.width + x];
  return t === GRASS || t === PATH;
}

/** Clamp a requested direction to length <= 1. Garbage (NaN, infinity) means stand still. */
export function sanitizeDir(dx: number, dy: number): [number, number] {
  if (!Number.isFinite(dx) || !Number.isFinite(dy)) return [0, 0];
  dx = f(dx);
  dy = f(dy);
  const len = f(Math.sqrt(f(f(dx * dx) + f(dy * dy))));
  return len > 1 ? [f(dx / len), f(dy / len)] : [dx, dy];
}

/** True when a box centred on (x, y) overlaps only walkable tiles. */
export function boxIsClear(g: Grid, x: number, y: number, half: number): boolean {
  const minTx = Math.floor(f(x - half));
  const maxTx = Math.ceil(f(x + half)) - 1;
  const minTy = Math.floor(f(y - half));
  const maxTy = Math.ceil(f(y + half)) - 1;
  for (let ty = minTy; ty <= maxTy; ty++) {
    for (let tx = minTx; tx <= maxTx; tx++) {
      if (!walkable(g, tx, ty)) return false;
    }
  }
  return true;
}

/** Largest part of `delta` that can be moved while `clear` holds (see the Rust version). */
function furthestClear(delta: number, clear: (d: number) => boolean): number {
  if (delta === 0 || clear(delta)) return delta;
  let lo = 0;
  let hi = 1;
  for (let i = 0; i < 20; i++) {
    const mid = f(f(lo + hi) * 0.5);
    if (clear(f(delta * mid))) lo = mid;
    else hi = mid;
  }
  return f(delta * lo);
}

/** Advance one whole tick in direction (dx, dy). */
export function step(g: Grid, x: number, y: number, dx: number, dy: number): { x: number; y: number } {
  return stepPart(g, x, y, dx, dy, 1);
}

/** Advance `part` of a tick (0 to 1), resolving each axis separately (slides along walls). */
export function stepPart(g: Grid, x: number, y: number, dx: number, dy: number, part: number): { x: number; y: number } {
  [dx, dy] = sanitizeDir(dx, dy);
  const dist = f(speedPerTick * f(part));
  const vx = f(dx * dist);
  const vy = f(dy * dist);
  const nx = f(x + furthestClear(vx, (d) => boxIsClear(g, f(x + d), y, PLAYER_HALF_SIZE)));
  const ny = f(y + furthestClear(vy, (d) => boxIsClear(g, nx, f(y + d), PLAYER_HALF_SIZE)));
  return { x: nx, y: ny };
}

/** One tick for a player who starts it moving in `dir` and changes direction at each
 *  sub-step in `changes` (sorted by sub-step). Same as the Rust `simulate_tick`.
 *  `until` (in sub-steps, may be fractional) stops part way through, for drawing
 *  positions between ticks; only the default (a whole tick) is authoritative. */
export function simulateTick(
  g: Grid, x: number, y: number, dir: { x: number; y: number }, changes: Change[], until = SUBSTEPS,
): { x: number; y: number; dir: { x: number; y: number } } {
  let pos = { x, y };
  let from = 0;
  for (const c of changes) {
    const at = Math.min(c.sub, SUBSTEPS);
    if (at > until) break;
    if (at > from) {
      pos = segment(g, pos, dir, from, at);
      from = at;
    }
    dir = { x: c.dx, y: c.dy };
  }
  pos = segment(g, pos, dir, from, until);
  return { ...pos, dir };
}

function segment(g: Grid, p: { x: number; y: number }, dir: { x: number; y: number }, from: number, to: number) {
  if (to <= from || (dir.x === 0 && dir.y === 0)) return p;
  return stepPart(g, p.x, p.y, dir.x, dir.y, (to - from) / SUBSTEPS);
}

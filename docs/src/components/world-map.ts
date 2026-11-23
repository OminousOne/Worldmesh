// The landing page's map: a world split into four areas, one per Spatial Layer Cell, with cyan
// seams between them like the logo. Shared by the server render and the browser script.

export const W = 520;
export const H = 420;
/** How far a player sees, in map units. */
export const VIEW = 88;

export type Region = 'forest' | 'volcano' | 'ocean' | 'sand';
export const REGIONS: Region[] = ['forest', 'volcano', 'ocean', 'sand'];

/** The north-south seam: its x at a given y. */
export const seamX = (y: number) => W * 0.53 + 22 * Math.sin((y / H) * Math.PI * 1.3 + 0.4);
/** The east-west seam: its y at a given x. */
export const seamY = (x: number) => H * 0.5 + 20 * Math.sin((x / W) * Math.PI * 1.5 + 2.2);

export function regionAt(x: number, y: number): Region {
  const west = x < seamX(y);
  const north = y < seamY(x);
  return north ? (west ? 'forest' : 'volcano') : west ? 'ocean' : 'sand';
}

/** Where the two seams cross. */
export function junction() {
  let x = W / 2;
  let y = H / 2;
  for (let i = 0; i < 30; i++) {
    y = seamY(x);
    x = seamX(y);
  }
  return { x, y };
}

/** The areas a view circle touches: their cells all add to that player's view. */
export function touched(x: number, y: number, r = VIEW): Set<Region> {
  const seen = new Set<Region>([regionAt(x, y)]);
  for (let i = 0; i < 24; i++) {
    const a = (i / 24) * Math.PI * 2;
    const px = Math.min(W, Math.max(0, x + Math.cos(a) * r));
    const py = Math.min(H, Math.max(0, y + Math.sin(a) * r));
    seen.add(regionAt(px, py));
  }
  return seen;
}

/** A small seeded random number generator, so the server and the first frame agree. */
export function seeded(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

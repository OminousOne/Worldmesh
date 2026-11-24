// A small world for the docs' chunk map demos: how cells split the map, how the orchestrator
// splits entities between groups, and which cells a "what's near me?" question reaches.
// The splitting code is a port of worldmesh-core's `partition` and the orchestrator's `plan`.

export const SIZE = 64; // tiles
export const CHUNK = 8; // tiles per chunk side
export const CHUNKS = SIZE / CHUNK;
export const PX = 10; // svg units per tile
export const LETTERS = ['A', 'B', 'C', 'D'];

/** Position of step `d` along an n x n Hilbert curve (worldmesh-core `hilbert_point`). */
function hilbertPoint(n: number, d: number): [number, number] {
  let x = 0;
  let y = 0;
  let t = d;
  for (let s = 1; s < n; s *= 2) {
    const rx = 1 & Math.floor(t / 2);
    const ry = 1 & (t ^ rx);
    if (ry === 0) {
      if (rx === 1) {
        x = s - 1 - x;
        y = s - 1 - y;
      }
      [x, y] = [y, x];
    }
    x += s * rx;
    y += s * ry;
    t = Math.floor(t / 4);
  }
  return [x, y];
}

/** Step along an n x n Hilbert curve that visits (x, y) (worldmesh-core `hilbert_index`). */
function hilbertIndex(n: number, x: number, y: number): number {
  let d = 0;
  for (let s = n / 2; s > 0; s = Math.floor(s / 2)) {
    const rx = (x & s) > 0 ? 1 : 0;
    const ry = (y & s) > 0 ? 1 : 0;
    d += s * s * ((3 * rx) ^ ry);
    if (ry === 0) {
      if (rx === 1) {
        x = n - 1 - x;
        y = n - 1 - y;
      }
      [x, y] = [y, x];
    }
  }
  return d;
}

/** Chunk indices in Hilbert order: any run of them is a compact area. */
export const chunkOrder: number[] = Array.from({ length: CHUNKS * CHUNKS }, (_, d) => {
  const [x, y] = hilbertPoint(CHUNKS, d);
  return y * CHUNKS + x;
});

/** Which cell owns each chunk: equal runs along the curve (worldmesh-core `partition`). */
export function cellOwners(cells: number): number[] {
  const owners = new Array<number>(chunkOrder.length).fill(0);
  const k = Math.min(cells, chunkOrder.length);
  const total = chunkOrder.length;
  let cell = 0;
  let count = 0;
  let acc = 0;
  chunkOrder.forEach((chunk, pos) => {
    const target = ((cell + 1) * total) / k;
    const mustMoveOn = chunkOrder.length - pos === k - cell - 1;
    if (cell + 1 < k && count > 0 && (mustMoveOn || acc + 0.5 > target)) {
      cell += 1;
      count = 0;
    }
    owners[chunk] = cell;
    acc += 1;
    count += 1;
  });
  return owners;
}

export interface Dot {
  id: number;
  x: number;
  y: number;
}

/** The same scattered crowd on every build: a busy town and people out in the fields. */
export function crowd(): Dot[] {
  let s = 11;
  const rand = () => {
    s = (s * 1103515245 + 12345) & 0x7fffffff;
    return s / 0x7fffffff;
  };
  const dots: Dot[] = [];
  const add = (x: number, y: number) =>
    dots.push({ id: dots.length + 1, x: Math.min(63.5, Math.max(0.5, x)), y: Math.min(63.5, Math.max(0.5, y)) });
  for (let i = 0; i < 70; i++) {
    const a = rand() * Math.PI * 2;
    const r = Math.sqrt(rand()) * 11;
    add(22 + Math.cos(a) * r, 38 + Math.sin(a) * r * 0.8);
  }
  for (let i = 0; i < 60; i++) add(2 + rand() * 60, 2 + rand() * 60);
  return dots;
}

/** Split entities between groups 0..k-1 (the orchestrator's `plan`): sorted along a Hilbert
 * curve, cut into equal runs, each run given to the group already holding most of it. */
export function plan(dots: Dot[], k: number, current: Map<number, number>): Map<number, number> {
  const sorted = dots
    .map((d) => ({ key: hilbertIndex(SIZE, Math.floor(d.x), Math.floor(d.y)), id: d.id }))
    .sort((a, b) => a.key - b.key || a.id - b.id);
  const n = sorted.length;
  const runs = Array.from({ length: k }, (_, r) =>
    sorted.slice(Math.floor((r * n) / k), Math.floor(((r + 1) * n) / k)),
  );
  const overlap = runs.map((run) =>
    Array.from({ length: k }, (_, g) => run.filter((e) => current.get(e.id) === g).length),
  );
  let best = { total: -1, order: [] as number[] };
  const perm = Array.from({ length: k }, (_, i) => i);
  const visit = (i: number) => {
    if (i === k) {
      const total = perm.reduce((sum, g, r) => sum + overlap[r][g], 0);
      if (total > best.total) best = { total, order: [...perm] };
      return;
    }
    for (let j = i; j < k; j++) {
      [perm[i], perm[j]] = [perm[j], perm[i]];
      visit(i + 1);
      [perm[i], perm[j]] = [perm[j], perm[i]];
    }
  };
  visit(0);
  const out = new Map<number, number>();
  runs.forEach((run, r) => run.forEach((e) => out.set(e.id, best.order[r])));
  return out;
}

/** Chunks a circle touches. */
export function chunksTouched(x: number, y: number, r: number): number[] {
  const out: number[] = [];
  for (let cy = 0; cy < CHUNKS; cy++) {
    for (let cx = 0; cx < CHUNKS; cx++) {
      const nx = Math.max(cx * CHUNK, Math.min(x, (cx + 1) * CHUNK));
      const ny = Math.max(cy * CHUNK, Math.min(y, (cy + 1) * CHUNK));
      if (Math.hypot(nx - x, ny - y) <= r) out.push(cy * CHUNKS + cx);
    }
  }
  return out;
}

export interface MapState {
  colorBy: 'groups' | 'cells' | 'none';
  cells: number;
  groupOf?: Map<number, number>;
  moved?: Set<number>;
  query?: { x: number; y: number; r: number };
  label?: string;
}

/** The map as SVG markup, for the server render and every redraw in the browser. */
export function renderMap(dots: Dot[], s: MapState): string {
  const owners = cellOwners(s.cells);
  const touched = s.query ? new Set(chunksTouched(s.query.x, s.query.y, s.query.r)) : null;
  const asked = touched ? new Set([...touched].map((c) => owners[c])) : null;
  const parts: string[] = [];
  for (let c = 0; c < owners.length; c++) {
    const cx = (c % CHUNKS) * CHUNK * PX;
    const cy = Math.floor(c / CHUNKS) * CHUNK * PX;
    const cls = s.colorBy === 'cells' ? `area cat-${owners[c] + 1}` : 'area plain';
    const dim = asked && !asked.has(owners[c]) ? ' dim' : '';
    parts.push(`<rect class="${cls}${dim}" x="${cx}" y="${cy}" width="${CHUNK * PX}" height="${CHUNK * PX}"/>`);
  }
  // Borders between cells' areas.
  const edges: string[] = [];
  for (let c = 0; c < owners.length; c++) {
    const x = c % CHUNKS;
    const y = Math.floor(c / CHUNKS);
    const px = x * CHUNK * PX;
    const py = y * CHUNK * PX;
    const side = CHUNK * PX;
    if (x + 1 < CHUNKS && owners[c + 1] !== owners[c]) edges.push(`M${px + side} ${py}v${side}`);
    if (y + 1 < CHUNKS && owners[c + CHUNKS] !== owners[c]) edges.push(`M${px} ${py + side}h${side}`);
  }
  if (s.colorBy === 'cells' || s.query) parts.push(`<path class="border" d="${edges.join('')}"/>`);
  if (touched) {
    for (const c of touched) {
      const cx = (c % CHUNKS) * CHUNK * PX;
      const cy = Math.floor(c / CHUNKS) * CHUNK * PX;
      parts.push(`<rect class="touched" x="${cx + 1.5}" y="${cy + 1.5}" width="${CHUNK * PX - 3}" height="${CHUNK * PX - 3}"/>`);
    }
  }
  // Cell letters, at the middle of each cell's chunks.
  if (s.colorBy === 'cells' || s.query) {
    for (let k = 0; k < s.cells; k++) {
      const mine = owners.map((o, c) => (o === k ? c : -1)).filter((c) => c >= 0);
      const mx = mine.reduce((a, c) => a + (c % CHUNKS), 0) / mine.length;
      const my = mine.reduce((a, c) => a + Math.floor(c / CHUNKS), 0) / mine.length;
      const pick = mine.reduce((best, c) => {
        const d = Math.hypot((c % CHUNKS) - mx, Math.floor(c / CHUNKS) - my);
        return d < best.d ? { c, d } : best;
      }, { c: mine[0], d: Infinity }).c;
      const lx = ((pick % CHUNKS) + 0.5) * CHUNK * PX;
      const ly = (Math.floor(pick / CHUNKS) + 0.5) * CHUNK * PX;
      parts.push(`<text class="cell-label" x="${lx}" y="${ly}">${LETTERS[k]}</text>`);
    }
  }
  if (s.query) {
    const { x, y, r } = s.query;
    parts.push(`<circle class="reach" cx="${x * PX}" cy="${y * PX}" r="${r * PX}"/>`);
  }
  for (const d of dots) {
    const g = s.groupOf?.get(d.id);
    let cls = 'dot';
    if (s.colorBy === 'groups' && g !== undefined) cls += ` cat-${g + 1}`;
    if (s.query) cls += Math.hypot(d.x - s.query.x, d.y - s.query.y) <= s.query.r ? ' hit' : ' far';
    parts.push(`<circle class="${cls}" cx="${(d.x * PX).toFixed(1)}" cy="${(d.y * PX).toFixed(1)}" r="5"/>`);
    if (s.moved?.has(d.id)) {
      parts.push(`<circle class="moved" cx="${(d.x * PX).toFixed(1)}" cy="${(d.y * PX).toFixed(1)}" r="9"/>`);
    }
  }
  if (s.query) {
    parts.push(`<circle class="asker" cx="${s.query.x * PX}" cy="${s.query.y * PX}" r="7"/>`);
  }
  if (s.label) parts.push(`<text class="map-label" x="12" y="28">${s.label}</text>`);
  return parts.join('');
}

/** A legend for the map's colors, with how much each group or cell holds. */
export function renderLegend(dots: Dot[], s: MapState): string {
  if (s.colorBy === 'groups' && s.groupOf) {
    const counts = new Map<number, number>();
    for (const d of dots) counts.set(s.groupOf.get(d.id)!, (counts.get(s.groupOf.get(d.id)!) ?? 0) + 1);
    return [...counts.keys()]
      .sort()
      .map((g) => `<li><span class="wm-swatch" style="background: var(--wm-cat-${g + 1})"></span>Group ${g + 1}: ${counts.get(g)} entities</li>`)
      .join('');
  }
  if (s.colorBy === 'cells') {
    const owners = cellOwners(s.cells);
    return Array.from({ length: s.cells }, (_, k) => {
      const n = owners.filter((o) => o === k).length;
      return `<li><span class="wm-swatch" style="background: var(--wm-cat-${k + 1})"></span>Cell ${LETTERS[k]}: ${n} chunks</li>`;
    }).join('');
  }
  return '';
}

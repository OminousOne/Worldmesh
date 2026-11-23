// Checks the TypeScript movement port against the Rust original, bit for bit.
// Usage (or just `npm run parity` here):
//   cargo run -q -p medieval --example movement_vectors > /tmp/vectors.json
//   node --experimental-strip-types scripts/check-movement.ts /tmp/vectors.json

import { readFileSync } from 'node:fs';
import { setTickMs, simulateTick, type Change } from '../src/movement.ts';

const data = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const fromBits = (bits: number) => new Float32Array(new Uint32Array([bits]).buffer)[0];
const map = { width: data.width, height: data.height, tiles: data.tiles };
setTickMs(data.tick_ms);

let mismatches = 0;
let withChanges = 0;
for (const raw of data.ticks as number[][]) {
  const [x, y, dx, dy, nx, ny, ndx, ndy] = raw.slice(0, 8).map(fromBits);
  const changes: Change[] = [];
  for (let i = 8; i < raw.length; i += 3) changes.push({ sub: raw[i], dx: fromBits(raw[i + 1]), dy: fromBits(raw[i + 2]) });
  if (changes.length) withChanges++;
  const r = simulateTick(map, x, y, { x: dx, y: dy }, changes);
  if (r.x !== nx || r.y !== ny || r.dir.x !== ndx || r.dir.y !== ndy) {
    if (mismatches++ < 5) console.log('mismatch', { x, y, dx, dy, changes }, 'rust', [nx, ny, ndx, ndy], 'ts', r);
  }
}
console.log(`${data.tick_ms} ms ticks: ${data.ticks.length} ticks checked (${withChanges} with mid-tick changes), ${mismatches} mismatches`);
process.exit(mismatches ? 1 : 0);

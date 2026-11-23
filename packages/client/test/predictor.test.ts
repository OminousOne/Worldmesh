import assert from 'node:assert/strict';
import { test } from 'node:test';

import { Predictor, type PredictionRules, type Timed } from '../src/predictor.ts';
import type { SnapshotEntity } from '../src/protocol.ts';

// A 1D world: one tile per tick in the held direction, changing at each input's sub-step.
type State = { x: number; dir: number };
const SUB = 16;
const rules: PredictionRules<State, number> = {
  idle: 0,
  fromServer: (e, held) => ({ x: e.x, dir: held }),
  simulateTick(s, changes: Timed<number>[], untilSub = SUB) {
    let { x, dir } = s;
    let from = 0;
    for (const c of changes) {
      const at = Math.min(c.sub, untilSub);
      if (at > from) x += (dir * (at - from)) / SUB;
      from = Math.max(from, at);
      if (c.sub <= untilSub) dir = c.input;
    }
    if (untilSub > from) x += (dir * (untilSub - from)) / SUB;
    return { x, dir };
  },
  position: (s) => ({ x: s.x, y: 0 }),
};

const entity = (x: number): SnapshotEntity<unknown> => ({ id: 1, x, y: 0, group: 1, spatial: 0x8001, player: true, kind: 'hero', view: {} });

function setup() {
  let clock = 1000; // server clock, in ticks
  const p = new Predictor(rules, { substeps: SUB, tickMs: 100, serverClock: () => clock });
  p.lead = 0; // run on the server clock, to keep the arithmetic readable
  (p as unknown as { leadTarget: number }).leadTarget = 0;
  return { p, setClock: (t: number) => (clock = t) };
}

test('an input takes effect at once locally, at the sub-step it was scheduled for', () => {
  const { p, setClock } = setup();
  p.onServer(1000, entity(5));
  setClock(1000.5);
  const at = p.schedule(1);
  assert.deepEqual(at, { tick: 1001, sub: 8 }, 'half way through tick 1001');
  setClock(1001);
  p.frame(0);
  assert.equal(p.predicted!.x, 5.5, 'moved for the second half of the tick');
});

test('a matching server position changes nothing; a different one is corrected and replayed', () => {
  const { p, setClock } = setup();
  p.onServer(1000, entity(5));
  setClock(1000.5);
  p.schedule(1);
  setClock(1003);
  p.frame(0);
  assert.equal(p.predicted!.x, 7.5);
  p.onServer(1001, entity(5.5));
  assert.equal(p.stats.corrections, 0);
  p.onServer(1002, entity(6)); // something held us back half a tile
  assert.equal(p.stats.corrections, 1);
  assert.equal(p.predicted!.x, 7, 'the server position for 1002, plus the tick after it, replayed');
});

test('without prediction inputs go out for "as soon as possible"', () => {
  const { p } = setup();
  p.enabled = false;
  assert.deepEqual(p.schedule(1), { tick: 0, sub: 0 });
  assert.equal(p.position(), undefined);
});

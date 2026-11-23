---
title: The browser client
description: Connect a browser to a world with @worldmesh/client, draw what's in view, and predict your own entity.
---

`@worldmesh/client` is the browser side: it connects to a gateway, decodes snapshots (compact binary or JSON) into plain objects with your game's views and events, keeps every entity in view with about a second of positions, tracks the server clock, and predicts your own entity if you give it your movement rules. It doesn't draw anything: use Pixi, Three.js, a canvas, the DOM.

```sh
npm install @worldmesh/client
```

## Connect

```ts
import { WorldmeshClient } from '@worldmesh/client';

type Look = { it: boolean };                 // your View, as JSON
type Event = { k: 'tagged'; by: number; who: number };
type Walk = { dx: number; dy: number };      // your Input
type Command = { k: 'tag'; target: number };

const client = new WorldmeshClient<Look, Event, null, Walk, Command>(
  { url: 'ws://localhost:8080/ws' },
  {
    welcome: (w) => console.log('I am', w.id, 'in a', w.world.width, 'x', w.world.height, 'world'),
    snapshot: (s) => s.events.forEach(showEvent),
    added: (e) => createSprite(e.state.id),
    removed: (id) => destroySprite(id),
  },
);
client.connect();
```

Type parameters, in order: your View, Event, Welcome, Input, Command, and (for prediction) your predicted state.

## Draw

Every frame:

```ts
function frame(dtMs: number) {
  client.frame(dtMs); // advances the prediction
  const t = client.renderTime();
  for (const [id, e] of client.entities) {
    const p = client.positionOf(id, t); // yours: predicted; others: interpolated
    if (p) drawAt(id, p.x, p.y, e.state.view);
  }
}
```

`e.state` is the entity as of the newest snapshot: `id`, `x`, `y`, `group`, `spatial`, `player`, and `view` (yours). Don't draw `e.state.x` directly: it jumps once per tick. `positionOf` is smooth.

## Send

```ts
client.sendInput({ dx: 1, dy: 0 });    // what's held from now on; predicted at once
client.sendCommand({ k: 'tag', target: 7 });
```

Send an input when it changes, not every frame.

## Predict your own entity

Give the client your movement rules, the same ones the server runs:

```ts
import { WorldmeshClient, type PredictionRules } from '@worldmesh/client';

type State = { x: number; y: number; dir: { x: number; y: number } };

const rules: PredictionRules<State, Walk, Look> = {
  idle: { dx: 0, dy: 0 },
  isIdle: (w) => w.dx === 0 && w.dy === 0,
  // The server's word on you after a tick, plus the input you held at its end.
  fromServer: (e, held) => ({ x: e.x, y: e.y, dir: { x: held.dx, y: held.dy } }),
  // One tick (or the first `untilSub` sub-steps of it), exactly like your Game::tick.
  simulateTick: (s, changes, untilSub) => mySimulateTick(s, changes, untilSub),
  position: (s) => ({ x: s.x, y: s.y }),
};

const client = new WorldmeshClient<Look, Event, null, Walk, Command, State>({
  url: 'ws://localhost:8080/ws',
  prediction: rules,
  shouldPredict: (own) => !own.view.dead, // optional: not while dead, say
});
```

`simulateTick` must match the server bit for bit. In JavaScript that means rounding with `Math.fround` exactly where Rust does `f32` math; the medieval example's `movement.ts` shows how, and its `npm run parity` script checks it against the Rust code. If they disagree, prediction still works, it just corrects (and glides) more often: `client.predictor.stats.corrections` counts them.

## Settings and testing tools

```ts
client.setSettings({ compact: true, debug: false }); // binary snapshots; no debug traffic
client.fakeLatency = 100;                             // ms each way, on everything
client.fakeJitter = 25;                               // up to this much more, without reordering
```

Compact snapshots are on by default: binary, and only what changed since the last one (an entity standing still costs nothing). JSON is there for reading in the browser's network tab.
